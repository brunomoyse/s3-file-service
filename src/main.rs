//! HTTP service that turns uploaded product images into PNG, WebP and AVIF variants on S3.

mod handlers;
mod image_processing;
mod rembg;
mod s3;

use std::env;

use actix_web::{App, HttpServer, web};
use anyhow::Context;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

const BIND_ADDRESS: &str = "0.0.0.0:8000";

pub(crate) struct AppState {
    pub(crate) s3_client: aws_sdk_s3::Client,
    pub(crate) s3_bucket: String,
    pub(crate) rembg_url: Option<String>,
    pub(crate) trim_transparent: bool,
    pub(crate) http_client: reqwest::Client,
}

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    // The file is a local convenience: deployed instances get real environment variables.
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();

    let app_state = web::Data::new(AppState {
        s3_client: s3::Settings::from_env()?.into_client(),
        s3_bucket: env::var("S3_BUCKET_NAME").context("S3_BUCKET_NAME must be set")?,
        rembg_url: env::var("REMBG_URL").ok(),
        trim_transparent: env::var("IMAGE_TRIM_TRANSPARENT").map_or(true, |value| value != "false"),
        http_client: reqwest::Client::new(),
    });

    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .service(handlers::upload_image)
            .service(handlers::upload_processed_image)
            .service(handlers::preview_processed_image)
    })
    .bind(BIND_ADDRESS)
    .with_context(|| format!("binding {BIND_ADDRESS}"))?
    .run()
    .await
    .context("running the HTTP server")
}
