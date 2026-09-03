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
//!
//! A broken decode lands far below any of them: wrong content measures 6.4 dB
//! and a two-byte payload corruption fails to decode at all.
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

    // 16-bit input.
    let img16: ImgVec<Rgb<u16>> = Img::new(
        img.buf()
            .iter()
            .map(|p| Rgb {
                r: u16::from(p.r) << 8,
                g: u16::from(p.g) << 8,
                b: u16::from(p.b) << 8,
            })
            .collect::<Vec<_>>(),
        64,
        64,
    );
    let e = zenavif::encode_rgb16(img16.as_ref(), &aom_config(), stop())
        .expect_err("16-bit must be refused");
    let msg = format!("{e}");
    assert!(
        msg.contains("Zenav1Aom") && msg.contains("no 16-bit input"),
        "16-bit refusal must name the backend and the limitation, got: {msg}"
    );
    assert!(
        !msg.contains("zenav1-svt"),
        "an Av1Backend::Zenav1Aom refusal must not name the zenav1-svt feature, got: {msg}"
    );

    // 10-bit output.
    let e = zenavif::encode_rgb8(
        img.as_ref(),
        &aom_config().bit_depth(zenavif::EncodeBitDepth::Ten),
        stop(),
    )
    .expect_err("10-bit must be refused");
    assert!(
        format!("{e}").contains("8-bit only"),
        "10-bit refusal must name the limitation, got: {e}"
    );

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
    for (cfg, what) in [
        (EncoderConfig::new().backend(Av1Backend::Zenav1Aom), "4:4:4"),
        (
            aom_config().bit_depth(zenavif::EncodeBitDepth::Ten),
            "10-bit",
        ),
        (
            aom_config().pixel_range(zenavif::EncodePixelRange::Full),
            "full range",
        ),
    ] {
        cfg.validate()
            .expect_err(&format!("{what} must fail validate()"));
    }
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
