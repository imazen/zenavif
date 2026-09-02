//! zenav1-svt backend (`zenav1-svt`) — encode/decode round-trip and
//! scope-rejection coverage.
//!
//! Everything here must pass TODAY on the pinned imazen/svtav1 rev.
//! Bitstream identity vs C SVT-AV1 is asserted UPSTREAM (the pinned rev's
//! own byte-identity battery, `rust/STATUS.md`) — duplicating those gates
//! here would pin zenavif to upstream's release cadence for no coverage
//! gain. Upstream decode conformance (aomdec, 2100 conformance cells at
//! the pin) is likewise zenav1-svt's own gate; what zenavif pins here is
//! the container + round-trip contract through its own decoder
//! (rav1d-safe) plus the seam's error/clamp composition.

#![cfg(feature = "zenav1-svt")]

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

/// A zenav1-svt-shaped config: 4:2:0 is the only subsampling the backend
/// implements (zenavif's default is 4:4:4, which it rejects honestly).
fn svt_config() -> EncoderConfig {
    EncoderConfig::new()
        .backend(Av1Backend::Zenav1Svt)
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

/// Issue #32: the 4:2:0 colour path now takes non-64-multiple dimensions at
/// **every** speed, including the low presets that used to be refused, and
/// the result decodes correctly at the TRUE size under both decoders.
///
/// History, so the direction of this gate is unambiguous: this test used to
/// be `svt_rs_rejects_partial_sb_dims_below_preset_6`, asserting a REFUSAL
/// at speeds 1 and 4. The premise of that refusal was that upstream gated
/// its C-faithful PD1 refinement walk on a complete superblock, so presets
/// 0–5 ran a search C never runs on a partial SB. Upstream removed that
/// `full_sb` gate on 2026-08-04 and `tools/partial_sb_gate.sh` gained a
/// 23-cell presets-0–5 block, every cell byte-identical to real
/// SvtAv1EncApp v4.2.0 (146/146 aarch64 / 145/145 x86-64 CI), with the
/// anti-vacuity control measured (restoring `&& full_sb` drops it to
/// 118/141, all 23 failures inside that block). The refusal is retired, so
/// the test is REPLACED by the behaviour it was standing in for.
///
/// The residual upstream still names is `screen`-content at p0/p1/p2 (+4
/// cells at p4) — issue #71's palette/IntraBC over-picking RD class, which
/// also fires on **64-ALIGNED** 256/384/512 screen frames that this seam
/// has always accepted at every speed. It is an RD divergence, not
/// corruption (`arbitrary_size_robustness.sh` 128/128, 0 refused), so it is
/// not a dimension question and a dimension gate never addressed it. See
/// `svt_rs_dims_error`'s docs for the full argument.
///
/// Speeds 1..4 map to SVT presets 0, 1, 3, 4 — exactly the low band that
/// was refused before.
#[test]
fn svt_rs_partial_sb_roundtrip_at_low_presets() {
    // (96,96) is the geometry the old refusal test used; (65,72) is odd on
    // one axis and 8-aligned on the other; (100,37) is neither axis
    // 8-aligned with an odd height.
    for (w, h) in [(96usize, 96usize), (65, 72), (100, 37)] {
        let img = gradient_rgb8(w, h);
        for speed in [1u8, 2, 3, 4] {
            let config = svt_config().quality(85.0).speed(speed);
            config
                .validate_for_input(PlanInput::rgb8(w as u32, h as u32))
                .unwrap_or_else(|e| {
                    panic!("{w}x{h} at speed {speed} must validate (the preset floor is gone): {e}")
                });
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
            // Mirror of `encoder_svt_rs::speed_to_svt_preset` (crate-private).
            let preset = ((u32::from(speed).clamp(1, 10) - 1) * 13 + 4) / 9;
            eprintln!(
                "svt_rs low-preset partial-SB {w}x{h} speed {speed} (SVT preset {preset}) q85: \
                 PSNR {p:.2} dB, {} payload bytes",
                encoded.color_byte_size
            );
            // Same floor and rationale as `svt_rs_partial_sb_roundtrip_at_preset_ge_6`:
            // a mis-signalled size, a stride wrap or a mis-coded edge SB
            // drops this to single digits (the mono path measured 12-26 dB
            // when it was actually broken).
            assert!(
                p > 38.0,
                "{w}x{h} speed {speed}: low-preset partial-SB roundtrip PSNR {p:.2} dB below floor"
            );
        }
    }

    // CROSS-DECODER BYTE AGREEMENT on the raw payload, driving the pipeline
    // directly at the low presets. A PSNR floor through one decoder cannot
    // tell "the encoder coded this edge SB correctly" from "this decoder
    // tolerated a mis-coded one" — that distinction is exactly what caught
    // the mono edge-leaf bug (`svt_rs_direct_mono_partial_sb_preset6_
    // roundtrips`: aomdec decoded a stream rav1d-safe scored at 27.89 dB).
    // The colour path gets the same oracle here.
    {
        use zenavif::{DecodeBackend, decode_av1_obu_yuv};

        for (w, h) in [(96usize, 96usize), (65, 72), (100, 37)] {
            for preset in [0u8, 1, 3, 4] {
                let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
                let y: Vec<u8> = (0..h)
                    .flat_map(|r| (0..w).map(move |c| (((r + c) * 255) / (w + h)) as u8))
                    .collect();
                let u: Vec<u8> = (0..ch)
                    .flat_map(|r| (0..cw).map(move |c| (128 + ((r + c) % 64) as u8) & 0xf0))
                    .collect();
                let v: Vec<u8> = (0..ch)
                    .flat_map(|r| (0..cw).map(move |c| (128 - ((r * 3 + c) % 48) as u8) | 0x03))
                    .collect();
                let rc = svtav1::encoder::rate_control::RcConfig {
                    mode: svtav1::encoder::rate_control::RcMode::Cqp,
                    qp: 20,
                    ..svtav1::encoder::rate_control::RcConfig::default()
                };
                let mut pipeline = svtav1::encoder::pipeline::EncodePipeline::new(
                    w as u32, h as u32, preset, rc, 0, 1,
                )
                .with_chroma_420(true);
                pipeline.bit_depth = 8;
                let payload = pipeline
                    .try_encode_frame_420(&y, &u, &v, w)
                    .unwrap_or_else(|e| {
                        panic!("upstream must accept 4:2:0 {w}x{h} at preset {preset}: {e}")
                    });

                let dec =
                    decode_av1_obu_yuv(&payload, DecodeBackend::Rav1dSafe).unwrap_or_else(|e| {
                        panic!("{w}x{h} preset {preset} must decode under rav1d-safe: {e}")
                    });
                assert_eq!((dec.width, dec.height), (w, h), "signalled size {w}x{h}");
                assert!(!dec.monochrome, "{w}x{h} preset {preset}: 4:2:0 stream");
                let mut se = 0u64;
                for r in 0..h {
                    for c in 0..w {
                        let d = i64::from(y[r * w + c]) - i64::from(dec.y[r * dec.width + c]);
                        se += (d * d) as u64;
                    }
                }
                let psnr =
                    10.0 * (255.0f64 * 255.0 / ((se as f64 / (w * h) as f64).max(1e-9))).log10();
                eprintln!("direct 4:2:0 {w}x{h} preset {preset} qp20: luma PSNR {psnr:.2} dB");
                assert!(
                    psnr > 45.0,
                    "4:2:0 {w}x{h} preset {preset} luma PSNR {psnr:.2} dB below floor — the \
                     partial-SB edge coding on the colour path has regressed"
                );

                #[cfg(feature = "zenav1-aom")]
                {
                    let aom = decode_av1_obu_yuv(&payload, DecodeBackend::Zenav1Aom)
                        .unwrap_or_else(|_| {
                            let mut with_td = vec![0x12, 0x00];
                            with_td.extend_from_slice(&payload);
                            decode_av1_obu_yuv(&with_td, DecodeBackend::Zenav1Aom).unwrap_or_else(
                                |e| {
                                    panic!(
                                        "{w}x{h} preset {preset} must decode under zenav1-aom: {e}"
                                    )
                                },
                            )
                        });
                    assert_eq!(
                        (aom.y, aom.u, aom.v),
                        (dec.y.clone(), dec.u.clone(), dec.v.clone()),
                        "rav1d-safe and zenav1-aom must byte-agree on 4:2:0 {w}x{h} preset {preset}"
                    );
                }
            }
        }
    }

    // 64-multiples stay accepted at every speed.
    svt_config()
        .speed(1)
        .validate_for_input(PlanInput::rgb8(128, 64))
        .expect("64-multiples validate at speed 1");
    // Empty images are still refused at every speed.
    assert!(matches!(
        svt_config()
            .speed(10)
            .validate_for_input(PlanInput::rgb8(0, 64)),
        Err(ValidationError::BackendUnsupportedParam { .. })
    ));
}

/// The MONO half of the dimension envelope keeps its preset floor, and it
/// must keep refusing below it: nothing upstream measures a monochrome
/// partial superblock below SVT preset 6 (`partial_sb_gate` is bd8 4:2:0 by
/// its own scope line; the mono evidence is the preset-6 edge-leaf fix
/// `b6a1737a` + `1ed7db46`). A grayscale or alpha-carrying encode at
/// non-64-multiple dims below speed 5 is therefore still a typed refusal,
/// even though the colour path at the same speed now encodes.
#[test]
fn svt_rs_mono_partial_sb_still_refused_below_preset_6() {
    for speed in [1u8, 4] {
        // RGBA: the alpha auxiliary item is the Cs400 stream.
        let img = gradient_rgba8(96, 96);
        let Err(err) = zenavif::encode_rgba8(img.as_ref(), &svt_config().speed(speed), stop())
        else {
            panic!("RGBA 96x96 at speed {speed} must be refused (mono preset floor)");
        };
        let msg = err.to_string();
        assert!(msg.contains("64"), "error must explain the 64 rule: {msg}");
        assert!(
            msg.contains("speed >= 5"),
            "error must name the speed that lifts the rule: {msg}"
        );
        assert!(
            msg.contains("grayscale") || msg.contains("alpha"),
            "the refusal must name the Cs400 path it is about: {msg}"
        );

        // Same rule at validate_for_input time.
        assert!(matches!(
            svt_config()
                .speed(speed)
                .validate_for_input(PlanInput::rgba8(96, 96)),
            Err(ValidationError::BackendUnsupportedParam { .. })
        ));
        // ...while the SAME geometry without alpha now validates.
        svt_config()
            .speed(speed)
            .validate_for_input(PlanInput::rgb8(96, 96))
            .unwrap_or_else(|e| panic!("RGB 96x96 at speed {speed} must validate: {e}"));
    }
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
            // 7+), rav1d-safe and zenav1-aom byte-agreeing on every cell. Floor
            // is measured-minus-margin: a mis-signalled size, a stride wrap
            // or a mis-coded edge SB drops this to single digits (the mono
            // path at preset 6 measured 12–26 dB before zenav1-svt
            // `b6a1737a` + `1ed7db46` — see `svt_rs_direct_mono_partial_sb_preset6_roundtrips`).
            assert!(
                p > 38.0,
                "{w}x{h} speed {speed}: partial-SB roundtrip PSNR {p:.2} dB below floor"
            );
        }
    }
}

