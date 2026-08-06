//! Tests for [`super::AvifEncoder`] and its encode helpers.

use super::*;
use crate::codec::decode_config::AvifDecoderConfig;
use crate::codec::encode_config::AvifEncoderConfig;
use std::borrow::Cow;
use zencodec::Metadata;

#[cfg(feature = "encode")]
#[test]
fn encoding_rgbx8() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let w = 16u32;
    let h = 16u32;
    // RGBX layout: byte 3 is padding; set to non-opaque value to catch leaks.
    let mut buf = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        buf.extend_from_slice(&[255, 128, 0, 0x13]);
    }
    let slice = PixelSlice::new(&buf, w, h, (w * 4) as usize, PixelDescriptor::RGBX8_SRGB).unwrap();

    let enc = AvifEncoderConfig::new().with_quality(80.0);
    let output = enc.job().encoder().unwrap().encode(slice.erase()).unwrap();
    assert!(!output.data().is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoding_bgrx8() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let w = 16u32;
    let h = 16u32;
    let mut buf = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        // BGR order, pad byte non-opaque
        buf.extend_from_slice(&[0, 128, 255, 0x42]);
    }
    let slice = PixelSlice::new(&buf, w, h, (w * 4) as usize, PixelDescriptor::BGRX8_SRGB).unwrap();

    let enc = AvifEncoderConfig::new().with_quality(80.0);
    let output = enc.job().encoder().unwrap().encode(slice.erase()).unwrap();
    assert!(!output.data().is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encode_rgbx8_matches_rgb8() {
    // RGBX8 should produce the same bitstream as an equivalent RGB8 encode
    // (both route through crate::encode_rgb8 with identical RGB bytes).
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let w = 16u32;
    let h = 16u32;

    let mut rgbx = Vec::with_capacity((w * h * 4) as usize);
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for i in 0..(w * h) {
        let r = (i & 0xff) as u8;
        let g = ((i >> 1) & 0xff) as u8;
        let b = ((i >> 2) & 0xff) as u8;
        rgbx.extend_from_slice(&[r, g, b, 0x55]);
        rgb.extend_from_slice(&[r, g, b]);
    }

    let rgbx_slice =
        PixelSlice::new(&rgbx, w, h, (w * 4) as usize, PixelDescriptor::RGBX8_SRGB).unwrap();
    let rgb_slice =
        PixelSlice::new(&rgb, w, h, (w * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();

    let rgbx_out = AvifEncoderConfig::new()
        .with_quality(80.0)
        .job()
        .encoder()
        .unwrap()
        .encode(rgbx_slice.erase())
        .unwrap();
    let rgb_out = AvifEncoderConfig::new()
        .with_quality(80.0)
        .job()
        .encoder()
        .unwrap()
        .encode(rgb_slice.erase())
        .unwrap();

    assert_eq!(
        rgbx_out.data(),
        rgb_out.data(),
        "RGBX8 must encode identically to RGB8 (padding byte stripped)"
    );
}

#[cfg(feature = "encode")]
#[test]
fn f32_roundtrip_all_simd_tiers() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    let report = for_each_token_permutation(CompileTimePolicy::Warn, |_perm| {
        let pixels: Vec<Rgb<f32>> = (0..16 * 16)
            .map(|i| {
                let t = i as f32 / 255.0;
                Rgb {
                    r: t,
                    g: (t * 0.7),
                    b: (t * 0.3),
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);

        let enc = AvifEncoderConfig::new()
            .with_quality(100.0)
            .with_effort_u32(10);
        let output = enc.encode_rgb_f32(img.as_ref()).unwrap();
        assert!(!output.data().is_empty());

        let dec = AvifDecoderConfig::new();
        let dst = vec![
            Rgb {
                r: 0.0f32,
                g: 0.0,
                b: 0.0,
            };
            16 * 16
        ];
        let mut dst_img = imgref::ImgVec::new(dst, 16, 16);
        let _info = dec
            .decode_into_rgb_f32(output.data(), dst_img.as_mut())
            .unwrap();

        for p in dst_img.buf().iter() {
            assert!(p.r >= 0.0 && p.r <= 1.0, "r out of range: {}", p.r);
            assert!(p.g >= 0.0 && p.g <= 1.0, "g out of range: {}", p.g);
            assert!(p.b >= 0.0 && p.b <= 1.0, "b out of range: {}", p.b);
        }
    });
    assert!(report.permutations_run >= 1);
}

#[cfg(feature = "encode")]
#[test]
fn f32_rgba_roundtrip() {
    let pixels: Vec<Rgba<f32>> = (0..16 * 16)
        .map(|i| {
            let t = i as f32 / 255.0;
            Rgba {
                r: t,
                g: (t * 0.7),
                b: (t * 0.3),
                a: 1.0,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);

    let enc = AvifEncoderConfig::new()
        .with_quality(100.0)
        .with_effort_u32(10);
    let output = enc.encode_rgba_f32(img.as_ref()).unwrap();
    assert!(!output.data().is_empty());

    let dec = AvifDecoderConfig::new();
    let mut dst_img = imgref::ImgVec::new(
        vec![
            Rgba {
                r: 0.0f32,
                g: 0.0,
                b: 0.0,
                a: 0.0
            };
            16 * 16
        ],
        16,
        16,
    );
    dec.decode_into_rgba_f32(output.data(), dst_img.as_mut())
        .unwrap();

    for p in dst_img.buf().iter() {
        assert!(p.r >= 0.0 && p.r <= 1.0, "r out of range: {}", p.r);
        assert!(p.g >= 0.0 && p.g <= 1.0, "g out of range: {}", p.g);
        assert!(p.b >= 0.0 && p.b <= 1.0, "b out of range: {}", p.b);
        assert!(p.a >= 0.0 && p.a <= 1.0, "a out of range: {}", p.a);
    }
}

#[cfg(feature = "encode")]
#[test]
fn f32_gray_roundtrip() {
    use rgb::Gray;

    let pixels: Vec<Gray<f32>> = (0..16 * 16).map(|i| Gray(i as f32 / 255.0)).collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);

    let enc = AvifEncoderConfig::new()
        .with_quality(100.0)
        .with_effort_u32(10);
    let output = enc.encode_gray_f32(img.as_ref()).unwrap();
    assert!(!output.data().is_empty());

    let dec = AvifDecoderConfig::new();
    let mut dst_img = imgref::ImgVec::new(vec![Gray(0.0f32); 16 * 16], 16, 16);
    dec.decode_into_gray_f32(output.data(), dst_img.as_mut())
        .unwrap();

    for p in dst_img.buf().iter() {
        assert!(
            p.value() >= 0.0 && p.value() <= 1.0,
            "gray out of range: {}",
            p.value()
        );
    }
}

// ── Encoder trait roundtrip tests ──────────────────────────────────────

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgb8() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let pixels: Vec<Rgb<u8>> = (0..16 * 16)
        .map(|i| Rgb {
            r: (i % 256) as u8,
            g: ((i * 3) % 256) as u8,
            b: ((i * 7) % 256) as u8,
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder
        .encode(PixelSlice::from(img.as_ref()).erase())
        .unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgba8() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let pixels: Vec<Rgba<u8>> = (0..16 * 16)
        .map(|i| Rgba {
            r: (i % 256) as u8,
            g: ((i * 3) % 256) as u8,
            b: ((i * 7) % 256) as u8,
            a: ((i * 5) % 256) as u8,
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder
        .encode(PixelSlice::from(img.as_ref()).erase())
        .unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_gray8() {
    use rgb::Gray;
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let pixels: Vec<Gray<u8>> = (0..16 * 16).map(|i| Gray((i % 256) as u8)).collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder
        .encode(PixelSlice::from(img.as_ref()).erase())
        .unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgb_f32() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let pixels: Vec<Rgb<f32>> = (0..16 * 16)
        .map(|i| {
            let t = i as f32 / 255.0;
            Rgb {
                r: t,
                g: t * 0.5,
                b: t * 0.25,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder
        .encode(PixelSlice::from(img.as_ref()).erase())
        .unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgba_f32() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let pixels: Vec<Rgba<f32>> = (0..16 * 16)
        .map(|i| {
            let t = i as f32 / 255.0;
            Rgba {
                r: t,
                g: t * 0.5,
                b: t * 0.25,
                a: 1.0,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder
        .encode(PixelSlice::from(img.as_ref()).erase())
        .unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_gray_f32() {
    use rgb::Gray;
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let pixels: Vec<Gray<f32>> = (0..16 * 16).map(|i| Gray(i as f32 / 255.0)).collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder
        .encode(PixelSlice::from(img.as_ref()).erase())
        .unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_dyn_encoder() {
    use zencodec::encode::{EncodeJob, EncoderConfig};

    let pixels: Vec<Rgb<u8>> = vec![
        Rgb {
            r: 100,
            g: 150,
            b: 200
        };
        32 * 32
    ];
    let img = imgref::ImgVec::new(pixels, 32, 32);
    let config = AvifEncoderConfig::new().with_quality(50.0);
    let dyn_enc = config.job().dyn_encoder().unwrap();
    let output = dyn_enc
        .encode(PixelSlice::from(img.as_ref()).erase())
        .unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

// ── HDR / 16-bit encoder tests ──────────────────────────────────────

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgb16_srgb() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let pixels: Vec<Rgb<u16>> = (0..16 * 16)
        .map(|i| {
            let v = (i * 256) as u16;
            Rgb {
                r: v,
                g: v / 2,
                b: v / 3,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder
        .encode(PixelSlice::from(img.as_ref()).erase())
        .unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgba16_srgb() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let pixels: Vec<Rgba<u16>> = (0..16 * 16)
        .map(|i| {
            let v = (i * 256) as u16;
            Rgba {
                r: v,
                g: v / 2,
                b: v / 3,
                a: 65535,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder
        .encode(PixelSlice::from(img.as_ref()).erase())
        .unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgb16_pq_bt2020() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::{ColorPrimaries, TransferFunction};

    let pixels: Vec<Rgb<u16>> = (0..16 * 16)
        .map(|i| {
            let v = (i * 256) as u16;
            Rgb {
                r: v,
                g: v / 2,
                b: v / 3,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let desc = PixelDescriptor::RGB16_SRGB
        .with_transfer(TransferFunction::Pq)
        .with_primaries(ColorPrimaries::Bt2020);
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder.encode(slice.erase()).unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgba16_pq_bt2020() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::{ColorPrimaries, TransferFunction};

    let pixels: Vec<Rgba<u16>> = (0..16 * 16)
        .map(|i| {
            let v = (i * 256) as u16;
            Rgba {
                r: v,
                g: v / 2,
                b: v / 3,
                a: 65535,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let desc = PixelDescriptor::RGBA16_SRGB
        .with_transfer(TransferFunction::Pq)
        .with_primaries(ColorPrimaries::Bt2020);
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder.encode(slice.erase()).unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgb16_hlg_bt2020() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::{ColorPrimaries, TransferFunction};

    let pixels: Vec<Rgb<u16>> = (0..16 * 16)
        .map(|i| {
            let v = (i * 256) as u16;
            Rgb {
                r: v,
                g: v / 2,
                b: v / 3,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let desc = PixelDescriptor::RGB16_SRGB
        .with_transfer(TransferFunction::Hlg)
        .with_primaries(ColorPrimaries::Bt2020);
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder.encode(slice.erase()).unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgba16_hlg_bt2020() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::{ColorPrimaries, TransferFunction};

    let pixels: Vec<Rgba<u16>> = (0..16 * 16)
        .map(|i| {
            let v = (i * 256) as u16;
            Rgba {
                r: v,
                g: v / 2,
                b: v / 3,
                a: 65535,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let desc = PixelDescriptor::RGBA16_SRGB
        .with_transfer(TransferFunction::Hlg)
        .with_primaries(ColorPrimaries::Bt2020);
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder.encode(slice.erase()).unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgb16_display_p3() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::ColorPrimaries;

    let pixels: Vec<Rgb<u16>> = (0..16 * 16)
        .map(|i| {
            let v = (i * 256) as u16;
            Rgb {
                r: v,
                g: v / 2,
                b: v / 3,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let desc = PixelDescriptor::RGB16_SRGB.with_primaries(ColorPrimaries::DisplayP3);
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder.encode(slice.erase()).unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgba16_display_p3() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::ColorPrimaries;

    let pixels: Vec<Rgba<u16>> = (0..16 * 16)
        .map(|i| {
            let v = (i * 256) as u16;
            Rgba {
                r: v,
                g: v / 2,
                b: v / 3,
                a: 65535,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let desc = PixelDescriptor::RGBA16_SRGB.with_primaries(ColorPrimaries::DisplayP3);
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder.encode(slice.erase()).unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_pq_bt2020_roundtrip() {
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::{ColorPrimaries, TransferFunction};

    // Encode with PQ/BT.2020 descriptor
    let pixels: Vec<Rgb<u16>> = (0..16 * 16)
        .map(|i| {
            let v = ((i as u32 * 256) % 65536) as u16;
            Rgb {
                r: v,
                g: v / 2,
                b: v / 3,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let desc = PixelDescriptor::RGB16_SRGB
        .with_transfer(TransferFunction::Pq)
        .with_primaries(ColorPrimaries::Bt2020);
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
    let config = AvifEncoderConfig::new().with_quality(80.0);
    let encoder = config.job().encoder().unwrap();
    let encoded = encoder.encode(slice.erase()).unwrap();
    assert!(!encoded.is_empty());

    // Decode and verify we get pixels back
    let dec_config = AvifDecoderConfig::new();
    let decoder = dec_config
        .job()
        .decoder(Cow::Borrowed(encoded.data()), &[])
        .unwrap();
    let decoded = decoder.decode().unwrap();
    assert_eq!(decoded.info().width, 16);
    assert_eq!(decoded.info().height, 16);
}

/// Regression for the `apply_descriptor_color` CICP-override bug: a
/// `Metadata`-set CICP must win over the pixel descriptor's color, and the
/// emitted nclx matrix must stay consistent with it. We hand pixels whose
/// descriptor reads sRGB / BT.709 (primaries=1) but set
/// `Metadata.cicp = DISPLAY_P3` (primaries=12); the decoded nclx must report
/// Display-P3, not the descriptor's BT.709.
#[cfg(feature = "encode")]
#[test]
fn caller_cicp_wins_over_descriptor_color() {
    use zencodec::Cicp;
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    // sRGB / BT.709 descriptor pixels.
    let pixels = vec![
        Rgb {
            r: 200u8,
            g: 100,
            b: 50,
        };
        16 * 16
    ];
    let img = imgref::ImgVec::new(pixels, 16, 16);
    // PixelDescriptor::RGB8_SRGB ⇒ BT.709 primaries, sRGB transfer.
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(PixelDescriptor::RGB8_SRGB);

    // Caller pins Display-P3 via Metadata. Use the blessed metadata path.
    let meta = Metadata::none().with_cicp(Cicp::DISPLAY_P3);
    let encoder = AvifEncoderConfig::new()
        .with_quality(90.0)
        .job()
        .with_metadata_policy(meta, zencodec::MetadataPolicy::PreserveExact)
        .encoder()
        .unwrap();
    let encoded = encoder.encode(slice.erase()).unwrap();
    assert!(!encoded.is_empty());

    // Decode and read back the nclx CICP.
    let decoder = AvifDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(encoded.data()), &[])
        .unwrap();
    let decoded = decoder.decode().unwrap();
    let cicp = decoded
        .info()
        .source_color
        .cicp
        .expect("decoded AVIF must carry CICP");

    // Caller's Display-P3 (primaries 12) wins over the descriptor's BT.709 (1).
    assert_eq!(
        cicp.color_primaries,
        Cicp::DISPLAY_P3.color_primaries,
        "caller's Display-P3 primaries must win over descriptor BT.709"
    );
    assert_eq!(
        cicp.transfer_characteristics,
        Cicp::DISPLAY_P3.transfer_characteristics,
        "transfer must match the caller's CICP"
    );
    // The matrix code point must honestly describe the YCbCr math the encoder
    // actually used. zenravif's default RGB path encodes via BT.601 YCbCr and
    // writes matrix_coefficients = 6 — so the consistent value here is 6, NOT
    // the caller CICP's Identity(0) (which describes an RGB-domain image, not
    // how AVIF stored it). The bug was a *missing/stale* MC; the fix makes the
    // emitted nclx a coherent triple {primaries:12, transfer:13, matrix:6}.
    assert_eq!(
        cicp.matrix_coefficients, 6,
        "matrix must reflect the encoder's actual YCbCr matrix (BT.601)"
    );
}

/// The descriptor still drives CICP when the caller supplies none — the
/// fallback the bug fix must preserve. sRGB/BT.709 descriptor with no
/// Metadata CICP ⇒ nclx reports BT.709 primaries with a consistent matrix.
#[cfg(feature = "encode")]
#[test]
fn descriptor_drives_cicp_without_caller_cicp() {
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let pixels = vec![
        Rgb {
            r: 30u8,
            g: 200,
            b: 120,
        };
        16 * 16
    ];
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(PixelDescriptor::RGB8_SRGB);

    // No Metadata CICP at all.
    let encoder = AvifEncoderConfig::new()
        .with_quality(90.0)
        .job()
        .encoder()
        .unwrap();
    let encoded = encoder.encode(slice.erase()).unwrap();

    let decoder = AvifDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(encoded.data()), &[])
        .unwrap();
    let decoded = decoder.decode().unwrap();
    let cicp = decoded
        .info()
        .source_color
        .cicp
        .expect("decoded AVIF must carry CICP");

    // Descriptor's BT.709 (primaries 1) flows through.
    assert_eq!(
        cicp.color_primaries, 1,
        "descriptor BT.709 primaries must drive nclx when no caller CICP"
    );
    // As above, the emitted matrix reflects the encoder's actual YCbCr math
    // (zenravif default RGB path = BT.601 = 6), kept consistent with the
    // descriptor-driven primaries/transfer.
    assert_eq!(
        cicp.matrix_coefficients, 6,
        "matrix must reflect the encoder's actual YCbCr matrix (BT.601)"
    );
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_pq_bt2020_narrow_range() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::{ColorPrimaries, SignalRange, TransferFunction};

    // PQ BT.2020 with narrow/limited signal range
    let pixels: Vec<Rgb<u16>> = (0..16 * 16)
        .map(|i| {
            let v = (i * 256) as u16;
            Rgb {
                r: v,
                g: v / 2,
                b: v / 3,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let desc = PixelDescriptor::RGB16_SRGB
        .with_transfer(TransferFunction::Pq)
        .with_primaries(ColorPrimaries::Bt2020)
        .with_signal_range(SignalRange::Narrow);
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder.encode(slice.erase()).unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgb_f32_pq_bt2020() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::{ColorPrimaries, TransferFunction};

    // f32 PQ BT.2020 — should route through u16 path, not linear_to_srgb_u8
    let pixels: Vec<Rgb<f32>> = (0..16 * 16)
        .map(|i| {
            let v = i as f32 / 256.0;
            Rgb {
                r: v,
                g: v * 0.8,
                b: v * 0.6,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let desc = PixelDescriptor::RGBF32_LINEAR
        .with_transfer(TransferFunction::Pq)
        .with_primaries(ColorPrimaries::Bt2020);
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder.encode(slice.erase()).unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_rgba_f32_hlg_bt2020() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::{ColorPrimaries, TransferFunction};

    // f32 HLG BT.2020 — should route through u16 path
    let pixels: Vec<Rgba<f32>> = (0..16 * 16)
        .map(|i| {
            let v = i as f32 / 256.0;
            Rgba {
                r: v,
                g: v * 0.7,
                b: v * 0.5,
                a: 1.0,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let desc = PixelDescriptor::RGBAF32_LINEAR
        .with_transfer(TransferFunction::Hlg)
        .with_primaries(ColorPrimaries::Bt2020);
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
    let config = AvifEncoderConfig::new().with_quality(60.0);
    let encoder = config.job().encoder().unwrap();
    let output = encoder.encode(slice.erase()).unwrap();
    assert!(!output.is_empty());
    assert_eq!(output.format(), ImageFormat::Avif);
}

#[cfg(feature = "encode")]
#[test]
fn encoder_trait_f32_pq_roundtrip_preserves_hdr() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::{ColorPrimaries, TransferFunction};

    // Encode f32 PQ data, decode, verify the output has >8-bit depth
    // (proving it went through the u16 path, not the sRGB u8 path)
    let pixels: Vec<Rgb<f32>> = (0..16 * 16)
        .map(|i| {
            let v = i as f32 / 256.0;
            Rgb {
                r: v,
                g: v * 0.9,
                b: v * 0.7,
            }
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);
    let desc = PixelDescriptor::RGBF32_LINEAR
        .with_transfer(TransferFunction::Pq)
        .with_primaries(ColorPrimaries::Bt2020);
    let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
    let config = AvifEncoderConfig::new().with_quality(90.0);
    let encoder = config.job().encoder().unwrap();
    let encoded = encoder.encode(slice.erase()).unwrap();

    // Decode and verify bit depth > 8 (proving 10-bit encode path was used)
    let dec = AvifDecoderConfig::new();
    let decoded = dec.decode(encoded.data()).unwrap();
    assert!(decoded.info().source_color.bit_depth.unwrap_or(8) >= 10);
}

#[cfg(feature = "encode")]
#[test]
fn encode_max_output_bytes_rejects() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let pixels: Vec<Rgb<u8>> = vec![
        Rgb {
            r: 100,
            g: 150,
            b: 200,
        };
        32 * 32
    ];
    let img = imgref::ImgVec::new(pixels, 32, 32);
    let config = AvifEncoderConfig::new().with_quality(80.0);
    // 100 bytes is too small for any AVIF output
    let limits = ResourceLimits::none().with_max_output(100);
    let encoder = config.job().with_limits(limits).encoder().unwrap();
    let result = encoder.encode(PixelSlice::from(img.as_ref()).erase());
    assert!(
        result.is_err(),
        "encode should fail with max_output_bytes=100"
    );
}

// Gain map zencodec extras tests are in tests/gainmap_decode.rs
// (integration test) to avoid pre-existing compile errors in this
// module when `encode` feature is not enabled.

// ── Memory-adaptive encode concurrency (max_memory_bytes on ENCODE) ──

/// A tight explicit cap must reject the encode via the CALIBRATED
/// thread-aware estimate — even though the raw input-buffer size
/// (`w*h*bpp`, the pre-2026-08 check) fits comfortably. 512×512 RGB8:
/// raw input = 768 KiB, calibrated single-thread conservative peak
/// ≈ 24 MB; a 4 MB cap passes the former and must fail the latter,
/// BEFORE any encoding work happens (the test is instant).
#[cfg(feature = "encode")]
#[test]
fn encode_max_memory_calibrated_rejects() {
    use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};

    let (w, h) = (512usize, 512usize);
    let cap = 4u64 * 1024 * 1024;
    assert!(
        (w * h * 3) as u64 <= cap,
        "precondition: the raw input buffer must fit the cap, so only \
         the calibrated estimate can reject"
    );
    let est1 = crate::heuristics::estimate_encode_threaded(w as u32, h as u32, 3, 4, 1)
        .unwrap()
        .peak_memory_bytes_max;
    assert!(
        est1 > cap,
        "precondition: the single-thread calibrated peak ({est1}) must exceed the cap"
    );

    let pixels: Vec<Rgb<u8>> = vec![
        Rgb {
            r: 90,
            g: 120,
            b: 40,
        };
        w * h
    ];
    let img = imgref::ImgVec::new(pixels, w, h);
    let result = AvifEncoderConfig::new()
        .job()
        .with_limits(ResourceLimits::none().with_max_memory(cap))
        .encoder()
        .unwrap()
        .encode(PixelSlice::from(img.as_ref()).erase());
    let err = result.expect_err("tight max_memory must reject the encode pre-flight");
    let msg = format!("{err}").to_lowercase();
    assert!(
        msg.contains("mem"),
        "error should be the memory-limit error, got: {msg}"
    );
}

/// A moderate explicit cap — above the calibrated worst-case peak at
/// every thread count (the tile bound caps the per-thread term) — must
/// let the encode run, with no thread pin and no reduction note.
#[cfg(feature = "encode")]
#[test]
fn encode_moderate_max_memory_succeeds() {
    use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};

    let (w, h) = (512usize, 512usize);
    // 512² has 4 tiles, so ≥ 4 threads all estimate alike; est at 4 is
    // the machine-independent worst case.
    let worst = crate::heuristics::estimate_encode_threaded(w as u32, h as u32, 3, 10, 4)
        .unwrap()
        .peak_memory_bytes_max;
    let cap = 64u64 * 1024 * 1024;
    assert!(
        worst < cap,
        "precondition: worst-case estimate fits the cap"
    );

    let pixels: Vec<Rgb<u8>> = vec![
        Rgb {
            r: 10,
            g: 200,
            b: 130,
        };
        w * h
    ];
    let img = imgref::ImgVec::new(pixels, w, h);
    let output = AvifEncoderConfig::new()
        .with_effort_u32(10) // speed 10 — fastest; memory model is speed-invariant
        .job()
        .with_limits(ResourceLimits::none().with_max_memory(cap))
        .encoder()
        .unwrap()
        .encode(PixelSlice::from(img.as_ref()).erase())
        .expect("encode under a moderate max_memory must succeed");
    assert!(!output.is_empty());
    assert!(
        output.extras::<String>().is_none(),
        "no thread reduction happened, so no note should be attached"
    );
}

/// A cap between the 1-thread and 2-thread conservative peaks forces the
/// fit to walk the (explicitly requested) 8 threads down to 2 — the
/// encode succeeds AND the reduction is recorded on the output
/// (reductions are never silent). Deterministic on any machine: the
/// start is the explicit request, and 512²'s tile bound (4) caps the
/// per-thread term independent of core count.
#[cfg(feature = "encode")]
#[test]
fn encode_thread_reduction_is_recorded() {
    use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};

    let (w, h) = (512usize, 512usize);
    let est = |threads: usize| {
        crate::heuristics::estimate_encode_threaded(w as u32, h as u32, 3, 10, threads)
            .unwrap()
            .peak_memory_bytes_max
    };
    // Admits 2 threads, not 3 (each extra thread costs the calibrated
    // per-thread term).
    let cap = est(2);
    assert!(
        est(3) > cap && est(1) < cap,
        "precondition: cap isolates 2 threads"
    );

    let mut config = AvifEncoderConfig::new().with_effort_u32(10);
    let inner = config.inner().clone().threads(Some(8));
    *config.inner_mut() = inner;

    let pixels: Vec<Rgb<u8>> = vec![
        Rgb {
            r: 200,
            g: 60,
            b: 60,
        };
        w * h
    ];
    let img = imgref::ImgVec::new(pixels, w, h);
    let output = config
        .job()
        .with_limits(ResourceLimits::none().with_max_memory(cap))
        .encoder()
        .unwrap()
        .encode(PixelSlice::from(img.as_ref()).erase())
        .expect("encode must succeed at the fitted thread count");
    let note = output
        .extras::<String>()
        .expect("thread reduction must be recorded on the output");
    assert!(
        note.contains("8") && note.contains("2"),
        "note should record 8 -> 2, got: {note}"
    );
}
