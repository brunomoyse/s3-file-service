use anyhow::anyhow;

/// Calls the rembg HTTP sidecar to remove the background from an image.
/// Sends raw image bytes and expects a PNG with transparency in response.
/// The sidecar must expose POST /api/remove (standard rembg server interface).
pub async fn remove_background(
    client: &reqwest::Client,
    rembg_url: &str,
    image_data: Vec<u8>,
) -> anyhow::Result<Vec<u8>> {
    let part = reqwest::multipart::Part::bytes(image_data)
        .file_name("image.png")
        .mime_str("image/png")?;
    let form = reqwest::multipart::Form::new().part("file", part);

    let response = client
        .post(format!("{}/api/remove", rembg_url))
        .multipart(form)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "rembg service returned HTTP {}",
            response.status()
        ));
    }

    Ok(response.bytes().await?.to_vec())
}
