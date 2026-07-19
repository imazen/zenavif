//! svtav1-rs backend (`encode-svt-rs`) — encode/decode round-trip and
//! scope-rejection coverage.
//!
//! Everything here must pass TODAY on the pinned imazen/svtav1 rev. The
//! bitstream-identity-vs-C-SVT parity assertion is deliberately absent:
//! it lands when svtav1-rs reaches decision-layer bitstream identity
//! (asserting it earlier would be a test designed to fail or a fake).
//! Upstream decode conformance (aomdec 525/525 mono + 700/700 4:2:0)
//! is svtav1-rs's own gate; what zenavif pins here is the container +
//! round-trip contract through its own decoder (rav1d-safe).

#![cfg(feature = "encode-svt-rs")]

use almost_enough::{StopToken, Unstoppable};
use imgref::Img;
use rgb::Rgb;
use zenavif::{
    Av1Backend, EncodeBitDepth, EncodeChromaSubsampling, EncodeColorModel, EncodePixelRange,
    EncoderConfig, PlanInput, ValidationError,
};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

/// A svtav1-rs-shaped config: 4:2:0 is the only subsampling the backend
/// implements (zenavif's default is 4:4:4, which it rejects honestly).
fn svt_config() -> EncoderConfig {
    EncoderConfig::new()
        .backend(Av1Backend::SvtRs)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
}

/// Smooth diagonal gradient — chroma-subsampling-friendly content so the
/// PSNR floor states something about the codec, not about 4:2:0 loss on
/// adversarial chroma edges.
fn gradient_rgb8(w: usize, h: usize) -> Img<Vec<Rgb<u8>>> {
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            pixels.push(Rgb {
                r: ((x * 255) / w.max(1)) as u8,
                g: ((y * 255) / h.max(1)) as u8,
                b: (((x + y) * 255) / (w + h).max(1)) as u8,
            });
        }
    }
    Img::new(pixels, w, h)
}

