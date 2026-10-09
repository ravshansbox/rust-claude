use std::io::Cursor;

use anyhow::{Context, Result, bail};
use image::{DynamicImage, ImageFormat, codecs::jpeg::JpegEncoder, imageops::FilterType};

const MAX_EDGE: u32 = 2000;
const MAX_ENCODED_BYTES: usize = 10_000_000;
const JPEG_QUALITIES: [u8; 4] = [85, 70, 55, 40];

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub media_type: &'static str,
    pub data: Vec<u8>,
}

impl Image {
    pub fn extension(&self) -> &'static str {
        match self.media_type {
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "png",
        }
    }
}

fn media_type(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Gif => Some("image/gif"),
        ImageFormat::WebP => Some("image/webp"),
        _ => None,
    }
}

fn fits(data: &[u8]) -> bool {
    data.len().div_ceil(3) * 4 <= MAX_ENCODED_BYTES
}

fn encode_png(image: &DynamicImage) -> Result<Image> {
    let mut data = Vec::new();
    image.write_to(&mut Cursor::new(&mut data), ImageFormat::Png)?;
    Ok(Image {
        media_type: "image/png",
        data,
    })
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> Result<Image> {
    let mut data = Vec::new();
    image
        .to_rgb8()
        .write_with_encoder(JpegEncoder::new_with_quality(&mut data, quality))?;
    Ok(Image {
        media_type: "image/jpeg",
        data,
    })
}

pub fn prepare(data: Vec<u8>) -> Result<Image> {
    let format = image::guess_format(&data).context("unsupported image format")?;
    let decoded =
        image::load_from_memory_with_format(&data, format).context("failed to decode image")?;
    let long_edge = decoded.width().max(decoded.height());
    if let Some(media_type) = media_type(format)
        && long_edge <= MAX_EDGE
        && fits(&data)
    {
        return Ok(Image { media_type, data });
    }
    let mut image = if long_edge > MAX_EDGE {
        decoded.resize(MAX_EDGE, MAX_EDGE, FilterType::Lanczos3)
    } else {
        decoded
    };
    loop {
        let png = encode_png(&image)?;
        if fits(&png.data) {
            return Ok(png);
        }
        for quality in JPEG_QUALITIES {
            let jpeg = encode_jpeg(&image, quality)?;
            if fits(&jpeg.data) {
                return Ok(jpeg);
            }
        }
        if image.width().max(image.height()) <= 1 {
            bail!("image is too large");
        }
        image = image.resize(
            (image.width() / 2).max(1),
            (image.height() / 2).max(1),
            FilterType::Lanczos3,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_EDGE, prepare};
    use image::{DynamicImage, GenericImageView, ImageFormat, RgbaImage};
    use std::io::Cursor;

    fn encoded(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let image = DynamicImage::ImageRgba8(RgbaImage::new(width, height));
        let mut data = Vec::new();
        image.write_to(&mut Cursor::new(&mut data), format).unwrap();
        data
    }

    #[test]
    fn keeps_small_supported_images() {
        let data = encoded(10, 20, ImageFormat::Gif);
        let image = prepare(data.clone()).unwrap();
        assert_eq!(image.media_type, "image/gif");
        assert_eq!(image.data, data);
    }

    #[test]
    fn converts_unsupported_formats_to_png() {
        let image = prepare(encoded(10, 20, ImageFormat::Tiff)).unwrap();
        assert_eq!(image.media_type, "image/png");
        let decoded = image::load_from_memory(&image.data).unwrap();
        assert_eq!(decoded.dimensions(), (10, 20));
    }

    #[test]
    fn scales_down_large_images() {
        let image = prepare(encoded(MAX_EDGE * 2, 100, ImageFormat::Png)).unwrap();
        let decoded = image::load_from_memory(&image.data).unwrap();
        assert_eq!(decoded.dimensions(), (MAX_EDGE, 50));
    }

    #[test]
    fn rejects_data_that_is_not_an_image() {
        assert!(prepare(b"not an image".to_vec()).is_err());
    }
}
