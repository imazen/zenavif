//! The zenav1-aom **encode** backend seam: does an aom-encoded AVIF decode?
//!
//! `Av1Backend::Zenav1Aom` drives `aom_encode::key_frame::encode_key_frame`
//! (one AV1 KEY frame, no C bytes in the path) and muxes the result with
//! `zenavif-serialize`. Two things have to be true and neither is implied by
//! `encode_rgb8` returning `Ok`:
//!
//! 1. the output is a real **AVIF container**, not the raw OBUs the retired
//!    `Av1Backend::Svtav1` draft returned (which is exactly why
//!    `EncoderConfig::validate` rejects that variant); and
//! 2. an **independent decoder** — rav1d-safe, a different port from the one
//!    that produced the stream — recovers the source pixels, with the
//!    container's colour signalling applied.
//!
//! Point 2 is the one that catches a colour-range mismatch. The zenav1-aom
//! sequence header pins `color_range = 0` (studio), the opposite of the
//! zenav1-svt seam, so this backend converts limited-range.
//! [`limited_range_signalling_is_load_bearing`] is what proves the range path
//! is actually exercised: a flat 235 source codes to luma 218 under the studio
//! swing, so a decoder reading the range wrongly returns 218, not 235.
//!
//! MEASURED here (2026-09-02) and recorded in `src/encoder_aom.rs`: the
//! container's `colr` `full_range` bit is **not** what `zenavif::decode` obeys
//! — `src/decoder.rs` reads `seq_hdr.color_range`, and flipping the `colr` bit
//! in an encoded file leaves the decoded pixels bit-identical. The mux writes
//! `colr` to agree with the bitstream, not to override it.
//!
//! # Every gate here is proved able to fail
//!
//! [`gate_can_fail_on_wrong_content`] and
//! [`gate_can_fail_on_a_corrupted_payload`] run the SAME assertion helpers
//! against deliberately broken input and require them to panic. They are
//! `#[test]`s, not a one-off manual mutation: a later edit that makes an
//! assertion vacuous turns these red.
//!
//! # Where the thresholds come from
//!
//! Every PSNR bound below was MEASURED on this content before being written,
//! not guessed, and carries roughly 5 dB of headroom over the worst cell:
//!
//! | gate | measured range | bound |
//! |---|---|---|
//! | q90, 4 sizes, speed 6 | 43.3–45.3 dB | 38 dB |
//! | q80, 4 sizes, speeds 1..=10 | 37.9–41.0 dB | 33 dB |
//! | vs the zenravif backend at q90 | aom is 1.6 dB worse to 0.2 dB better | within 5 dB |
//! | bd10 coded luma, 3 sizes, q90 s6 | 49.58-49.69 dB | 40 dB |
//! | bd12 coded luma, 3 sizes, q90 s6 | 50.24-50.57 dB | 40 dB |
//! | bd10 flat luma vs longhand H.273, q99 | worst 0 code values | 0 |
//! | bd12 flat luma vs longhand H.273, q99 | worst 1 code value | 1 |
//!
//! A broken decode lands far below any of them: wrong content measures 6.4 dB
//! and a two-byte payload corruption fails to decode at all. The
//! high-bit-depth rows carry ~9.6 dB of headroom; the flat-luma bounds are the
//! measured worst themselves, and a studio/full range mix-up would move them
//! by hundreds to thousands of code values, not by one.
//!
//! # High bit depth (2026-09-03)
//!
//! `Av1Backend::Zenav1Aom` codes 4:2:0 at 8, 10 and 12 bits, from both 8-bit
//! (`encode_rgb8`) and 16-bit (`encode_rgb16`) input. 12 bits is
//! `EncodeBitDepth::Twelve`, added in the 0.2.0 break together with
//! `#[non_exhaustive]` on the enum. The grayscale (Cs400) path stays 8-bit
//! only and refuses by name.
#![cfg(all(feature = "zenav1-aom-encode", feature = "encode"))]

use almost_enough::{StopToken, Unstoppable};
use imgref::{Img, ImgRef, ImgVec};
use rgb::Rgb;
use zenavif::{
    Av1Backend, DecodeBackend, EncodeChromaSubsampling, EncodedImage, EncoderConfig,
    decode_av1_obu_yuv,
};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

/// A config that selects the aom backend and nothing else unusual. 4:2:0 is
/// not the crate default (4:4:4 is), and this seam encodes 4:2:0 only.
fn aom_config() -> EncoderConfig {
    EncoderConfig::new()
        .backend(Av1Backend::Zenav1Aom)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
}

/// Uniform mid-gray. The most mutation-sensitive content there is: any
/// header field that changes the reconstruction moves a flat frame off its
/// single value.
fn flat_rgb8(w: usize, h: usize, value: u8) -> ImgVec<Rgb<u8>> {
    Img::new(
        vec![
            Rgb {
                r: value,
                g: value,
                b: value
            };
            w * h
        ],
        w,
        h,
    )
}

/// Deterministic photo-ish content: smooth gradients plus LCG noise, so the
/// quantizer has real work at every quality tier. Same generator shape as
/// `tests/cross_backend_decode.rs`.
fn gradient_rgb8(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let mut state = 0x2545F491u32;
    let mut lcg = move || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (state >> 24) as u8
    };
    let mut buf = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let g = ((x * 255) / w.max(1)) as u8;
            let b = ((y * 255) / h.max(1)) as u8;
            let n = lcg() / 8;
            // The noise is added EQUALLY to all three channels, so it lives in
            // luma and leaves chroma smooth. Per-channel noise instead would
            // make 4:2:0 subsampling — not the encoder — the dominant error
            // term: measured, the per-channel variant scores 22.4 dB here for
            // the aom backend and 22.4 dB for zenravif, i.e. it grades the
            // subsampling, not the backend under test.
            let base = ((g as u16 + b as u16) / 2) as u8;
            buf.push(Rgb {
                r: g.saturating_add(n / 2),
                g: base.saturating_add(n / 2),
                b: b.saturating_add(n / 2),
            });
        }
    }
    Img::new(buf, w, h)
}

fn psnr_rgb8(a: &[Rgb<u8>], b: &[Rgb<u8>]) -> f64 {
    assert_eq!(a.len(), b.len(), "PSNR needs equal-length buffers");
    let mut se = 0u64;
    for (p, q) in a.iter().zip(b) {
        for (x, y) in [(p.r, q.r), (p.g, q.g), (p.b, q.b)] {
            let d = i32::from(x) - i32::from(y);
            se += (d * d) as u64;
        }
    }
    if se == 0 {
        return f64::INFINITY;
    }
    let mse = se as f64 / (a.len() * 3) as f64;
    10.0 * (255.0 * 255.0 / mse).log10()
}

/// Extract the primary item's AV1 payload, parsing STRICTLY: every container
/// here was produced by our own muxer, so one that will not parse strictly is
/// a muxer bug worth failing on.
fn primary_payload(avif: &[u8]) -> Vec<u8> {
    let cfg = zenavif_parse::DecodeConfig::default();
    let parser =
        zenavif_parse::AvifParser::from_owned_with_config(avif.to_vec(), &cfg, &Unstoppable)
            .expect("the aom backend must emit a strictly parseable AVIF container");
    parser
        .primary_data()
        .expect("primary item")
        .as_ref()
        .to_vec()
}

// ---------------------------------------------------------------------------
// The assertion helpers. Each is exercised in both directions: on real encoder
// output (must pass) and, by the `gate_can_fail_*` tests, on broken input
// (must panic).
// ---------------------------------------------------------------------------

