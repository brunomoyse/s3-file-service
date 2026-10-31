//! Client for the rembg background-removal sidecar.

use anyhow::{Context, ensure};
use reqwest::multipart::{Form, Part};

/// Asks the sidecar's `POST /api/remove` for a PNG of `image_data` with its background removed.
pub(crate) async fn remove_background(
    client: &reqwest::Client,
    rembg_url: &str,
    image_data: Vec<u8>,
) -> anyhow::Result<Vec<u8>> {
    let part = Part::bytes(image_data)
        .file_name("image.png")
        .mime_str("image/png")?;

    let response = client
        .post(format!("{rembg_url}/api/remove"))
        .multipart(Form::new().part("file", part))
        .send()
        .await
        .context("calling the rembg service")?;

    let status = response.status();
    ensure!(status.is_success(), "rembg service returned HTTP {status}");

    let body = response
        .bytes()
        .await
        .context("reading the rembg response")?;
    Ok(body.to_vec())
}
