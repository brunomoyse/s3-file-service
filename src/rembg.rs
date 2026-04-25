use anyhow::anyhow;

/// Calls the rembg HTTP sidecar to remove the background from an image.
/// Sends raw image bytes and expects a PNG with transparency in response.
/// The sidecar must expose POST /api/remove (standard rembg server interface).
pub async fn remove_background(
    client: &reqwest::Client,
    rembg_url: &str,
    image_data: Vec<u8>,
) -> anyhow::Result<Vec<u8>> {
    let response = client
        .post(format!("{}/api/remove", rembg_url))
        .header("Content-Type", "image/png")
        .body(image_data)
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