/// Issue #32: the Cs400 alpha stream rides the port's monochrome path,
/// which pads no partial 8x8 block — so RGBA takes multiples of 8 from
/// speed 5 (SVT preset 6); below that the 64 rule holds for the alpha
/// stream and odd dimensions are refused with a reason. (Until zenav1-svt
/// `b6a1737a` + `1ed7db46` the mono path also mis-coded partial SBs at
/// preset 6 and this test pinned speed 5 as REFUSED for RGBA; see
/// `svt_rs_direct_mono_partial_sb_preset6_roundtrips`.)
#[test]
fn svt_rs_rgba_partial_sb_needs_8_aligned_dims_at_speed_5() {
    // 96x80 at speed 4 (preset 4): the 64 rule, ALPHA ONLY — the 4:2:0
    // colour path takes this geometry at every speed now, so what refuses
    // here is the Cs400 alpha item (see
    // `svt_rs_mono_partial_sb_still_refused_below_preset_6`).
    let img = gradient_rgba8(96, 80);
    let err = zenavif::encode_rgba8(img.as_ref(), &svt_config().speed(4), stop())
        .expect_err("96x80 RGBA at speed 4 must be refused (64 rule below preset 6)");
    let msg = err.to_string();
    assert!(msg.contains("64"), "error must explain the 64 rule: {msg}");
    assert!(
        msg.contains("speed >= 5"),
        "error must name the speed that lifts the rule: {msg}"
    );
    assert!(matches!(
        svt_config()
            .speed(4)
            .validate_for_input(PlanInput::rgba8(96, 80)),
        Err(ValidationError::BackendUnsupportedParam { .. })
    ));

    // 96x80 at speed 5 (preset 6): partial SBs, 8-aligned — encodes with a
    // live alpha item on the mono path.
    let config = svt_config().quality(85.0).speed(5);
    config
        .validate_for_input(PlanInput::rgba8(96, 80))
        .expect("8-aligned RGBA validates at speed 5");
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
    eprintln!("svt_rs 96x80 RGBA speed 5: alpha PSNR {psnr_a:.2} dB");
    // Measured 2026-08-27 (zenav1-svt b6a1737a, aarch64): mono 8-aligned
    // partial-SB cells 51–55 dB at speeds 6–10 and 96x80 at speed 5 in the
    // same band once the edge-leaf fix landed. Floor is
    // measured-minus-margin; the pre-fix mono path measured 12–26 dB.
    assert!(psnr_a > 38.0, "alpha PSNR {psnr_a:.2} dB below floor");

    // 65x65 at speed 5: the colour path would take it, the alpha stream
    // cannot (no partial 8x8 padding on the mono path).
    let odd = gradient_rgba8(65, 65);
    let err = zenavif::encode_rgba8(odd.as_ref(), &config, stop())
        .expect_err("65x65 RGBA at speed 5 must be refused (mono path needs multiples of 8)");
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
        .expect("65x65 RGB validates at speed 5");
}

