use anyhow::Result;
use image::{DynamicImage, GenericImageView, ImageFormat};
use ravif::{Encoder, Img};
use rgb::RGBA;
use std::io::Cursor;
use webp::Encoder as WebpEncoder;

/// Resizes the image data (in memory) to the given target width while preserving aspect ratio
pub fn resize_image(data: &[u8], target_width: u32) -> Result<DynamicImage> {
    let img = image::load_from_memory(data)?;
    let (width, height) = img.dimensions();
    let aspect_ratio = height as f32 / width as f32;
    let target_height = (target_width as f32 * aspect_ratio).round() as u32;
    let resized = img.resize_exact(
        target_width,
        target_height,
        image::imageops::FilterType::Lanczos3,
    );
    Ok(resized)
}

/// Encodes the image as PNG
pub fn encode_to_png(img: &DynamicImage) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)?;
    Ok(buf.into_inner())
}

/// Encodes the image as WebP using specified quality (0.0-100.0)
pub fn encode_to_webp(img: &DynamicImage, quality: f32) -> Result<Vec<u8>> {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    let encoder = WebpEncoder::from_rgba(rgba.as_raw(), width, height);
    let webp_data = encoder.encode(quality);
    Ok(webp_data.to_vec())
}

/// Encodes the image as AVIF using specified quality (0.0-100.0)
pub fn encode_to_avif(img: &DynamicImage, quality: f32) -> Result<Vec<u8>> {
    // Convert the image to RGBA8 format
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    // Convert the raw pixel data into a correctly structured slice
    let pixels: &[RGBA<u8>] = bytemuck::cast_slice(rgba.as_raw());

    // Create an Img instance from the raw pixel data
    let img_data = Img::new(pixels, width as usize, height as usize);

    // Initialize the AVIF encoder with the specified quality and speed settings
    let encoder = Encoder::new().with_quality(quality).with_speed(6); // Speed ranges from 0 (best quality) to 10 (fastest)

    // Encode the image data to AVIF format
    let encoded_avif = encoder.encode_rgba(img_data)?;

    // Return the encoded AVIF data
    Ok(encoded_avif.avif_file)
}
