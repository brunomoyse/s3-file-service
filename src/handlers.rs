use crate::image_processing;
use crate::rembg;
use crate::s3;
use crate::AppState;
use actix_multipart::Multipart;
use actix_web::{Error as ActixError, HttpResponse, post, web};
use futures::{StreamExt, TryStreamExt, try_join};
use serde::Serialize;

// ---- Existing handler (unchanged) ----

#[post("/upload")]
pub async fn upload_image(
    state: web::Data<AppState>,
    mut payload: Multipart,
) -> Result<HttpResponse, ActixError> {
    let mut product_slug = String::new();
    let mut image_data = Vec::new();

    while let Some(mut field) = payload.try_next().await? {
        if let Some(content_disposition) = field.content_disposition() {
            match content_disposition.get_name() {
                Some("product_slug") => {
                    while let Some(chunk) = field.next().await {
                        product_slug = String::from_utf8(chunk?.to_vec())
                            .map_err(actix_web::error::ErrorBadRequest)?;
                    }
                }
                Some("image") => {
                    while let Some(chunk) = field.next().await {
                        image_data.extend_from_slice(&chunk?);
                    }
                }
                _ => {}
            }
        }
    }

    if product_slug.is_empty() || image_data.is_empty() {
        return Ok(HttpResponse::BadRequest().body("Missing product_slug or image"));
    }

    let bucket = state.s3_bucket.clone();
    let s3_client = state.s3_client.clone();

    let image_data_clone = image_data.clone();
    let (resized_normal, resized_thumb) = try_join!(
        web::block(move || image_processing::resize_image(&image_data, 600)),
        web::block(move || image_processing::resize_image(&image_data_clone, 350)),
    )?;

    let resized_normal = resized_normal.map_err(actix_web::error::ErrorInternalServerError)?; // Map ImageError to ActixError
    let resized_thumb = resized_thumb.map_err(actix_web::error::ErrorInternalServerError)?; // Map ImageError to ActixError

    let mut upload_tasks = vec![];
    let formats = vec!["avif", "webp", "png"];

    for fmt in formats {
        let s3 = s3_client.clone();
        let bucket = bucket.clone();
        let slug = product_slug.clone();
        let normal = resized_normal.clone();
        let thumb = resized_thumb.clone();

        upload_tasks.push(async move {
            // Normal image processing
            let data = web::block(move || match fmt {
                "png" => image_processing::encode_to_png(&normal),
                "webp" => image_processing::encode_to_webp(&normal, 75.0),
                "avif" => image_processing::encode_to_avif(&normal, 75.0),
                _ => unreachable!(),
            })
            .await?
            .map_err(actix_web::error::ErrorInternalServerError)?;

            s3::upload_file(&s3, &bucket, &format!("images/{}.{}", slug, fmt), data)
                .await
                .map_err(|e| actix_web::error::ErrorInternalServerError(e.to_string()))?;

            // Thumbnail processing
            let data = web::block(move || match fmt {
                "png" => image_processing::encode_to_png(&thumb),
                "webp" => image_processing::encode_to_webp(&thumb, 75.0),
                "avif" => image_processing::encode_to_avif(&thumb, 75.0),
                _ => unreachable!(),
            })
            .await?
            .map_err(actix_web::error::ErrorInternalServerError)?;

            s3::upload_file(
                &s3,
                &bucket,
                &format!("images/thumbnails/{}.{}", slug, fmt),
                data,
            )
            .await
            .map_err(|e| actix_web::error::ErrorInternalServerError(e.to_string()))
        });
    }

    let results: Vec<Result<_, ActixError>> = futures::future::join_all(upload_tasks).await;
    for result in results {
        result?;
    }

    Ok(HttpResponse::Ok().body("Images uploaded successfully"))
}

// ---- Response types ----

#[derive(Serialize)]
struct Dimensions {
    width: u32,
    height: u32,
}

#[derive(Serialize)]
struct ProcessedUploadResponse {
    status: &'static str,
    original_dimensions: Dimensions,
    post_rembg_dimensions: Dimensions,
    post_trim_dimensions: Dimensions,
    trim_applied: bool,
    upload_to_s3: bool,
}

