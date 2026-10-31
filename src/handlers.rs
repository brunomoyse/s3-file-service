//! HTTP routes for uploading and previewing product images.

use std::fmt::{Debug, Display};
use std::sync::Arc;

use actix_multipart::{Field, Multipart, MultipartError};
use actix_web::error::{ErrorBadRequest, ErrorInternalServerError, ErrorServiceUnavailable};
use actix_web::{Error, HttpResponse, post, web};
use bytes::Bytes;
use futures::{TryStreamExt, future, try_join};
use image::{DynamicImage, ImageError};
use serde::Serialize;
use tracing::{error, info};

use crate::AppState;
use crate::image_processing::{self, Dimensions, OutputFormat};
use crate::{rembg, s3};

/// Width of the `images/{slug}.{ext}` variants, in pixels.
const REGULAR_WIDTH: u32 = 600;

/// Width of the `images/thumbnails/{slug}.{ext}` variants, in pixels.
const THUMBNAIL_WIDTH: u32 = 350;

/// Logs a failure with its full cause chain and turns it into a 500 response.
fn internal(cause: impl Debug + Display + 'static) -> Error {
    error!("{cause:#}");
    ErrorInternalServerError(cause)
}

#[derive(Default)]
struct UploadForm {
    slug: Vec<u8>,
    image: Vec<u8>,
}

impl UploadForm {
    async fn read(payload: &mut Multipart) -> Result<Self, Error> {
        let mut form = Self::default();
        while let Some(mut field) = payload.try_next().await? {
            match field.content_disposition().and_then(|cd| cd.get_name()) {
                Some("product_slug") => form.slug = read_field(&mut field).await?,
                Some("image") => form.image.extend(read_field(&mut field).await?),
                _ => {}
            }
        }
        Ok(form)
    }

    fn into_slug_and_image(self) -> Result<(String, Vec<u8>), Error> {
        if self.slug.is_empty() || self.image.is_empty() {
            return Err(ErrorBadRequest("Missing product_slug or image"));
        }
        let slug = String::from_utf8(self.slug).map_err(ErrorBadRequest)?;
        Ok((slug, self.image))
    }

    fn into_image(self) -> Result<Vec<u8>, Error> {
        if self.image.is_empty() {
            return Err(ErrorBadRequest("Missing image"));
        }
        Ok(self.image)
    }
}

async fn read_field(field: &mut Field) -> Result<Vec<u8>, MultipartError> {
    let mut content = Vec::new();
    while let Some(chunk) = field.try_next().await? {
        content.extend_from_slice(&chunk);
    }
    Ok(content)
}

/// Resizes `image` to both widths, encodes each in every format and uploads all six variants.
async fn store_variants(state: &AppState, slug: &str, image: Vec<u8>) -> Result<(), Error> {
    let image = Bytes::from(image);
    let thumbnail_source = image.clone();
    let (regular, thumbnail) = try_join!(
        web::block(move || image_processing::resize_image(&image, REGULAR_WIDTH)),
        web::block(move || image_processing::resize_image(&thumbnail_source, THUMBNAIL_WIDTH)),
    )?;
    let regular = Arc::new(regular.map_err(internal)?);
    let thumbnail = Arc::new(thumbnail.map_err(internal)?);

    let uploads = OutputFormat::ALL.map(|format| {
        let regular = Arc::clone(&regular);
        let thumbnail = Arc::clone(&thumbnail);
        let extension = format.extension();
        async move {
            let key = format!("images/{slug}.{extension}");
            encode_and_upload(state, &key, regular, format).await?;
            let key = format!("images/thumbnails/{slug}.{extension}");
            encode_and_upload(state, &key, thumbnail, format).await
        }
    });
    // Waits for every format even after a failure, so no upload is cancelled midway.
    future::join_all(uploads).await.into_iter().collect()
}

async fn encode_and_upload(
    state: &AppState,
    key: &str,
    image: Arc<DynamicImage>,
    format: OutputFormat,
) -> Result<(), Error> {
    let data = web::block(move || format.encode(&image))
        .await?
        .map_err(internal)?;
    s3::upload_file(&state.s3_client, &state.s3_bucket, key, data)
        .await
        .map_err(internal)
}

