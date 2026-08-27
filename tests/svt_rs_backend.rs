//! svtav1-rs backend (`encode-svt-rs`) — encode/decode round-trip and
//! scope-rejection coverage.
//!
//! Everything here must pass TODAY on the pinned imazen/svtav1 rev.
//! Bitstream identity vs C SVT-AV1 is asserted UPSTREAM (the pinned rev's
//! own byte-identity battery, `rust/STATUS.md`) — duplicating those gates
//! here would pin zenavif to upstream's release cadence for no coverage
//! gain. Upstream decode conformance (aomdec, 2100 conformance cells at
//! the pin) is likewise svtav1-rs's own gate; what zenavif pins here is
//! the container + round-trip contract through its own decoder
//! (rav1d-safe) plus the seam's error/clamp composition.

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
    // Measured 52.83 dB / 1591 payload bytes on 2026-07-20 at the pinned
    // svtav1 rev (3cad660b7), x86_64, with the in-house forward RGB->YUV
    // kernel (was 51.67 dB / 1509 B via the yuv crate's converter — the
    // f32 box-average-before-quantize chroma gains ~1.2 dB here). Floor is
    // measured-minus-margin, absorbing per-arch rounding differences only.
    assert!(
        p > 45.0,
        "q85 4:2:0 svt-rs roundtrip PSNR {p:.2} dB below floor \
         (measured 52.83 dB at the pinned rev)"
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

/// Issue #32: non-64-multiple dimensions are accepted only where the port
/// codes partial superblocks C-identically (SVT preset >= 6 = speed >= 5).
/// Below that the seam keeps refusing rather than emitting a stream whose
/// partition search is known to diverge from C.
#[test]
fn svt_rs_rejects_partial_sb_dims_below_preset_6() {
    let img = gradient_rgb8(96, 96); // not a multiple of 64
    for speed in [1u8, 4] {
        // speed 4 maps to preset 4 (the default speed); speed 1 to preset 0.
        let Err(err) = zenavif::encode_rgb8(img.as_ref(), &svt_config().speed(speed), stop())
        else {
            panic!("96x96 at speed {speed} must be rejected, not padded");
        };
        let msg = err.to_string();
        assert!(msg.contains("64"), "error must explain the 64 rule: {msg}");
        assert!(
            msg.contains("speed >= 5"),
            "error must name the speed that lifts the rule: {msg}"
        );

        // Same rule at validate_for_input time.
        assert!(matches!(
            svt_config()
                .speed(speed)
                .validate_for_input(PlanInput::rgb8(96, 96)),
            Err(ValidationError::BackendUnsupportedParam { .. })
        ));
    }
    // 64-multiples stay accepted at every speed.
    svt_config()
        .speed(1)
        .validate_for_input(PlanInput::rgb8(128, 64))
        .expect("64-multiples validate at speed 1");
    // Empty images are refused at every speed.
    assert!(matches!(
        svt_config()
            .speed(10)
            .validate_for_input(PlanInput::rgb8(0, 64)),
        Err(ValidationError::BackendUnsupportedParam { .. })
    ));
}

/// Issue #32: at speed >= 5 the 4:2:0 colour path takes arbitrary
/// dimensions — partial superblocks, odd sizes, both axes partial — and
/// the container + rav1d-safe decode round-trip at the TRUE size.
#[test]
fn svt_rs_partial_sb_roundtrip_at_preset_ge_6() {
    // (96,80): one partial SB on each axis, 8-aligned.
    // (65,65): odd on both axes (upstream partial_sb_gate cell).
    // (65,72): odd width, 8-aligned height.
    // (100,37): neither axis 8-aligned, odd height.
    for (w, h) in [(96usize, 80usize), (65, 65), (65, 72), (100, 37)] {
        let img = gradient_rgb8(w, h);
        for speed in [5u8, 10] {
            let config = svt_config().quality(85.0).speed(speed);
            config
                .validate_for_input(PlanInput::rgb8(w as u32, h as u32))
                .unwrap_or_else(|e| panic!("{w}x{h} at speed {speed} must validate: {e}"));
            let encoded = zenavif::encode_rgb8(img.as_ref(), &config, stop())
                .unwrap_or_else(|e| panic!("{w}x{h} at speed {speed} must encode: {e}"));
            assert!(encoded.color_byte_size > 0);

            let decoded = zenavif::decode(&encoded.avif_file)
                .unwrap_or_else(|e| panic!("{w}x{h} at speed {speed} must decode: {e}"));
            assert_eq!(decoded.width() as usize, w, "true width must be signalled");
            assert_eq!(
                decoded.height() as usize,
                h,
                "true height must be signalled"
            );
            let out = decoded
                .try_as_imgref::<Rgb<u8>>()
                .expect("no-alpha decode yields RGB8");
            // Row-wise: the decoded buffer may be stride-padded.
            let mut se: u64 = 0;
            for (row_a, row_b) in img.rows().zip(out.rows()) {
                for (pa, pb) in row_a.iter().zip(row_b.iter()) {
                    for (ca, cb) in [(pa.r, pb.r), (pa.g, pb.g), (pa.b, pb.b)] {
                        let d = i64::from(ca) - i64::from(cb);
                        se += (d * d) as u64;
                    }
                }
            }
            let mse = se as f64 / (w * h * 3) as f64;
            let p = 10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10();
            eprintln!(
                "svt_rs partial-SB {w}x{h} speed {speed} q85: PSNR {p:.2} dB, {} payload bytes",
                encoded.color_byte_size
            );
            // Measured 2026-08-27 at the pinned rev (aarch64), q85: 47.8–50.7
            // dB across these cells at speeds 5–10 (lowest 100x37 @ speed
            // 7+), rav1d-safe and aom-rs byte-agreeing on every cell. Floor
            // is measured-minus-margin: a mis-signalled size, a stride wrap
            // or a mis-coded edge SB drops this to single digits (the mono
            // path at preset 6 measured 12–26 dB — see the canary below).
            assert!(
                p > 38.0,
                "{w}x{h} speed {speed}: partial-SB roundtrip PSNR {p:.2} dB below floor"
            );
        }
    }
}

/// Issue #32: the Cs400 alpha stream rides the port's monochrome path,
/// which mis-codes partial SBs at preset 6 and pads no partial 8x8 block —
/// so RGBA takes multiples of 8 at speed >= 6 only; speed 5 keeps the 64
/// rule and odd dimensions are refused with a reason.
#[test]
fn svt_rs_rgba_partial_sb_needs_8_aligned_dims_at_speed_6() {
    // 96x80 at speed 5 (preset 6): the colour path would take it, the
    // alpha stream must not (measured 18 dB garbage upstream).
    let img = gradient_rgba8(96, 80);
    let err = zenavif::encode_rgba8(img.as_ref(), &svt_config().speed(5), stop())
        .expect_err("96x80 RGBA at speed 5 must be refused (mono partial SBs need preset 7)");
    let msg = err.to_string();
    assert!(msg.contains("64"), "error must explain the 64 rule: {msg}");
    assert!(
        msg.contains("speed >= 6"),
        "error must name the speed that lifts the rule for alpha: {msg}"
    );
    assert!(matches!(
        svt_config()
            .speed(5)
            .validate_for_input(PlanInput::rgba8(96, 80)),
        Err(ValidationError::BackendUnsupportedParam { .. })
    ));
    // ...while the same size without alpha validates at speed 5.
    svt_config()
        .speed(5)
        .validate_for_input(PlanInput::rgb8(96, 80))
        .expect("96x80 RGB validates at speed 5");

    // 96x80 at speed 6 (preset 7): partial SBs, 8-aligned — encodes with a
    // live alpha item.
    let config = svt_config().quality(85.0).speed(6);
    config
        .validate_for_input(PlanInput::rgba8(96, 80))
        .expect("8-aligned RGBA validates at speed 6");
    let encoded = zenavif::encode_rgba8(img.as_ref(), &config, stop()).expect("96x80 RGBA encode");
    assert!(encoded.alpha_byte_size > 0, "alpha item must carry bytes");
    let decoded = zenavif::decode(&encoded.avif_file).expect("decode");
    assert_eq!((decoded.width(), decoded.height()), (96, 80));
    assert!(
        decoded.has_alpha(),
        "alpha plane must survive the container"
    );
    let out = decoded
        .try_as_imgref::<rgb::Rgba<u8>>()
        .expect("alpha decode yields RGBA8");
    let mut se_a = 0u64;
    for (row_a, row_b) in img.rows().zip(out.rows()) {
        for (pa, pb) in row_a.iter().zip(row_b.iter()) {
            let d = i64::from(pa.a) - i64::from(pb.a);
            se_a += (d * d) as u64;
        }
    }
    let psnr_a = 10.0 * (255.0f64 * 255.0 / ((se_a as f64 / (96.0 * 80.0)).max(1e-9))).log10();
    eprintln!("svt_rs 96x80 RGBA speed 6: alpha PSNR {psnr_a:.2} dB");
    // Measured 2026-08-27 (pinned rev, aarch64): mono 8-aligned partial-SB
    // cells 51–55 dB at speeds 6–10. Floor is measured-minus-margin.
    assert!(psnr_a > 38.0, "alpha PSNR {psnr_a:.2} dB below floor");

    // 65x65 at speed 6: the colour path would take it, the alpha stream
    // cannot (no partial 8x8 padding on the mono path).
    let odd = gradient_rgba8(65, 65);
    let err = zenavif::encode_rgba8(odd.as_ref(), &config, stop())
        .expect_err("65x65 RGBA at speed 6 must be refused (mono path needs multiples of 8)");
    let msg = err.to_string();
    assert!(
        msg.contains("multiples of 8"),
        "error must explain the 8-multiple alpha rule: {msg}"
    );
    assert!(matches!(
        config.validate_for_input(PlanInput::rgba8(65, 65)),
        Err(ValidationError::BackendUnsupportedParam { .. })
    ));
    // ...while the same size without alpha validates.
    config
        .validate_for_input(PlanInput::rgb8(65, 65))
        .expect("65x65 RGB validates at speed 6");
}

/// Upstream-gate canary for [`MONO_PARTIAL_SB_MIN_PRESET`]: the pinned
/// port asserts monochrome partial-SB support from preset 6, but at
/// preset 6 a 96x80 mono encode decodes to garbage (18 dB, rav1d-safe and
/// aom-rs byte-agreeing) and 128x80 / 96x64 / 200x136 are undecodable —
/// which is why the seam gates mono partial SBs at preset >= 7. This
/// drives the pipeline directly, exactly as the seam would if the gate
/// were 6. It FAILS the day upstream fixes the mono path: lower
/// `MONO_PARTIAL_SB_MIN_PRESET` to 6 and delete this test then.
#[test]
fn svt_rs_direct_mono_partial_sb_preset6_still_broken() {
    use zenavif::{DecodeBackend, decode_av1_obu_yuv};

    let (w, h) = (96usize, 80usize);
    let plane: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (((x + y) * 255) / (w + h)) as u8))
        .collect();
    let rc = svtav1::encoder::rate_control::RcConfig {
        mode: svtav1::encoder::rate_control::RcMode::Cqp,
        qp: 10,
        ..svtav1::encoder::rate_control::RcConfig::default()
    };
    let mut pipeline =
        svtav1::encoder::pipeline::EncodePipeline::new(w as u32, h as u32, 6, rc, 0, 1);
    pipeline.bit_depth = 8;
    let payload = pipeline
        .try_encode_frame(&plane, w)
        .expect("upstream accepts mono 96x80 at preset 6");
    let still_broken = match decode_av1_obu_yuv(&payload, DecodeBackend::Rav1dSafe) {
        Err(_) => true,
        Ok(dec) => {
            let mut se = 0u64;
            for y in 0..h {
                for x in 0..w {
                    let d = i64::from(plane[y * w + x]) - i64::from(dec.y[y * dec.width + x]);
                    se += (d * d) as u64;
                }
            }
            let psnr = 10.0 * (255.0f64 * 255.0 / ((se as f64 / (w * h) as f64).max(1e-9))).log10();
            eprintln!(
                "direct mono 96x80 preset 6 qp10: PSNR {psnr:.2} dB (measured 18.4 dB at q85)"
            );
            psnr < 40.0
        }
    };
    assert!(
        still_broken,
        "upstream mono partial-SB coding at preset 6 now round-trips: lower \
         MONO_PARTIAL_SB_MIN_PRESET to 6 in src/encoder_svt_rs.rs and remove this canary"
    );
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
    // Measured 52.83 dB color / 138.13 dB alpha (color 1591 B + alpha 131 B)
    // on 2026-07-20 (in-house forward kernel; RGB and RGBA color payloads
    // are byte-identical by construction now). Floors are
    // measured-minus-margin. Finding this path 20.10 dB on 2026-07-19 is
    // what uncovered the yuv-crate dropped-last-row-pair bug
    // (src/yuv_bilinear_fix.rs).
    assert!(psnr_rgb > 45.0, "RGBA color PSNR {psnr_rgb:.2} below floor");
    assert!(psnr_a > 45.0, "alpha-plane PSNR {psnr_a:.2} below floor");
}