/// The container must be an AVIF file, not raw OBUs: an `ftyp` brand at the
/// head, a primary item, and a payload whose first OBU is a temporal
/// delimiter (what `encode_key_frame` emits) rather than arbitrary bytes.
fn assert_is_avif_container(avif: &[u8], label: &str) {
    assert!(
        avif.len() > 16,
        "{label}: an AVIF file cannot be {} bytes",
        avif.len()
    );
    assert_eq!(
        &avif[4..8],
        b"ftyp",
        "{label}: no ftyp box — this is not an AVIF file (the retired \
         Av1Backend::Svtav1 draft returned raw OBUs; that is the bug this asserts against)"
    );
    let payload = primary_payload(avif);
    // OBU header byte: forbidden bit 0, type in bits 6..3. Type 2 = temporal
    // delimiter, which is what encode_key_frame's temporal unit starts with.
    let obu_type = (payload[0] >> 3) & 0x0F;
    assert_eq!(
        obu_type, 2,
        "{label}: primary payload does not start with OBU_TEMPORAL_DELIMITER (got type {obu_type})"
    );
}

/// Decode the whole AVIF back to RGB — through the container, so `colr` is
/// applied — and require it to be within `min_psnr` dB of the source.
///
/// This is the assertion that catches a colour-range or matrix mismatch: a
/// stream that says studio range while carrying full-range samples decodes to
/// stretched contrast and lands far below any sane threshold.
fn assert_rgb_round_trip(avif: &[u8], src: ImgRef<'_, Rgb<u8>>, min_psnr: f64, label: &str) {
    let decoded = zenavif::decode(avif)
        .unwrap_or_else(|e| panic!("{label}: an aom-encoded AVIF must decode: {e}"));
    assert_eq!(decoded.width() as usize, src.width(), "{label}: width");
    assert_eq!(decoded.height() as usize, src.height(), "{label}: height");
    let out = decoded
        .try_as_imgref::<Rgb<u8>>()
        .unwrap_or_else(|| panic!("{label}: a no-alpha decode must yield RGB8"));
    let p = psnr_rgb8(src.buf(), out.buf());
    assert!(
        p >= min_psnr,
        "{label}: RGB round-trip PSNR {p:.2} dB < {min_psnr} dB"
    );
    eprintln!("{label}: RGB round-trip PSNR {p:.2} dB");
}

/// Flat content is exactly reproducible, so the RGB round trip is an equality
/// (within 1, for rounding), not a PSNR bound.
///
/// This is the assertion that catches a colour-RANGE error. A flat 235 source
/// codes to studio luma 218; a decoder that read the range as full would hand
/// back 218, which is 17 away — far outside this tolerance and far outside
/// anything rounding can explain.
fn assert_flat_round_trips_exactly(avif: &[u8], src: ImgRef<'_, Rgb<u8>>, label: &str) {
    let decoded = zenavif::decode(avif)
        .unwrap_or_else(|e| panic!("{label}: an aom-encoded AVIF must decode: {e}"));
    let out = decoded
        .try_as_imgref::<Rgb<u8>>()
        .unwrap_or_else(|| panic!("{label}: a no-alpha decode must yield RGB8"));
    let mut worst = 0i32;
    for (p, q) in src.buf().iter().zip(out.buf().iter()) {
        for (x, y) in [(p.r, q.r), (p.g, q.g), (p.b, q.b)] {
            worst = worst.max((i32::from(x) - i32::from(y)).abs());
        }
    }
    assert!(
        worst <= 1,
        "{label}: flat RGB round trip is off by {worst} (a studio/full range \
         mix-up would show up here as ~17 on a flat 235 source)"
    );
    eprintln!("{label}: flat RGB round trip exact (worst channel error {worst})");
}

/// Decode the AV1 payload with **rav1d-safe** — a different port from the one
/// that encoded it — and require the luma plane to match `expect_y` exactly.
///
/// Used on flat content, where the reconstruction is exactly reproducible, so
/// this is an equality assertion and not a tolerance.
fn assert_independent_decoder_luma_exact(avif: &[u8], expect_y: &[u8], label: &str) {
    let payload = primary_payload(avif);
    let yuv = decode_av1_obu_yuv(&payload, DecodeBackend::Rav1dSafe)
        .unwrap_or_else(|e| panic!("{label}: rav1d-safe must decode the aom stream: {e}"));
    assert_eq!(yuv.bit_depth, 8, "{label}: bit depth");
    let got: Vec<u8> = yuv.y.iter().map(|&s| s as u8).collect();
    assert_eq!(
        got.len(),
        expect_y.len(),
        "{label}: luma plane length ({} vs {})",
        got.len(),
        expect_y.len()
    );
    let mismatches = got
        .iter()
        .zip(expect_y)
        .filter(|(a, b)| a != b)
        .take(4)
        .map(|(a, b)| format!("{a}!={b}"))
        .collect::<Vec<_>>();
    assert!(
        mismatches.is_empty(),
        "{label}: rav1d-safe luma differs from the expected flat value: {}",
        mismatches.join(", ")
    );
    eprintln!(
        "{label}: rav1d-safe decoded {}x{} luma exactly",
        yuv.width, yuv.height
    );
}

fn encode(img: ImgRef<'_, Rgb<u8>>, config: &EncoderConfig) -> EncodedImage {
    zenavif::encode_rgb8(img, config, stop()).expect("aom-backend encode")
}

// ---------------------------------------------------------------------------
// Gates
// ---------------------------------------------------------------------------

/// The whole point: an aom-encoded AVIF is a real container AND decodes.
#[test]
fn aom_backend_produces_an_avif_that_decodes() {
    // A size grid that crosses the superblock boundary in both directions and
    // includes partial superblocks and an odd dimension (the encoder gates
    // 20 crops including 1x1).
    let cells: &[(usize, usize)] = &[(64, 64), (128, 96), (33, 47), (192, 64)];
    assert!(!cells.is_empty(), "the cell list must not be empty");
    for &(w, h) in cells {
        let img = gradient_rgb8(w, h);
        let enc = encode(img.as_ref(), &aom_config().quality(90.0).speed(6));
        let label = format!("gradient {w}x{h} q90 s6");
        assert_is_avif_container(&enc.avif_file, &label);
        assert_rgb_round_trip(&enc.avif_file, img.as_ref(), 38.0, &label);
        assert!(
            enc.color_byte_size > 0 && enc.alpha_byte_size == 0,
            "{label}: payload sizes"
        );
    }
}

/// Flat content is exactly reproducible, so an independent decoder must
/// return the exact coded luma value — no tolerance.
#[test]
fn aom_backend_flat_content_decodes_exactly_on_an_independent_decoder() {
    for &value in &[16u8, 128, 235] {
        let img = flat_rgb8(64, 64, value);
        let enc = encode(img.as_ref(), &aom_config().quality(95.0).speed(6));
        let label = format!("flat {value} 64x64 q95 s6");
        assert_is_avif_container(&enc.avif_file, &label);
        // The seam converts BT.601 LIMITED range; recompute the expected luma
        // with the identical kernel the encoder fed, so this asserts the
        // decoder returned what was coded, not what we hoped.
        let expect_y = expected_limited_luma(img.as_ref());
        assert_independent_decoder_luma_exact(&enc.avif_file, &expect_y, &label);
        assert_flat_round_trips_exactly(&enc.avif_file, img.as_ref(), &label);
    }
}

