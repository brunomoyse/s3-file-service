use actix_web::{App, HttpServer};
use dotenv::dotenv;

mod aws;
mod handlers;
mod image_processing;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();

    HttpServer::new(|| App::new().service(handlers::upload_image))
        .bind("127.0.0.1:8080")?
        .run()
        .await
}
