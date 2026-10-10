use std::io::Cursor;

use anyhow::{Context, Result, bail};
use image::{
    DynamicImage, ImageDecoder, ImageFormat, ImageReader, Rgb, RgbImage, codecs::jpeg::JpegEncoder,
    imageops::FilterType, metadata::Orientation,
};

const MAX_EDGE: u32 = 2000;
const MAX_ENCODED_BYTES: usize = 5 * 1024 * 1024;
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
    // JPEG has no transparency, so blend transparent areas onto white
    // instead of keeping their stored colour, which is usually black.
    let rgb = if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
            let [red, green, blue, alpha] = rgba.get_pixel(x, y).0;
            let blend = |channel: u8| {
                ((u16::from(channel) * u16::from(alpha) + 255 * (255 - u16::from(alpha))) / 255)
                    as u8
            };
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
mod tests {
    use super::{MAX_EDGE, prepare};
    use image::{
        DynamicImage, GenericImageView, ImageEncoder, ImageFormat, RgbImage, RgbaImage,
        codecs::jpeg::JpegEncoder,
    };
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
    fn turns_photos_upright_using_their_exif_orientation() {
        // Exif block with one entry: orientation 6, "rotate 90° clockwise".
        let exif = vec![
            0x49, 0x49, 0x2a, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let mut data = Vec::new();
        let mut encoder = JpegEncoder::new(&mut data);
        encoder.set_exif_metadata(exif).unwrap();
        DynamicImage::ImageRgb8(RgbImage::new(10, 20))
            .write_with_encoder(encoder)
            .unwrap();
        let image = prepare(data).unwrap();
        let decoded = image::load_from_memory(&image.data).unwrap();
        assert_eq!(decoded.dimensions(), (20, 10));
    }

    #[test]
    fn rejects_data_that_is_not_an_image() {
        assert!(prepare(b"not an image".to_vec()).is_err());
    }

    #[test]
    fn shrinks_images_over_the_api_5_mb_base64_limit() {
        // Noise compresses poorly: this PNG is about 9 MB in base64.
        let mut seed: u32 = 1;
        let noise = RgbImage::from_fn(1500, 1500, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            image::Rgb((seed >> 8).to_le_bytes()[..3].try_into().unwrap())
        });
        let mut data = Vec::new();
        DynamicImage::ImageRgb8(noise)
            .write_to(&mut Cursor::new(&mut data), ImageFormat::Png)
            .unwrap();
        let base64_len = data.len().div_ceil(3) * 4;
        assert!(base64_len > 5_250_000 && base64_len < 10_000_000);
        let image = prepare(data).unwrap();
        assert!(image.data.len().div_ceil(3) * 4 <= 5 * 1024 * 1024);
    }

    #[test]
    fn shows_transparent_areas_as_white_in_jpeg() {
        // Noise too large for PNG, with a fully transparent top-left corner.
        let mut seed: u32 = 1;
        let noise = RgbaImage::from_fn(1500, 1500, |x, y| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            if x < 100 && y < 100 {
                image::Rgba([0, 0, 0, 0])
            } else {
                let [red, green, blue, _] = (seed >> 8).to_le_bytes();
                image::Rgba([red, green, blue, 255])
            }
        });
        let mut data = Vec::new();
        DynamicImage::ImageRgba8(noise)
            .write_to(&mut Cursor::new(&mut data), ImageFormat::Png)
            .unwrap();
        let image = prepare(data).unwrap();
        assert_eq!(image.media_type, "image/jpeg");
        let decoded = image::load_from_memory(&image.data).unwrap().to_rgb8();
        let corner = decoded.get_pixel(10, 10);
        assert!(corner.0.iter().all(|channel| *channel > 240), "{corner:?}");
    }
}