// ---- Shared pipeline helpers ----

async fn parse_upload_form(payload: &mut Multipart) -> Result<(String, Vec<u8>), ActixError> {
    let mut product_slug = String::new();
    let mut image_data: Vec<u8> = Vec::new();

    while let Some(mut field) = payload.try_next().await? {
        if let Some(cd) = field.content_disposition() {
            match cd.get_name() {
                Some("product_slug") => {
                    while let Some(chunk) = field.next().await {
                        product_slug = String::from_utf8(chunk?.to_vec())
                            .map_err(actix_web::error::ErrorBadRequest)?;
                    }
                }
                Some("image") => {
                    while let Some(chunk) = field.next().await {
                        image_data.extend_from_slice(&chunk?);
                    }
                }
                _ => {}
            }
        }
    }

    if product_slug.is_empty() || image_data.is_empty() {
        return Err(actix_web::error::ErrorBadRequest("Missing product_slug or image"));
    }

    Ok((product_slug, image_data))
}

async fn parse_image_only(payload: &mut Multipart) -> Result<Vec<u8>, ActixError> {
    let mut image_data: Vec<u8> = Vec::new();

    while let Some(mut field) = payload.try_next().await? {
        if let Some(cd) = field.content_disposition()
            && cd.get_name() == Some("image")
        {
            while let Some(chunk) = field.next().await {
                image_data.extend_from_slice(&chunk?);
            }
        }
    }

    if image_data.is_empty() {
        return Err(actix_web::error::ErrorBadRequest("Missing image"));
    }

    Ok(image_data)
}

struct RembgPipelineResult {
    processed_bytes: Vec<u8>,
    original_dims: (u32, u32),
    post_rembg_dims: (u32, u32),
    post_trim_dims: (u32, u32),
    trim_applied: bool,
}

/// Runs rembg background removal then optionally trims transparent borders.
/// Shared by both the upload and preview processed routes.
async fn process_with_rembg_and_trim(
    http_client: &reqwest::Client,
    rembg_url: &str,
    trim_transparent: bool,
    image_data: Vec<u8>,
) -> Result<RembgPipelineResult, ActixError> {
    let original_dims = image_processing::image_dimensions(&image_data)
        .map_err(actix_web::error::ErrorBadRequest)?;

    let rembg_bytes = rembg::remove_background(http_client, rembg_url, image_data)
        .await
        .map_err(|e| actix_web::error::ErrorInternalServerError(e.to_string()))?;

    let post_rembg_dims = image_processing::image_dimensions(&rembg_bytes)
        .map_err(actix_web::error::ErrorInternalServerError)?;

    let (processed_bytes, post_trim_dims, trim_applied) = if trim_transparent {
        let bytes_for_trim = rembg_bytes.clone();
        let trimmed =
            web::block(move || image_processing::trim_transparent_borders(&bytes_for_trim))
                .await?
                .map_err(actix_web::error::ErrorInternalServerError)?;
        let dims = image_processing::image_dimensions(&trimmed)
            .map_err(actix_web::error::ErrorInternalServerError)?;
        let applied = dims != post_rembg_dims;
        (trimmed, dims, applied)
    } else {
        (rembg_bytes, post_rembg_dims, false)
    };

    println!(
        "[rembg] original={}x{} post_rembg={}x{} post_trim={}x{} trim_applied={}",
        original_dims.0,
        original_dims.1,
        post_rembg_dims.0,
        post_rembg_dims.1,
        post_trim_dims.0,
        post_trim_dims.1,
        trim_applied,
    );

    Ok(RembgPipelineResult {
        processed_bytes,
        original_dims,
        post_rembg_dims,
        post_trim_dims,
        trim_applied,
    })
}

