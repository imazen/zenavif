//! Permutation, idempotency, and forwarding-parity coverage for the
//! `__expert` feature's `InternalParams` knobs.
//!
//! Each non-default field must produce a different bitstream than
//! baseline; applying the same `InternalParams` twice must be
//! idempotent; setting all four fields must produce a valid encode;
//! `Default` must be byte-identical to no override; resetting via
//! `Default` must restore baseline; and zenavif's wrapper must
//! produce the same bytes as zenravif's `Encoder::with_internal_params`
//! directly.

#![cfg(feature = "__expert")]

use almost_enough::{StopToken, Unstoppable};
use imgref::{Img, ImgRef};
use rgb::Rgb;
use zenavif::{EncoderConfig, encode_rgb8, expert::InternalParams};

const W: u32 = 96;
const H: u32 = 96;

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

/// 96×96 synthetic RGB image: gradient + xor pattern.
///
/// The xor texture has frequencies that exercise both small partitions
/// (high-frequency speckle) and larger ones (smooth gradient).
fn make_image() -> Img<Vec<Rgb<u8>>> {
    let w = W as usize;
    let h = H as usize;
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let xor = ((x ^ y) & 0xff) as u8;
            let gx = ((x * 255) / (w - 1)) as u8;
            let gy = ((y * 255) / (h - 1)) as u8;
            pixels.push(Rgb {
                r: gx.wrapping_add(xor),
                g: gy.wrapping_add(xor.rotate_left(3)),
                b: xor.wrapping_add(gx.wrapping_mul(2)),
            });
        }
    }
    Img::new(pixels, w, h)
}

fn baseline_config() -> EncoderConfig {
    EncoderConfig::new().quality(60.0).speed(6).threads(Some(1))
}

fn encode_with_params(img: ImgRef<'_, Rgb<u8>>, params: InternalParams) -> Vec<u8> {
    let cfg = baseline_config().with_internal_params(params);
    encode_rgb8(img, &cfg, stop())
        .expect("encode should succeed")
        .avif_file
}

fn baseline_bytes(img: ImgRef<'_, Rgb<u8>>) -> Vec<u8> {
    let cfg = baseline_config();
    encode_rgb8(img, &cfg, stop())
        .expect("encode should succeed")
        .avif_file
}

// ---------------------------------------------------------------------------
// (a) Per-field tests: each field set to non-None changes the bitstream.
// ---------------------------------------------------------------------------

#[test]
fn partition_range_4_16_differs_from_baseline() {
    let img = make_image();
    let baseline = baseline_bytes(img.as_ref());
    let mut p = InternalParams::default();
    p.partition_range = Some((4, 16));
    let overridden = encode_with_params(img.as_ref(), p);
    assert_ne!(
        baseline, overridden,
        "partition_range Some((4, 16)) must change the bitstream"
    );
}

#[test]
fn partition_range_16_64_differs_from_baseline() {
    let img = make_image();
    let baseline = baseline_bytes(img.as_ref());
    let mut p = InternalParams::default();
    p.partition_range = Some((16, 64));
    let overridden = encode_with_params(img.as_ref(), p);
    assert_ne!(
        baseline, overridden,
        "partition_range Some((16, 64)) must change the bitstream"
    );
}

#[test]
fn partition_range_4_64_produces_valid_encode() {
    // Full range — exercises the upper-bound zenrav1e clamp at 64.
    // (128 is rejected by zenrav1e debug-asserts, so the maximum
    // valid value is 64.)
    let img = make_image();
    let mut p = InternalParams::default();
    p.partition_range = Some((4, 64));
    let bytes = encode_with_params(img.as_ref(), p);
    assert!(!bytes.is_empty());
    let decoded = zenavif::decode(&bytes).expect("decode should succeed");
    assert_eq!(decoded.width(), W);
    assert_eq!(decoded.height(), H);
}

#[test]
fn complex_prediction_modes_true_differs_from_false() {
    let img = make_image();
    let mut p_on = InternalParams::default();
    p_on.complex_prediction_modes = Some(true);
    let mut p_off = InternalParams::default();
    p_off.complex_prediction_modes = Some(false);
    let on = encode_with_params(img.as_ref(), p_on);
    let off = encode_with_params(img.as_ref(), p_off);
    assert_ne!(
        on, off,
        "complex_prediction_modes Some(true) vs Some(false) must differ"
    );
}

#[test]
fn lrf_true_differs_from_false() {
    let img = make_image();
    let mut p_on = InternalParams::default();
    p_on.lrf = Some(true);
    let mut p_off = InternalParams::default();
    p_off.lrf = Some(false);
    let on = encode_with_params(img.as_ref(), p_on);
    let off = encode_with_params(img.as_ref(), p_off);
    assert_ne!(on, off, "lrf Some(true) vs Some(false) must differ");
}

#[test]
fn fast_deblock_true_differs_from_false() {
    let img = make_image();
    let mut p_fast = InternalParams::default();
    p_fast.fast_deblock = Some(true);
    let mut p_full = InternalParams::default();
    p_full.fast_deblock = Some(false);
    let fast = encode_with_params(img.as_ref(), p_fast);
    let full = encode_with_params(img.as_ref(), p_full);
    assert_ne!(
        fast, full,
        "fast_deblock Some(true) vs Some(false) must differ"
    );
}

// ---------------------------------------------------------------------------
// (b) Idempotency: applying the same InternalParams twice == once.
// ---------------------------------------------------------------------------