/// Round-trip gate for the port's MONOCHROME partial-SB path at SVT
/// preset 6 (zenavif speed 5), driving the pipeline directly as the seam
/// does. History: on the 2026-08-27 tree every 8-aligned non-64-multiple
/// mono cell at preset 6 was mis-coded — 96x80 18 dB garbage (rav1d-safe
/// and zenav1-aom byte-agreeing), 64x72 / 72x64 26 dB, 16x72 12 dB, 128x80 /
/// 96x64 / 200x136 undecodable — because `encode_fixed_tree`'s mono arm
/// coded a one-false edge leaf as a PARTITION_NONE square (its pack
/// `debug_assert` is what made the old `still_broken` canary PANIC in the
/// dev profile). zenav1-svt `b6a1737a` + `1ed7db46` codes the single legal HORZ/VERT
/// rect instead. That exposed a second defect this gate caught on its first
/// run: the now-rect edge block STRADDLES a thin right edge (200x136 —
/// aligned width ≡ 8 mod 64) and the mono leaf coder's recon store wrapped
/// into the next row, corrupting the encoder's above reference for every
/// later SB row (rav1d-safe 27.89 dB, first SB row 55 dB, second row 23 dB
/// at column 0; aomdec still decoded it). zenav1-svt `1ed7db46` clips the
/// store. Measured after both fixes (rav1d-safe, qp 10): 96x80 56.18 dB,
/// 200x136 56.96 dB, every geometry 55–57 dB. Floor is measured-minus-
/// margin; the broken paths sat at 12–28 dB or failed to decode, so 45 dB
/// separates them by a wide band. Every geometry here exercises a
/// right-edge, bottom-edge and/or both-false superblock; 200x136 is the
/// thin-right-edge + multi-row case only the straddle clip fixes.
#[test]
fn svt_rs_direct_mono_partial_sb_preset6_roundtrips() {
    use zenavif::{DecodeBackend, decode_av1_obu_yuv};

    for (w, h) in [
        (96usize, 80usize),
        (64, 72),
        (72, 64),
        (16, 72),
        (128, 80),
        (96, 64),
        (200, 136),
    ] {
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
            .unwrap_or_else(|e| panic!("upstream must accept mono {w}x{h} at preset 6: {e}"));
        let dec = decode_av1_obu_yuv(&payload, DecodeBackend::Rav1dSafe)
            .unwrap_or_else(|e| panic!("mono {w}x{h} preset 6 must decode under rav1d-safe: {e}"));
        assert_eq!(
            (dec.width, dec.height),
            (w, h),
            "signalled size for {w}x{h}"
        );
        assert!(dec.monochrome, "{w}x{h} must be a mono_chrome stream");
        let mut se = 0u64;
        for y in 0..h {
            for x in 0..w {
                let d = i64::from(plane[y * w + x]) - i64::from(dec.y[y * dec.width + x]);
                se += (d * d) as u64;
            }
        }
        let psnr = 10.0 * (255.0f64 * 255.0 / ((se as f64 / (w * h) as f64).max(1e-9))).log10();
        eprintln!("direct mono {w}x{h} preset 6 qp10: PSNR {psnr:.2} dB (96x80 measured 56.18 dB)");
        assert!(
            psnr > 45.0,
            "mono {w}x{h} preset 6 round-trip PSNR {psnr:.2} dB below floor — the partial-SB \
             edge coding on the mono path has regressed (zenav1-svt b6a1737a + 1ed7db46)"
        );
        // Cross-decoder byte agreement: a mis-coded edge SB that one decoder
        // happens to tolerate must not pass on that decoder's leniency.
        #[cfg(feature = "zenav1-aom")]
        {
            let aom = decode_av1_obu_yuv(&payload, DecodeBackend::Zenav1Aom).unwrap_or_else(|_| {
                let mut with_td = vec![0x12, 0x00];
                with_td.extend_from_slice(&payload);
                decode_av1_obu_yuv(&with_td, DecodeBackend::Zenav1Aom).unwrap_or_else(|e| {
                    panic!("mono {w}x{h} preset 6 must decode under zenav1-aom: {e}")
                })
            });
            assert_eq!(
                aom.y, dec.y,
                "rav1d-safe and zenav1-aom must byte-agree on mono {w}x{h} preset 6"
            );
        }
    }
}

#[test]
fn svt_rs_rejects_default_yuv444_at_encode_time() {
    let img = gradient_rgb8(64, 64);
    let config = EncoderConfig::new().backend(Av1Backend::Zenav1Svt); // 4:4:4 default
    let err = zenavif::encode_rgb8(img.as_ref(), &config, stop())
        .expect_err("4:4:4 must be rejected, not silently downsampled");
    assert!(err.to_string().contains("4:2:0"), "got: {err}");
}