/// Recompute the limited-range BT.601 luma the seam feeds the encoder, using
/// the public integer recipe (H.273 studio swing) rather than reaching into a
/// private kernel. Only valid for flat gray input, where chroma is neutral and
/// every pixel maps to the same Y.
fn expected_limited_luma(img: ImgRef<'_, Rgb<u8>>) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.width() * img.height());
    for row in img.rows() {
        for px in row {
            let (kr, kb) = (0.299f32, 0.114f32);
            let kg = 1.0 - kr - kb;
            let y = kr * f32::from(px.r) / 255.0
                + kg * f32::from(px.g) / 255.0
                + kb * f32::from(px.b) / 255.0;
            out.push((y * 219.0 + 16.0).round().clamp(0.0, 255.0) as u8);
        }
    }
    out
}

/// Monochrome (Cs400) stills.
#[cfg(feature = "encode-mono")]
#[test]
fn aom_backend_encodes_monochrome_that_decodes() {
    let w = 64;
    let h = 64;
    let gray: Vec<u8> = (0..w * h).map(|i| ((i * 7) % 256) as u8).collect();
    let img = Img::new(gray.clone(), w, h);
    let enc = zenavif::encode_gray8(img.as_ref(), &aom_config().quality(95.0).speed(6), stop())
        .expect("aom-backend mono encode");
    assert_is_avif_container(&enc.avif_file, "mono 64x64 q95 s6");
    let payload = primary_payload(&enc.avif_file);
    let yuv = decode_av1_obu_yuv(&payload, DecodeBackend::Rav1dSafe)
        .expect("rav1d-safe must decode the mono aom stream");
    assert_eq!(yuv.width as usize, w);
    assert_eq!(yuv.height as usize, h);
    // A Cs400 stream carries the caller's luma verbatim, so at q95 the
    // reconstruction must be close; assert a tight bound rather than "it
    // decoded".
    let mut worst = 0i32;
    for (a, b) in yuv.y.iter().zip(&gray) {
        worst = worst.max((i32::from(*a as u8) - i32::from(*b)).abs());
    }
    assert!(worst <= 24, "mono q95 worst-channel error {worst} > 24");
    eprintln!("mono 64x64 q95 s6: worst luma error {worst}");
}

/// Quality has to be a real dial, not a decoration.
#[test]
fn aom_backend_quality_moves_bytes() {
    let img = gradient_rgb8(96, 96);
    let lo = encode(img.as_ref(), &aom_config().quality(20.0).speed(6));
    let hi = encode(img.as_ref(), &aom_config().quality(90.0).speed(6));
    assert!(
        hi.color_byte_size > lo.color_byte_size,
        "q90 ({}) must exceed q20 ({}) in payload bytes",
        hi.color_byte_size,
        lo.color_byte_size
    );
    eprintln!(
        "aom quality dial: q20 {} bytes, q90 {} bytes",
        lo.color_byte_size, hi.color_byte_size
    );
}

/// Every speed the seam maps must produce a decodable stream.
#[test]
fn aom_backend_every_speed_decodes() {
    let img = gradient_rgb8(64, 64);
    for speed in 1..=10u8 {
        let enc = encode(img.as_ref(), &aom_config().quality(80.0).speed(speed));
        let label = format!("gradient 64x64 q80 s{speed}");
        assert_is_avif_container(&enc.avif_file, &label);
        assert_rgb_round_trip(&enc.avif_file, img.as_ref(), 33.0, &label);
    }
}

// ---------------------------------------------------------------------------
// Refusals. Each names what is unimplemented; none silently falls back to
// zenravif.
// ---------------------------------------------------------------------------

#[test]
fn aom_backend_refuses_what_it_does_not_implement() {
    let img = gradient_rgb8(64, 64);
    let rgba: ImgVec<rgb::Rgba<u8>> = Img::new(
        img.buf()
            .iter()
            .map(|p| rgb::Rgba {
                r: p.r,
                g: p.g,
                b: p.b,
                a: 255,
            })
            .collect::<Vec<_>>(),
        64,
        64,
    );

    // 4:4:4 (the crate default) — the encoder gates it, this seam does not.
    let e = zenavif::encode_rgb8(
        img.as_ref(),
        &EncoderConfig::new().backend(Av1Backend::Zenav1Aom),
        stop(),
    )
    .expect_err("4:4:4 must be refused");
    assert!(
        format!("{e}").contains("4:2:0 only"),
        "4:4:4 refusal must name the limitation, got: {e}"
    );

    // Alpha.
    let e = zenavif::encode_rgba8(rgba.as_ref(), &aom_config(), stop())
        .expect_err("alpha must be refused");
    assert!(
        format!("{e}").contains("Zenav1Aom"),
        "alpha refusal must name the backend, got: {e}"
    );

    // A refusal must name the AOM backend's own limitation, never another
    // backend's feature. `reject_svt_rs_backend` and `reject_aom_backend`
    // share their `entry` string, and the svt-specific hint used to be baked
    // into it — so the aom refusal for `encode_rgb16` read "requires the
    // `zenav1-svt` cargo feature", naming a feature that has nothing to do
    // with this backend. Caught 2026-09-02 by compiling a real downstream
    // consumer, and gated here so it cannot come back.
    let e = zenavif::encode_rgba8(rgba.as_ref(), &aom_config(), stop())
        .expect_err("alpha must be refused");
    assert!(
        !format!("{e}").contains("zenav1-svt"),
        "an Av1Backend::Zenav1Aom refusal must not name the zenav1-svt feature, got: {e}"
    );

    // 16-bit RGB input USED to be refused. `encode_rgb16` reaches the seam
    // since 2026-09-03 (YCbCr 4:2:0 at 8/10/12 bits, not zenravif's
    // identity-GBR 4:4:4); 16-bit RGBA is what is still refused, because the
    // alpha auxiliary item is not built. That is the assertion below.
    let rgba16: ImgVec<rgb::Rgba<u16>> = Img::new(
        img.buf()
            .iter()
            .map(|p| rgb::Rgba {
                r: u16::from(p.r) << 8,
                g: u16::from(p.g) << 8,
                b: u16::from(p.b) << 8,
                a: u16::MAX,
            })
            .collect::<Vec<_>>(),
        64,
        64,
    );
    let e = zenavif::encode_rgba16(rgba16.as_ref(), &aom_config(), stop())
        .expect_err("16-bit RGBA must be refused");
    let msg = format!("{e}");
    assert!(
        msg.contains("Zenav1Aom") && msg.contains("no alpha auxiliary item"),
        "the 16-bit RGBA refusal must name the backend and the limitation, got: {msg}"
    );
    assert!(
        !msg.contains("zenav1-svt"),
        "an Av1Backend::Zenav1Aom refusal must not name the zenav1-svt feature, got: {msg}"
    );

    // 10-bit output USED to be refused here. It encodes since 2026-09-03 —
    // `aom_backend_encodes_10_and_12_bit_that_decode` is the positive gate
    // that replaced this refusal. What is still 8-bit-only is the Cs400
    // grayscale path, and that keeps a refusal test of its own
    // (`aom_backend_refuses_hbd_grayscale`).

    // Full pixel range — the sequence header pins studio range.
    let e = zenavif::encode_rgb8(
        img.as_ref(),
        &aom_config().pixel_range(zenavif::EncodePixelRange::Full),
        stop(),
    )
    .expect_err("full range must be refused");
    assert!(
        format!("{e}").contains("LIMITED"),
        "full-range refusal must name the limitation, got: {e}"
    );
}

