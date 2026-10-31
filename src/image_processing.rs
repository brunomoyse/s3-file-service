//! Decoding, resizing, trimming and encoding of product images.

use std::io::{self, Cursor};

use image::imageops::{self, FilterType};
use image::{DynamicImage, GenericImageView, ImageError, ImageFormat, RgbaImage};
use ravif::{Encoder as AvifEncoder, Img};
use rgb::FromSlice;
use serde::Serialize;
use webp::Encoder as WebpEncoder;

/// Lossy quality for WebP and AVIF, on their 0-100 scale.
const LOSSY_QUALITY: f32 = 75.0;

/// `ravif` encoder effort, from 1 (slowest, smallest) to 10 (fastest, largest).
const AVIF_SPEED: u8 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Dimensions {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum OutputFormat {
    Avif,
    Webp,
    Png,
}

impl OutputFormat {
    pub(crate) const ALL: [Self; 3] = [Self::Avif, Self::Webp, Self::Png];

    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Avif => "avif",
            Self::Webp => "webp",
            Self::Png => "png",
        }
    }

    pub(crate) fn encode(self, image: &DynamicImage) -> Result<Vec<u8>, ImageError> {
        match self {
            Self::Avif => encode_to_avif(image),
            Self::Webp => Ok(encode_to_webp(image)),
            Self::Png => encode_to_png(image),
        }
    }
}

/// Decodes `data` and scales it to `width`, keeping the aspect ratio.
pub(crate) fn resize_image(data: &[u8], width: u32) -> Result<DynamicImage, ImageError> {
    let image = image::load_from_memory(data)?;
    let (original_width, original_height) = image.dimensions();
    let height = scaled_height(width, original_width, original_height);
    Ok(image.resize_exact(width, height, FilterType::Lanczos3))
}

/// Rounds to the nearest pixel, but never below one so that very wide images stay encodable.
fn scaled_height(width: u32, original_width: u32, original_height: u32) -> u32 {
    let original_width = u64::from(original_width);
    let exact = u64::from(width) * u64::from(original_height);
    let rounded = (exact + original_width / 2) / original_width;
    u32::try_from(rounded).unwrap_or(u32::MAX).max(1)
}

fn encode_to_png(image: &DynamicImage) -> Result<Vec<u8>, ImageError> {
    let mut buffer = Cursor::new(Vec::new());
    image.write_to(&mut buffer, ImageFormat::Png)?;
    Ok(buffer.into_inner())
}

fn encode_to_webp(image: &DynamicImage) -> Vec<u8> {
    let rgba = image.to_rgba8();
    WebpEncoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
        .encode(LOSSY_QUALITY)
        .to_vec()
}

fn encode_to_avif(image: &DynamicImage) -> Result<Vec<u8>, ImageError> {
    let rgba = image.to_rgba8();
    let pixels = Img::new(
        rgba.as_raw().as_rgba(),
        to_usize(rgba.width()),
        to_usize(rgba.height()),
    );
    AvifEncoder::new()
        .with_quality(LOSSY_QUALITY)
        .with_speed(AVIF_SPEED)
        .encode_rgba(pixels)
        .map(|encoded| encoded.avif_file)
        .map_err(|error| ImageError::IoError(io::Error::other(error)))
}

fn to_usize(value: u32) -> usize {
    usize::try_from(value).expect("u32 fits in usize on every supported target")
}

/// Reads the size of an encoded image, fully decoding it so that corrupt data is rejected.
pub(crate) fn dimensions(data: &[u8]) -> Result<Dimensions, ImageError> {
    let (width, height) = image::load_from_memory(data)?.dimensions();
    Ok(Dimensions { width, height })
}

/// Crops the fully transparent rows and columns around an image's visible pixels.
///
/// The input comes back untouched when it has no such border or no visible pixel at all.
/// A cropped result is always a PNG.
pub(crate) fn trim_transparent_borders(data: Vec<u8>) -> Result<Vec<u8>, ImageError> {
    let image = image::load_from_memory(&data)?.to_rgba8();
    let Some(bounds) = visible_bounds(&image) else {
        return Ok(data);
    };
    if bounds.width == image.width() && bounds.height == image.height() {
        return Ok(data);
    }

    let cropped =
        imageops::crop_imm(&image, bounds.left, bounds.top, bounds.width, bounds.height).to_image();
    encode_to_png(&DynamicImage::ImageRgba8(cropped))
}

struct Bounds {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
}

