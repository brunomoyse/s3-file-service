// main.rs
use actix_web::{App, HttpServer, web};
use aws_sdk_s3::Client;
use dotenv::dotenv;
use std::env;

mod handlers;
mod image_processing;
mod rembg;
mod s3;

pub struct AppState {
    pub s3_client: Client,
    pub s3_bucket: String,
    pub rembg_url: Option<String>,
    pub trim_transparent: bool,
    pub http_client: reqwest::Client,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();

    let s3_client = s3::init_s3_client().await;
    let s3_bucket = env::var("S3_BUCKET_NAME").expect("S3_BUCKET_NAME must be set");
    let rembg_url = env::var("REMBG_URL").ok();
    let trim_transparent = env::var("IMAGE_TRIM_TRANSPARENT")
        .map(|v| v != "false")
        .unwrap_or(true);

    let app_state = web::Data::new(AppState {
        s3_client,
        s3_bucket,
        rembg_url,
        trim_transparent,
        http_client: reqwest::Client::new(),
    });

    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .service(handlers::upload_image)
            .service(handlers::upload_processed_image)
            .service(handlers::preview_processed_image)
    })
    .bind("0.0.0.0:8000")?
    .run()
    .await
}