/// Identical color payloads must decode to identical RGB whether or not an
/// alpha item is present: the RGBA decode path reuses the no-alpha path's
/// conversion kernel by construction. SvtRs encodes of the same pixels as
/// RGB and as RGBA produce byte-identical color OBUs (alpha travels as a
/// separate aux item), which makes this directly testable end-to-end.
#[test]
fn rgb_and_rgba_decodes_of_same_color_payload_agree_exactly() {
    let rgba = gradient_rgba8(128, 128);
    let rgb: Img<Vec<Rgb<u8>>> = Img::new(
        rgba.buf()
            .iter()
            .map(|p| Rgb {
                r: p.r,
                g: p.g,
                b: p.b,
            })
            .collect(),
        128,
        128,
    );
    let cfg = svt_config().quality(85.0).speed(6);
    let enc_rgb = zenavif::encode_rgb8(rgb.as_ref(), &cfg, stop()).expect("rgb encode");
    let enc_rgba = zenavif::encode_rgba8(rgba.as_ref(), &cfg, stop()).expect("rgba encode");
    assert_eq!(
        enc_rgb.color_byte_size, enc_rgba.color_byte_size,
        "premise: identical color payloads"
    );

    let dec_rgb = zenavif::decode(&enc_rgb.avif_file).expect("decode rgb file");
    let dec_rgba = zenavif::decode(&enc_rgba.avif_file).expect("decode rgba file");
    let out_rgb = dec_rgb.try_as_imgref::<Rgb<u8>>().expect("rgb out");
    let out_rgba = dec_rgba.try_as_imgref::<rgb::Rgba<u8>>().expect("rgba out");
    for (y, (row3, row4)) in out_rgb.rows().zip(out_rgba.rows()).enumerate() {
        for (x, (p3, p4)) in row3.iter().zip(row4.iter()).enumerate() {
            assert_eq!(
                (p3.r, p3.g, p3.b),
                (p4.r, p4.g, p4.b),
                "RGB-vs-RGBA decode divergence at ({x},{y})"
            );
        }
    }
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

/// Issue #32, grayscale half of the mono rule: 8-aligned partial-SB dims
/// encode as Cs400 at speed >= 6; odd dims are refused; below speed 6 the
/// 64 rule holds (preset 6 mis-codes mono partial SBs upstream).
#[cfg(feature = "encode-mono")]
#[test]
fn svt_rs_gray8_partial_sb_needs_8_aligned_dims_at_speed_6() {
    let gray = |w: usize, h: usize| -> Img<Vec<u8>> {
        let mut pixels = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                pixels.push((((x + y) * 255) / (w + h)) as u8);
            }
        }
        Img::new(pixels, w, h)
    };
    let config = svt_config().quality(85.0).speed(6);
    let img = gray(96, 80);
    let encoded = zenavif::encode_gray8(img.as_ref(), &config, stop()).expect("96x80 gray encode");
    let decoded = zenavif::decode(&encoded.avif_file).expect("decode");
    assert_eq!((decoded.width(), decoded.height()), (96, 80));
    let out = decoded
        .try_as_imgref::<Rgb<u8>>()
        .expect("mono decode yields RGB8");
    let mut se = 0u64;
    for (row_a, row_b) in img.rows().zip(out.rows()) {
        for (ya, pb) in row_a.iter().zip(row_b.iter()) {
            let d = i64::from(*ya) - i64::from(pb.g);
            se += (d * d) as u64;
        }
    }
    let psnr = 10.0 * (255.0f64 * 255.0 / ((se as f64 / (96.0 * 80.0)).max(1e-9))).log10();
    eprintln!("svt_rs 96x80 gray speed 6: PSNR {psnr:.2} dB");
    // Measured 55.2 dB on 2026-08-27 (pinned rev, aarch64); floor is
    // measured-minus-margin.
    assert!(psnr > 38.0, "gray PSNR {psnr:.2} dB below floor");

    let odd = gray(65, 72);
    let err = zenavif::encode_gray8(odd.as_ref(), &config, stop())
        .expect_err("65x72 gray at speed 6 must be refused (mono path needs multiples of 8)");
    assert!(err.to_string().contains("multiples of 8"), "got: {err}");
    let err = zenavif::encode_gray8(img.as_ref(), &config.clone().speed(5), stop())
        .expect_err("96x80 gray at speed 5 must be refused (mono partial SBs need preset 7)");
    let msg = err.to_string();
    assert!(
        msg.contains("64") && msg.contains("speed >= 6"),
        "got: {msg}"
    );
    let err = zenavif::encode_gray8(img.as_ref(), &config.clone().speed(4), stop())
        .expect_err("96x80 gray at speed 4 must be refused (64 rule below preset 6)");
    assert!(err.to_string().contains("64"), "got: {err}");
}

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