// --------------------------------------------------------------------
// 10-bit (issue #33): 16-bit input and EncodeBitDepth::Ten
// --------------------------------------------------------------------

/// HDR-shaped 16-bit gradient (smooth, so 4:2:0 loss stays small and the
/// PSNR floor speaks about the 10-bit codec path).
fn gradient_rgb16(w: usize, h: usize) -> Img<Vec<Rgb<u16>>> {
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            pixels.push(Rgb {
                r: ((x * 65535) / w.max(1)) as u16,
                g: ((y * 65535) / h.max(1)) as u16,
                b: (((x + y) * 65535) / (w + h).max(1)) as u16,
            });
        }
    }
    Img::new(pixels, w, h)
}

/// PSNR in the native 10-bit domain between a 16-bit source and a 16-bit
/// decode (both `>> 6`; the decoder expands 10-bit by LSB replication, so
/// the shift inverts it exactly).
fn psnr_10bit<P: Copy>(
    src: &Img<Vec<P>>,
    out: imgref::ImgRef<'_, P>,
    channels: impl Fn(P) -> [u16; 3],
) -> f64 {
    let mut se = 0u64;
    let mut n = 0u64;
    for (row_a, row_b) in src.rows().zip(out.rows()) {
        for (pa, pb) in row_a.iter().zip(row_b.iter()) {
            for (a, b) in channels(*pa).into_iter().zip(channels(*pb)) {
                let d = i64::from(a >> 6) - i64::from(b >> 6);
                se += (d * d) as u64;
                n += 1;
            }
        }
    }
    10.0 * (1023.0f64 * 1023.0 / ((se as f64 / n as f64).max(1e-9))).log10()
}

fn decode_16bit(avif: &[u8]) -> zenavif::PixelBuffer {
    zenavif::decode_with(
        avif,
        &zenavif::DecoderConfig::new().prefer_8bit(false),
        &Unstoppable,
    )
    .expect("must decode via rav1d-safe")
}

/// Issue #33: 16-bit RGB codes a 10-bit profile-0 4:2:0 stream, with
/// BT.2020/PQ CICP + clli + mdcv carried by the container, on a
/// partial-SB geometry (96x80, speed 5) — the product HDR shape.
#[test]
fn svt_rs_rgb16_roundtrip_10bit_pq_with_hdr_metadata() {
    let (w, h) = (96usize, 80usize);
    let img = gradient_rgb16(w, h);
    let config = svt_config()
        .quality(85.0)
        .speed(5)
        .color_primaries(9) // BT.2020
        .transfer_characteristics(16) // PQ
        .content_light_level(4000, 400)
        .mastering_display(zenavif::MasteringDisplayConfig {
            primaries: [(8500, 39850), (6550, 2300), (35400, 14600)],
            white_point: (15635, 16450),
            max_luminance: 1000 * 10000,
            min_luminance: 50,
        });
    config
        .validate_for_input(PlanInput {
            width: w as u32,
            height: h as u32,
            input_is_16bit: true,
            input_has_alpha: false,
        })
        .expect("16-bit RGB over Zenav1Svt must validate");

    let encoded =
        zenavif::encode_rgb16(img.as_ref(), &config, stop()).expect("svt-rs RGB16 encode");
    assert!(encoded.color_byte_size > 0);
    assert_eq!(encoded.alpha_byte_size, 0);

    let decoder =
        zenavif::ManagedAvifDecoder::new(&encoded.avif_file, &zenavif::DecoderConfig::default())
            .expect("parse");
    let info = decoder.probe_info().expect("probe");
    assert_eq!((info.width, info.height), (w as u32, h as u32));
    assert_eq!(info.bit_depth, 10, "16-bit input must produce 10-bit AV1");
    assert!(!info.monochrome);
    assert_eq!(info.color_primaries.0, 9);
    assert_eq!(info.transfer_characteristics.0, 16);
    assert_eq!(info.matrix_coefficients.0, 6, "must signal BT.601");
    assert_eq!(info.color_range, zenavif::ColorRange::Full);
    let cll = info.content_light_level.expect("clli must survive");
    assert_eq!(cll.max_content_light_level, 4000);
    assert_eq!(cll.max_pic_average_light_level, 400);
    let md = info.mastering_display.expect("mdcv must survive");
    assert_eq!(md.primaries, [(8500, 39850), (6550, 2300), (35400, 14600)]);
    assert_eq!(md.white_point, (15635, 16450));
    assert_eq!(md.max_luminance, 1000 * 10000);
    assert_eq!(md.min_luminance, 50);

    let pixels = decode_16bit(&encoded.avif_file);
    let out = pixels
        .try_as_imgref::<Rgb<u16>>()
        .expect("10-bit decode must expose an Rgb16 view");
    assert_eq!((out.width(), out.height()), (w, h));
    let p = psnr_10bit(&img, out, |p| [p.r, p.g, p.b]);
    eprintln!("svt_rs RGB16 -> 10-bit 96x80 speed 5 q85: PSNR(10-bit) {p:.2} dB");
    // Measured 54.87 dB on 2026-08-27 (pinned rev, aarch64). Floor is
    // measured-minus-margin; a mis-signalled geometry or a truncated
    // conversion lands far below it (the low-bit proof is
    // `svt_rs_10bit_path_keeps_low_bits_vs_8bit_at_qp_floor`).
    assert!(
        p > 40.0,
        "RGB16 10-bit roundtrip PSNR {p:.2} dB below floor"
    );
}

