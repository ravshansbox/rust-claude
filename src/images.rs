use std::io::Cursor;

use anyhow::{Context, Result, bail};
use image::{
    DynamicImage, ImageDecoder, ImageFormat, ImageReader, Rgb, RgbImage, codecs::jpeg::JpegEncoder,
    imageops::FilterType, metadata::Orientation,
};

const MAX_EDGE: u32 = 2000;
const MAX_ENCODED_BYTES: usize = 5 * 1024 * 1024;
const JPEG_QUALITIES: [u8; 4] = [85, 70, 55, 40];

#[derive(Clone, Debug, PartialEq, Eq)]
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
    // JPEG has no transparency, so blend transparent areas onto white
    // instead of keeping their stored colour, which is usually black.
    let rgb = if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
            let [red, green, blue, alpha] = rgba.get_pixel(x, y).0;
            let alpha = u16::from(alpha);
            let blend =
                |channel: u8| ((u16::from(channel) * alpha + 255 * (255 - alpha)) / 255) as u8;
            Rgb([blend(red), blend(green), blend(blue)])
        })
    } else {
        image.to_rgb8()
    };
    rgb.write_with_encoder(JpegEncoder::new_with_quality(&mut data, quality))?;
    Ok(Image {
        media_type: "image/jpeg",
        data,
    })
}

pub fn prepare(data: Vec<u8>) -> Result<Image> {
    let format = image::guess_format(&data).context("unsupported image format")?;
    let mut decoder = ImageReader::with_format(Cursor::new(&data), format)
        .into_decoder()
        .context("failed to decode image")?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    // Decoding straight from the decoder skips the reader's memory limit,
    // so a tiny file claiming huge dimensions could exhaust memory.
    if image::Limits::default()
        .reserve(decoder.total_bytes())
        .is_err()
    {
        bail!("image is too large");
    }
    let mut decoded = DynamicImage::from_decoder(decoder).context("failed to decode image")?;
    // Photos are often stored sideways with an Exif note saying how to turn
    // them. Turn them upright, since the re-encoded image has no Exif data.
    let upright = matches!(orientation, Orientation::NoTransforms);
    decoded.apply_orientation(orientation);
    let long_edge = decoded.width().max(decoded.height());
    if upright
        && let Some(media_type) = media_type(format)
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
mod tests;