/// Encodes a normal+thumbnail pair to AVIF/WebP/PNG and uploads all 6 variants to S3.
/// Mirrors the upload logic in `upload_image` for use by new routes.
async fn encode_and_upload_variants(
    s3_client: &aws_sdk_s3::Client,
    bucket: &str,
    slug: &str,
    normal: image::DynamicImage,
    thumb: image::DynamicImage,
) -> Result<(), ActixError> {
    let formats = ["avif", "webp", "png"];
    let mut upload_tasks = vec![];

    for fmt in formats {
        let s3 = s3_client.clone();
        let bucket = bucket.to_string();
        let slug = slug.to_string();
        let normal = normal.clone();
        let thumb = thumb.clone();

        upload_tasks.push(async move {
            let normal_data = web::block(move || match fmt {
                "png" => image_processing::encode_to_png(&normal),
                "webp" => image_processing::encode_to_webp(&normal, 75.0),
                "avif" => image_processing::encode_to_avif(&normal, 75.0),
                _ => unreachable!(),
            })
            .await?
            .map_err(actix_web::error::ErrorInternalServerError)?;

            s3::upload_file(&s3, &bucket, &format!("images/{}.{}", slug, fmt), normal_data)
                .await
                .map_err(|e| actix_web::error::ErrorInternalServerError(e.to_string()))?;

            let thumb_data = web::block(move || match fmt {
                "png" => image_processing::encode_to_png(&thumb),
                "webp" => image_processing::encode_to_webp(&thumb, 75.0),
                "avif" => image_processing::encode_to_avif(&thumb, 75.0),
                _ => unreachable!(),
            })
            .await?
            .map_err(actix_web::error::ErrorInternalServerError)?;

            s3::upload_file(
                &s3,
                &bucket,
                &format!("images/thumbnails/{}.{}", slug, fmt),
                thumb_data,
            )
            .await
            .map_err(|e| actix_web::error::ErrorInternalServerError(e.to_string()))
        });
    }

    let results: Vec<Result<_, ActixError>> = futures::future::join_all(upload_tasks).await;
    for r in results {
        r?;
    }
    Ok(())
}

// ---- New handlers ----

/// POST /images/upload/processed
///
/// Accepts: multipart with `product_slug` (text) and `image` (file).
/// Pipeline: rembg → optional trim → resize 600/350 → encode AVIF/WebP/PNG → S3 upload (6 variants).
/// Returns: JSON with processing metadata.
#[post("/images/upload/processed")]
pub async fn upload_processed_image(
    state: web::Data<AppState>,
    mut payload: Multipart,
) -> Result<HttpResponse, ActixError> {
    let (product_slug, image_data) = parse_upload_form(&mut payload).await?;

    let rembg_url = state
        .rembg_url
        .as_deref()
        .ok_or_else(|| actix_web::error::ErrorServiceUnavailable("REMBG_URL not configured"))?;

    let RembgPipelineResult {
        processed_bytes,
        original_dims,
        post_rembg_dims,
        post_trim_dims,
        trim_applied,
    } = process_with_rembg_and_trim(
        &state.http_client,
        rembg_url,
        state.trim_transparent,
        image_data,
    )
    .await?;

    let bytes_clone = processed_bytes.clone();
    let (normal, thumb) = try_join!(
        web::block(move || image_processing::resize_image(&processed_bytes, 600)),
        web::block(move || image_processing::resize_image(&bytes_clone, 350)),
    )?;

    let normal = normal.map_err(actix_web::error::ErrorInternalServerError)?;
    let thumb = thumb.map_err(actix_web::error::ErrorInternalServerError)?;

    encode_and_upload_variants(&state.s3_client, &state.s3_bucket, &product_slug, normal, thumb)
        .await?;

    println!("[upload processed] slug={} upload_to_s3=true", product_slug);

    Ok(HttpResponse::Ok().json(ProcessedUploadResponse {
        status: "ok",
        original_dimensions: Dimensions {
            width: original_dims.0,
            height: original_dims.1,
        },
        post_rembg_dimensions: Dimensions {
            width: post_rembg_dims.0,
            height: post_rembg_dims.1,
        },
        post_trim_dimensions: Dimensions {
            width: post_trim_dims.0,
            height: post_trim_dims.1,
        },
        trim_applied,
        upload_to_s3: true,
    }))
}