/// The unified perceptual-quality mechanism across backends: the
/// encode->decode->score secant search (`encode_rgb8_with_target`) dispatches
/// through `config.backend`, so a SvtRs config must converge on a requested
/// SSIMULACRA2 score exactly like the zenravif backend does. This is the
/// "approximate a unified ssim2 target across backends" contract: the same
/// TargetMetric lands in the same band regardless of which AV1 encoder runs.
#[cfg(feature = "target-quality")]
#[test]
fn svt_rs_target_quality_search_converges_on_ssim2() {
    use zenavif::{TargetMetric, TargetOptions, encode_rgb8_with_target};

    // Noisy gradient (192x128, 64-aligned): quantization has real work at
    // every tier, so the score-vs-quality curve brackets a mid-range target.
    // The noise is LUMA-correlated (same delta on all three channels) — pure
    // chroma noise would be destroyed by 4:2:0 subsampling regardless of
    // quality, capping the reachable ssim2 below any useful target.
    let mut state = 0x2545F491u32;
    let mut lcg = move || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (state >> 24) as u8
    };
    let (w, h) = (192usize, 128usize);
    let mut buf = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let g = ((x * 255) / w) as u8;
            let b = ((y * 255) / h) as u8;
            let n = lcg() / 6;
            buf.push(Rgb {
                r: g.saturating_add(n),
                g: b.saturating_add(n),
                b: (((g as u16 + b as u16) / 2) as u8).saturating_add(n),
            });
        }
    }
    let img = Img::new(buf, w, h);

    let target = 70.0;
    let options = TargetOptions {
        tolerance: 3.0,
        max_encodes: 8,
        ..TargetOptions::default()
    };
    let result = encode_rgb8_with_target(
        img.as_ref(),
        &svt_config().speed(6),
        TargetMetric::Ssim2(target),
        &options,
        stop(),
    )
    .expect("target search over SvtRs");

    assert!(
        result.converged,
        "SvtRs target search did not converge: score {:.2} after {} encodes",
        result.score, result.encodes
    );
    assert!(
        (result.score - target).abs() <= options.tolerance + 1e-6,
        "converged score {:.2} outside the {target}±{} band",
        result.score,
        options.tolerance
    );
    // The result must be a decodable SvtRs AVIF.
    let decoded = zenavif::decode(&result.encoded.avif_file).expect("decode targeted encode");
    assert_eq!(decoded.width(), w as u32);
    assert_eq!(decoded.height(), h as u32);
}