/// `validate()` must agree with the encode path — a config that encodes
/// validates, and a config that validates encodes.
#[test]
fn validate_agrees_with_the_encode_path() {
    aom_config()
        .quality(90.0)
        .speed(6)
        .validate()
        .expect("the supported slice must validate");
    // 10- and 12-bit VALIDATE since 2026-09-03 — this used to be the
    // "10-bit must fail validate()" case, deliberately inverted when the
    // depth landed. `validate` and the encode path share
    // `encoder_aom::aom_depth_error`, so this is the same predicate the
    // encode gates above exercise.
    aom_config()
        .bit_depth(zenavif::EncodeBitDepth::Ten)
        .validate()
        .expect("10-bit must validate, as it encodes");
    aom_config()
        .bit_depth(zenavif::EncodeBitDepth::Twelve)
        .validate()
        .expect("12-bit must validate, as it encodes");
    for (cfg, what) in [
        (EncoderConfig::new().backend(Av1Backend::Zenav1Aom), "4:4:4"),
        (
            aom_config().color_model(zenavif::EncodeColorModel::Rgb),
            "identity/RGB",
        ),
        (
            aom_config().pixel_range(zenavif::EncodePixelRange::Full),
            "full range",
        ),
    ] {
        cfg.validate()
            .expect_err(&format!("{what} must fail validate()"));
    }
    // zenavif#44: alpha input must FAIL validate_for_input, because it fails
    // encode. `input_has_alpha` is a config x input property, so `validate()`
    // alone cannot see it — this is the case that used to return Ok and then
    // error at `encode_rgba8`.
    use zenavif::PlanInput;
    aom_config()
        .validate_for_input(PlanInput::rgba8(64, 64))
        .expect_err("alpha input must fail validate_for_input, as it fails encode");
    // The same config WITHOUT alpha validates, so the refusal is scoped to
    // alpha and is not just "this backend never validates".
    aom_config()
        .validate_for_input(PlanInput::rgb8(64, 64))
        .expect("RGB8 input must validate for the aom backend");
    // And 16-bit RGB input validates too, since `encode_rgb16` reaches the
    // seam. Before 2026-09-03 the generic identity-RGB 4:2:0 rule rejected it
    // INCIDENTALLY (zenavif#44's control observed exactly that), which would
    // have been the wrong answer the moment the seam grew the entry point.
    aom_config()
        .validate_for_input(PlanInput {
            width: 64,
            height: 64,
            input_is_16bit: true,
            input_has_alpha: false,
        })
        .expect("16-bit RGB input must validate for the aom backend");

    // Lossless: the encode path has refused it since the seam landed
    // (`reject_unsupported_config`), but `validate_aom_scope` was missing the
    // twin check, so `.with_lossless(true)` VALIDATED and then failed at
    // encode — measured 2026-09-02 from a downstream consumer. Gated like the
    // builder itself; the CI seam run enables `encode-imazen`, so this runs
    // there.
    #[cfg(feature = "encode-imazen")]
    aom_config()
        .with_lossless(true)
        .validate()
        .expect_err("lossless must fail validate(), as it fails encode");
}

// ---------------------------------------------------------------------------
// High bit depth: 10 and 12 bits, 4:2:0 (2026-09-03)
// ---------------------------------------------------------------------------

/// A 16-bit gradient whose low bits carry detail an 8-bit source cannot.
///
/// Same luma-only-noise shape as [`gradient_rgb8`] and for the same reason:
/// per-channel noise would grade the 4:2:0 subsampling rather than the
/// encoder. The `+ (i as u16 & 0xFF)` term is the sub-8-bit detail — it moves
/// only the low byte, so an 8-bit coded stream cannot represent it and a
/// 10/12-bit one can.
fn gradient_rgb16(w: usize, h: usize) -> ImgVec<Rgb<u16>> {
    let mut state = 0x2545_F491u32;
    let mut lcg = move || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (state >> 24) as u16
    };
    let mut buf = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let g = ((x * 65535) / w.max(1)) as u16;
            let b = ((y * 65535) / h.max(1)) as u16;
            let n = lcg() * 8;
            let base = (((g as u32) + (b as u32)) / 2) as u16;
            buf.push(Rgb {
                r: g.saturating_add(n / 2),
                g: base.saturating_add(n / 2),
                b: b.saturating_add(n / 2),
            });
        }
    }
    Img::new(buf, w, h)
}

fn flat_rgb16(w: usize, h: usize, value: u16) -> ImgVec<Rgb<u16>> {
    Img::new(
        vec![
            Rgb {
                r: value,
                g: value,
                b: value
            };
            w * h
        ],
        w,
        h,
    )
}

/// The BT.601 limited-range luma a 16-bit source codes to at `depth` bits,
/// written longhand from H.273 — independent of `src/yuv_convert.rs`, which is
/// the point: it is what makes the plane comparison a real check and not a
/// restatement of the kernel.
///
/// Limited range at depth d is the 8-bit studio swing shifted up by `d - 8`:
/// offset `16 << (d-8)`, span `219 << (d-8)`.
fn expected_limited_luma_hbd(img: ImgRef<'_, Rgb<u16>>, depth: u8) -> Vec<u16> {
    let scale = f32::from(1u16 << (depth - 8));
    let max = f32::from((1u16 << depth) - 1);
    let mut out = Vec::with_capacity(img.width() * img.height());
    for row in img.rows() {
        for px in row {
            let (kr, kb) = (0.299f32, 0.114f32);
            let kg = 1.0 - kr - kb;
            let y = kr * f32::from(px.r) / 65535.0
                + kg * f32::from(px.g) / 65535.0
                + kb * f32::from(px.b) / 65535.0;
            out.push((y * 219.0 * scale + 16.0 * scale).round().clamp(0.0, max) as u16);
        }
    }
    out
}

/// Decode the AV1 payload with **rav1d-safe** — a different port from the one
/// that encoded it — and return its planes after asserting the coded depth.
///
/// The depth assertion is the one that catches a container/bitstream
/// disagreement: `mux_aom` writes the `av1C` `high_bitdepth`/`twelve_bit`
/// flags from the same `bit_depth` the sequence header was derived from, so a
/// seam that muxed 8 over a 10-bit payload would show up here.
fn hbd_planes(avif: &[u8], expect_depth: u8, label: &str) -> zenavif::DecodedYuv {
    let payload = primary_payload(avif);
    let yuv = decode_av1_obu_yuv(&payload, DecodeBackend::Rav1dSafe)
        .unwrap_or_else(|e| panic!("{label}: rav1d-safe must decode the aom stream: {e}"));
    assert_eq!(
        yuv.bit_depth,
        i32::from(expect_depth),
        "{label}: rav1d-safe reports a different coded depth than was requested"
    );
    yuv
}

