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
//! | bd10/bd12 container RGB round trip, 3 sizes x q80/q90 | 43.56-48.12 dB | 38 dB |
//! | q100 (cq 0, coded-lossless) Y/U/V planes, 6 cells, 2 decoders | 0 mismatched samples | 0 (equality) |
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

/// [`aom_config`] at a coded depth of `depth` bits. One place spells the
/// depth, so a test cannot ask for 12 and assert about 10.
fn hbd_config(depth: u8) -> EncoderConfig {
    aom_config().bit_depth(match depth {
        8 => zenavif::EncodeBitDepth::Eight,
        10 => zenavif::EncodeBitDepth::Ten,
        12 => zenavif::EncodeBitDepth::Twelve,
        other => panic!("hbd_config: {other} is not a depth this seam codes"),
    })
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
/// lands one below. (`--cq-level 0` IS `base_qindex` 0 — coded-lossless —
/// and reconstructs every plane exactly at all three depths since the pin
/// moved past zenav1-aom `21544fde`; see
/// [`aom_cq0_encodes_and_reconstructs_the_coded_planes_exactly`].)
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
    for depth in [10u8, 12] {
        let cfg = hbd_config(depth);
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
        // quality 99, not 100, on purpose: this gate measures the NEAR-lossless
        // end (cq 1) against the longhand with a per-depth bound. cq 0 is the
        // coded-lossless end and has its own zero-tolerance gate,
        // `aom_cq0_encodes_and_reconstructs_the_coded_planes_exactly`.
        let cfg = hbd_config(depth).quality(99.0).speed(6);
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
        let cfg = hbd_config(depth).quality(90.0).speed(6);
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

/// A high-bit-depth AVIF also decodes correctly **through the container** —
/// `zenavif::decode`, the way a caller actually consumes one — and not only
/// through the raw-OBU seam the other high-bit-depth gates use.
///
/// This is a different path from `hbd_planes`: it applies the container's
/// `colr`, undoes the studio swing and converts YUV back to RGB, so it is what
/// catches a mux-level colour error that a coded-luma comparison would not.
/// It also pins the OUTPUT PIXEL TYPE, which changes with the coded depth:
/// `Rgb<u16>` at 10 and 12 bits, `Rgb<u8>` at 8.
///
/// MEASURED over 3 sizes x 2 depths x 2 qualities before the bound was
/// written: **43.56–48.12 dB**, worst cell 192x128 bd12 q80. Bound 38 dB,
/// ~5.5 dB of headroom, matching the 8-bit gates' convention.
#[test]
fn aom_hbd_decodes_correctly_through_the_container() {
    let mut worst = f64::INFINITY;
    for &(w, h) in &[(64usize, 64usize), (65, 33), (192, 128)] {
        let src = gradient_rgb16(w, h);
        for depth in [10u8, 12] {
            for q in [80.0f32, 90.0] {
                let cfg = hbd_config(depth).quality(q).speed(6);
                let enc = zenavif::encode_rgb16(src.as_ref(), &cfg, stop())
                    .unwrap_or_else(|e| panic!("bd{depth} {w}x{h} q{q}: {e}"));
                let label = format!("bd{depth} {w}x{h} q{q}");
                let p = container_rgb16_psnr(&enc.avif_file, src.as_ref(), &label);
                assert!(p >= 38.0, "{label}: container PSNR {p:.2} dB < 38 dB");
                worst = worst.min(p);
            }
        }
    }
    eprintln!("container round trip: worst {worst:.2} dB over 12 cells");
    // 8 bits comes back as Rgb<u8>, not Rgb<u16> — the pixel type follows the
    // coded depth, and a caller that assumed otherwise would get `None`.
    let src8 = gradient_rgb8(64, 64);
    let enc8 = encode(src8.as_ref(), &aom_config().quality(90.0).speed(6));
    let dec8 = zenavif::decode(&enc8.avif_file).expect("bd8 decode");
    assert!(
        dec8.try_as_imgref::<Rgb<u8>>().is_some(),
        "an 8-bit aom AVIF must decode to Rgb<u8>"
    );
    assert!(
        dec8.try_as_imgref::<Rgb<u16>>().is_none(),
        "an 8-bit aom AVIF must NOT present as Rgb<u16>"
    );
}

/// `zenavif::decode` -> `Rgb<u16>` PSNR against the source, over the full u16
/// scale. Panics if the decode does not present as `Rgb<u16>`, which is itself
/// part of the assertion (see the caller).
fn container_rgb16_psnr(avif: &[u8], src: ImgRef<'_, rgb::Rgb<u16>>, label: &str) -> f64 {
    let dec = zenavif::decode(avif)
        .unwrap_or_else(|e| panic!("{label}: a high-bit-depth aom AVIF must decode: {e}"));
    assert_eq!(dec.width() as usize, src.width(), "{label}: width");
    assert_eq!(dec.height() as usize, src.height(), "{label}: height");
    let out = dec
        .try_as_imgref::<rgb::Rgb<u16>>()
        .unwrap_or_else(|| panic!("{label}: a 10/12-bit decode must yield Rgb<u16>"));
    let mut se = 0f64;
    for (p, q) in src.buf().iter().zip(out.buf().iter()) {
        for (a, b) in [(p.r, q.r), (p.g, q.g), (p.b, q.b)] {
            let d = f64::from(a) - f64::from(b);
            se += d * d;
        }
    }
    if se == 0.0 {
        return f64::INFINITY;
    }
    let mse = se / (src.buf().len() * 3) as f64;
    10.0 * (65535.0 * 65535.0 / mse).log10()
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
    for cfg in [hbd_config(10), hbd_config(12)] {
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
/// The pre-vs-post comparison itself is NOT this test — it is
/// `benchmarks/aom_bd8_identity_2026-09-03.*`, which drives the same emitter
/// from a `git archive` of `ec6728b` and from this tree and diffs the two:
/// **60/60 cells byte-identical**, with a full-range mutation moving 60/60 as
/// the anti-vacuity control. This test is the cheap FORWARD anchor on one of
/// those cells, so a later edit that moves 8-bit output turns it red in the
/// normal test run rather than only in a benchmark nobody re-runs.
///
/// Note what this does NOT prove: the u8-kernel special case in
/// `color_planes_420` is not what holds the bytes still. The same benchmark
/// measured routing that cell through the depth-generic u16 recipe instead and
/// got **0 of 60** cells changed.
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
        "8-bit aom output changed (len {}). Depth 8 was byte-identical across the \
         high-bit-depth wiring (60/60 cells, benchmarks/aom_bd8_identity_2026-09-03.*) \
         and is expected to stay that way; explain the change, do not re-pin the hash \
         reflexively.",
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

/// The six cells the retired canary `aom_cq0_still_panics_on_flat_content`
/// covered, unchanged: {flat, gradient} x bd {8, 10, 12}, 64x64, speed 6,
/// 4:2:0. The bd8 cells come from 8-bit sources through `encode_rgb8`, the
/// bd10/bd12 cells from 16-bit sources through `encode_rgb16`, exactly as the
/// canary drove them.
struct Cq0Cell {
    label: String,
    depth: u8,
    src8: ImgVec<Rgb<u8>>,
    src16: ImgVec<Rgb<u16>>,
}

fn cq0_cells() -> Vec<Cq0Cell> {
    let mut cells = Vec::new();
    for depth in [8u8, 10, 12] {
        // 235 at 8 bits and 60293 at 16 — the canary's own values.
        cells.push(Cq0Cell {
            label: format!("bd{depth} flat 64x64 q100 s6"),
            depth,
            src8: flat_rgb8(64, 64, 235),
            src16: flat_rgb16(64, 64, 60293),
        });
        cells.push(Cq0Cell {
            label: format!("bd{depth} gradient 64x64 q100 s6"),
            depth,
            src8: gradient_rgb8(64, 64),
            src16: gradient_rgb16(64, 64),
        });
    }
    cells
}

/// Encode one [`cq0_cells`] cell at `quality`; 8-bit sources feed the bd8
/// cells, 16-bit sources the bd10/bd12 cells.
fn encode_cq0_cell(
    depth: u8,
    src8: ImgRef<'_, Rgb<u8>>,
    src16: ImgRef<'_, Rgb<u16>>,
    quality: f32,
    label: &str,
) -> EncodedImage {
    let cfg = hbd_config(depth).quality(quality).speed(6);
    if depth == 8 {
        zenavif::encode_rgb8(src8, &cfg, stop())
    } else {
        zenavif::encode_rgb16(src16, &cfg, stop())
    }
    .unwrap_or_else(|e| panic!("{label}: encode must succeed: {e}"))
}

/// The luma / chroma planes as **real numbers before quantisation**, plus
/// their rounded code values — the BT.601 limited-range 4:2:0 conversion
/// written longhand from H.273 in f64, independent of `src/yuv_convert.rs`.
///
/// `px(x, y)` yields the source pixel normalised to `0.0..=1.0`. Chroma is
/// the mean of the four co-sited samples of a 2x2 block (right/bottom edges
/// clamp), quantised once at the output depth: offset `16 << (d-8)`, spans
/// `219 << (d-8)` (luma) and `224 << (d-8)` (chroma), centre `1 << (d-1)`.
/// Rounding is H.273's `Round(x) = Sign(x) * Floor(Abs(x) + 0.5)`.
struct LonghandPlanes {
    /// Pre-round values, Y then U then V, in plane order.
    real: [Vec<f64>; 3],
    /// H.273-rounded code values in the same order.
    code: [Vec<u16>; 3],
}

fn longhand_limited_yuv420(
    w: usize,
    h: usize,
    depth: u8,
    px: impl Fn(usize, usize) -> (f64, f64, f64),
) -> LonghandPlanes {
    let (kr, kb) = (0.299f64, 0.114f64);
    let kg = 1.0 - kr - kb;
    let scale = f64::from(1u32 << (depth - 8));
    let max = f64::from((1u32 << depth) - 1);
    let q = |v: f64| -> u16 { (v.abs() + 0.5).floor().copysign(v).clamp(0.0, max) as u16 };
    let mut y_real = vec![0f64; w * h];
    let mut pb = vec![0f64; w * h];
    let mut pr = vec![0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = px(x, y);
            let ey = kr * r + kg * g + kb * b;
            y_real[y * w + x] = 219.0 * scale * ey + 16.0 * scale;
            pb[y * w + x] = (b - ey) / (2.0 * (1.0 - kb));
            pr[y * w + x] = (r - ey) / (2.0 * (1.0 - kr));
        }
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut u_real = vec![0f64; cw * ch];
    let mut v_real = vec![0f64; cw * ch];
    let centre = f64::from(1u32 << (depth - 1));
    for cy in 0..ch {
        for cx in 0..cw {
            let (x0, y0) = (2 * cx, 2 * cy);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let idx = [y0 * w + x0, y0 * w + x1, y1 * w + x0, y1 * w + x1];
            let mu = idx.iter().map(|&i| pb[i]).sum::<f64>() / 4.0;
            let mv = idx.iter().map(|&i| pr[i]).sum::<f64>() / 4.0;
            u_real[cy * cw + cx] = 224.0 * scale * mu + centre;
            v_real[cy * cw + cx] = 224.0 * scale * mv + centre;
        }
    }
    let code = [
        y_real.iter().map(|&v| q(v)).collect(),
        u_real.iter().map(|&v| q(v)).collect(),
        v_real.iter().map(|&v| q(v)).collect(),
    ];
    LonghandPlanes {
        real: [y_real, u_real, v_real],
        code,
    }
}

/// The planes the seam actually hands `encode_key_frame`: a bit-exact
/// mirror of the `src/yuv_convert.rs` forward recipe (f32, `mul_add` in the
/// kernel's association, `round_ties_even`; the u8 kernel the bd8 cell takes
/// and the u16 kernel the others take are the same arithmetic at their
/// depth, and `mul_add` is a single exactly-rounded FMA on every dispatch
/// tier). A mirror rather than the longhand, on purpose: the coded-lossless
/// claim is "reconstruction == the encoder's INPUT", and this is that input
/// to the last code value, so the comparison can be a zero-tolerance
/// equality. Where this and the H.273 longhand disagree is pinned separately
/// — see [`assert_longhand_agrees_up_to_ties`].
fn seam_mirror_limited_yuv420(
    w: usize,
    h: usize,
    depth: u8,
    max: f32,
    px: impl Fn(usize, usize) -> (f32, f32, f32),
) -> [Vec<u16>; 3] {
    let (kr, kb) = (0.299f32, 0.114f32);
    let kg = 1.0 - kr - kb;
    let scale = (1u32 << (depth - 8)) as f32;
    let (y_off, y_span, uv_span) = (16.0 * scale, 219.0 * scale, 224.0 * scale);
    let inv_ub = 1.0 / (2.0 * (1.0 - kb));
    let inv_vr = 1.0 / (2.0 * (1.0 - kr));
    let inv_max = 1.0 / max;
    let out_max = ((1u32 << depth) - 1) as f32;
    let uv_center = (1u32 << (depth - 1)) as f32;
    let mut y_plane = vec![0u16; w * h];
    let mut u_rows = vec![0f32; w * h];
    let mut v_rows = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = px(x, y);
            let (rn, gn, bn) = (r * inv_max, g * inv_max, b * inv_max);
            let yl = kb.mul_add(bn, kr.mul_add(rn, kg * gn));
            y_plane[y * w + x] = yl
                .mul_add(y_span, y_off)
                .clamp(0.0, out_max)
                .round_ties_even() as u16;
            u_rows[y * w + x] = (bn - yl) * inv_ub;
            v_rows[y * w + x] = (rn - yl) * inv_vr;
        }
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut u_plane = vec![0u16; cw * ch];
    let mut v_plane = vec![0u16; cw * ch];
    for cy in 0..ch {
        for cx in 0..cw {
            let (x0, y0) = (2 * cx, 2 * cy);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let ua = 0.25
                * (u_rows[y0 * w + x0]
                    + u_rows[y0 * w + x1]
                    + u_rows[y1 * w + x0]
                    + u_rows[y1 * w + x1]);
            let va = 0.25
                * (v_rows[y0 * w + x0]
                    + v_rows[y0 * w + x1]
                    + v_rows[y1 * w + x0]
                    + v_rows[y1 * w + x1]);
            u_plane[cy * cw + cx] = ua
                .mul_add(uv_span, uv_center)
                .clamp(0.0, out_max)
                .round_ties_even() as u16;
            v_plane[cy * cw + cx] = va
                .mul_add(uv_span, uv_center)
                .clamp(0.0, out_max)
                .round_ties_even() as u16;
        }
    }
    [y_plane, u_plane, v_plane]
}

/// Both expectations for one [`cq0_cells`] cell: the 8-bit source at depth
/// 8, the 16-bit source otherwise (mirroring [`encode_cq0_cell`]).
fn expected_cq0_planes(
    depth: u8,
    src8: ImgRef<'_, Rgb<u8>>,
    src16: ImgRef<'_, Rgb<u16>>,
) -> ([Vec<u16>; 3], LonghandPlanes) {
    if depth == 8 {
        let (w, h) = (src8.width(), src8.height());
        (
            seam_mirror_limited_yuv420(w, h, depth, 255.0, |x, y| {
                let p = src8[(x, y)];
                (f32::from(p.r), f32::from(p.g), f32::from(p.b))
            }),
            longhand_limited_yuv420(w, h, depth, |x, y| {
                let p = src8[(x, y)];
                (
                    f64::from(p.r) / 255.0,
                    f64::from(p.g) / 255.0,
                    f64::from(p.b) / 255.0,
                )
            }),
        )
    } else {
        let (w, h) = (src16.width(), src16.height());
        (
            seam_mirror_limited_yuv420(w, h, depth, 65535.0, |x, y| {
                let p = src16[(x, y)];
                (f32::from(p.r), f32::from(p.g), f32::from(p.b))
            }),
            longhand_limited_yuv420(w, h, depth, |x, y| {
                let p = src16[(x, y)];
                (
                    f64::from(p.r) / 65535.0,
                    f64::from(p.g) / 65535.0,
                    f64::from(p.b) / 65535.0,
                )
            }),
        )
    }
}

/// Count the samples where a decoded plane differs from its expectation,
/// keeping the first few `(got, expected)` pairs for the failure message.
fn plane_mismatches(got: &[u16], expect: &[u16], name: &str, label: &str) -> (usize, String) {
    assert_eq!(
        got.len(),
        expect.len(),
        "{label}: {name} plane length ({} vs {})",
        got.len(),
        expect.len()
    );
    let n = got.iter().zip(expect).filter(|(a, b)| a != b).count();
    let first = got
        .iter()
        .zip(expect)
        .filter(|(a, b)| a != b)
        .take(4)
        .map(|(a, b)| format!("{a}!={b}"))
        .collect::<Vec<_>>()
        .join(", ");
    (n, first)
}

/// Decode `avif` with `backend` and require every sample of every plane to
/// equal the seam's input — an equality on the CODED planes, not a PSNR
/// bound. `--cq-level 0` is coded-lossless (`base_qindex == 0` selects the
/// Walsh-Hadamard path), so the reconstruction must be the encoder's input
/// bit for bit; anything else is a codec defect, however small.
fn assert_planes_exact(
    avif: &[u8],
    backend: DecodeBackend,
    depth: u8,
    expect: &[Vec<u16>; 3],
    label: &str,
) {
    let payload = primary_payload(avif);
    let yuv = decode_av1_obu_yuv(&payload, backend)
        .unwrap_or_else(|e| panic!("{label}: {backend:?} must decode the aom stream: {e}"));
    assert_eq!(
        yuv.bit_depth,
        i32::from(depth),
        "{label}: {backend:?} reports a different coded depth than was requested"
    );
    assert_eq!(
        (yuv.subsampling_x, yuv.subsampling_y),
        (1, 1),
        "{label}: 4:2:0"
    );
    let (ny, fy) = plane_mismatches(&yuv.y, &expect[0], "Y", label);
    let (nu, fu) = plane_mismatches(&yuv.u, &expect[1], "U", label);
    let (nv, fv) = plane_mismatches(&yuv.v, &expect[2], "V", label);
    assert!(
        ny + nu + nv == 0,
        "{label}: {backend:?} reconstruction is NOT the encoder's input — cq 0 is \
         coded-lossless, so this is a codec defect, not rounding. Mismatched samples: \
         Y {ny} of {} ({fy}), U {nu} of {} ({fu}), V {nv} of {} ({fv})",
        expect[0].len(),
        expect[1].len(),
        expect[2].len()
    );
    eprintln!(
        "{label}: {backend:?} {depth}-bit Y/U/V exact ({} + {} + {} samples)",
        expect[0].len(),
        expect[1].len(),
        expect[2].len()
    );
}

/// A pre-round value this close to a half-integer is a rounding TIE: which
/// side it lands on is decided by floating-point noise, not by the
/// conversion. 0.002 code values is 1/500 of the smallest error a codec can
/// make, and an order of magnitude above the f32 recipe's own error at 12
/// bits (ulp ~2.4e-4 at 4095). The ties observed on these cells sit
/// 1.7e-5 (bd12 gradient, luma 1695.499983) and ~1e-14 (bd8 gradient,
/// luma 125.5 exactly) from a half.
const TIE_EPS: f64 = 0.002;

/// The seam mirror must agree with the INDEPENDENT H.273 longhand on every
/// sample, except where the longhand's real value is a rounding tie — there
/// the two may differ by exactly one code value. This is what keeps the
/// zero-tolerance gate above honest: the "encoder's input" it compares
/// against is pinned to the standard's conversion, not merely to whatever
/// the kernel emits. Returns the number of tolerated ties.
fn assert_longhand_agrees_up_to_ties(
    mirror: &[Vec<u16>; 3],
    longhand: &LonghandPlanes,
    label: &str,
) -> usize {
    let mut ties = 0usize;
    for (plane, name) in ["Y", "U", "V"].iter().enumerate() {
        for (i, ((&m, &c), &real)) in mirror[plane]
            .iter()
            .zip(&longhand.code[plane])
            .zip(&longhand.real[plane])
            .enumerate()
        {
            if m == c {
                continue;
            }
            let delta = (i32::from(m) - i32::from(c)).abs();
            let from_half = ((real - real.floor()) - 0.5).abs();
            assert!(
                delta == 1 && from_half < TIE_EPS,
                "{label}: {name}[{i}] — the seam converts to {m} but H.273 longhand says {c} \
                 (real {real:.6}, {from_half:.2e} from a half). Not a tie: the seam's \
                 conversion disagrees with the standard."
            );
            ties += 1;
        }
    }
    eprintln!("{label}: seam mirror agrees with the H.273 longhand up to {ties} rounding tie(s)");
    ties
}

/// **Quality 100 (`--cq-level 0`) encodes and reconstructs EXACTLY**, on the
/// six cells where it used to panic: {flat, gradient} x bd {8, 10, 12},
/// 64x64, speed 6, 4:2:0.
///
/// This replaces the canary `aom_cq0_still_panics_on_flat_content`, which
/// pinned zenavif#45 — `assertion failed: depth <= MAX_VARTX_DEPTH` inside
/// the port at the previous pin `c3e1b4ab`. Fixed at the root in
/// imazen/zenav1-aom `21544fde` (`count_leaf` had paraphrased libaom's
/// `txb_split_count` predicate as `tx_size_to_depth(..) != 0`; C writes the
/// inequality and never walks the depth under `ONLY_4X4`, which is what a
/// coded-lossless frame selects). The pin moved to `45c53ddb`, and the six
/// cells now encode, so the canary's `must_panic` went red — as designed —
/// and became this.
///
/// The assertion is an **equality on every sample of every coded plane**,
/// not a PSNR bound: cq 0 is coded-lossless (`base_qindex == 0`, WHT), so a
/// conforming decode returns the encoder's input bit for bit. "Lossless"
/// here is the CODED planes — the RGB round trip is still through 4:2:0
/// subsampling and studio range, which the longhand accounts for. Decoded by
/// rav1d-safe — a different port from the one that encoded — and, with
/// `zenav1-aom`, by aom-decode as well.
///
/// Proved able to fail by [`cq0_gate_can_fail_on_a_lossy_encode`], which
/// runs the same helper on quality 99 (cq 1) output of the same cells and
/// requires it to go red.
#[test]
fn aom_cq0_encodes_and_reconstructs_the_coded_planes_exactly() {
    let mut ties_total = 0usize;
    for Cq0Cell {
        label,
        depth,
        src8,
        src16,
    } in cq0_cells()
    {
        let enc = encode_cq0_cell(depth, src8.as_ref(), src16.as_ref(), 100.0, &label);
        assert_is_avif_container(&enc.avif_file, &label);
        let (mirror, longhand) = expected_cq0_planes(depth, src8.as_ref(), src16.as_ref());
        ties_total += assert_longhand_agrees_up_to_ties(&mirror, &longhand, &label);
        assert_planes_exact(
            &enc.avif_file,
            DecodeBackend::Rav1dSafe,
            depth,
            &mirror,
            &label,
        );
        #[cfg(feature = "zenav1-aom")]
        assert_planes_exact(
            &enc.avif_file,
            DecodeBackend::Zenav1Aom,
            depth,
            &mirror,
            &label,
        );
        eprintln!("{label}: {} bytes", enc.avif_file.len());
    }
    // MEASURED 2026-09-04: exactly two ties across the six cells (one luma
    // sample each on the bd8 and bd12 gradients; every flat cell and the
    // bd10 gradient are tie-free). The bound is a ceiling on how much of the
    // comparison the tie exception may absorb, so the gate cannot drift into
    // "everything is a tie" if the longhand is ever edited.
    assert!(
        ties_total <= 4,
        "{ties_total} rounding ties across the six cells; measured 2 — if the content or \
         the conversion changed, re-measure rather than raising this"
    );
}

/// The exact-planes gate must go RED on a lossy encode of the same six cells.
///
/// `--cq-level 1` (quality 99) is the neighbouring quantiser and NOT
/// coded-lossless; on the gradient cells its reconstruction differs from the
/// encoder's input, so [`assert_planes_exact`] must panic there. This is the
/// mutation "request quality 99 and keep the exactness assert" made
/// permanent: a later edit that makes the equality vacuous turns this red.
///
/// MEASURED 2026-09-04 at the pinned rev: at quality 99 the three gradient
/// cells are inexact, and so is bd12 flat (a flat frame's DC survives cq 1
/// at 8 and 10 bits but lands one code value off at 12 —
/// `assert_hbd_flat_luma_within` measures the same). The proof is pinned to
/// the gradient cells, where it is unconditional, and merely reports the
/// flat ones.
#[test]
fn cq0_gate_can_fail_on_a_lossy_encode() {
    let mut inexact = Vec::new();
    for Cq0Cell {
        label,
        depth,
        src8,
        src16,
    } in cq0_cells()
    {
        let label = label.replace("q100", "q99");
        let enc = encode_cq0_cell(depth, src8.as_ref(), src16.as_ref(), 99.0, &label);
        let (mirror, _) = expected_cq0_planes(depth, src8.as_ref(), src16.as_ref());
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_planes_exact(
                &enc.avif_file,
                DecodeBackend::Rav1dSafe,
                depth,
                &mirror,
                &label,
            );
        }));
        std::panic::set_hook(prev);
        eprintln!(
            "{label}: cq 1 reconstruction {}",
            if r.is_err() {
                "INEXACT (gate fires)"
            } else {
                "exact"
            }
        );
        if r.is_err() {
            inexact.push(label);
        }
    }
    for cell in ["bd8 gradient", "bd10 gradient", "bd12 gradient"] {
        assert!(
            inexact.iter().any(|l| l.starts_with(cell)),
            "MUTATION PROOF FAILED: the exact-planes gate did not fire on the {cell} cell at \
             quality 99 (cq 1) — the equality is vacuous. Fired on: {inexact:?}"
        );
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
    // The CONTAINER round trip must also compare against the source, not
    // merely decode — it is a different code path from the coded-luma gate
    // (it applies `colr`, undoes the studio swing, converts back to RGB).
    let cenc = zenavif::encode_rgb16(a.as_ref(), &hbd_config(12).quality(90.0).speed(6), stop())
        .expect("bd12 encode");
    let cp = container_rgb16_psnr(&cenc.avif_file, a.as_ref(), "control-container");
    assert!(cp >= 38.0, "control-container: {cp:.2} dB");
    let cq = container_rgb16_psnr(&cenc.avif_file, b.as_ref(), "mutant-container");
    assert!(
        cq < 38.0,
        "MUTATION PROOF FAILED: a bd12 encode of A scored {cq:.2} dB against source B \
         through the container — that comparison is vacuous"
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

/// The same two-decoder agreement at **10 and 12 bits**.
///
/// The high-bit-depth gates above all decode with rav1d-safe alone. Agreement
/// with a second, independently-written decoder is a stronger statement about
/// the bitstream than any single decoder's output, and it is exactly the
/// property the 8-bit gate above already asserts — there is no reason for the
/// high-bit-depth path to be held to a weaker bar.
#[cfg(feature = "zenav1-aom")]
#[test]
fn two_independent_decoders_agree_at_10_and_12_bits() {
    let src = gradient_rgb16(96, 64);
    for depth in [10u8, 12] {
        let enc = zenavif::encode_rgb16(
            src.as_ref(),
            &hbd_config(depth).quality(90.0).speed(6),
            stop(),
        )
        .unwrap_or_else(|e| panic!("bd{depth}: {e}"));
        let payload = primary_payload(&enc.avif_file);
        let rav = decode_av1_obu_yuv(&payload, DecodeBackend::Rav1dSafe)
            .unwrap_or_else(|e| panic!("bd{depth}: rav1d-safe: {e}"));
        let aom = decode_av1_obu_yuv(&payload, DecodeBackend::Zenav1Aom)
            .unwrap_or_else(|e| panic!("bd{depth}: zenav1-aom: {e}"));
        assert_eq!(
            (rav.width, rav.height, rav.bit_depth),
            (aom.width, aom.height, aom.bit_depth),
            "bd{depth}: decoders disagree on geometry or depth"
        );
        assert_eq!(rav.bit_depth, i32::from(depth), "bd{depth}: coded depth");
        assert_eq!(rav.y, aom.y, "bd{depth}: decoders disagree on luma");
        assert_eq!(rav.u, aom.u, "bd{depth}: decoders disagree on Cb");
        assert_eq!(rav.v, aom.v, "bd{depth}: decoders disagree on Cr");
        eprintln!("bd{depth}: rav1d-safe and zenav1-aom agree bit-exactly");
    }
}