fn psnr_rgb8(a: &[Rgb<u8>], b: &[Rgb<u8>]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mut se: u64 = 0;
    for (pa, pb) in a.iter().zip(b) {
        for (ca, cb) in [(pa.r, pb.r), (pa.g, pb.g), (pa.b, pb.b)] {
            let d = i64::from(ca) - i64::from(cb);
            se += (d * d) as u64;
        }
    }
    if se == 0 {
        return 100.0;
    }
    let mse = se as f64 / (a.len() * 3) as f64;
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

// --------------------------------------------------------------------
// Round trip through zenavif's own decoder
// --------------------------------------------------------------------

#[test]
fn svt_rs_roundtrip_gradient_128() {
    let img = gradient_rgb8(128, 128);
    let config = svt_config().quality(85.0).speed(6);
    config
        .validate_for_input(PlanInput::rgb8(128, 128))
        .expect("supported svt-rs config must validate");

    let encoded =
        zenavif::encode_rgb8(img.as_ref(), &config, stop()).expect("svt-rs encode must succeed");
    assert!(!encoded.avif_file.is_empty());
    assert!(encoded.color_byte_size > 0);
    assert_eq!(encoded.alpha_byte_size, 0);
    // ISO-BMFF sniff: box size (4) + "ftyp" + "avif" major brand.
    assert_eq!(&encoded.avif_file[4..8], b"ftyp");
    assert_eq!(&encoded.avif_file[8..12], b"avif");

    // Container must signal what the backend converted with: BT.601
    // matrix, full range (the config left CICP unset → BT.709/sRGB
    // primaries/transfer defaults).
    let decoder =
        zenavif::ManagedAvifDecoder::new(&encoded.avif_file, &zenavif::DecoderConfig::default())
            .expect("parse");
    let info = decoder.probe_info().expect("probe");
    assert_eq!(info.width, 128);
    assert_eq!(info.height, 128);
    assert_eq!(info.matrix_coefficients.0, 6, "must signal BT.601");
    assert_eq!(info.color_range, zenavif::ColorRange::Full);

    let decoded = zenavif::decode(&encoded.avif_file).expect("must decode via rav1d-safe");
    assert_eq!(decoded.width(), 128);
    assert_eq!(decoded.height(), 128);
    assert!(!decoded.has_alpha());

    let out = decoded
        .try_as_imgref::<Rgb<u8>>()
        .expect("no-alpha decode yields RGB8");
    let p = psnr_rgb8(img.buf(), out.buf());
    eprintln!(
        "svt_rs q85 4:2:0 roundtrip: PSNR {p:.2} dB, {} payload bytes",
        encoded.color_byte_size
    );
    // Measured 51.67 dB / 1509 payload bytes on 2026-07-13 at the pinned
    // svtav1 rev (3cad660b7), x86_64 (deterministic encoder; decoder is
    // conformance-exact). Floor is measured-minus-margin — the margin
    // absorbs only the yuv crate's per-arch RGB↔YUV rounding differences.
    assert!(
        p > 45.0,
        "q85 4:2:0 svt-rs roundtrip PSNR {p:.2} dB below floor \
         (measured 51.67 dB at the pinned rev)"
    );
}

#[test]
fn svt_rs_roundtrip_non_square_and_speeds() {
    // Non-square 64-aligned dims + the speed extremes (preset 0 and 13).
    let img = gradient_rgb8(192, 64);
    for speed in [1u8, 10] {
        let config = svt_config().quality(70.0).speed(speed);
        let encoded = zenavif::encode_rgb8(img.as_ref(), &config, stop())
            .unwrap_or_else(|e| panic!("speed {speed} encode failed: {e}"));
        let decoded = zenavif::decode(&encoded.avif_file)
            .unwrap_or_else(|e| panic!("speed {speed} decode failed: {e}"));
        assert_eq!(decoded.width(), 192);
        assert_eq!(decoded.height(), 64);
    }
}

#[test]
fn svt_rs_quality_moves_bytes() {
    let img = gradient_rgb8(128, 128);
    let lo = zenavif::encode_rgb8(img.as_ref(), &svt_config().quality(20.0).speed(6), stop())
        .expect("q20 encode");
    let hi = zenavif::encode_rgb8(img.as_ref(), &svt_config().quality(90.0).speed(6), stop())
        .expect("q90 encode");
    assert!(
        hi.color_byte_size > lo.color_byte_size,
        "q90 payload ({}) must out-size q20 payload ({}) on gradient content",
        hi.color_byte_size,
        lo.color_byte_size
    );
}

// --------------------------------------------------------------------
// Honest scope rejection — encode time
// --------------------------------------------------------------------

#[test]
fn svt_rs_rejects_unaligned_dims() {
    let img = gradient_rgb8(96, 96); // not a multiple of 64
    let err = zenavif::encode_rgb8(img.as_ref(), &svt_config(), stop())
        .expect_err("96x96 must be rejected, not padded");
    let msg = err.to_string();
    assert!(msg.contains("64"), "error must explain the 64 rule: {msg}");

    // Same rule at validate_for_input time.
    assert!(matches!(
        svt_config().validate_for_input(PlanInput::rgb8(96, 96)),
        Err(ValidationError::BackendUnsupportedParam { .. })
    ));
}

#[test]
fn svt_rs_rejects_default_yuv444_at_encode_time() {
    let img = gradient_rgb8(64, 64);
    let config = EncoderConfig::new().backend(Av1Backend::SvtRs); // 4:4:4 default
    let err = zenavif::encode_rgb8(img.as_ref(), &config, stop())
        .expect_err("4:4:4 must be rejected, not silently downsampled");
    assert!(err.to_string().contains("4:2:0"), "got: {err}");
}

#[test]
fn svt_rs_rejects_16bit_entry_points() {
    let rgb16: Img<Vec<Rgb<u16>>> = Img::new(vec![Rgb { r: 0, g: 0, b: 0 }; 64 * 64], 64, 64);
    assert!(
        zenavif::encode_rgb16(rgb16.as_ref(), &svt_config(), stop()).is_err(),
        "16-bit must be rejected (8-bit only)"
    );
}

// --------------------------------------------------------------------
// RGBA: color 4:2:0 item + Cs400 alpha auxiliary item
// --------------------------------------------------------------------

/// Gradient RGBA with a smooth alpha ramp (same rationale as
/// [`gradient_rgb8`]: subsampling-friendly content).
fn gradient_rgba8(w: usize, h: usize) -> Img<Vec<rgb::Rgba<u8>>> {
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            pixels.push(rgb::Rgba {
                r: ((x * 255) / w.max(1)) as u8,
                g: ((y * 255) / h.max(1)) as u8,
                b: (((x + y) * 255) / (w + h).max(1)) as u8,
                a: (64 + (x * 191) / w.max(1)) as u8,
            });
        }
    }
    Img::new(pixels, w, h)
}