/// QP-0 corruption gate (2026-07-22): on the pinned svtav1-rs rev, QP 0
/// (quality >= ~99.3) emits a syntactically-valid bitstream that decodes to
/// garbage pixels (measured ssim2 ~= -700 on every q100 sweep cell, both
/// decode backends byte-agreeing on the garbage — encoder-side corruption).
/// The seam clamps QP to >= 1, so quality 100 must round-trip with HIGHER
/// fidelity than quality 90, not collapse.
#[cfg(feature = "target-quality")]
#[test]
fn svt_rs_quality_100_does_not_corrupt() {
    use fast_ssim2::compute_ssimulacra2;
    use imgref::ImgRef;

    let img = gradient_rgb8(128, 128);
    let score = |quality: f32| -> f64 {
        let enc = zenavif::encode_rgb8(img.as_ref(), &svt_config().quality(quality), stop())
            .expect("svt encode");
        let decoded = zenavif::decode(&enc.avif_file).expect("decode");
        let dec: ImgRef<'_, Rgb<u8>> = decoded.try_as_imgref().expect("rgb8 view");
        let tri = |src: ImgRef<'_, Rgb<u8>>| {
            let mut out = Vec::with_capacity(src.width() * src.height());
            for row in src.rows() {
                out.extend(row.iter().map(|p| [p.r, p.g, p.b]));
            }
            Img::new(out, src.width(), src.height())
        };
        compute_ssimulacra2(tri(img.as_ref()).as_ref(), tri(dec).as_ref()).expect("ssim2")
    };
    let s90 = score(90.0);
    let s100 = score(100.0);
    assert!(
        s100 >= s90 - 1.0 && s100 > 60.0,
        "quality 100 must not corrupt: ssim2(q100)={s100:.2} vs ssim2(q90)={s90:.2}"
    );
}

