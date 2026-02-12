//! Roundtrip encode/decode tests for the encode feature

#![cfg(all(feature = "encode", feature = "managed"))]

use imgref::Img;
use rgb::{Rgb, Rgba};
use zenavif::{
    DecodedImage, EncodeBitDepth, EncodeColorModel, EncoderConfig, encode, encode_rgb8,
    encode_rgb16, encode_rgba8, encode_rgba16, encode_with,
};

/// Create a simple 16x16 RGB8 test image with a gradient
fn make_rgb8_image() -> Img<Vec<Rgb<u8>>> {
    let mut pixels = Vec::with_capacity(16 * 16);
    for y in 0..16u8 {
        for x in 0..16u8 {
            pixels.push(Rgb {
                r: x * 16,
                g: y * 16,
                b: 128,
            });
        }
    }
    Img::new(pixels, 16, 16)
}

/// Create a simple 16x16 RGBA8 test image with a gradient and alpha
fn make_rgba8_image() -> Img<Vec<Rgba<u8>>> {
    let mut pixels = Vec::with_capacity(16 * 16);
    for y in 0..16u8 {
        for x in 0..16u8 {
            pixels.push(Rgba {
                r: x * 16,
                g: y * 16,
                b: 128,
                a: 200,
            });
        }
    }
    Img::new(pixels, 16, 16)
}

#[test]
fn roundtrip_rgb8() {
    let img = make_rgb8_image();
    let config = EncoderConfig::new().quality(80.0).speed(10);
    let encoded =
        encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).expect("encode should succeed");

    assert!(!encoded.avif_file.is_empty());
    assert!(encoded.color_byte_size > 0);
    assert_eq!(encoded.alpha_byte_size, 0);

    // Decode it back
    let decoded = zenavif::decode(&encoded.avif_file).expect("decode should succeed");
    assert_eq!(decoded.width(), 16);
    assert_eq!(decoded.height(), 16);
    assert!(!decoded.has_alpha());
}

#[test]
fn roundtrip_rgba8() {
    let img = make_rgba8_image();
    let config = EncoderConfig::new().quality(80.0).speed(10);
    let encoded =
        encode_rgba8(img.as_ref(), &config, &enough::Unstoppable).expect("encode should succeed");

    assert!(!encoded.avif_file.is_empty());
    assert!(encoded.color_byte_size > 0);
    assert!(encoded.alpha_byte_size > 0);

    // Decode it back
    let decoded = zenavif::decode(&encoded.avif_file).expect("decode should succeed");
    assert_eq!(decoded.width(), 16);
    assert_eq!(decoded.height(), 16);
    assert!(decoded.has_alpha());
}

#[test]
fn convenience_encode_rgb8() {
    let img = make_rgb8_image();
    let decoded = DecodedImage::Rgb8(img);
    let encoded = encode(&decoded).expect("convenience encode should succeed");

    assert!(!encoded.avif_file.is_empty());
    let roundtrip = zenavif::decode(&encoded.avif_file).expect("decode should succeed");
    assert_eq!(roundtrip.width(), 16);
    assert_eq!(roundtrip.height(), 16);
}

#[test]
fn convenience_encode_rgba8() {
    let img = make_rgba8_image();
    let decoded = DecodedImage::Rgba8(img);
    let encoded = encode(&decoded).expect("convenience encode should succeed");

    assert!(!encoded.avif_file.is_empty());
    let roundtrip = zenavif::decode(&encoded.avif_file).expect("decode should succeed");
    assert_eq!(roundtrip.width(), 16);
    assert_eq!(roundtrip.height(), 16);
    assert!(roundtrip.has_alpha());
}

#[test]
fn encoder_config_builder_chains() {
    let config = EncoderConfig::new()
        .quality(90.0)
        .speed(3)
        .alpha_quality(85.0)
        .bit_depth(EncodeBitDepth::Eight)
        .color_model(EncodeColorModel::YCbCr)
        .threads(Some(1))
        .exif(vec![0xFF, 0xD8]);

    let img = make_rgb8_image();
    let encoded =
        encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).expect("encode should succeed");
    assert!(!encoded.avif_file.is_empty());
}

/// Create a simple 16x16 RGB16 test image with 10-bit values
fn make_rgb16_image() -> Img<Vec<Rgb<u16>>> {
    let mut pixels = Vec::with_capacity(16 * 16);
    for y in 0..16u16 {
        for x in 0..16u16 {
            pixels.push(Rgb {
                r: x * 64,  // 0-960, within 10-bit range
                g: y * 64,
                b: 512,
            });
        }
    }
    Img::new(pixels, 16, 16)
}

/// Create a simple 16x16 RGBA16 test image with 10-bit values
fn make_rgba16_image() -> Img<Vec<Rgba<u16>>> {
    let mut pixels = Vec::with_capacity(16 * 16);
    for y in 0..16u16 {
        for x in 0..16u16 {
            pixels.push(Rgba {
                r: x * 64,
                g: y * 64,
                b: 512,
                a: 800,
            });
        }
    }
    Img::new(pixels, 16, 16)
}

#[test]
fn roundtrip_rgb16() {
    let img = make_rgb16_image();
    let config = EncoderConfig::new().quality(80.0).speed(10);
    let encoded =
        encode_rgb16(img.as_ref(), &config, &enough::Unstoppable).expect("encode should succeed");

    assert!(!encoded.avif_file.is_empty());
    assert!(encoded.color_byte_size > 0);
    assert_eq!(encoded.alpha_byte_size, 0);

    // Decode it back
    let decoded = zenavif::decode(&encoded.avif_file).expect("decode should succeed");
    assert_eq!(decoded.width(), 16);
    assert_eq!(decoded.height(), 16);
    assert!(!decoded.has_alpha());
}

#[test]
fn roundtrip_rgba16() {
    let img = make_rgba16_image();
    let config = EncoderConfig::new().quality(80.0).speed(10);
    let encoded =
        encode_rgba16(img.as_ref(), &config, &enough::Unstoppable).expect("encode should succeed");

    assert!(!encoded.avif_file.is_empty());
    assert!(encoded.color_byte_size > 0);
    assert!(encoded.alpha_byte_size > 0);

    // Decode it back
    let decoded = zenavif::decode(&encoded.avif_file).expect("decode should succeed");
    assert_eq!(decoded.width(), 16);
    assert_eq!(decoded.height(), 16);
    assert!(decoded.has_alpha());
}

#[test]
fn convenience_encode_rgb16() {
    let img = make_rgb16_image();
    let decoded = DecodedImage::Rgb16(img);
    let encoded = encode(&decoded).expect("convenience encode should succeed");

    assert!(!encoded.avif_file.is_empty());
    let roundtrip = zenavif::decode(&encoded.avif_file).expect("decode should succeed");
    assert_eq!(roundtrip.width(), 16);
    assert_eq!(roundtrip.height(), 16);
}

#[test]
fn unsupported_grayscale_input() {
    let pixels: Vec<u8> = vec![128; 4];
    let img = Img::new(pixels, 2, 2);
    let decoded = DecodedImage::Gray8(img);

    let result = encode(&decoded);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("grayscale"),
        "error should mention grayscale: {err}"
    );
}

#[test]
fn encode_with_custom_config() {
    let img = make_rgb8_image();
    let decoded = DecodedImage::Rgb8(img);
    let config = EncoderConfig::new().quality(50.0).speed(10);

    let encoded =
        encode_with(&decoded, &config, &enough::Unstoppable).expect("encode_with should succeed");
    assert!(!encoded.avif_file.is_empty());
}
