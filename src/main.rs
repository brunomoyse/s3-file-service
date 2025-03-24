// main.rs
use actix_web::{App, HttpServer};
use dotenv::dotenv;

mod aws;
mod handlers;
mod image_processing;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();
    HttpServer::new(|| App::new().service(handlers::upload_image))
        .bind("0.0.0.0:8000")?
        .run()
        .await
}
