// main.rs
use actix_web::{App, HttpServer, web};
use aws_sdk_s3::Client;
use dotenv::dotenv;
use std::env;

mod handlers;
mod image_processing;
mod s3;

pub struct AppState {
    pub s3_client: Client,
    pub s3_bucket: String,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();

    let s3_client = s3::init_s3_client().await;
    let s3_bucket = env::var("S3_BUCKET_NAME").expect("S3_BUCKET_NAME must be set");

    let app_state = web::Data::new(AppState {
        s3_client,
        s3_bucket,
    });

    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .service(handlers::upload_image)
    })
    .bind("0.0.0.0:8000")?
    .run()
    .await
}
