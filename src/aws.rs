use aws_config::meta::region::RegionProviderChain;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::{Client, Error};

/// Initializes the AWS S3 client.
pub async fn init_s3_client() -> Client {
    let region_provider = RegionProviderChain::default_provider().or_else("eu-west-3");
    let config = aws_config::from_env().region(region_provider).load().await;
    Client::new(&config)
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
