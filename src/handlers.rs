use crate::aws;
use crate::image_processing;
use actix_multipart::Multipart;
use actix_web::{Error as ActixError, HttpResponse, post, web};
use futures::{StreamExt, TryStreamExt, try_join};
use std::env;

#[post("/upload")]
pub async fn upload_image(mut payload: Multipart) -> Result<HttpResponse, ActixError> {
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

    let bucket =
        env::var("AWS_S3_BUCKET_NAME").map_err(actix_web::error::ErrorInternalServerError)?;
    let s3_client = aws::init_s3_client().await;

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

            aws::upload_file(&s3, &bucket, &format!("images/{}.{}", slug, fmt), data)
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

            aws::upload_file(
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
