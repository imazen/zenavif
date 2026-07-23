//! Product-path aom backend (`DecoderConfig::decode_backend(AomRs)`):
//! decoding a real AVIF container through the PUBLIC API with the aom
//! backend must produce byte-identical `PixelBuffer`s to the default
//! rav1d-safe backend, across bit depths, subsamplings, alpha, and mono —
//! and reject the not-yet-supported shapes honestly.
#![cfg(all(feature = "aom-backend", feature = "encode"))]

use almost_enough::{StopToken, Unstoppable};
use imgref::{Img, ImgVec};
use rgb::Rgb;
use zenavif::{DecodeBackend, DecoderConfig, EncodeChromaSubsampling, EncoderConfig};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

fn test_image(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let mut state = 0x9E3779B9u32;
    let mut lcg = move || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (state >> 24) as u8
    };
    let mut buf = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let g = ((x * 255) / w.max(1)) as u8;
            let b = ((y * 255) / h.max(1)) as u8;
            let n = lcg() / 6;
            buf.push(Rgb {
                r: g.saturating_add(n),
                g: b.saturating_add(n),
                b: (((g as u16 + b as u16) / 2) as u8).saturating_add(n),
            });
        }
    }
    Img::new(buf, w, h)
}

/// Decode `avif` with both backends through the public API and assert the
/// decoded buffers (and headline info fields) are byte-identical.
fn assert_product_identical(avif: &[u8], label: &str) {
    let rav = zenavif::decode_with(avif, &DecoderConfig::new(), &Unstoppable)
        .unwrap_or_else(|e| panic!("{label}: rav1d-safe decode: {e}"));
    let aom = zenavif::decode_with(
        avif,
        &DecoderConfig::new().decode_backend(DecodeBackend::AomRs),
        &Unstoppable,
    )
    .unwrap_or_else(|e| panic!("{label}: aom decode: {e}"));
    assert_eq!(rav.width(), aom.width(), "{label}: width");
    assert_eq!(rav.height(), aom.height(), "{label}: height");
    assert_eq!(
        rav.descriptor(),
        aom.descriptor(),
        "{label}: pixel descriptor (format) differs"
    );
    assert_eq!(
        rav.as_slice().contiguous_bytes(),
        aom.as_slice().contiguous_bytes(),
        "{label}: decoded pixels diverge between backends"
    );
}

#[test]
fn stills_decode_identically_across_backends() {
    let img = test_image(120, 88); // odd-ish dims: exercises chroma edges
    for (label, subsampling, quality) in [
        ("444-q85", EncodeChromaSubsampling::Yuv444, 85.0),
        ("444-q30", EncodeChromaSubsampling::Yuv444, 30.0),
        ("420-q85", EncodeChromaSubsampling::Yuv420, 85.0),
        ("420-q30", EncodeChromaSubsampling::Yuv420, 30.0),
    ] {
        let config = EncoderConfig::new()
            .quality(quality)
            .speed(8)
            .chroma_subsampling(subsampling);
        let enc = zenavif::encode_rgb8(img.as_ref(), &config, stop()).expect("encode");
        assert_product_identical(&enc.avif_file, label);
    }
}

#[test]
fn rgba_alpha_decodes_identically_across_backends() {
    let base = test_image(96, 64);
    let buf: Vec<rgb::Rgba<u8>> = base
        .buf()
        .iter()
        .enumerate()
        .map(|(i, p)| rgb::Rgba {
            r: p.r,
            g: p.g,
            b: p.b,
            a: (i % 251) as u8,
        })
        .collect();
    let img = Img::new(buf, base.width(), base.height());
    let config = EncoderConfig::new()
        .quality(80.0)
        .speed(8)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420);
    let enc = zenavif::encode_rgba8(img.as_ref(), &config, stop()).expect("rgba encode");
    assert_product_identical(&enc.avif_file, "rgba-420-q80");
}

#[test]
fn ten_bit_decodes_identically_across_backends() {
    let img8 = test_image(96, 64);
    let buf16: Vec<rgb::Rgb<u16>> = img8
        .buf()
        .iter()
        .map(|p| rgb::Rgb {
            r: (p.r as u16) << 8 | p.r as u16,
            g: (p.g as u16) << 8 | p.g as u16,
            b: (p.b as u16) << 8 | p.b as u16,
        })
        .collect();
    let img16 = Img::new(buf16, img8.width(), img8.height());
    let config = EncoderConfig::new()
        .quality(80.0)
        .speed(8)
        .bit_depth(zenavif::EncodeBitDepth::Ten);
    let enc = zenavif::encode_rgb16(img16.as_ref(), &config, stop()).expect("10-bit encode");
    // Native 16-bit output AND the prefer_8bit downscale path.
    assert_product_identical(&enc.avif_file, "b10-444-q80");
    let rav = zenavif::decode_with(
        &enc.avif_file,
        &DecoderConfig::new().prefer_8bit(true),
        &Unstoppable,
    )
    .expect("rav1d prefer_8bit");
    let aom = zenavif::decode_with(
        &enc.avif_file,
        &DecoderConfig::new()
            .prefer_8bit(true)
            .decode_backend(DecodeBackend::AomRs),
        &Unstoppable,
    )
    .expect("aom prefer_8bit");
    assert_eq!(
        rav.as_slice().contiguous_bytes(),
        aom.as_slice().contiguous_bytes(),
        "prefer_8bit path diverges"
    );
}

#[cfg(feature = "encode-mono")]
#[test]
fn monochrome_decodes_identically_across_backends() {
    let base = test_image(96, 64);
    let buf: Vec<u8> = base.buf().iter().map(|p| p.r).collect();
    let img: Img<Vec<u8>> = Img::new(buf, base.width(), base.height());
    let config = EncoderConfig::new().quality(80.0).speed(8);
    let enc = zenavif::encode_gray8(img.as_ref(), &config, stop()).expect("gray encode");
    assert_product_identical(&enc.avif_file, "mono-q80");
}

#[test]
fn animation_is_rejected_honestly_on_aom() {
    // Any bytes: the backend gate fires before parsing in AnimationDecoder::new.
    let img = test_image(64, 64);
    let enc = zenavif::encode_rgb8(
        img.as_ref(),
        &EncoderConfig::new()
            .quality(60.0)
            .speed(10)
            .chroma_subsampling(EncodeChromaSubsampling::Yuv420),
        stop(),
    )
    .expect("encode");
    let err = match zenavif::AnimationDecoder::new(
        &enc.avif_file,
        &DecoderConfig::new().decode_backend(DecodeBackend::AomRs),
    ) {
        Ok(_) => panic!("animation via aom must be rejected"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("AomRs"),
        "rejection must name the backend: {err}"
    );
}

/// The decode caps must be live on the aom product path too.
#[test]
fn frame_size_limit_fires_on_aom_product_path() {
    let img = test_image(128, 96); // 12,288 px
    let enc = zenavif::encode_rgb8(
        img.as_ref(),
        &EncoderConfig::new()
            .quality(60.0)
            .speed(10)
            .chroma_subsampling(EncodeChromaSubsampling::Yuv420),
        stop(),
    )
    .expect("encode");
    let err = zenavif::decode_with(
        &enc.avif_file,
        &DecoderConfig::new()
            .decode_backend(DecodeBackend::AomRs)
            .frame_size_limit(10_000),
        &Unstoppable,
    )
    .expect_err("10k-px cap must reject a 12k-px frame");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("limit") || msg.contains("large"),
        "limit rejection should say so: {msg}"
    );
}