fn visible_bounds(image: &RgbaImage) -> Option<Bounds> {
    let (left, top, right, bottom) = image
        .enumerate_pixels()
        .filter(|(_, _, pixel)| pixel.0[3] > 0)
        .fold(None, |extent, (x, y, _)| match extent {
            None => Some((x, y, x, y)),
            Some((left, top, right, bottom)) => {
                Some((x.min(left), y.min(top), x.max(right), y.max(bottom)))
            }
        })?;
    Some(Bounds {
        left,
        top,
        width: right - left + 1,
        height: bottom - top + 1,
    })
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;

    const OPAQUE: Rgba<u8> = Rgba([200, 100, 50, 255]);
    const CLEAR: Rgba<u8> = Rgba([0, 0, 0, 0]);

    fn png_from(image: RgbaImage) -> Vec<u8> {
        encode_to_png(&DynamicImage::ImageRgba8(image)).unwrap()
    }

    fn decoded_dimensions(data: &[u8]) -> (u32, u32) {
        image::load_from_memory(data).unwrap().dimensions()
    }

    #[test]
    fn scaled_height_keeps_the_aspect_ratio() {
        assert_eq!(scaled_height(600, 1000, 500), 300);
        assert_eq!(scaled_height(350, 3, 2), 233);
        assert_eq!(scaled_height(600, 20, 20), 600);
    }

    #[test]
    fn scaled_height_rounds_to_nearest() {
        assert_eq!(scaled_height(3, 2, 1), 2, "1.5 rounds up");
        assert_eq!(scaled_height(10, 3, 1), 3, "3.33 rounds down");
    }

    #[test]
    fn scaled_height_never_reaches_zero() {
        assert_eq!(scaled_height(350, 65_000, 1), 1);
    }

    #[test]
    fn resize_handles_an_extremely_wide_image() {
        let png = png_from(RgbaImage::from_pixel(4_000, 1, OPAQUE));

        let resized = resize_image(&png, 350).unwrap();

        assert_eq!(resized.dimensions(), (350, 1));
    }

    #[test]
    fn resize_rejects_data_that_is_not_an_image() {
        assert!(resize_image(b"not an image", 350).is_err());
    }

    #[test]
    fn every_format_produces_a_decodable_image_of_the_same_size() {
        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(16, 8, OPAQUE));

        for format in OutputFormat::ALL {
            let encoded = format.encode(&image).unwrap();

            let guessed = image::guess_format(&encoded).unwrap();
            assert_eq!(
                guessed.extensions_str().first().copied(),
                Some(format.extension()),
                "{format:?}"
            );
            // The `image` crate is built without an AVIF decoder.
            if !matches!(format, OutputFormat::Avif) {
                assert_eq!(decoded_dimensions(&encoded), (16, 8), "{format:?}");
            }
        }
    }

    #[test]
    fn dimensions_reads_the_image_size() {
        let png = png_from(RgbaImage::from_pixel(7, 3, OPAQUE));

        assert_eq!(
            dimensions(&png).unwrap(),
            Dimensions {
                width: 7,
                height: 3
            }
        );
    }

    #[test]
    fn trim_returns_the_same_bytes_without_a_transparent_border() {
        let png = png_from(RgbaImage::from_pixel(3, 3, OPAQUE));

        assert_eq!(trim_transparent_borders(png.clone()).unwrap(), png);
    }

    #[test]
    fn trim_returns_the_same_bytes_when_fully_transparent() {
        let png = png_from(RgbaImage::from_pixel(4, 4, CLEAR));

        assert_eq!(trim_transparent_borders(png.clone()).unwrap(), png);
    }

    #[test]
    fn trim_crops_to_the_visible_pixels_and_keeps_their_color() {
        let mut image = RgbaImage::from_pixel(5, 5, CLEAR);
        image.put_pixel(2, 2, Rgba([255, 0, 0, 255]));

        let trimmed = trim_transparent_borders(png_from(image)).unwrap();

        let trimmed = image::load_from_memory(&trimmed).unwrap().to_rgba8();
        assert_eq!(trimmed.dimensions(), (1, 1));
        assert_eq!(trimmed.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn trim_crops_a_border_on_only_some_sides() {
        let image = RgbaImage::from_fn(4, 4, |x, y| if x == 0 || y == 0 { CLEAR } else { OPAQUE });

        let trimmed = trim_transparent_borders(png_from(image)).unwrap();

        assert_eq!(decoded_dimensions(&trimmed), (3, 3));
    }

    #[test]
    fn trim_treats_partial_transparency_as_visible() {
        let mut image = RgbaImage::from_pixel(3, 3, CLEAR);
        image.put_pixel(0, 0, Rgba([0, 0, 0, 1]));
        image.put_pixel(1, 1, OPAQUE);

        let trimmed = trim_transparent_borders(png_from(image)).unwrap();

        assert_eq!(decoded_dimensions(&trimmed), (2, 2));
    }

    #[test]
    fn trim_rejects_data_that_is_not_an_image() {
        assert!(trim_transparent_borders(b"not an image".to_vec()).is_err());
    }
}