/// POST /images/preview/processed
///
/// Accepts: multipart with `image` (file). `product_slug` is not required.
/// Pipeline: rembg → optional trim. No S3 upload.
/// Returns: image/png bytes. Headers carry dimension metadata:
///   X-Original-Width, X-Original-Height
///   X-Post-Rembg-Width, X-Post-Rembg-Height
///   X-Post-Trim-Width, X-Post-Trim-Height
///   X-Trim-Applied
#[post("/images/preview/processed")]
pub async fn preview_processed_image(
    state: web::Data<AppState>,
    mut payload: Multipart,
) -> Result<HttpResponse, ActixError> {
    let image_data = parse_image_only(&mut payload).await?;

    let rembg_url = state
        .rembg_url
        .as_deref()
        .ok_or_else(|| actix_web::error::ErrorServiceUnavailable("REMBG_URL not configured"))?;

    let RembgPipelineResult {
        processed_bytes,
        original_dims,
        post_rembg_dims,
        post_trim_dims,
        trim_applied,
    } = process_with_rembg_and_trim(
        &state.http_client,
        rembg_url,
        state.trim_transparent,
        image_data,
    )
    .await?;

    println!(
        "[preview processed] dims={}x{} upload_to_s3=false",
        post_trim_dims.0, post_trim_dims.1
    );

    Ok(HttpResponse::Ok()
        .content_type("image/png")
        .append_header(("X-Original-Width", original_dims.0.to_string()))
        .append_header(("X-Original-Height", original_dims.1.to_string()))
        .append_header(("X-Post-Rembg-Width", post_rembg_dims.0.to_string()))
        .append_header(("X-Post-Rembg-Height", post_rembg_dims.1.to_string()))
        .append_header(("X-Post-Trim-Width", post_trim_dims.0.to_string()))
        .append_header(("X-Post-Trim-Height", post_trim_dims.1.to_string()))
        .append_header(("X-Trim-Applied", trim_applied.to_string()))
        .body(processed_bytes))
}

