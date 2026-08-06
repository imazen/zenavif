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

/// Animations decode identically through both backends: every frame's
/// pixels AND duration must byte-agree (the aom path eagerly decodes the
/// whole track through `decode_frames` — DPB/CDF state spans samples).
/// Vectors are the libavif animated set CI provisions (fail-loud loader,
/// no-graceful-skips policy).
#[test]
fn animations_decode_identically_across_backends() {
    for name in [
        "colors-animated-8bpc.avif",
        "colors-animated-8bpc-alpha-exif-xmp.avif",
        "colors-animated-8bpc-depth-exif-xmp.avif",
        "colors-animated-8bpc-audio.avif",
        "colors-animated-12bpc-keyframes-0-2-3.avif",
    ] {
        let path = format!("tests/vectors/libavif/{name}");
        let data = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("read {path}: {e} (run: just download-vectors)"));
        let mut rav = zenavif::AnimationDecoder::new(&data, &DecoderConfig::new())
            .unwrap_or_else(|e| panic!("{name}: rav1d open: {e}"));
        let mut aom = zenavif::AnimationDecoder::new(
            &data,
            &DecoderConfig::new().decode_backend(DecodeBackend::AomRs),
        )
        .unwrap_or_else(|e| panic!("{name}: aom open: {e}"));
        assert_eq!(rav.info().frame_count, aom.info().frame_count, "{name}");
        let mut n = 0usize;
        loop {
            let rf = rav.next_frame(&Unstoppable).expect("rav1d frame");
            let af = aom.next_frame(&Unstoppable).expect("aom frame");
            match (rf, af) {
                (None, None) => break,
                (Some(rf), Some(af)) => {
                    assert_eq!(rf.duration_ms, af.duration_ms, "{name} frame {n} duration");
                    assert_eq!(
                        rf.pixels.as_slice().contiguous_bytes(),
                        af.pixels.as_slice().contiguous_bytes(),
                        "{name} frame {n} pixels diverge between backends"
                    );
                    n += 1;
                }
                _ => panic!("{name}: backends returned different frame counts at {n}"),
            }
        }
        assert!(n > 1, "{name}: expected multiple frames, got {n}");
    }
}

/// Grid (container-tiled) AVIFs decode identically through both backends —
/// each grid cell is an independent AV1 still; the stitch is shared.
/// (AV1 bitstream tiles inside a single frame are separate and were always
/// in the aom envelope.) Vectors CI provisions; fail-loud loader.
#[test]
fn grids_decode_identically_across_backends() {
    for name in [
        "sofa_grid1x5_420.avif",
        "sofa_grid1x5_420_dimg_repeat.avif",
        "color_grid_alpha_nogrid.avif",
    ] {
        let path = format!("tests/vectors/libavif/{name}");
        let data = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("read {path}: {e} (run: just download-vectors)"));
        assert_product_identical(&data, name);
    }
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