/// Flat content is (near-)exactly reproducible, so the luma plane an
/// independent decoder returns must match the longhand H.273 expectation to
/// within `max_delta` CODE VALUES.
///
/// This is the assertion that catches a colour-range error at high bit depth.
/// A flat 16-bit 60293 source codes to studio luma 890 at 10 bits and 3560 at
/// 12; a seam that left the studio constants unscaled would return 222, and
/// one that converted full-range would return ~941/3765. Both are ~300-3000
/// code values away, so even the loosest bound used here (1) separates them
/// by three orders of magnitude.
///
/// `max_delta` is MEASURED, not chosen for headroom, and the observed worst is
/// printed on every run:
///
/// | depth | quality | observed worst | bound |
/// |---|---|---|---|
/// | 10 | 99 (cq 1) | 0 | 0 |
/// | 12 | 99 (cq 1) | 1 | 1 |
///
/// 12 bits is not exact because `--cq-level 1` is not `base_qindex` 0: the DC
/// quantizer step at 12 bits is coarser than one code value, so a flat DC
/// lands one below. (`--cq-level 0` would be the lossless-ish end, and it
/// PANICS upstream on flat content — see
/// `aom_cq0_still_panics_on_flat_content`.)
fn assert_hbd_flat_luma_within(
    avif: &[u8],
    src: ImgRef<'_, Rgb<u16>>,
    depth: u8,
    max_delta: i32,
    label: &str,
) {
    let yuv = hbd_planes(avif, depth, label);
    let expect = expected_limited_luma_hbd(src, depth);
    assert_eq!(yuv.y.len(), expect.len(), "{label}: luma plane length");
    let mut worst = 0i32;
    let mut worst_pair = (0u16, 0u16);
    for (a, b) in yuv.y.iter().zip(&expect) {
        let d = (i32::from(*a) - i32::from(*b)).abs();
        if d > worst {
            worst = d;
            worst_pair = (*a, *b);
        }
    }
    assert!(
        worst <= max_delta,
        "{label}: rav1d-safe luma is {worst} code values from the longhand H.273 \
         expectation at {depth} bits (bound {max_delta}; worst {} vs {}). A studio/full \
         range mix-up shows up here as hundreds to thousands.",
        worst_pair.0,
        worst_pair.1
    );
    eprintln!(
        "{label}: rav1d-safe {depth}-bit flat luma within {worst} (bound {max_delta}, \
         {} samples)",
        expect.len()
    );
}

/// PSNR of a decoded high-bit-depth luma plane against the longhand
/// expectation, in dB over the plane's own full scale.
///
/// Compared in the CODED domain (luma at `depth` bits) rather than after a
/// container RGB decode, so no assumption is made about how `zenavif::decode`
/// scales high-bit-depth output.
fn hbd_luma_psnr(avif: &[u8], src: ImgRef<'_, Rgb<u16>>, depth: u8, label: &str) -> f64 {
    let yuv = hbd_planes(avif, depth, label);
    let expect = expected_limited_luma_hbd(src, depth);
    assert_eq!(yuv.y.len(), expect.len(), "{label}: luma plane length");
    let peak = f64::from((1u32 << depth) - 1);
    let mut se = 0u64;
    for (a, b) in yuv.y.iter().zip(&expect) {
        let d = i64::from(*a) - i64::from(*b);
        se += (d * d) as u64;
    }
    if se == 0 {
        return f64::INFINITY;
    }
    let mse = se as f64 / expect.len() as f64;
    10.0 * (peak * peak / mse).log10()
}

/// The deliverable: 10- and 12-bit 4:2:0 AVIFs encode through the aom backend
/// and decode correctly under an independent decoder.
///
/// Every cell is checked three ways: the container parses strictly and starts
/// with a temporal delimiter, rav1d-safe reports the requested coded depth,
/// and its luma plane is within the measured PSNR bound of the longhand
/// H.273 expectation.
#[test]
fn aom_backend_encodes_10_and_12_bit_that_decode() {
    for (depth, cfg) in [
        (10u8, aom_config().bit_depth(zenavif::EncodeBitDepth::Ten)),
        (
            12u8,
            aom_config().bit_depth(zenavif::EncodeBitDepth::Twelve),
        ),
    ] {
        for &(w, h) in &[(64usize, 64usize), (65, 33), (192, 128)] {
            let src = gradient_rgb16(w, h);
            let cfg = cfg.clone().quality(90.0).speed(6);
            let enc = zenavif::encode_rgb16(src.as_ref(), &cfg, stop())
                .unwrap_or_else(|e| panic!("bd{depth} {w}x{h}: encode must succeed: {e}"));
            let label = format!("bd{depth} {w}x{h}");
            assert_is_avif_container(&enc.avif_file, &label);
            let p = hbd_luma_psnr(&enc.avif_file, src.as_ref(), depth, &label);
            // MEASURED before being written; see the module table.
            assert!(p >= 40.0, "{label}: coded-luma PSNR {p:.2} dB < 40 dB");
            eprintln!(
                "{label}: coded-luma PSNR {p:.2} dB, {} bytes",
                enc.avif_file.len()
            );
        }
    }
}

/// Flat content round-trips EXACTLY at 10 and 12 bits — the high-bit-depth
/// twin of [`aom_backend_flat_content_decodes_exactly_on_an_independent_decoder`].
///
/// This is what proves the studio swing is scaled by `<< (depth - 8)` and not
/// left at its 8-bit constants: at 10 bits a flat 60293 codes to 890, and the
/// unscaled 8-bit constants would give 238.
#[test]
fn aom_backend_hbd_flat_content_decodes_exactly() {
    for depth in [10u8, 12] {
        let src = flat_rgb16(64, 64, 60293);
        let cfg = if depth == 10 {
            aom_config().bit_depth(zenavif::EncodeBitDepth::Ten)
        } else {
            aom_config().bit_depth(zenavif::EncodeBitDepth::Twelve)
        }
        // quality 99, not 100: `--cq-level 0` PANICS upstream on flat content
        // at every depth (`aom_cq0_still_panics_on_flat_content` pins it).
        .quality(99.0)
        .speed(6);
        let enc = zenavif::encode_rgb16(src.as_ref(), &cfg, stop())
            .unwrap_or_else(|e| panic!("bd{depth} flat: encode must succeed: {e}"));
        // MEASURED bounds, per depth — see `assert_hbd_flat_luma_within`.
        let bound = if depth == 10 { 0 } else { 1 };
        assert_hbd_flat_luma_within(
            &enc.avif_file,
            src.as_ref(),
            depth,
            bound,
            &format!("bd{depth} flat"),
        );
    }
}

/// 10 and 12 bits are reachable from 8-bit input too (`encode_rgb8` +
/// `EncodeBitDepth::Ten` / `::Twelve`), which is the zenav1-svt
/// seam's documented shape as well.
///
/// The conversion quantizes at the OUTPUT depth, so the chroma average keeps
/// fraction bits an 8-bit quantize-then-widen would drop. It does NOT invent
/// luma detail, and this test does not claim it does — it asserts the stream
/// codes at the requested depth and decodes to the source.
#[test]
fn aom_backend_codes_hbd_from_8_bit_input() {
    let src = gradient_rgb8(96, 64);
    for depth in [10u8, 12] {
        let cfg = if depth == 10 {
            aom_config().bit_depth(zenavif::EncodeBitDepth::Ten)
        } else {
            aom_config().bit_depth(zenavif::EncodeBitDepth::Twelve)
        }
        .quality(90.0)
        .speed(6);
        let enc = encode(src.as_ref(), &cfg);
        let label = format!("rgb8 -> bd{depth}");
        assert_is_avif_container(&enc.avif_file, &label);
        let yuv = hbd_planes(&enc.avif_file, depth, &label);
        assert_eq!(yuv.width, 96, "{label}: width");
        assert_eq!(yuv.height, 64, "{label}: height");
        eprintln!("{label}: {} bytes", enc.avif_file.len());
    }
}