#[test]
fn svt_rs_roundtrip_rgba_alpha_plane() {
    let img = gradient_rgba8(128, 128);
    let config = svt_config().quality(85.0).speed(6);

    let encoded = zenavif::encode_rgba8(img.as_ref(), &config, stop()).expect("svt-rs RGBA encode");
    assert!(encoded.color_byte_size > 0);
    assert!(
        encoded.alpha_byte_size > 0,
        "alpha auxiliary item must carry bytes"
    );

    let decoded = zenavif::decode(&encoded.avif_file).expect("must decode via rav1d-safe");
    assert_eq!(decoded.width(), 128);
    assert_eq!(decoded.height(), 128);
    assert!(
        decoded.has_alpha(),
        "alpha plane must survive the container"
    );

    let out = decoded
        .try_as_imgref::<rgb::Rgba<u8>>()
        .expect("alpha decode yields RGBA8");
    // Color PSNR over RGB channels; alpha checked separately. Row-wise:
    // the decoded buffer may be stride-padded.
    let (mut se_rgb, mut se_a) = (0u64, 0u64);
    for (row_a, row_b) in img.rows().zip(out.rows()) {
        for (pa, pb) in row_a.iter().zip(row_b.iter()) {
            for (ca, cb) in [(pa.r, pb.r), (pa.g, pb.g), (pa.b, pb.b)] {
                let d = i64::from(ca) - i64::from(cb);
                se_rgb += (d * d) as u64;
            }
            let d = i64::from(pa.a) - i64::from(pb.a);
            se_a += (d * d) as u64;
        }
    }
    let n = (img.width() * img.height()) as f64;
    let psnr_rgb = 10.0 * (255.0f64 * 255.0 / (se_rgb as f64 / (n * 3.0))).log10();
    let psnr_a = 10.0 * (255.0f64 * 255.0 / ((se_a as f64 / n).max(1e-9))).log10();
    eprintln!(
        "svt_rs q85 RGBA roundtrip: color PSNR {psnr_rgb:.2} dB, alpha PSNR {psnr_a:.2} dB, \
         color {} B + alpha {} B",
        encoded.color_byte_size, encoded.alpha_byte_size
    );
    // Measured 50.20 dB color / 138.13 dB alpha (color 1509 B + alpha 131 B)
    // on 2026-07-19 at the pinned svtav1 rev (3cad660b7), x86_64. Floors are
    // measured-minus-margin. Finding this path 20.10 dB on 2026-07-19 is
    // what uncovered the yuv-crate dropped-last-row-pair bug
    // (src/yuv_bilinear_fix.rs).
    assert!(psnr_rgb > 45.0, "RGBA color PSNR {psnr_rgb:.2} below floor");
    assert!(psnr_a > 45.0, "alpha-plane PSNR {psnr_a:.2} below floor");
}

