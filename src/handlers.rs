use crate::aws;
use crate::image_processing;
use actix_multipart::Multipart;
use actix_web::{Error as ActixError, HttpResponse, post, web};
use bytes::BytesMut;
use futures::{StreamExt, TryStreamExt};
use std::env;

#[post("/upload")]
pub async fn upload_image(mut payload: Multipart) -> Result<HttpResponse, ActixError> {
    // Retrieve product_slug and image data from multipart form.
    let mut product_slug = String::new();
    let mut image_data: Option<Vec<u8>> = None;

    while let Ok(Some(mut field)) = payload.try_next().await {
        let disposition = field.content_disposition();
        if let Some(disposition) = disposition {
            if let Some(name) = disposition.get_name() {
                match name {
                    "product_slug" => {
                        let mut bytes = BytesMut::new();
                        while let Some(chunk) = field.next().await {
                            let data = chunk?;
                            bytes.extend_from_slice(&data);
                        }
                        product_slug = String::from_utf8(bytes.to_vec()).unwrap_or_default();
                    }
                    "image" => {
                        let mut bytes = BytesMut::new();
                        while let Some(chunk) = field.next().await {
                            let data = chunk?;
                            bytes.extend_from_slice(&data);
                        }
                        image_data = Some(bytes.to_vec());
                    }
                    _ => {}
                }
            }
        }
    }

    if product_slug.is_empty() || image_data.is_none() {
        return Ok(HttpResponse::BadRequest().body("Missing product_slug or image"));
    }
    let image_data = image_data.unwrap();

    let bucket = env::var("AWS_S3_BUCKET_NAME").expect("AWS_S3_BUCKET_NAME must be set");
    let s3_client = aws::init_s3_client().await;

    let formats = vec!["avif", "webp", "png"];
    let time_suffix = ""; // You can adjust if you need to add a time suffix

    for fmt in formats {
        let fmt_str = fmt.to_string();
        let file_name = format!("{}{}.{fmt}", product_slug, time_suffix);

        // Process the normal size image (600px width)
        let data_normal = {
            let img_data = image_data.clone();
            let fmt_clone = fmt_str.clone();
            web::block(move || -> Result<Vec<u8>, anyhow::Error> {
                let resized = image_processing::resize_image(&img_data, 600)?;
                match fmt_clone.as_str() {
                    "png" => image_processing::encode_to_png(&resized),
                    "webp" => image_processing::encode_to_webp(&resized, 75.0),
                    "avif" => image_processing::encode_to_avif(&resized, 75.0),
                    _ => Err(anyhow::anyhow!("Unsupported format")),
                }
            })
            .await
            .map_err(actix_web::error::BlockingError::from)?
        };

        // Upload normal image to S3 (e.g., key: images/<filename>)
        let key_normal = format!("images/{}", file_name);
        aws::upload_file(
            &s3_client,
            &bucket,
            &key_normal,
            data_normal.map_err(|e| actix_web::error::ErrorInternalServerError(e.to_string()))?,
        )
        .await
        .map_err(|e| actix_web::error::ErrorInternalServerError(e))?;

        // Process the thumbnail image (350px width)
        let data_thumb = {
            let img_data = image_data.clone();
            let fmt_clone = fmt_str.clone();
            web::block(move || -> Result<Vec<u8>, anyhow::Error> {
                let resized = image_processing::resize_image(&img_data, 350)?;
                match fmt_clone.as_str() {
                    "png" => image_processing::encode_to_png(&resized),
                    "webp" => image_processing::encode_to_webp(&resized, 75.0),
                    "avif" => image_processing::encode_to_avif(&resized, 75.0),
                    _ => Err(anyhow::anyhow!("Unsupported format")),
                }
            })
            .await
            .map_err(actix_web::error::BlockingError::from)?
        };

        // Upload thumbnail to S3 (e.g., key: images/thumbnails/<filename>)
        let key_thumb = format!("images/thumbnails/{}", file_name);
        aws::upload_file(
            &s3_client,
            &bucket,
            &key_thumb,
            data_thumb.map_err(|e| actix_web::error::ErrorInternalServerError(e.to_string()))?,
        )
        .await
        .map_err(|e| actix_web::error::ErrorInternalServerError(e))?;
    }

    Ok(HttpResponse::Ok().body("Image and thumbnails saved in multiple formats"))
}
