// image_processing.rs
use image::{DynamicImage, GenericImageView, ImageError, ImageFormat, imageops::FilterType};
use ravif::{Encoder, Img};
use std::io::Cursor;
use webp::Encoder as WebpEncoder;

pub fn resize_image(data: &[u8], width: u32) -> Result<DynamicImage, image::ImageError> {
    let img = image::load_from_memory(data)?;
    let (orig_w, orig_h) = img.dimensions();
    let height = (width as f32 * orig_h as f32 / orig_w as f32).round() as u32;
    Ok(img.resize_exact(width, height, FilterType::Lanczos3))
}

pub fn encode_to_png(img: &DynamicImage) -> Result<Vec<u8>, image::ImageError> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)?;
    Ok(buf.into_inner())
}

pub fn encode_to_webp(img: &DynamicImage, quality: f32) -> Result<Vec<u8>, image::ImageError> {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(WebpEncoder::from_rgba(rgba.as_raw(), w, h)
        .encode(quality)
        .to_vec())
}

pub fn encode_to_avif(img: &DynamicImage, quality: f32) -> Result<Vec<u8>, ImageError> {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let img = Img::new(bytemuck::cast_slice(rgba.as_raw()), w as usize, h as usize);
    Encoder::new()
        .with_quality(quality)
        .with_speed(6)
        .encode_rgba(img)
        .map(|res| res.avif_file)
        .map_err(|e| image::ImageError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e)))
}