/// Issue #33 precision proof: at the QP floor the 10-bit path must
/// reconstruct a 16-bit gradient with far less 10-bit-domain error than
/// the 8-bit path from the same source. An encode that quantized at 8 bits
/// under a 10-bit sequence header (the failure the port's
/// `bit_depth_config_error` exists to refuse) would land at the 8-bit
/// figure, not below it.
#[test]
fn svt_rs_10bit_path_keeps_low_bits_vs_8bit_at_qp_floor() {
    let (w, h) = (128usize, 64usize);
    let img = gradient_rgb16(w, h);
    let rms = |depth: EncodeBitDepth| -> f64 {
        let config = svt_config().quality(100.0).speed(6).bit_depth(depth);
        let encoded = zenavif::encode_rgb16(img.as_ref(), &config, stop()).expect("encode");
        let pixels = decode_16bit(&encoded.avif_file);
        // Decoded samples on the 10-bit grid: a 10-bit stream decodes to
        // Rgb16 by LSB replication (>> 6 inverts it exactly); an 8-bit
        // stream decodes to Rgb8, mapped by the exact inverse of the
        // widening (round(v * 1023 / 255)).
        let decoded10: Vec<[u16; 3]> = if let Some(out) = pixels.try_as_imgref::<Rgb<u16>>() {
            out.rows()
                .flat_map(|r| r.iter().map(|p| [p.r >> 6, p.g >> 6, p.b >> 6]))
                .collect()
        } else {
            let out = pixels.try_as_imgref::<Rgb<u8>>().expect("Rgb8 view");
            let up = |v: u8| ((u32::from(v) * 1023 + 127) / 255) as u16;
            out.rows()
                .flat_map(|r| r.iter().map(|p| [up(p.r), up(p.g), up(p.b)]))
                .collect()
        };
        let mut se = 0f64;
        let mut n = 0f64;
        for (pa, pb) in img.buf().iter().zip(decoded10.iter()) {
            for (a, b) in [pa.r, pa.g, pa.b].into_iter().zip(*pb) {
                let d = f64::from(a >> 6) - f64::from(b);
                se += d * d;
                n += 1.0;
            }
        }
        (se / n).sqrt()
    };
    let rms8 = rms(EncodeBitDepth::Eight);
    let rms10 = rms(EncodeBitDepth::Ten);
    eprintln!(
        "svt_rs RGB16 128x64 speed 6 q100: 10-bit-domain RMS error 8-bit {rms8:.3} vs 10-bit {rms10:.3}"
    );
    // Measured 2026-08-27 (pinned rev, aarch64): 8-bit 2.328 vs 10-bit
    // 1.066 (ratio 2.18; the 8-bit quantization floor alone is ~1.15 RMS
    // on this gradient, so the 10-bit figure can only be reached with the
    // low bits coded). Gate at 1.5x = measured-minus-margin; an 8-bit-
    // quantized stream under a 10-bit header sits at ratio ~1.0.
    assert!(
        rms10 * 1.5 < rms8,
        "10-bit path must beat the 8-bit path by 1.5x in 10-bit-domain RMS at the QP floor: \
         8-bit {rms8:.3}, 10-bit {rms10:.3}"
    );
}

/// Issue #33: `EncodeBitDepth::Ten` on 8-bit input codes a 10-bit stream
/// (RGB -> YCbCr at 10-bit precision) that round-trips the 8-bit source.
#[test]
fn svt_rs_rgb8_bit_depth_ten_codes_10bit_stream() {
    let (w, h) = (128usize, 64usize);
    let img = gradient_rgb8(w, h);
    let config = svt_config()
        .quality(85.0)
        .speed(6)
        .bit_depth(EncodeBitDepth::Ten);
    config.validate().expect("Ten validates over Zenav1Svt");
    config
        .validate_for_input(PlanInput::rgb8(w as u32, h as u32))
        .expect("RGB8 + Ten validates");
    let encoded = zenavif::encode_rgb8(img.as_ref(), &config, stop()).expect("RGB8 + Ten encode");

    let decoder =
        zenavif::ManagedAvifDecoder::new(&encoded.avif_file, &zenavif::DecoderConfig::default())
            .expect("parse");
    assert_eq!(decoder.probe_info().expect("probe").bit_depth, 10);

    let pixels = decode_16bit(&encoded.avif_file);
    let out = pixels
        .try_as_imgref::<Rgb<u16>>()
        .expect("10-bit decode yields Rgb16");
    // Compare in the 8-bit domain of the source: decoded 16-bit >> 8.
    let mut se = 0u64;
    for (row_a, row_b) in img.rows().zip(out.rows()) {
        for (pa, pb) in row_a.iter().zip(row_b.iter()) {
            for (a, b) in [(pa.r, pb.r), (pa.g, pb.g), (pa.b, pb.b)] {
                let d = i64::from(a) - i64::from(b >> 8);
                se += (d * d) as u64;
            }
        }
    }
    let p = 10.0 * (255.0f64 * 255.0 / ((se as f64 / (w * h * 3) as f64).max(1e-9))).log10();
    eprintln!("svt_rs RGB8 + Ten 128x64 speed 6 q85: PSNR {p:.2} dB");
    // Measured 54.13 dB on 2026-08-27 (pinned rev, aarch64).
    assert!(p > 40.0, "RGB8 + Ten roundtrip PSNR {p:.2} dB below floor");
}

