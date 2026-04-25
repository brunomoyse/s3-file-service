// image_processing.rs
use image::{DynamicImage, GenericImageView, ImageError, ImageFormat, imageops::FilterType};
use ravif::{Encoder, Img};
use std::io::Cursor;
use webp::Encoder as WebpEncoder;

// ---- Error type ----

#[derive(Debug)]
pub enum ImageProcessError {
    ImageError(ImageError),
}

impl std::fmt::Display for ImageProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageProcessError::ImageError(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for ImageProcessError {}

impl From<ImageError> for ImageProcessError {
    fn from(e: ImageError) -> Self {
        ImageProcessError::ImageError(e)
    }
}

// ---- Existing functions (unchanged) ----

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
        .map_err(|e| image::ImageError::IoError(std::io::Error::other(e)))
}

// ---- New utilities ----

/// Returns (width, height) from raw image bytes without fully decoding.
pub fn image_dimensions(data: &[u8]) -> Result<(u32, u32), ImageError> {
    Ok(image::load_from_memory(data)?.dimensions())
}

/// Crops transparent border rows/columns from a PNG.
/// - Preserves alpha channel.
/// - Fully transparent image: returns unchanged bytes, no panic.
/// - No transparent border: returns unchanged bytes.
pub fn trim_transparent_borders(png_bytes: &[u8]) -> Result<Vec<u8>, ImageProcessError> {
    let img = image::load_from_memory(png_bytes)?.to_rgba8();
    let (width, height) = img.dimensions();

    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut found = false;

    for (x, y, pixel) in img.enumerate_pixels() {
        if pixel[3] > 0 {
            if !found {
                min_x = x;
                max_x = x;
                min_y = y;
                max_y = y;
                found = true;
            } else {
                if x < min_x {
                    min_x = x;
                }
                if x > max_x {
                    max_x = x;
                }
                if y < min_y {
                    min_y = y;
                }
                if y > max_y {
                    max_y = y;
                }
            }
        }
    }

    // Fully transparent: return original unchanged
    if !found {
        return Ok(png_bytes.to_vec());
    }

    // Bounding box already fills the whole image: nothing to trim
    if min_x == 0 && min_y == 0 && max_x == width - 1 && max_y == height - 1 {
        return Ok(png_bytes.to_vec());
    }

    let crop_w = max_x - min_x + 1;
    let crop_h = max_y - min_y + 1;
    let cropped =
        image::imageops::crop_imm(&img, min_x, min_y, crop_w, crop_h).to_image();
    encode_to_png(&DynamicImage::ImageRgba8(cropped)).map_err(Into::into)
}

// ---- Tests ----

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rgba_png(width: u32, height: u32, pixels: Vec<[u8; 4]>) -> Vec<u8> {
        assert_eq!(pixels.len() as u32, width * height);
        let raw: Vec<u8> = pixels.into_iter().flatten().collect();
        let img = image::RgbaImage::from_raw(width, height, raw).unwrap();
        let mut buf = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    #[test]
    fn trim_no_border_returns_original_dimensions() {
        // 3×3 fully opaque: bounding box == full image, should not crop
        let png = make_rgba_png(3, 3, vec![[200, 100, 50, 255]; 9]);
        let result = trim_transparent_borders(&png).unwrap();
        let out = image::load_from_memory(&result).unwrap();
        assert_eq!(out.dimensions(), (3, 3));
    }

    #[test]
    fn trim_transparent_border_crops_to_content() {
        // 5×5: only the center pixel (2,2) is opaque
        let mut pixels = vec![[0u8, 0, 0, 0]; 25];
        pixels[2 * 5 + 2] = [255, 0, 0, 255];
        let png = make_rgba_png(5, 5, pixels);
        let result = trim_transparent_borders(&png).unwrap();
        let out = image::load_from_memory(&result).unwrap().to_rgba8();
        assert_eq!(out.dimensions(), (1, 1));
        // The sole pixel must retain the original color
        assert_eq!(out.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    #[test]
    fn trim_fully_transparent_returns_unchanged() {
        // All transparent: must not panic and must return the original dimensions
        let png = make_rgba_png(4, 4, vec![[0u8, 0, 0, 0]; 16]);
        let result = trim_transparent_borders(&png).unwrap();
        let out = image::load_from_memory(&result).unwrap();
        assert_eq!(out.dimensions(), (4, 4));
    }

    #[test]
    fn trim_partial_border_crops_correctly() {
        // 4×4: top row and left column transparent, rest opaque
        let mut pixels = vec![[200u8, 200, 200, 255]; 16];
        for x in 0..4 {
            pixels[x] = [0, 0, 0, 0]; // row 0
        }
        for y in 0..4 {
            pixels[y * 4] = [0, 0, 0, 0]; // col 0
        }
        let png = make_rgba_png(4, 4, pixels);
        let result = trim_transparent_borders(&png).unwrap();
        let out = image::load_from_memory(&result).unwrap();
        // Should be cropped to 3×3 (columns 1-3, rows 1-3)
        assert_eq!(out.dimensions(), (3, 3));
    }
}