// ---- Tests ----

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App};
    use aws_sdk_s3::config::{Credentials, Region};
    use image::{DynamicImage, ImageFormat};
    use std::io::Cursor;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

    fn make_test_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(width, height, |_, _| image::Rgba([200, 100, 50, 255]));
        let mut buf = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn build_multipart(boundary: &str, slug: &str, image_bytes: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"product_slug\"\r\n\r\n",
        );
        body.extend_from_slice(slug.as_bytes());
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"image\"; filename=\"test.png\"\r\nContent-Type: image/png\r\n\r\n",
        );
        body.extend_from_slice(image_bytes);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());
        body
    }

    fn build_multipart_image_only(boundary: &str, image_bytes: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"image\"; filename=\"test.png\"\r\nContent-Type: image/png\r\n\r\n",
        );
        body.extend_from_slice(image_bytes);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());
        body
    }

    async fn make_test_state(
        s3_server: &MockServer,
        rembg_server: Option<&MockServer>,
    ) -> web::Data<AppState> {
        let s3_client = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::Config::builder()
                .credentials_provider(Credentials::new("test", "test", None, None, "test"))
                .region(Region::new("us-east-1"))
                .endpoint_url(s3_server.uri())
                .force_path_style(true)
                .behavior_version_latest()
                .build(),
        );
        web::Data::new(AppState {
            s3_client,
            s3_bucket: "test-bucket".into(),
            rembg_url: rembg_server.map(|s| s.uri()),
            trim_transparent: true,
            http_client: reqwest::Client::new(),
        })
    }

    #[actix_web::test]
    async fn test_regular_upload_returns_200_and_uploads_six_variants() {
        let s3 = MockServer::start().await;

        // Accept any PUT (S3 upload) and return 200
        Mock::given(matchers::method("PUT"))
            .respond_with(ResponseTemplate::new(200).set_body_string(""))
            .expect(6) // 3 formats × 2 sizes
            .mount(&s3)
            .await;

        let state = make_test_state(&s3, None).await;
        let app = test::init_service(
            App::new()
                .app_data(state)
                .service(upload_image),
        )
        .await;

        let boundary = "testboundary";
        let body = build_multipart(boundary, "my-product", &make_test_png(20, 20));
        let req = test::TestRequest::post()
            .uri("/upload")
            .insert_header((
                "Content-Type",
                format!("multipart/form-data; boundary={}", boundary),
            ))
            .set_payload(body)
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);

        let body = test::read_body(resp).await;
        assert_eq!(body, "Images uploaded successfully");

        s3.verify().await;
    }

    #[actix_web::test]
    async fn test_regular_upload_missing_fields_returns_400() {
        let s3 = MockServer::start().await;
        let state = make_test_state(&s3, None).await;
        let app = test::init_service(
            App::new()
                .app_data(state)
                .service(upload_image),
        )
        .await;

        let boundary = "testboundary";
        // No product_slug field
        let body = build_multipart_image_only(boundary, &make_test_png(10, 10));
        let req = test::TestRequest::post()
            .uri("/upload")
            .insert_header((
                "Content-Type",
                format!("multipart/form-data; boundary={}", boundary),
            ))
            .set_payload(body)
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 400);
    }

    #[actix_web::test]
    async fn test_processed_upload_calls_rembg_trim_and_s3() {
        let s3 = MockServer::start().await;
        let rembg = MockServer::start().await;

        // rembg returns the same opaque PNG (simulates background-removed result)
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/api/remove"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(make_test_png(20, 20))
                    .insert_header("Content-Type", "image/png"),
            )
            .expect(1)
            .mount(&rembg)
            .await;

        Mock::given(matchers::method("PUT"))
            .respond_with(ResponseTemplate::new(200).set_body_string(""))
            .expect(6)
            .mount(&s3)
            .await;

        let state = make_test_state(&s3, Some(&rembg)).await;
        let app = test::init_service(
            App::new()
                .app_data(state)
                .service(upload_processed_image),
        )
        .await;

        let boundary = "testboundary";
        let body = build_multipart(boundary, "my-product", &make_test_png(20, 20));
        let req = test::TestRequest::post()
            .uri("/images/upload/processed")
            .insert_header((
                "Content-Type",
                format!("multipart/form-data; boundary={}", boundary),
            ))
            .set_payload(body)
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);

        let body_bytes = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["upload_to_s3"], true);

        rembg.verify().await;
        s3.verify().await;
    }

    #[actix_web::test]
    async fn test_preview_calls_rembg_trim_but_no_s3_upload() {
        let s3 = MockServer::start().await;
        let rembg = MockServer::start().await;

        Mock::given(matchers::method("POST"))
            .and(matchers::path("/api/remove"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(make_test_png(20, 20))
                    .insert_header("Content-Type", "image/png"),
            )
            .expect(1)
            .mount(&rembg)
            .await;

        // No S3 mock registered — any PUT would cause the test to fail
        Mock::given(matchers::method("PUT"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&s3)
            .await;

        let state = make_test_state(&s3, Some(&rembg)).await;
        let app = test::init_service(
            App::new()
                .app_data(state)
                .service(preview_processed_image),
        )
        .await;

        let boundary = "testboundary";
        let body = build_multipart_image_only(boundary, &make_test_png(20, 20));
        let req = test::TestRequest::post()
            .uri("/images/preview/processed")
            .insert_header((
                "Content-Type",
                format!("multipart/form-data; boundary={}", boundary),
            ))
            .set_payload(body)
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers().get("Content-Type").unwrap(),
            "image/png"
        );
        assert!(resp.headers().contains_key("X-Post-Trim-Width"));

        rembg.verify().await;
        s3.verify().await;
    }

    #[actix_web::test]
    async fn test_processed_routes_503_without_rembg_url() {
        let s3 = MockServer::start().await;
        // No rembg server URL in state
        let state = make_test_state(&s3, None).await;

        let app = test::init_service(
            App::new()
                .app_data(state)
                .service(upload_processed_image)
                .service(preview_processed_image),
        )
        .await;

        let boundary = "testboundary";
        let img = make_test_png(10, 10);

        for uri in ["/images/upload/processed", "/images/preview/processed"] {
            let body = if uri.contains("upload") {
                build_multipart(boundary, "slug", &img)
            } else {
                build_multipart_image_only(boundary, &img)
            };
            let req = test::TestRequest::post()
                .uri(uri)
                .insert_header((
                    "Content-Type",
                    format!("multipart/form-data; boundary={}", boundary),
                ))
                .set_payload(body)
                .to_request();

            let resp = test::call_service(&app, req).await;
            assert_eq!(resp.status(), 503);
        }
    }
}