/// Issue #33: a 10-bit alpha item rides the port's bd10 monochrome level
/// pass, which exists at SVT preset >= 9 (speed >= 7) only — RGBA16 and
/// RGBA8 + Ten are refused below that with a reason, and round-trip
/// (colour + alpha, 10-bit) at speed 7.
#[test]
fn svt_rs_10bit_alpha_needs_speed_7() {
    let (w, h) = (96usize, 80usize);
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            pixels.push(rgb::Rgba {
                r: ((x * 65535) / w) as u16,
                g: ((y * 65535) / h) as u16,
                b: (((x + y) * 65535) / (w + h)) as u16,
                a: (16384 + (x * 49151) / w) as u16,
            });
        }
    }
    let img: Img<Vec<rgb::Rgba<u16>>> = Img::new(pixels, w, h);
    let plan = PlanInput {
        width: w as u32,
        height: h as u32,
        input_is_16bit: true,
        input_has_alpha: true,
    };

    // speed 6 = preset 7: refused.
    let cfg6 = svt_config().quality(85.0).speed(6);
    let err = zenavif::encode_rgba16(img.as_ref(), &cfg6, stop())
        .expect_err("RGBA16 at speed 6 must be refused (10-bit mono needs preset 9)");
    let msg = err.to_string();
    assert!(
        msg.contains("speed >= 7"),
        "must name the speed that lifts the rule: {msg}"
    );
    assert!(matches!(
        cfg6.validate_for_input(plan),
        Err(ValidationError::BackendUnsupportedParam { .. })
    ));
    // Same rule for 8-bit RGBA asked to code 10-bit.
    let rgba8 = gradient_rgba8(w, h);
    let cfg6_ten = cfg6.clone().bit_depth(EncodeBitDepth::Ten);
    assert!(
        zenavif::encode_rgba8(rgba8.as_ref(), &cfg6_ten, stop()).is_err(),
        "RGBA8 + Ten at speed 6 must be refused"
    );
    assert!(matches!(
        cfg6_ten.validate_for_input(PlanInput::rgba8(w as u32, h as u32)),
        Err(ValidationError::BackendUnsupportedParam { .. })
    ));
    // ...while 8-bit RGBA at speed 6 and 16-bit RGB at speed 6 both validate.
    cfg6.validate_for_input(PlanInput::rgba8(w as u32, h as u32))
        .expect("RGBA8 at speed 6 validates");
    cfg6.validate_for_input(PlanInput {
        input_has_alpha: false,
        ..plan
    })
    .expect("RGB16 at speed 6 validates");

    // speed 7 = preset 9: colour + 10-bit alpha round-trip.
    let cfg7 = svt_config().quality(85.0).speed(7);
    cfg7.validate_for_input(plan)
        .expect("RGBA16 at speed 7 validates");
    let encoded = zenavif::encode_rgba16(img.as_ref(), &cfg7, stop()).expect("RGBA16 encode");
    assert!(encoded.alpha_byte_size > 0, "alpha item must carry bytes");
    let decoder =
        zenavif::ManagedAvifDecoder::new(&encoded.avif_file, &zenavif::DecoderConfig::default())
            .expect("parse");
    let info = decoder.probe_info().expect("probe");
    assert_eq!(info.bit_depth, 10);
    assert!(info.has_alpha);
    let pixels = decode_16bit(&encoded.avif_file);
    let out = pixels
        .try_as_imgref::<rgb::Rgba<u16>>()
        .expect("10-bit alpha decode yields Rgba16");
    assert_eq!((out.width(), out.height()), (w, h));
    let p_rgb = psnr_10bit(&img, out, |p| [p.r, p.g, p.b]);
    let p_a = psnr_10bit(&img, out, |p| [p.a, p.a, p.a]);
    eprintln!(
        "svt_rs RGBA16 -> 10-bit 96x80 speed 7 q85: PSNR(10-bit) rgb {p_rgb:.2} dB, alpha {p_a:.2} dB"
    );
    // Measured 54.68 / 62.93 dB on 2026-08-27 (pinned rev, aarch64).
    assert!(p_rgb > 40.0, "RGBA16 colour PSNR {p_rgb:.2} dB below floor");
    assert!(p_a > 40.0, "RGBA16 alpha PSNR {p_a:.2} dB below floor");

    // RGBA8 + Ten at speed 7 also encodes 10-bit with alpha.
    let cfg7_ten = cfg7.clone().bit_depth(EncodeBitDepth::Ten);
    let encoded = zenavif::encode_rgba8(rgba8.as_ref(), &cfg7_ten, stop()).expect("RGBA8 + Ten");
    let decoder =
        zenavif::ManagedAvifDecoder::new(&encoded.avif_file, &zenavif::DecoderConfig::default())
            .expect("parse");
    let info = decoder.probe_info().expect("probe");
    assert_eq!(info.bit_depth, 10);
    assert!(info.has_alpha);
}

/// Issue #33, grayscale: `EncodeBitDepth::Ten` widens to a 10-bit Cs400
/// stream at speed >= 7 and is refused below.
#[cfg(feature = "encode-mono")]
#[test]
fn svt_rs_gray8_bit_depth_ten_needs_speed_7() {
    let (w, h) = (128usize, 64usize);
    let pixels: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (((x + y) * 255) / (w + h)) as u8))
        .collect();
    let img: Img<Vec<u8>> = Img::new(pixels, w, h);
    let cfg6 = svt_config()
        .quality(85.0)
        .speed(6)
        .bit_depth(EncodeBitDepth::Ten);
    let err = zenavif::encode_gray8(img.as_ref(), &cfg6, stop())
        .expect_err("gray + Ten at speed 6 must be refused");
    assert!(err.to_string().contains("speed >= 7"), "got: {err}");

    let cfg7 = cfg6.clone().speed(7);
    let encoded = zenavif::encode_gray8(img.as_ref(), &cfg7, stop()).expect("gray + Ten encode");
    let decoder =
        zenavif::ManagedAvifDecoder::new(&encoded.avif_file, &zenavif::DecoderConfig::default())
            .expect("parse");
    let info = decoder.probe_info().expect("probe");
    assert_eq!(info.bit_depth, 10);
    assert!(info.monochrome);
    let pixels = decode_16bit(&encoded.avif_file);
    let out = pixels
        .try_as_imgref::<Rgb<u16>>()
        .expect("10-bit mono decode yields Rgb16");
    let mut se = 0u64;
    for (row_a, row_b) in img.rows().zip(out.rows()) {
        for (ya, pb) in row_a.iter().zip(row_b.iter()) {
            let d = i64::from(*ya) - i64::from(pb.g >> 8);
            se += (d * d) as u64;
        }
    }
    let p = 10.0 * (255.0f64 * 255.0 / ((se as f64 / (w * h) as f64).max(1e-9))).log10();
    eprintln!("svt_rs gray8 + Ten 128x64 speed 7 q85: PSNR {p:.2} dB");
    // Measured 54.67 dB on 2026-08-27 (pinned rev, aarch64).
    assert!(p > 40.0, "gray + Ten PSNR {p:.2} dB below floor");
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
/// conversion kernel by construction. Zenav1Svt encodes of the same pixels as
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
/// encode as Cs400 from speed 5 (SVT preset 6, since zenav1-svt
/// `b6a1737a` + `1ed7db46` fixed the mono edge-leaf coding there); odd dims are
/// refused; below speed 5 the 64 rule holds.
#[cfg(feature = "encode-mono")]
#[test]
fn svt_rs_gray8_partial_sb_needs_8_aligned_dims_at_speed_5() {
    let gray = |w: usize, h: usize| -> Img<Vec<u8>> {
        let mut pixels = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                pixels.push((((x + y) * 255) / (w + h)) as u8);
            }
        }
        Img::new(pixels, w, h)
    };
    let config = svt_config().quality(85.0).speed(5);
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
    eprintln!("svt_rs 96x80 gray speed 5: PSNR {psnr:.2} dB");
    // Measured 55.2 dB at speed 6 on 2026-08-27 (pre-fix tree, aarch64) and
    // in the same band at speed 5 once zenav1-svt b6a1737a landed; floor is
    // measured-minus-margin (the pre-fix speed-5 path measured 12–26 dB).
    assert!(psnr > 38.0, "gray PSNR {psnr:.2} dB below floor");

    let odd = gray(65, 72);
    let err = zenavif::encode_gray8(odd.as_ref(), &config, stop())
        .expect_err("65x72 gray at speed 5 must be refused (mono path needs multiples of 8)");
    assert!(err.to_string().contains("multiples of 8"), "got: {err}");
    let err = zenavif::encode_gray8(img.as_ref(), &config.clone().speed(4), stop())
        .expect_err("96x80 gray at speed 4 must be refused (64 rule below preset 6)");
    let msg = err.to_string();
    assert!(
        msg.contains("64") && msg.contains("speed >= 5"),
        "got: {msg}"
    );
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
    let cfg = EncoderConfig::new().backend(Av1Backend::Zenav1Svt);
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::BackendUnsupportedParam { .. })
    ));

    // 10-bit validates (issue #33); the alpha/gray speed rule is a
    // config x input concern (validate_for_input).
    svt_config()
        .bit_depth(EncodeBitDepth::Ten)
        .validate()
        .expect("Ten validates over Zenav1Svt");

    // RGB color model rejected (Rgb+420 is globally invalid; the
    // backend check must fire before/alongside it, so use its own path:
    // Rgb+444 → still rejected for this backend).
    let cfg = EncoderConfig::new()
        .backend(Av1Backend::Zenav1Svt)
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
/// through `config.backend`, so a Zenav1Svt config must converge on a requested
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
    .expect("target search over Zenav1Svt");

    assert!(
        result.converged,
        "Zenav1Svt target search did not converge: score {:.2} after {} encodes",
        result.score, result.encodes
    );
    assert!(
        (result.score - target).abs() <= options.tolerance + 1e-6,
        "converged score {:.2} outside the {target}±{} band",
        result.score,
        options.tolerance
    );
    // The result must be a decodable Zenav1Svt AVIF.
    let decoded = zenavif::decode(&result.encoded.avif_file).expect("decode targeted encode");
    assert_eq!(decoded.width(), w as u32);
    assert_eq!(decoded.height(), h as u32);
}

