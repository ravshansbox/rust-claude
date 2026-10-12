use super::{MAX_EDGE, prepare};
use image::{
    DynamicImage, GenericImageView, ImageEncoder, ImageFormat, RgbImage, RgbaImage,
    codecs::jpeg::JpegEncoder,
};
use std::io::Cursor;

fn encoded(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(RgbImage::new(width, height));
    let mut data = Vec::new();
    image.write_to(&mut Cursor::new(&mut data), format).unwrap();
    data
}

#[test]
fn keeps_small_supported_images() {
    for (format, media_type) in [
        (ImageFormat::Png, "image/png"),
        (ImageFormat::Jpeg, "image/jpeg"),
        (ImageFormat::Gif, "image/gif"),
        (ImageFormat::WebP, "image/webp"),
    ] {
        let data = encoded(10, 20, format);
        let image = prepare(data.clone()).unwrap();
        assert_eq!(image.media_type, media_type);
        assert_eq!(image.data, data);
    }
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
fn rejects_images_too_large_to_decode() {
    // A tiny 1x1 greyscale PNG whose header claims 24000x24000, which
    // would need 576 MiB to decode: over the limit but survivable if
    // the limit were ignored.
    let mut data = Vec::new();
    DynamicImage::ImageLuma8(image::GrayImage::new(1, 1))
        .write_to(&mut Cursor::new(&mut data), ImageFormat::Png)
        .unwrap();
    data[16..20].copy_from_slice(&24000u32.to_be_bytes());
    data[20..24].copy_from_slice(&24000u32.to_be_bytes());
    let mut crc = !0u32;
    for byte in &data[12..29] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    data[29..33].copy_from_slice(&(!crc).to_be_bytes());
    let error = prepare(data).unwrap_err();
    assert_eq!(error.to_string(), "image is too large");
}

#[test]
fn turns_photos_upright_using_their_exif_orientation() {
    // Exif block with one entry: orientation 6, "rotate 90° clockwise".
    let exif = vec![
        0x49, 0x49, 0x2a, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
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