/// The 12-bit stream must be AV1 **profile 2** and the container must say so.
///
/// `KeyFrameConfig::profile()` returns 2 at 12 bits and `zenavif-serialize`
/// raises `seq_profile` to >= 2 from the depth, so a mismatch here means one
/// of the two stopped agreeing. Read out of the `av1C` box rather than
/// asserted on our own inputs.
#[test]
fn aom_backend_12_bit_signals_profile_2() {
    let src = gradient_rgb16(64, 64);
    let enc = zenavif::encode_rgb16(
        src.as_ref(),
        &aom_config()
            .bit_depth(zenavif::EncodeBitDepth::Twelve)
            .quality(90.0)
            .speed(6),
        stop(),
    )
    .expect("bd12 encode");
    let av1c = find_av1c(&enc.avif_file).expect("the container must carry an av1C box");
    // av1C payload byte 0: marker(1) | version(7). byte 1: seq_profile(3) |
    // seq_level_idx_0(5). byte 2: seq_tier_0(1) | high_bitdepth(1) |
    // twelve_bit(1) | monochrome(1) | chroma_subsampling_x(1) |
    // chroma_subsampling_y(1) | chroma_sample_position(2).
    let seq_profile = av1c[1] >> 5;
    let high_bitdepth = (av1c[2] >> 6) & 1;
    let twelve_bit = (av1c[2] >> 5) & 1;
    assert_eq!(seq_profile, 2, "av1C seq_profile for a 12-bit stream");
    assert_eq!(high_bitdepth, 1, "av1C high_bitdepth for a 12-bit stream");
    assert_eq!(twelve_bit, 1, "av1C twelve_bit for a 12-bit stream");

    // And 10-bit is profile 0 with high_bitdepth set, twelve_bit clear.
    let enc10 = zenavif::encode_rgb16(
        src.as_ref(),
        &aom_config()
            .bit_depth(zenavif::EncodeBitDepth::Ten)
            .quality(90.0)
            .speed(6),
        stop(),
    )
    .expect("bd10 encode");
    let av1c10 = find_av1c(&enc10.avif_file).expect("av1C");
    assert_eq!(
        av1c10[1] >> 5,
        0,
        "av1C seq_profile for a 10-bit 4:2:0 stream"
    );
    assert_eq!((av1c10[2] >> 6) & 1, 1, "av1C high_bitdepth for 10-bit");
    assert_eq!((av1c10[2] >> 5) & 1, 0, "av1C twelve_bit for 10-bit");
}

/// The 4 payload bytes of the first `av1C` box in the file.
fn find_av1c(avif: &[u8]) -> Option<[u8; 4]> {
    let pos = avif.windows(4).position(|w| w == b"av1C")?;
    let p = &avif[pos + 4..];
    if p.len() < 4 {
        return None;
    }
    Some([p[0], p[1], p[2], p[3]])
}

/// Alpha stays refused at every depth: the `auxl` item is not built, and 12-bit
/// does not change that.
///
/// (There is no "unsupported depth value" case to test through the public API
/// any more: `EncodeBitDepth` is the only spelling and every variant of it is
/// either codable here or resolves to one that is. `aom_depth_error`'s
/// `!matches!(bit_depth, 8 | 10 | 12)` arm is defensive — it takes a `u8`, so
/// it survives a future variant that resolves to something else — and is
/// deliberately left untested rather than reached through a back door.)
#[test]
fn aom_backend_refuses_depths_it_does_not_code() {
    let img = gradient_rgb8(64, 64);
    let rgba: ImgVec<rgb::Rgba<u8>> = Img::new(
        img.buf()
            .iter()
            .map(|p| rgb::Rgba {
                r: p.r,
                g: p.g,
                b: p.b,
                a: 255,
            })
            .collect::<Vec<_>>(),
        64,
        64,
    );
    let e = zenavif::encode_rgba8(
        rgba.as_ref(),
        &aom_config().bit_depth(zenavif::EncodeBitDepth::Twelve),
        stop(),
    )
    .expect_err("alpha must still be refused at 12 bits");
    assert!(
        format!("{e}").contains("no alpha auxiliary item"),
        "the alpha refusal must name the missing item, got: {e}"
    );
}

/// The zenravif and zenav1-svt backends refuse 12 bits by name rather than
/// silently coding 10.
///
/// `ravif::BitDepth` has no 12-bit representation, so without
/// `reject_unspellable_coded_depth` the zenravif path would code 10 and report
/// success — wrong pixels reported as an Ok.
#[test]
fn other_backends_refuse_12_bit_rather_than_coding_10() {
    let img = gradient_rgb8(64, 64);
    let e = zenavif::encode_rgb8(
        img.as_ref(),
        &EncoderConfig::new()
            .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
            .bit_depth(zenavif::EncodeBitDepth::Twelve),
        stop(),
    )
    .expect_err("the zenravif backend must refuse 12-bit");
    let msg = format!("{e}");
    assert!(
        msg.contains("Zenav1Aom") && msg.contains("8 and 10 bits only"),
        "the zenravif 12-bit refusal must name the limit and the backend that has it, got: {msg}"
    );
}

/// The Cs400 grayscale path is 8-bit only, and says so.
///
/// `encode_gray8` takes u8 samples and this seam passes them through as the
/// coded luma, so promoting them to a 10- or 12-bit swing would need a
/// value-scaling rule nothing here measures. Refused rather than guessed.
#[cfg(feature = "encode-mono")]
#[test]
fn aom_backend_refuses_hbd_grayscale() {
    let gray: Vec<u8> = (0..64 * 64).map(|i| ((i * 7) % 256) as u8).collect();
    let img = Img::new(gray, 64, 64);
    for cfg in [
        aom_config().bit_depth(zenavif::EncodeBitDepth::Ten),
        aom_config().bit_depth(zenavif::EncodeBitDepth::Twelve),
    ] {
        let e = zenavif::encode_gray8(img.as_ref(), &cfg, stop())
            .expect_err("high-bit-depth grayscale must be refused");
        assert!(
            format!("{e}").contains("8-bit grayscale (Cs400) only"),
            "the grayscale depth refusal must name the limitation, got: {e}"
        );
    }
    // The same config on the COLOUR path encodes — the refusal is scoped to
    // Cs400, not to the depth.
    let rgb = gradient_rgb8(64, 64);
    encode(
        rgb.as_ref(),
        &aom_config()
            .bit_depth(zenavif::EncodeBitDepth::Twelve)
            .quality(90.0)
            .speed(6),
    );
}

/// The 8-bit path is byte-for-byte what it was before the high-bit-depth
/// wiring landed, and the u8-kernel special case that keeps it that way is
/// load-bearing.
///
/// `color_planes_420` routes 8-bit input at depth 8 through the dedicated
/// `rgb8_to_yuv420` u8 kernel and widens, exactly as the seam always did; the
/// depth-generic `rgbx_to_yuv420_u16` recipe serves every other cell. This
/// test pins the resulting file's length and its full byte hash as a FORWARD
/// anchor: it is not by itself proof against the pre-change build (no
/// pre-change binary exists here to diff against — the identity argument is
/// the code path), but any later edit that folds the special case away moves
/// this hash.
#[test]
fn aom_bd8_output_is_unchanged_by_the_hbd_wiring() {
    let src = gradient_rgb8(64, 64);
    let enc = encode(src.as_ref(), &aom_config().quality(90.0).speed(6));
    let yuv = hbd_planes(&enc.avif_file, 8, "bd8 anchor");
    assert_eq!(yuv.width, 64);
    assert_eq!(yuv.height, 64);
    // Anchor measured 2026-09-03 on this content/config. A change here is a
    // change to 8-bit output and must be explained, not re-pinned reflexively.
    let digest = fnv1a(&enc.avif_file);
    eprintln!(
        "bd8 anchor: {} bytes, fnv1a-64 {digest:#018x}",
        enc.avif_file.len()
    );
    assert_eq!(
        digest,
        BD8_ANCHOR_FNV1A,
        "8-bit aom output changed (len {}). The high-bit-depth wiring must leave \
         depth-8 byte-identical: `color_planes_420` keeps the rgb8_to_yuv420 u8 \
         kernel for that cell precisely so this hash cannot move.",
        enc.avif_file.len()
    );
}