/// QP-0 corruption gate (2026-07-22): on the pinned zenav1-svt rev, QP 0
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

/// A 4:2:0 8-bit source whose luma AND chroma planes both carry structure,
/// so a lossless claim has to hold on all three planes (upstream's own
/// `lossless_fh_c_capture` gate uses flat 128 chroma; flat chroma is
/// trivially reconstructable and would not catch a broken chroma WHT arm).
fn yuv420_structured(w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (cw, ch) = (w / 2, h / 2);
    let y = (0..h)
        .flat_map(|r| (0..w).map(move |c| (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8)))
        .collect();
    let u = (0..ch)
        .flat_map(|r| (0..cw).map(move |c| (((c * 251) / cw.max(1)) as u8).wrapping_add(r as u8)))
        .collect();
    let v = (0..ch)
        .flat_map(|r| (0..cw).map(move |c| (255u8 - (((r * 199) / ch.max(1)) as u8)) ^ (c as u8)))
        .collect();
    (y, u, v)
}

/// QP-0 BEHAVIOR gate (imazen/zenav1-svt#5, issue #9): a DIRECT qp=0
/// request on the 8-bit 4:2:0 key-frame path must now produce a real,
/// **coded-lossless** AV1 stream — an independent decoder must reconstruct
/// the source planes EXACTLY.
///
/// History, so the direction of this gate is unambiguous:
/// * rev 3e25f52b: qp0 emitted syntactically-valid streams that decoded to
///   garbage (ssim2 ~= −700, `benchmarks/backend_sweep_2026-07-22.tsv`).
/// * upstream `f0f0a70ca` (issue #5): qp0 became a typed
///   `EncodeError::UnsupportedConfig` — corruption no longer reachable, and
///   this test asserted THAT refusal (`svt_rs_direct_qp0_rejected_typed`).
/// * upstream issue #9 (2026-08-28/29): coded-lossless is IMPLEMENTED and
///   byte-verified against the C v4.2.0 oracle on the 4:2:0 8-bit key-frame
///   envelope (`lossless_config_error`'s complement; upstream's own gate is
///   `qp0_coded_lossless_stream_matches_c_capture`, encoder-recon + C
///   byte-identity). The capability refusal this test used to pin is
///   RETIRED, so the test is REPLACED by the stronger property the retired
///   refusal was standing in for — not deleted, not loosened.
///
/// Complementary to upstream's gate by construction: upstream compares the
/// ENCODER's own recon and its bytes to a C capture; this decodes the
/// muxable payload with zenavif's own independent decoders (rav1d-safe,
/// plus zenav1-aom byte-agreement) and demands recon == source. A stream that
/// is C-byte-identical but that our decoders reconstruct differently, or a
/// lossless claim that quietly quantizes, fails here.
///
/// MUTATION-VERIFIED 2026-08-29 (aarch64): re-running this body at qp 1
/// instead of 0 fails every plane equality (luma first mismatch within the
/// first row), so the equality assertions are load-bearing rather than
/// vacuously true. The seam's own quality path still cannot reach QP 0
/// (`quality_to_qp_gated` clamps to >= 1 so quality 100 ENCODES rather than
/// switching coding mode); this drives the pipeline directly, exactly as
/// the seam would if that clamp were ever lifted.
#[test]
fn svt_rs_direct_qp0_codes_lossless_420() {
    use zenavif::{DecodeBackend, decode_av1_obu_yuv};

    // 64x64 = one full SB; 128x64 = two, so the SB-to-SB carry (above
    // reference, CDF state) is exercised, not just a single-block frame.
    for (w, h) in [(64usize, 64usize), (128, 64)] {
        // Preset 6 is the seam's mono partial-SB floor, 7 is the preset
        // upstream's C capture was taken at, 9 is the fast tier (a
        // different PD0 arm). Lossless must hold on all three. These sit
        // inside upstream's byte-identical lossless band (presets 4-13 are
        // 96/96 in `lossless_gate.sh`); presets 0-3 are lossless but only
        // self-promotingly pinned there, so they are not asserted here.
        for preset in [6u8, 7, 9] {
            let (y, u, v) = yuv420_structured(w, h);
            let rc = svtav1::encoder::rate_control::RcConfig {
                mode: svtav1::encoder::rate_control::RcMode::Cqp,
                qp: 0,
                ..svtav1::encoder::rate_control::RcConfig::default()
            };
            let mut pipeline = svtav1::encoder::pipeline::EncodePipeline::new(
                w as u32, h as u32, preset, rc, 0, 1,
            )
            .with_chroma_420(true);
            pipeline.bit_depth = 8;
            let payload = pipeline
                .try_encode_frame_420(&y, &u, &v, w)
                .unwrap_or_else(|e| {
                    panic!(
                        "qp0 coded-lossless {w}x{h} preset {preset} must ENCODE (upstream #9 \
                         retired the refusal): {e}"
                    )
                });
            assert!(
                !payload.is_empty(),
                "qp0 {w}x{h} preset {preset}: empty payload"
            );

            let dec = decode_av1_obu_yuv(&payload, DecodeBackend::Rav1dSafe).unwrap_or_else(|e| {
                panic!("qp0 {w}x{h} preset {preset} must decode under rav1d-safe: {e}")
            });
            assert_eq!(
                (dec.width, dec.height),
                (w, h),
                "qp0 {w}x{h} preset {preset}: signalled size"
            );
            assert!(
                !dec.monochrome,
                "qp0 {w}x{h} preset {preset}: must be a 4:2:0 stream"
            );
            assert_eq!(
                (dec.subsampling_x, dec.subsampling_y),
                (1, 1),
                "qp0 {w}x{h} preset {preset}: 4:2:0 subsampling"
            );

            // THE property: coded-lossless means the decoder reconstructs
            // the source exactly, on every plane.
            let mismatch = |name: &str, src: &[u8], out: &[u16], sw: usize, sh: usize| {
                for r in 0..sh {
                    for c in 0..sw {
                        let (a, b) = (u16::from(src[r * sw + c]), out[r * sw + c]);
                        assert_eq!(
                            a, b,
                            "qp0 {w}x{h} preset {preset}: {name} plane is NOT lossless at \
                             ({c},{r}): source {a}, decoded {b} — coded-lossless (base_qindex 0) \
                             must reconstruct the source exactly (zenav1-svt #5/#9)"
                        );
                    }
                }
            };
            mismatch("luma", &y, &dec.y, w, h);
            mismatch("Cb", &u, &dec.u, dec.width_uv, dec.height_uv);
            mismatch("Cr", &v, &dec.v, dec.width_uv, dec.height_uv);

            // Cross-decoder byte agreement: a stream one decoder happens to
            // reconstruct exactly must not pass on that decoder's leniency.
            #[cfg(feature = "zenav1-aom")]
            {
                let aom =
                    decode_av1_obu_yuv(&payload, DecodeBackend::Zenav1Aom).unwrap_or_else(|_| {
                        let mut with_td = vec![0x12, 0x00];
                        with_td.extend_from_slice(&payload);
                        decode_av1_obu_yuv(&with_td, DecodeBackend::Zenav1Aom).unwrap_or_else(|e| {
                            panic!("qp0 {w}x{h} preset {preset} must decode under zenav1-aom: {e}")
                        })
                    });
                assert_eq!(
                    (aom.y, aom.u, aom.v),
                    (dec.y.clone(), dec.u.clone(), dec.v.clone()),
                    "qp0 {w}x{h} preset {preset}: rav1d-safe and zenav1-aom must byte-agree"
                );
            }
            eprintln!(
                "svt_rs qp0 lossless {w}x{h} preset {preset}: exact recon, {} payload bytes",
                payload.len()
            );
        }
    }
}

