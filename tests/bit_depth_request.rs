//! `EncodeBitDepth` must be honoured on the 16-bit entry points.
//!
//! Registered defect (zenmetrics `benchmarks/bitdepth_capability_matrix_2026-09-02.md`
//! §2): `encode_rgb16` / `encode_rgba16` scaled every sample to 10 bits and
//! called `encode_raw_planes_10_bit` unconditionally, reading `config.bit_depth`
//! **not at all**. `EncoderConfig { bit_depth: Eight, .. }` plus a 16-bit buffer
//! silently produced a 10-bit file — reachable from the generic zencodec route
//! for any `Rgb16` / `Rgba16` input. Same defect class as the
//! `AvifEncoder::with_bit_depth` coercion the same lane fixed in `zenav1-svt`.
//!
//! Depth is read back **from the stored bitstream**, never from the request —
//! per the matrix's §4 rule that a read-back gate reads the file.

#![cfg(feature = "encode-imazen")]

use almost_enough::{StopToken, Unstoppable};
use imgref::ImgVec;
use rgb::{Rgb, Rgba};
use zenavif::{EncodeBitDepth, EncoderConfig};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

/// Coded depth of a stored AVIF, read from the bitstream by the AV1
/// sequence header (`zenavif::detect::probe`, which goes through the R1
/// owner `zenavif_parse`). This is the request-independent ground truth.
fn coded_bit_depth(avif: &[u8]) -> u8 {
    zenavif::detect::probe(avif)
        .expect("probe stored AVIF")
        .bit_depth
}

/// 8-bit content promoted to 16 bits by LSB replication — exactly the
/// shape the generic zencodec route delivers when it widens an 8-bit
/// source. Under the crate's narrowing rule these values survive the
/// 16 -> 8 trip unchanged.
fn replicated(v8: u8) -> u16 {
    u16::from(v8) * 257
}

fn rgb16_gradient(w: usize, h: usize) -> ImgVec<Rgb<u16>> {
    let pixels: Vec<Rgb<u16>> = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| Rgb {
                r: replicated((x * 255 / w.max(1)) as u8),
                g: replicated((y * 255 / h.max(1)) as u8),
                b: replicated(((x + y) * 255 / (w + h).max(1)) as u8),
            })
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

fn rgba16_gradient(w: usize, h: usize) -> ImgVec<Rgba<u16>> {
    let pixels: Vec<Rgba<u16>> = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| Rgba {
                r: replicated((x * 255 / w.max(1)) as u8),
                g: replicated((y * 255 / h.max(1)) as u8),
                b: replicated(((x + y) * 255 / (w + h).max(1)) as u8),
                a: replicated(255 - (x * 255 / w.max(1)) as u8),
            })
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

fn cfg(bit_depth: EncodeBitDepth) -> EncoderConfig {
    EncoderConfig::new()
        .quality(80.0)
        .speed(10)
        .threads(Some(1))
        .bit_depth(bit_depth)
}

/// THE defect: an explicit 8-bit request on a 16-bit buffer produced a
/// 10-bit file. Fails before the fix (reads 10).
#[test]
fn rgb16_honours_an_explicit_eight_bit_request() {
    let img = rgb16_gradient(64, 64);
    let enc = zenavif::encode_rgb16(img.as_ref(), &cfg(EncodeBitDepth::Eight), stop())
        .expect("encode rgb16 at 8 bits");
    assert_eq!(
        coded_bit_depth(&enc.avif_file),
        8,
        "bit_depth: Eight on a 16-bit buffer must code 8 bits, not silently 10"
    );
}

/// Same defect on the alpha entry point.
#[test]
fn rgba16_honours_an_explicit_eight_bit_request() {
    let img = rgba16_gradient(64, 64);
    let enc = zenavif::encode_rgba16(img.as_ref(), &cfg(EncodeBitDepth::Eight), stop())
        .expect("encode rgba16 at 8 bits");
    assert_eq!(
        coded_bit_depth(&enc.avif_file),
        8,
        "bit_depth: Eight on a 16-bit buffer must code 8 bits, not silently 10"
    );
}

/// `Ten` and `Auto` keep coding 10 bits on a 16-bit buffer. `Auto`'s
/// documented contract is "16-bit input -> 10-bit AV1"; pinning it here
/// means the fix above cannot change it as a side effect.
#[test]
fn rgb16_ten_and_auto_still_code_ten() {
    let img = rgb16_gradient(64, 64);
    for depth in [EncodeBitDepth::Ten, EncodeBitDepth::Auto] {
        let enc = zenavif::encode_rgb16(img.as_ref(), &cfg(depth), stop())
            .unwrap_or_else(|e| panic!("encode rgb16 at {depth:?}: {e}"));
        assert_eq!(
            coded_bit_depth(&enc.avif_file),
            10,
            "{depth:?} on a 16-bit buffer must stay 10-bit"
        );
    }
}

#[test]
fn rgba16_ten_and_auto_still_code_ten() {
    let img = rgba16_gradient(64, 64);
    for depth in [EncodeBitDepth::Ten, EncodeBitDepth::Auto] {
        let enc = zenavif::encode_rgba16(img.as_ref(), &cfg(depth), stop())
            .unwrap_or_else(|e| panic!("encode rgba16 at {depth:?}: {e}"));
        assert_eq!(
            coded_bit_depth(&enc.avif_file),
            10,
            "{depth:?} on a 16-bit buffer must stay 10-bit"
        );
    }
}

/// Fidelity of the narrowing route: 8-bit content promoted to 16 bits by
/// LSB replication and encoded losslessly at `bit_depth: Eight` must come
/// back as the original bytes. This is what proves the 16 -> 8 step uses
/// the inverse-of-replication rule (`scale_from_u16`, the crate's owner)
/// and not a rule that shifts samples — half-up rounding moves 128 of the
/// 256 bytes and overflows at 0xFFFF (pinned in `convert::narrow_16_to_8`).
///
/// Tolerance is the same +/-2 the sibling identity tests carry for
/// zenrav1e#9 (`with_lossless` is not yet bit-exact); a wrong narrowing
/// rule shifts every sample by a full 8-bit step in one direction, which
/// this still catches.
#[test]
fn narrowed_eight_bit_encode_preserves_replicated_bytes() {
    let (w, h) = (64usize, 64usize);
    let img = rgb16_gradient(w, h);
    let cfg = EncoderConfig::new()
        .speed(6)
        .threads(Some(1))
        .bit_depth(EncodeBitDepth::Eight)
        .with_lossless(true);
    let enc = zenavif::encode_rgb16(img.as_ref(), &cfg, stop()).expect("lossless encode at 8");
    assert_eq!(coded_bit_depth(&enc.avif_file), 8, "must be an 8-bit file");

    let out = zenavif::decode(&enc.avif_file).expect("decode");
    let got = out.try_as_imgref::<Rgb<u8>>().expect("rgb8 view");
    assert_eq!((got.width(), got.height()), (w, h));

    let mut worst = 0i32;
    for y in 0..h {
        for x in 0..w {
            let e = img.buf()[y * w + x];
            let g = got.buf()[y * got.stride() + x];
            // Expected 8-bit sample = the byte the 16-bit value was built from.
            let d = |a: u16, b: u8| (i32::from(a >> 8) - i32::from(b)).abs();
            worst = worst.max(d(e.r, g.r).max(d(e.g, g.g)).max(d(e.b, g.b)));
        }
    }
    assert!(
        worst <= 2,
        "narrowed 8-bit roundtrip worst channel delta {worst} (> 2): the 16 -> 8 \
         narrowing is not the inverse of LSB replication"
    );
}