#[test]
fn idempotency_partition_range() {
    let img = make_image();
    let mut p = InternalParams::default();
    p.partition_range = Some((4, 16));

    let once = encode_with_params(img.as_ref(), p.clone());

    // Apply twice through the builder.
    let cfg = baseline_config()
        .with_internal_params(p.clone())
        .with_internal_params(p);
    let twice = encode_rgb8(img.as_ref(), &cfg, stop())
        .expect("encode should succeed")
        .avif_file;

    assert_eq!(
        once, twice,
        "applying same InternalParams twice must equal applying it once"
    );
}

#[test]
fn idempotency_all_fields() {
    let img = make_image();
    let mut p = InternalParams::default();
    p.partition_range = Some((4, 64));
    p.complex_prediction_modes = Some(true);
    p.lrf = Some(true);
    p.fast_deblock = Some(false);

    let once = encode_with_params(img.as_ref(), p.clone());

    let cfg = baseline_config()
        .with_internal_params(p.clone())
        .with_internal_params(p);
    let twice = encode_rgb8(img.as_ref(), &cfg, stop())
        .expect("encode should succeed")
        .avif_file;

    assert_eq!(once, twice);
}

// ---------------------------------------------------------------------------
// (c) Combined: all four fields set produces a valid encode.
// ---------------------------------------------------------------------------

#[test]
fn combined_all_fields_set_produces_valid_encode() {
    let img = make_image();
    let mut p = InternalParams::default();
    p.partition_range = Some((4, 64));
    p.complex_prediction_modes = Some(true);
    p.lrf = Some(true);
    p.fast_deblock = Some(false);

    let bytes = encode_with_params(img.as_ref(), p);
    assert!(
        !bytes.is_empty(),
        "combined override must produce > 0 bytes"
    );

    let decoded = zenavif::decode(&bytes).expect("decode should succeed");
    assert_eq!(decoded.width(), W);
    assert_eq!(decoded.height(), H);
}

// ---------------------------------------------------------------------------
// (d) Default = baseline: bit-exact same bytes as no override.
// ---------------------------------------------------------------------------

#[test]
fn default_internal_params_is_byte_identical_to_baseline() {
    let img = make_image();
    let baseline = baseline_bytes(img.as_ref());
    let with_default = encode_with_params(img.as_ref(), InternalParams::default());
    assert_eq!(
        baseline, with_default,
        "InternalParams::default() must be bit-identical to no override"
    );
}

// ---------------------------------------------------------------------------
// (e) Reset: setting one field, then re-applying Default, restores baseline.
// ---------------------------------------------------------------------------

#[test]
fn reset_via_default_restores_baseline() {
    let img = make_image();
    let baseline = baseline_bytes(img.as_ref());

    let mut p = InternalParams::default();
    p.partition_range = Some((4, 16));
    p.lrf = Some(true);

    // Apply override, then reset by re-applying a fresh Default.
    let cfg = baseline_config()
        .with_internal_params(p)
        .with_internal_params(InternalParams::default());
    let after_reset = encode_rgb8(img.as_ref(), &cfg, stop())
        .expect("encode should succeed")
        .avif_file;

    assert_eq!(
        baseline, after_reset,
        "re-applying Default must reset all fields to baseline"
    );
}

// ---------------------------------------------------------------------------
// (f) Forwarding parity: zenavif's wrapper produces the same bytes as
//     ravif's `Encoder::with_internal_params` directly.
// ---------------------------------------------------------------------------

#[test]
fn forwarding_parity_with_ravif_direct() {
    use ravif::Encoder as RavifEncoder;

    let img = make_image();

    // Build params on both sides with the same field values.
    let mut zen_params = InternalParams::default();
    zen_params.partition_range = Some((4, 64));
    zen_params.complex_prediction_modes = Some(true);
    zen_params.lrf = Some(false);
    zen_params.fast_deblock = Some(true);

    let mut ravif_params = ravif::expert::InternalParams::default();
    ravif_params.partition_range = Some((4, 64));
    ravif_params.complex_prediction_modes = Some(true);
    ravif_params.lrf = Some(false);
    ravif_params.fast_deblock = Some(true);

    // Encode via zenavif's wrapper at quality=60 / speed=6 / 1 thread,
    // QM enabled (zenavif default), VAQ off, still-image off, lossless off.
    let zen_cfg = baseline_config().with_internal_params(zen_params);
    let zen_bytes = encode_rgb8(img.as_ref(), &zen_cfg, stop())
        .expect("zenavif encode should succeed")
        .avif_file;

    // Build the equivalent ravif encoder directly with the same knobs
    // that build_ravif_encoder() applies under encode-imazen + __expert.
    // Defaults match EncoderConfig::default(): qm=true, vaq=false,
    // tune_still=false, lossless=false; bit_depth=8 (auto on 8-bit input).
    let ravif_enc = RavifEncoder::new()
        .with_quality(60.0)
        .with_speed(6)
        .with_bit_depth(ravif::BitDepth::Eight)
        .with_internal_color_model(ravif::ColorModel::YCbCr)
        .with_alpha_color_mode(ravif::AlphaColorMode::UnassociatedClean)
        .with_num_threads(Some(1))
        .with_qm(true)
        .with_vaq(false, 1.0)
        .with_still_image_tuning(false)
        .with_lossless(false)
        .with_cdef(None)
        .with_rdo_tx_decision(None)
        .with_sgr_full(None)
        .with_lru_on_skip(None)
        .with_segmentation_complex(None)
        .with_encode_bottomup(None)
        .with_internal_params(ravif_params)
        .with_stop(stop());

    let ravif_bytes = ravif_enc
        .encode_rgb(img.as_ref())
        .expect("ravif direct encode should succeed")
        .avif_file;

    assert_eq!(
        zen_bytes, ravif_bytes,
        "zenavif wrapper must forward InternalParams faithfully to ravif"
    );
}