/// The arms of the QP-0 envelope that upstream still REFUSES must keep
/// refusing with a typed `EncodeError::UnsupportedConfig` — never a panic,
/// never a silently-lossy "lossless" stream. This is the surviving half of
/// the retired blanket refusal (upstream `lossless_config_error`, the
/// complement of the byte-verified envelope): monochrome (the mono leaf
/// coder has no WHT / TX_4X4 arm and C v4.2.0 cannot produce a mono oracle)
/// and 10-bit (neither bd10 level producer has a WHT / TX_4X4 arm).
///
/// If either arm gains coded-lossless upstream, this test must be REPLACED
/// with a lossless round-trip for it — the same way
/// `svt_rs_direct_qp0_codes_lossless_420` replaced the 4:2:0 refusal — not
/// deleted.
#[test]
fn svt_rs_direct_qp0_typed_refusal_outside_420_8bit() {
    use svtav1::types::EncodeError;

    let rc = || svtav1::encoder::rate_control::RcConfig {
        mode: svtav1::encoder::rate_control::RcMode::Cqp,
        qp: 0,
        ..svtav1::encoder::rate_control::RcConfig::default()
    };
    let unsupported = |e: whereat::At<EncodeError>, what: &str| -> String {
        let (err, _trace) = e.decompose();
        let EncodeError::UnsupportedConfig(why) = err else {
            panic!("expected EncodeError::UnsupportedConfig for qp0 {what}, got: {err:?}");
        };
        why.to_string()
    };

    // (a) MONOCHROME (the Cs400 alpha / grayscale path) at qp0.
    let mut mono = svtav1::encoder::pipeline::EncodePipeline::new(64, 64, 7, rc(), 0, 1);
    mono.bit_depth = 8;
    let plane = vec![0u8; 64 * 64];
    let why = unsupported(
        mono.try_encode_frame(&plane, 64)
            .expect_err("qp0 on the monochrome path is still refused upstream"),
        "monochrome",
    );
    assert!(
        why.contains("monochrome"),
        "the mono qp0 refusal must name the monochrome path, got: {why}"
    );

    // (b) 10-BIT 4:2:0 at qp0. 64-aligned dims + preset 9 clear the
    // `hbd_source_consumed` gate, so the refusal that fires is the
    // coded-lossless one and not the native-bd10-consumer one.
    let mut hbd =
        svtav1::encoder::pipeline::EncodePipeline::new(64, 64, 9, rc(), 0, 1).with_chroma_420(true);
    hbd.bit_depth = 10;
    let (y10, u10, v10) = (
        vec![512u16; 64 * 64],
        vec![512u16; 32 * 32],
        vec![512u16; 32 * 32],
    );
    let why = unsupported(
        hbd.try_encode_frame_420_hbd(&y10, &u10, &v10, 64)
            .expect_err("qp0 at 10-bit is still refused upstream"),
        "10-bit",
    );
    assert!(
        why.contains("8-bit only"),
        "the bd10 qp0 refusal must name the 8-bit-only coded-lossless envelope (not the \
         unrelated bd10-consumer gate), got: {why}"
    );
}