struct ProcessedImage {
    bytes: Vec<u8>,
    original: Dimensions,
    post_rembg: Dimensions,
    post_trim: Dimensions,
    trim_applied: bool,
}

/// Runs rembg background removal, then trims transparent borders if the instance is set to.
async fn remove_background_and_trim(
    state: &AppState,
    image: Vec<u8>,
) -> Result<ProcessedImage, Error> {
    let rembg_url = state
        .rembg_url
        .as_deref()
        .ok_or_else(|| ErrorServiceUnavailable("REMBG_URL not configured"))?;

    let (image, original) = web::block(move || {
        let original = image_processing::dimensions(&image);
        (image, original)
    })
    .await?;
    let original = original.map_err(ErrorBadRequest)?;

    let rembg_output = rembg::remove_background(&state.http_client, rembg_url, image)
        .await
        .map_err(internal)?;

    let trim_transparent = state.trim_transparent;
    let (bytes, post_rembg, post_trim) = web::block(move || {
        let post_rembg = image_processing::dimensions(&rembg_output)?;
        if !trim_transparent {
            return Ok((rembg_output, post_rembg, post_rembg));
        }
        let trimmed = image_processing::trim_transparent_borders(rembg_output)?;
        let post_trim = image_processing::dimensions(&trimmed)?;
        Ok::<_, ImageError>((trimmed, post_rembg, post_trim))
    })
    .await?
    .map_err(internal)?;

    let trim_applied = post_trim != post_rembg;
    info!(
        ?original,
        ?post_rembg,
        ?post_trim,
        trim_applied,
        "rembg pipeline finished"
    );

    Ok(ProcessedImage {
        bytes,
        original,
        post_rembg,
        post_trim,
        trim_applied,
    })
}