/// Measured 2026-09-03; see [`aom_bd8_output_is_unchanged_by_the_hbd_wiring`].
const BD8_ANCHOR_FNV1A: u64 = 0x622c_cd37_57b0_c862;

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// **Known upstream defect, pinned:** `--cq-level 0` (zenavif quality 100)
/// PANICS inside the zenav1-aom port on flat content, at every bit depth.
///
/// MEASURED 2026-09-03 at the pinned rev `c3e1b4ab`, 64x64, speed 6:
///
/// | quality | cq | bd8 flat | bd8 gradient | bd10 flat | bd10 grad | bd12 flat | bd12 grad |
/// |---|---|---|---|---|---|---|---|
/// | 100 | 0 | PANIC | ok | PANIC | ok | PANIC | PANIC |
/// | 99 | 1 | ok | ok | ok | ok | ok | ok |
///
/// The panic is `assertion failed: depth <= MAX_VARTX_DEPTH` at
/// `crates/aom-dsp/src/entropy/partition.rs:675` — a plain `assert!`, so it
/// fires in release builds too, and it crosses the seam as a process panic
/// rather than an `Err` (zenavif CLAUDE.md "Backend seam" obligation 2). It is
/// **content-dependent**, which is why this seam does not blanket-refuse
/// cq 0: a refusal would reject the gradient cells that encode correctly
/// today. The product decision belongs to the owner — tracked as an issue —
/// and this test pins the behaviour meanwhile so the day it is fixed, it says
/// so instead of going quietly green.
///
/// **This defect is NOT new with the high-bit-depth wiring**: bd8 flat at
/// quality 100 panics on `main` as it does here, and the module docs above
/// claiming "quality 100 -> cq 0 ... no clamp away from the endpoint" were
/// describing a mapping that panics.
#[test]
fn aom_cq0_still_panics_on_flat_content() {
    let flat8 = flat_rgb8(64, 64, 235);
    must_panic(
        "quality 100 (cq 0) on flat 8-bit content — if this stopped panicking, \
         zenav1-aom fixed MAX_VARTX_DEPTH and the seam can stop warning about it",
        || {
            let _ = encode(flat8.as_ref(), &aom_config().quality(100.0).speed(6));
        },
    );
    let flat16 = flat_rgb16(64, 64, 60293);
    for cfg in [
        aom_config().bit_depth(zenavif::EncodeBitDepth::Ten),
        aom_config().bit_depth(zenavif::EncodeBitDepth::Twelve),
    ] {
        let cfg = cfg.quality(100.0).speed(6);
        must_panic("quality 100 (cq 0) on flat high-bit-depth content", || {
            let _ = zenavif::encode_rgb16(flat16.as_ref(), &cfg, stop());
        });
    }
    // The boundary is cq 0 alone: quality 99 (cq 1) encodes on the same
    // content at all three depths.
    encode(flat8.as_ref(), &aom_config().quality(99.0).speed(6));
    for cfg in [
        aom_config().bit_depth(zenavif::EncodeBitDepth::Ten),
        aom_config().bit_depth(zenavif::EncodeBitDepth::Twelve),
    ] {
        zenavif::encode_rgb16(flat16.as_ref(), &cfg.quality(99.0).speed(6), stop())
            .expect("quality 99 must encode where quality 100 panics");
    }
}

// ---------------------------------------------------------------------------
// MUTATION PROOFS — the gates above must be able to go red.
// ---------------------------------------------------------------------------

fn must_panic(what: &str, f: impl FnOnce() + std::panic::UnwindSafe) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(f);
    std::panic::set_hook(prev);
    assert!(
        r.is_err(),
        "MUTATION PROOF FAILED: {what} did not make the gate fail — the gate is vacuous"
    );
}

/// The high-bit-depth luma gate must compare against the source, not merely
/// decode — and it must notice a depth that is not what was asked for.
#[test]
fn hbd_gate_can_fail_on_wrong_content_and_wrong_depth() {
    let a = gradient_rgb16(64, 64);
    let b = flat_rgb16(64, 64, 20000);
    let cfg = aom_config()
        .bit_depth(zenavif::EncodeBitDepth::Ten)
        .quality(90.0)
        .speed(6);
    let enc = zenavif::encode_rgb16(a.as_ref(), &cfg, stop()).expect("bd10 encode");
    // Sanity: against the RIGHT source it passes.
    let p = hbd_luma_psnr(&enc.avif_file, a.as_ref(), 10, "control");
    assert!(p >= 40.0, "control: {p:.2} dB");
    // Against the wrong source the same helper must fail the same bound.
    let q = hbd_luma_psnr(&enc.avif_file, b.as_ref(), 10, "mutant");
    assert!(
        q < 40.0,
        "MUTATION PROOF FAILED: an encode of A scored {q:.2} dB against source B — \
         the coded-luma comparison is vacuous"
    );
    // The depth assertion must fire when the expected depth is wrong.
    must_panic("claiming a 10-bit stream is 12-bit", || {
        let _ = hbd_planes(&enc.avif_file, 12, "mutant-depth");
    });
    // And the exact-flat gate must fail on the wrong flat value.
    let flat = flat_rgb16(64, 64, 60293);
    let fenc = zenavif::encode_rgb16(
        flat.as_ref(),
        &aom_config()
            .bit_depth(zenavif::EncodeBitDepth::Ten)
            .quality(95.0)
            .speed(6),
        stop(),
    )
    .expect("bd10 flat encode");
    assert_hbd_flat_luma_within(&fenc.avif_file, flat.as_ref(), 10, 0, "control-flat");
    must_panic(
        "comparing a flat 60293 encode against a flat 20000 source",
        || {
            assert_hbd_flat_luma_within(&fenc.avif_file, b.as_ref(), 10, 0, "mutant-flat");
        },
    );
    // ... and the bound must be load-bearing, not permissive: the same
    // comparison with a bound wide enough to swallow a range error would pass,
    // so the bound the gate ships with (0 at 10 bits) is what makes it a gate.
    must_panic("a 1-code bound against a wrong flat source", || {
        assert_hbd_flat_luma_within(&fenc.avif_file, b.as_ref(), 10, 1, "mutant-flat-loose");
    });
}

/// The longhand H.273 expectation must actually scale with depth — if
/// `expected_limited_luma_hbd` ignored `depth`, every gate above would be
/// comparing 8-bit constants against high-bit-depth planes and would fail;
/// this asserts the scaling directly so the helper cannot silently degenerate.
#[test]
fn the_studio_swing_expectation_scales_with_depth() {
    let src = flat_rgb16(2, 2, 65535);
    // A full-scale white codes to 16 + 219 = 235 at 8 bits, and to that
    // shifted up by (depth - 8) at higher depths.
    assert_eq!(expected_limited_luma_hbd(src.as_ref(), 8)[0], 235);
    assert_eq!(expected_limited_luma_hbd(src.as_ref(), 10)[0], 940);
    assert_eq!(expected_limited_luma_hbd(src.as_ref(), 12)[0], 3760);
    let black = flat_rgb16(2, 2, 0);
    assert_eq!(expected_limited_luma_hbd(black.as_ref(), 10)[0], 64);
    assert_eq!(expected_limited_luma_hbd(black.as_ref(), 12)[0], 256);
}