#[test]
fn svt_rs_alpha_quality_fallback_contract() {
    // alpha_quality defaults to the color quality; setting it must move
    // the alpha payload independently of the color payload.
    let img = gradient_rgba8(128, 128);
    let hi = zenavif::encode_rgba8(img.as_ref(), &svt_config().quality(85.0).speed(6), stop())
        .expect("default alpha quality");
    let lo = zenavif::encode_rgba8(
        img.as_ref(),
        &svt_config().quality(85.0).alpha_quality(20.0).speed(6),
        stop(),
    )
    .expect("low alpha quality");
    assert!(
        lo.alpha_byte_size < hi.alpha_byte_size,
        "alpha_quality(20) payload ({}) must under-size the fallback-to-color-quality \
         payload ({})",
        lo.alpha_byte_size,
        hi.alpha_byte_size
    );
    assert_eq!(
        lo.color_byte_size, hi.color_byte_size,
        "alpha_quality must not perturb the color encode"
    );
}

// --------------------------------------------------------------------
// Grayscale: monochrome (Cs400) color item
// --------------------------------------------------------------------

#[cfg(feature = "encode-mono")]
#[test]
fn svt_rs_roundtrip_gray8_mono() {
    let w = 128usize;
    let h = 128usize;
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            pixels.push((((x + y) * 255) / (w + h)) as u8);
        }
    }
    let img: Img<Vec<u8>> = Img::new(pixels, w, h);
    let config = svt_config().quality(85.0).speed(6);

    let encoded = zenavif::encode_gray8(img.as_ref(), &config, stop()).expect("svt-rs gray encode");
    assert!(encoded.color_byte_size > 0);
    assert_eq!(encoded.alpha_byte_size, 0);

    let decoded = zenavif::decode(&encoded.avif_file).expect("must decode via rav1d-safe");
    assert_eq!(decoded.width(), 128);
    assert_eq!(decoded.height(), 128);
    // zenavif expands mono to RGB on decode; all three channels carry Y.
    let out = decoded
        .try_as_imgref::<Rgb<u8>>()
        .expect("mono decode yields RGB8");
    let mut se = 0u64;
    for (ya, pb) in img.buf().iter().zip(out.buf().iter()) {
        let d = i64::from(*ya) - i64::from(pb.g);
        se += (d * d) as u64;
    }
    let psnr = 10.0 * (255.0f64 * 255.0 / ((se as f64 / img.buf().len() as f64).max(1e-9))).log10();
    eprintln!(
        "svt_rs q85 gray roundtrip: PSNR {psnr:.2} dB, {} payload bytes",
        encoded.color_byte_size
    );
    // Measured 138.13 dB (numerically exact luma round-trip on this
    // gradient) / 214 payload bytes on 2026-07-19 at the pinned svtav1 rev
    // (3cad660b7). The floor only guards against gross regressions.
    assert!(psnr > 48.0, "gray roundtrip PSNR {psnr:.2} below floor");
}

// --------------------------------------------------------------------
// Honest scope rejection — validate() time
// --------------------------------------------------------------------

#[test]
fn svt_rs_validate_scope() {
    // In-scope config validates.
    svt_config().validate().expect("Yuv420 svt-rs validates");

    // Default 4:4:4 rejected.
    let cfg = EncoderConfig::new().backend(Av1Backend::SvtRs);
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::BackendUnsupportedParam { .. })
    ));

    // 10-bit rejected.
    let cfg = svt_config().bit_depth(EncodeBitDepth::Ten);
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::BackendUnsupportedParam { .. })
    ));

    // RGB color model rejected (Rgb+420 is globally invalid; the
    // backend check must fire before/alongside it, so use its own path:
    // Rgb+444 → still rejected for this backend).
    let cfg = EncoderConfig::new()
        .backend(Av1Backend::SvtRs)
        .color_model(EncodeColorModel::Rgb);
    assert!(cfg.validate().is_err());

    // Limited range rejected.
    let cfg = svt_config().pixel_range(EncodePixelRange::Limited);
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::BackendUnsupportedParam { .. })
    ));
}
