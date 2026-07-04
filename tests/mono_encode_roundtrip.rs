//! True monochrome (Cs400) encode roundtrip — imazen/zenavif#6.
//!
//! With the `encode-mono` feature, `Gray8` input is encoded as a luma-only
//! AV1 bitstream (no chroma planes, no chroma RDO) through zenravif's
//! `Encoder::encode_gray8`. This gate proves, end to end:
//!
//! 1. the **bitstream** signals `mono_chrome = 1` (checked by parsing the
//!    AV1 sequence header out of the container, not just the `av1C` claim);
//! 2. the file decodes through zenavif's own decode path (rav1d-safe) back
//!    to native Gray8;
//! 3. the decoded luma matches the input within normal lossy bounds.

#![cfg(all(feature = "encode-mono", feature = "zencodec"))]

use std::borrow::Cow;
use zencodec::decode::{Decode as _, DecodeJob as _, DecodeOutput, DecoderConfig as _};
use zenpixels::PixelFormat;

/// Gradient + texture: real luma structure, not a flat fill.
fn gray_pixels(w: usize, h: usize) -> Vec<u8> {
    (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| {
                let base = (16 + (x * 2 + y) % 224) as u8;
                if (x / 4 + y / 4) % 2 == 0 {
                    base.saturating_add(24)
                } else {
                    base
                }
            })
        })
        .collect()
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let se: u64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| {
            let d = i64::from(x) - i64::from(y);
            (d * d) as u64
        })
        .sum();
    if se == 0 {
        100.0
    } else {
        10.0 * (255.0f64 * 255.0 / (se as f64 / a.len() as f64)).log10()
    }
}

fn decode_gray(data: &[u8]) -> (Vec<u8>, usize, usize) {
    let out: DecodeOutput = zenavif::AvifDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(data), &[])
        .expect("decoder")
        .decode()
        .expect("mono encode must decode through rav1d-safe");
    let p = out.pixels();
    assert_eq!(
        p.descriptor().pixel_format(),
        PixelFormat::Gray8,
        "mono AVIF must decode to native Gray8"
    );
    let (w, h, stride) = (p.width() as usize, p.rows() as usize, p.stride());
    let bytes = p.as_strided_bytes();
    let gray = (0..h)
        .flat_map(|y| bytes[y * stride..][..w].to_vec())
        .collect();
    (gray, w, h)
}

#[test]
fn gray8_mono_encode_roundtrip() {
    let (w, h) = (96usize, 64usize);
    let src = gray_pixels(w, h);

    let img = imgref::ImgVec::new(src.iter().map(|&g| rgb::Gray::new(g)).collect(), w, h);
    let out = zenavif::AvifEncoderConfig::new()
        .with_quality(85.0)
        .encode_gray8(img.as_ref())
        .expect("mono encode");
    let data = out.data();
    assert!(!data.is_empty());

    // 1. Bitstream-level proof: the AV1 sequence header signals monochrome.
    let parsed = zenavif_parse::AvifParser::from_bytes(data).expect("container parses");
    let md = parsed.primary_metadata().expect("seq header parses");
    assert!(
        md.monochrome,
        "encode-mono must produce a mono_chrome=1 bitstream, not gray-as-RGB"
    );
    assert_eq!(md.max_frame_width.get() as usize, w);
    assert_eq!(md.max_frame_height.get() as usize, h);

    // 2 + 3. Decodes via rav1d-safe to native Gray8, close to the input.
    let (gray, dw, dh) = decode_gray(data);
    assert_eq!((dw, dh), (w, h));
    let p = psnr(&src, &gray);
    assert!(p > 35.0, "q85 mono roundtrip PSNR {p:.2} dB too low");
}

/// Odd dimensions through the whole encode+decode chain.
#[test]
fn gray8_mono_encode_roundtrip_odd_dims() {
    let (w, h) = (129usize, 101usize);
    let src = gray_pixels(w, h);
    let img = imgref::ImgVec::new(src.iter().map(|&g| rgb::Gray::new(g)).collect(), w, h);
    let out = zenavif::AvifEncoderConfig::new()
        .with_quality(85.0)
        .encode_gray8(img.as_ref())
        .expect("mono encode");

    let (gray, dw, dh) = decode_gray(out.data());
    assert_eq!((dw, dh), (w, h));
    let p = psnr(&src, &gray);
    assert!(p > 35.0, "odd-dims mono roundtrip PSNR {p:.2} dB too low");
}
