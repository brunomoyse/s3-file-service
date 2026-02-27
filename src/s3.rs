use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::{Client, Error};
use std::env;

/// Initializes the S3 client based on the S3_PROVIDER env var ("aws" or "ovh").
pub async fn init_s3_client() -> Client {
    let provider = env::var("S3_PROVIDER").unwrap_or_else(|_| "aws".to_string());
    let access_key = env::var("S3_ACCESS_KEY_ID").expect("S3_ACCESS_KEY_ID must be set");
    let secret_key = env::var("S3_SECRET_ACCESS_KEY").expect("S3_SECRET_ACCESS_KEY must be set");

    let credentials = Credentials::new(access_key, secret_key, None, None, "env");

    match provider.as_str() {
        "ovh" => {
            let region = env::var("S3_REGION").unwrap_or_else(|_| "gra".to_string());
            let endpoint = env::var("S3_ENDPOINT_URL")
                .unwrap_or_else(|_| format!("https://s3.{}.io.cloud.ovh.net", region));
            let force_path_style = env::var("S3_FORCE_PATH_STYLE")
                .map(|v| v == "true")
                .unwrap_or(true);

            let config = aws_sdk_s3::Config::builder()
                .credentials_provider(credentials)
                .region(Region::new(region))
                .endpoint_url(endpoint)
                .force_path_style(force_path_style)
                .behavior_version_latest()
                .build();

            Client::from_conf(config)
        }
        _ => {
            let region = env::var("S3_REGION").unwrap_or_else(|_| "eu-west-3".to_string());

            let mut builder = aws_sdk_s3::Config::builder()
                .credentials_provider(credentials)
                .region(Region::new(region))
                .behavior_version_latest();

            if let Ok(endpoint) = env::var("S3_ENDPOINT_URL") {
                builder = builder.endpoint_url(endpoint);
            }

            if env::var("S3_FORCE_PATH_STYLE").as_deref() == Ok("true") {
                builder = builder.force_path_style(true);
            }

            Client::from_conf(builder.build())
        }
    }
}

/// Uploads a file (given as bytes) to the specified bucket/key.
pub async fn upload_file(
    client: &Client,
    bucket: &str,
    key: &str,
    data: Vec<u8>,
) -> Result<(), Error> {
    let byte_stream = ByteStream::from(data);
    let resp = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(byte_stream)
        .send()
        .await?;

    println!("Upload response for {}: {:?}", key, resp);
    Ok(())
}