/// Upstream-gate side of the QP-0 composition (imazen/zenav1-svt#5): a
/// DIRECT qp=0 request to the pipeline must surface as a typed
/// `EncodeError::UnsupportedConfig` — not a panic, not a garbage stream.
/// The zenavif quality path can no longer reach QP 0 at all
/// (`quality_to_qp_gated` clamps to >= 1; the clamp side is covered by
/// `svt_rs_quality_100_does_not_corrupt` above), so this drives the
/// pinned pipeline directly, exactly as the seam would if the clamp were
/// ever lost — proving the failure mode is a clean typed error.
#[test]
fn svt_rs_direct_qp0_rejected_typed() {
    use svtav1::types::EncodeError;

    let rc = svtav1::encoder::rate_control::RcConfig {
        mode: svtav1::encoder::rate_control::RcMode::Cqp,
        qp: 0,
        ..svtav1::encoder::rate_control::RcConfig::default()
    };
    let mut pipeline =
        svtav1::encoder::pipeline::EncodePipeline::new(64, 64, 6, rc, 0, 1).with_chroma_420(true);
    pipeline.bit_depth = 8;
    let y = vec![128u8; 64 * 64];
    let u = vec![128u8; 32 * 32];
    let v = vec![128u8; 32 * 32];
    let err = pipeline
        .try_encode_frame_420(&y, &u, &v, 64)
        .expect_err("QP 0 (coded-lossless) must be rejected upstream (imazen/zenav1-svt#5)");
    let (err, _trace) = err.decompose();
    assert!(
        matches!(err, EncodeError::UnsupportedConfig(_)),
        "expected EncodeError::UnsupportedConfig for QP 0, got: {err:?}"
    );
}