/// `POST /upload`: stores the six variants of the `image` field under `product_slug`.
#[post("/upload")]
pub(crate) async fn upload_image(
    state: web::Data<AppState>,
    mut payload: Multipart,
) -> Result<HttpResponse, Error> {
    let (slug, image) = UploadForm::read(&mut payload)
        .await?
        .into_slug_and_image()?;

    store_variants(&state, &slug, image).await?;

    Ok(HttpResponse::Ok().body("Images uploaded successfully"))
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

/// `POST /images/upload/processed`: removes the background of the `image` field, trims it and
/// stores the six variants under `product_slug`. Responds with the dimensions at each step.
#[post("/images/upload/processed")]
pub(crate) async fn upload_processed_image(
    state: web::Data<AppState>,
    mut payload: Multipart,
) -> Result<HttpResponse, Error> {
    let (slug, image) = UploadForm::read(&mut payload)
        .await?
        .into_slug_and_image()?;

    let processed = remove_background_and_trim(&state, image).await?;
    store_variants(&state, &slug, processed.bytes).await?;
    info!(slug, "stored processed image");

    Ok(HttpResponse::Ok().json(ProcessedUploadResponse {
        status: "ok",
        original_dimensions: processed.original,
        post_rembg_dimensions: processed.post_rembg,
        post_trim_dimensions: processed.post_trim,
        trim_applied: processed.trim_applied,
        upload_to_s3: true,
    }))
}

/// `POST /images/preview/processed`: runs the same pipeline without storing anything. Responds
/// with the PNG and the dimensions at each step in `X-Original-*`, `X-Post-Rembg-*`,
/// `X-Post-Trim-*` and `X-Trim-Applied` headers.
#[post("/images/preview/processed")]
pub(crate) async fn preview_processed_image(
    state: web::Data<AppState>,
    mut payload: Multipart,
) -> Result<HttpResponse, Error> {
    let image = UploadForm::read(&mut payload).await?.into_image()?;

    let processed = remove_background_and_trim(&state, image).await?;

    Ok(HttpResponse::Ok()
        .content_type("image/png")
        .append_header(("X-Original-Width", processed.original.width.to_string()))
        .append_header(("X-Original-Height", processed.original.height.to_string()))
        .append_header(("X-Post-Rembg-Width", processed.post_rembg.width.to_string()))
        .append_header((
            "X-Post-Rembg-Height",
            processed.post_rembg.height.to_string(),
        ))
        .append_header(("X-Post-Trim-Width", processed.post_trim.width.to_string()))
        .append_header(("X-Post-Trim-Height", processed.post_trim.height.to_string()))
        .append_header(("X-Trim-Applied", processed.trim_applied.to_string()))
        .body(processed.bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::dev::ServiceResponse;
    use actix_web::error::PayloadError;
    use actix_web::http::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
    use actix_web::{App, test};
    use aws_sdk_s3::config::{Credentials, Region};
    use futures::StreamExt;
    use image::{GenericImageView, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

    const BOUNDARY: &str = "testboundary";

    fn png_from(image: &RgbaImage) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        image.write_to(&mut buf, ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    fn opaque_png(width: u32, height: u32) -> Vec<u8> {
        png_from(&RgbaImage::from_pixel(
            width,
            height,
            Rgba([200, 100, 50, 255]),
        ))
    }

    /// A 5x5 PNG whose only visible pixel is the center one, like a rembg cutout.
    fn cutout_png() -> Vec<u8> {
        let mut image = RgbaImage::new(5, 5);
        image.put_pixel(2, 2, Rgba([255, 0, 0, 255]));
        png_from(&image)
    }

    fn multipart_body(slug: Option<&str>, image: Option<&[u8]>) -> Vec<u8> {
        let mut body = Vec::new();
        if let Some(slug) = slug {
            body.extend_from_slice(
                format!(
                    "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"product_slug\"\r\n\r\n{slug}\r\n"
                )
                .as_bytes(),
            );
        }
        if let Some(image) = image {
            body.extend_from_slice(
                format!(
                    "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"test.png\"\r\nContent-Type: image/png\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(image);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        body
    }

    fn app_state(s3_server: &MockServer, rembg_server: Option<&MockServer>) -> AppState {
        let s3_client = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::Config::builder()
                .credentials_provider(Credentials::new("test", "test", None, None, "test"))
                .region(Region::new("us-east-1"))
                .endpoint_url(s3_server.uri())
                .force_path_style(true)
                .behavior_version_latest()
                .build(),
        );
        AppState {
            s3_client,
            s3_bucket: "test-bucket".into(),
            rembg_url: rembg_server.map(MockServer::uri),
            trim_transparent: true,
            http_client: reqwest::Client::new(),
        }
    }

    async fn post_multipart(state: AppState, uri: &str, body: Vec<u8>) -> ServiceResponse {
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .service(upload_image)
                .service(upload_processed_image)
                .service(preview_processed_image),
        )
        .await;
        let request = test::TestRequest::post()
            .uri(uri)
            .insert_header((
                CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            ))
            .set_payload(body)
            .to_request();
        test::call_service(&app, request).await
    }

    async fn mock_s3(expected_puts: u64) -> MockServer {
        let s3 = MockServer::start().await;
        Mock::given(matchers::method("PUT"))
            .respond_with(ResponseTemplate::new(200))
            .expect(expected_puts)
            .mount(&s3)
            .await;
        s3
    }

    async fn mock_rembg(response: ResponseTemplate, expected_calls: u64) -> MockServer {
        let rembg = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/api/remove"))
            .respond_with(response)
            .expect(expected_calls)
            .mount(&rembg)
            .await;
        rembg
    }

    fn png_response(bytes: Vec<u8>) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .set_body_bytes(bytes)
            .insert_header("Content-Type", "image/png")
    }

    async fn stored_keys(s3: &MockServer) -> Vec<String> {
        let mut keys: Vec<String> = s3
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.url.path().to_owned())
            .collect();
        keys.sort();
        keys
    }

    fn all_variant_keys(slug: &str) -> Vec<String> {
        let mut keys: Vec<String> = ["avif", "png", "webp"]
            .iter()
            .flat_map(|ext| {
                [
                    format!("/test-bucket/images/{slug}.{ext}"),
                    format!("/test-bucket/images/thumbnails/{slug}.{ext}"),
                ]
            })
            .collect();
        keys.sort();
        keys
    }

    fn header<'resp>(response: &'resp ServiceResponse, name: &str) -> &'resp str {
        response.headers().get(name).unwrap().to_str().unwrap()
    }

    #[actix_web::test]
    async fn upload_stores_all_six_variants_under_their_keys() {
        let s3 = mock_s3(6).await;
        let body = multipart_body(Some("my-product"), Some(&opaque_png(20, 20)));

        let response = post_multipart(app_state(&s3, None), "/upload", body).await;

        assert_eq!(response.status(), 200);
        assert_eq!(
            test::read_body(response).await,
            "Images uploaded successfully"
        );
        assert_eq!(stored_keys(&s3).await, all_variant_keys("my-product"));
    }

    #[actix_web::test]
    async fn upload_rejects_a_form_missing_a_field() {
        let image = opaque_png(10, 10);
        let forms = [
            multipart_body(None, Some(&image)),
            multipart_body(Some("my-product"), None),
        ];
        for body in forms {
            let s3 = mock_s3(0).await;

            let response = post_multipart(app_state(&s3, None), "/upload", body).await;

            assert_eq!(response.status(), 400);
            assert_eq!(
                test::read_body(response).await,
                "Missing product_slug or image"
            );
        }
    }

    #[actix_web::test]
    async fn upload_of_an_undecodable_image_is_a_server_error() {
        let s3 = mock_s3(0).await;
        let body = multipart_body(Some("my-product"), Some(b"not an image"));

        let response = post_multipart(app_state(&s3, None), "/upload", body).await;

        assert_eq!(response.status(), 500);
    }

    #[actix_web::test]
    async fn upload_reports_an_s3_failure() {
        let s3 = MockServer::start().await;
        Mock::given(matchers::method("PUT"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&s3)
            .await;
        let body = multipart_body(Some("my-product"), Some(&opaque_png(10, 10)));

        let response = post_multipart(app_state(&s3, None), "/upload", body).await;

        assert_eq!(response.status(), 500);
    }

    #[actix_web::test]
    async fn processed_upload_reports_dimensions_and_stores_variants() {
        let s3 = mock_s3(6).await;
        let rembg = mock_rembg(png_response(cutout_png()), 1).await;
        let body = multipart_body(Some("my-product"), Some(&opaque_png(20, 10)));

        let response = post_multipart(
            app_state(&s3, Some(&rembg)),
            "/images/upload/processed",
            body,
        )
        .await;

        assert_eq!(response.status(), 200);
        let json: serde_json::Value =
            serde_json::from_slice(&test::read_body(response).await).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "status": "ok",
                "original_dimensions": {"width": 20, "height": 10},
                "post_rembg_dimensions": {"width": 5, "height": 5},
                "post_trim_dimensions": {"width": 1, "height": 1},
                "trim_applied": true,
                "upload_to_s3": true,
            })
        );
        assert_eq!(stored_keys(&s3).await, all_variant_keys("my-product"));
        rembg.verify().await;
    }

    #[actix_web::test]
    async fn processed_upload_rejects_an_undecodable_image_before_calling_rembg() {
        let s3 = mock_s3(0).await;
        let rembg = mock_rembg(png_response(cutout_png()), 0).await;
        let body = multipart_body(Some("my-product"), Some(b"not an image"));

        let response = post_multipart(
            app_state(&s3, Some(&rembg)),
            "/images/upload/processed",
            body,
        )
        .await;

        assert_eq!(response.status(), 400);
        rembg.verify().await;
    }

    #[actix_web::test]
    async fn processed_upload_reports_a_rembg_failure() {
        let s3 = mock_s3(0).await;
        let rembg = mock_rembg(ResponseTemplate::new(502), 1).await;
        let body = multipart_body(Some("my-product"), Some(&opaque_png(10, 10)));

        let response = post_multipart(
            app_state(&s3, Some(&rembg)),
            "/images/upload/processed",
            body,
        )
        .await;

        assert_eq!(response.status(), 500);
        assert_eq!(
            test::read_body(response).await,
            "rembg service returned HTTP 502 Bad Gateway"
        );
    }

    #[actix_web::test]
    async fn processed_upload_reports_undecodable_rembg_output() {
        let s3 = mock_s3(0).await;
        let rembg = mock_rembg(png_response(b"garbage".to_vec()), 1).await;
        let body = multipart_body(Some("my-product"), Some(&opaque_png(10, 10)));

        let response = post_multipart(
            app_state(&s3, Some(&rembg)),
            "/images/upload/processed",
            body,
        )
        .await;

        assert_eq!(response.status(), 500);
    }

    #[actix_web::test]
    async fn preview_returns_the_trimmed_png_and_dimension_headers_without_storing() {
        let s3 = mock_s3(0).await;
        let rembg = mock_rembg(png_response(cutout_png()), 1).await;
        let body = multipart_body(None, Some(&opaque_png(20, 10)));

        let response = post_multipart(
            app_state(&s3, Some(&rembg)),
            "/images/preview/processed",
            body,
        )
        .await;

        assert_eq!(response.status(), 200);
        assert_eq!(header(&response, "Content-Type"), "image/png");
        assert_eq!(header(&response, "X-Original-Width"), "20");
        assert_eq!(header(&response, "X-Original-Height"), "10");
        assert_eq!(header(&response, "X-Post-Rembg-Width"), "5");
        assert_eq!(header(&response, "X-Post-Rembg-Height"), "5");
        assert_eq!(header(&response, "X-Post-Trim-Width"), "1");
        assert_eq!(header(&response, "X-Post-Trim-Height"), "1");
        assert_eq!(header(&response, "X-Trim-Applied"), "true");
        let png = test::read_body(response).await;
        assert_eq!(image::load_from_memory(&png).unwrap().dimensions(), (1, 1));
        rembg.verify().await;
    }

    #[actix_web::test]
    async fn preview_passes_rembg_output_through_when_trimming_is_disabled() {
        let s3 = mock_s3(0).await;
        let cutout = cutout_png();
        let rembg = mock_rembg(png_response(cutout.clone()), 1).await;
        let mut state = app_state(&s3, Some(&rembg));
        state.trim_transparent = false;
        let body = multipart_body(None, Some(&opaque_png(20, 10)));

        let response = post_multipart(state, "/images/preview/processed", body).await;

        assert_eq!(response.status(), 200);
        assert_eq!(header(&response, "X-Post-Trim-Width"), "5");
        assert_eq!(header(&response, "X-Trim-Applied"), "false");
        assert_eq!(test::read_body(response).await, cutout);
    }

    #[actix_web::test]
    async fn preview_rejects_a_form_without_an_image() {
        let s3 = mock_s3(0).await;
        let rembg = mock_rembg(png_response(cutout_png()), 0).await;
        let body = multipart_body(Some("my-product"), None);

        let response = post_multipart(
            app_state(&s3, Some(&rembg)),
            "/images/preview/processed",
            body,
        )
        .await;

        assert_eq!(response.status(), 400);
        assert_eq!(test::read_body(response).await, "Missing image");
    }

    #[actix_web::test]
    async fn processed_routes_are_unavailable_without_rembg() {
        let image = opaque_png(10, 10);
        let requests = [
            (
                "/images/upload/processed",
                multipart_body(Some("slug"), Some(&image)),
            ),
            (
                "/images/preview/processed",
                multipart_body(None, Some(&image)),
            ),
        ];
        for (uri, body) in requests {
            let s3 = mock_s3(0).await;

            let response = post_multipart(app_state(&s3, None), uri, body).await;

            assert_eq!(response.status(), 503, "{uri}");
        }
    }

    #[actix_web::test]
    async fn form_fields_split_across_chunks_are_reassembled() {
        let body = multipart_body(Some("smoked-salmon-roll"), Some(&opaque_png(2, 2)));
        let chunks: Vec<Result<Bytes, PayloadError>> = body
            .chunks(3)
            .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
            .collect();
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(&format!("multipart/form-data; boundary={BOUNDARY}")).unwrap(),
        );
        // Yield between chunks so the parser sees them arrive separately, as from a socket.
        let stream = futures::stream::iter(chunks).then(|chunk| async move {
            actix_web::rt::task::yield_now().await;
            chunk
        });
        let mut payload = Multipart::new(&headers, stream);

        let form = UploadForm::read(&mut payload).await.unwrap();

        assert_eq!(form.slug, b"smoked-salmon-roll");
        assert_eq!(form.image, opaque_png(2, 2));
    }
}