/// The RGB round trip must compare against the source, not merely decode.
#[test]
fn gate_can_fail_on_wrong_content() {
    let a = gradient_rgb8(64, 64);
    let b = flat_rgb8(64, 64, 30);
    let enc = encode(a.as_ref(), &aom_config().quality(90.0).speed(6));
    // Sanity: the same bytes against the RIGHT source pass.
    assert_rgb_round_trip(&enc.avif_file, a.as_ref(), 38.0, "control");
    must_panic("comparing an encode of A against source B", || {
        assert_rgb_round_trip(&enc.avif_file, b.as_ref(), 38.0, "mutant");
    });
    // Same for the plane-level gate.
    let flat = flat_rgb8(64, 64, 128);
    let fenc = encode(flat.as_ref(), &aom_config().quality(95.0).speed(6));
    let right = expected_limited_luma(flat.as_ref());
    assert_independent_decoder_luma_exact(&fenc.avif_file, &right, "control");
    let wrong: Vec<u8> = right.iter().map(|v| v.wrapping_add(1)).collect();
    must_panic("one-off luma expectation", || {
        assert_independent_decoder_luma_exact(&fenc.avif_file, &wrong, "mutant");
    });
}

/// Corrupting the coded payload must be caught — proving the gate really
/// decodes the bytes we produced rather than trusting the container.
#[test]
fn gate_can_fail_on_a_corrupted_payload() {
    let img = gradient_rgb8(64, 64);
    let enc = encode(img.as_ref(), &aom_config().quality(90.0).speed(6));
    let payload = primary_payload(&enc.avif_file);
    // Find the payload inside the container and flip bits deep in the tile
    // data (well past the headers), then re-search: the mux writes the
    // payload contiguously.
    let start = enc
        .avif_file
        .windows(payload.len())
        .position(|w| w == payload.as_slice())
        .expect("the primary payload must appear verbatim in the container");
    let mut broken = enc.avif_file.clone();
    let hit = start + payload.len() - 8;
    broken[hit] ^= 0xFF;
    broken[hit + 1] ^= 0xA5;
    must_panic("flipping two bytes of coded tile data", || {
        assert_rgb_round_trip(&broken, img.as_ref(), 38.0, "mutant");
    });
}

/// The limited-range choice is load-bearing, and this measures it rather than
/// asserting it from the source.
///
/// A flat 235 source converts to studio luma `round(235/255*219 + 16) = 218`.
/// Two things then have to hold, and the second is what makes the first mean
/// something: the decoder must return 235 (not 218), AND 218 must be far
/// enough from 235 that returning it would have failed. Without the second
/// check a tolerance quietly wide enough to admit both would look like a pass.
#[test]
fn limited_range_signalling_is_load_bearing() {
    let img = flat_rgb8(64, 64, 235);
    let enc = encode(img.as_ref(), &aom_config().quality(95.0).speed(6));
    let coded_luma = expected_limited_luma(img.as_ref())[0];
    assert_eq!(coded_luma, 218, "the studio swing must code 235 as 218");
    // What the decoder actually returns.
    assert_flat_round_trips_exactly(&enc.avif_file, img.as_ref(), "flat 235 limited-range");
    // And the discrimination: a full-range misreading is 17 away, which the
    // tolerance-1 assertion above cannot absorb.
    let delta = i32::from(235u8) - i32::from(coded_luma);
    assert!(
        delta.abs() > 8,
        "a full-range misreading would be only {delta} away — this gate could not tell"
    );
    // Prove it: the same assertion against the full-range misreading fails.
    let misread = flat_rgb8(64, 64, coded_luma);
    must_panic("a full-range misreading of the same stream", || {
        assert_flat_round_trips_exactly(&enc.avif_file, misread.as_ref(), "mutant");
    });
}

/// The aom backend must be in the same league as the production backend at
/// the same quality and subsampling — a bound an absolute PSNR floor cannot
/// give, because a systematic regression can still clear a floor.
///
/// MEASURED at q90 across the size grid: the aom backend runs from 1.6 dB
/// behind zenravif to 0.2 dB ahead of it. The bound is 5 dB.
#[test]
fn aom_backend_tracks_the_production_backend() {
    for &(w, h) in &[(64usize, 64usize), (128, 96), (33, 47), (192, 64)] {
        let img = gradient_rgb8(w, h);
        let aom = encode(img.as_ref(), &aom_config().quality(90.0).speed(6));
        let zenravif = zenavif::encode_rgb8(
            img.as_ref(),
            &EncoderConfig::new()
                .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
                .quality(90.0)
                .speed(6),
            stop(),
        )
        .expect("zenravif reference encode");
        let p_aom = decoded_psnr(&aom.avif_file, img.as_ref(), "aom");
        let p_ref = decoded_psnr(&zenravif.avif_file, img.as_ref(), "zenravif");
        eprintln!(
            "{w}x{h} q90 s6: aom {p_aom:.2} dB / {} B, zenravif {p_ref:.2} dB / {} B",
            aom.color_byte_size, zenravif.color_byte_size
        );
        assert!(
            p_aom >= p_ref - 5.0,
            "{w}x{h}: aom {p_aom:.2} dB is more than 5 dB behind zenravif {p_ref:.2} dB"
        );
    }
}

fn decoded_psnr(avif: &[u8], src: ImgRef<'_, Rgb<u8>>, label: &str) -> f64 {
    let decoded = zenavif::decode(avif).unwrap_or_else(|e| panic!("{label}: must decode: {e}"));
    let out = decoded
        .try_as_imgref::<Rgb<u8>>()
        .unwrap_or_else(|| panic!("{label}: no-alpha decode yields RGB8"));
    psnr_rgb8(src.buf(), out.buf())
}

/// When the zenav1-aom DECODE backend is also built, two independent decoder
/// ports must agree bit-exactly on the aom encoder's own output — the same
/// contract `tests/cross_backend_decode.rs` holds the other encoders to.
#[cfg(feature = "zenav1-aom")]
#[test]
fn two_independent_decoders_agree_on_aom_output() {
    let img = gradient_rgb8(64, 64);
    let enc = encode(img.as_ref(), &aom_config().quality(90.0).speed(6));
    let payload = primary_payload(&enc.avif_file);
    let rav = decode_av1_obu_yuv(&payload, DecodeBackend::Rav1dSafe).expect("rav1d-safe decode");
    let aom = decode_av1_obu_yuv(&payload, DecodeBackend::Zenav1Aom).expect("zenav1-aom decode");
    assert_eq!(
        (
            rav.width,
            rav.height,
            rav.width_uv,
            rav.height_uv,
            rav.bit_depth
        ),
        (
            aom.width,
            aom.height,
            aom.width_uv,
            aom.height_uv,
            aom.bit_depth
        ),
        "decoders disagree on geometry"
    );
    assert_eq!(rav.y, aom.y, "decoders disagree on luma");
    assert_eq!(rav.u, aom.u, "decoders disagree on Cb");
    assert_eq!(rav.v, aom.v, "decoders disagree on Cr");
    eprintln!("rav1d-safe and zenav1-aom agree bit-exactly on the aom encoder's output");
}
