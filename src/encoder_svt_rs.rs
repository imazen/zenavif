//! svtav1-rs AVIF encode backend (`encode-svt-rs` feature, EXPERIMENTAL).
//!
//! Routes [`crate::encoder::encode_rgb8`] through the pure-Rust SVT-AV1 port
//! ([imazen/svtav1](https://github.com/imazen/svtav1), `svtav1-rs/`) when
//! [`crate::Av1Backend::SvtRs`] is selected. Unlike the zenravif backend —
//! where zenravif itself muxes the AVIF container — this backend drives the
//! `svtav1_encoder::pipeline::EncodePipeline` directly and muxes in-crate via
//! `zenavif-serialize`.
//!
//! # Scope (v1, deliberately narrow)
//!
//! * Still images only: RGB/RGBA → 4:2:0 YCbCr (BT.601, full range),
//!   plus grayscale → monochrome (Cs400). RGBA's straight alpha plane is a
//!   separate Cs400 encode muxed as an `auxl` auxiliary item, honoring the
//!   [`crate::EncoderConfig::alpha_quality`] fallback contract.
//! * 8-bit and 10-bit (issue #33). 16-bit input (`encode_rgb16` /
//!   `encode_rgba16`) or [`crate::EncodeBitDepth::Ten`] codes a 10-bit
//!   profile-0 stream: RGB → YCbCr runs at 10-bit precision (the in-house
//!   f32 recipe quantized at the output depth, so an 8-bit source keeps
//!   its chroma-average fraction bits) and the u16 planes go through the
//!   port's native `try_encode_frame_420_hbd`. The low two bits reach the
//!   mode decision, the coded levels **and** the post-filter searches: as
//!   of upstream hbd chunk 2 (zenav1-svt `f319ec298`, on top of chunk 1
//!   `35743ebd5`) the deblock level search's SSE, the CDEF strength
//!   search's distortion and the Wiener tap search all read the caller's
//!   native u16 planes instead of the old `u8 << 2` widening — nothing on
//!   the bd10 path truncates the source any more, and a native source that
//!   went unconsumed is a typed refusal rather than a silent truncation
//!   (`pipeline.rs` "native 10-bit source went unconsumed"). Upstream gate:
//!   `tools/bd10_hbd_src_gate.sh`, 100/100 cells byte-identical to real C.
//!   10-bit **monochrome** (alpha, gray) has a native level
//!   producer at SVT preset ≥ 9 (speed ≥ 7) only, and an AVIF alpha item
//!   must match the colour item's depth, so 10-bit RGBA / gray encodes
//!   need speed ≥ 7 ([`svt_rs_depth_error`]). HDR static metadata (`clli`,
//!   `mdcv`) is written container-side by zenavif-serialize from
//!   [`crate::EncoderConfig::content_light_level`] /
//!   [`crate::EncoderConfig::mastering_display`]; BT.2020 / PQ / HLG CICP
//!   is signalled in both the sequence header and `colr`.
//! * Dimensions (issue #32): the 4:2:0 colour path takes **arbitrary**
//!   dimensions at **every** speed — upstream pads TRUE→ALIGNED internally,
//!   signals the TRUE dimensions in the sequence header, and codes partial
//!   superblocks byte-identically to C on the PD0 path (upstream
//!   `partial_sb_gate` 146/146 on aarch64 / 145/145 on x86-64 CI, incl. odd
//!   dims and a 23-cell presets-0–5 block added 2026-08-04). The preset ≥ 6
//!   floor this backend used to impose on the colour path is GONE; see
//!   [`svt_rs_dims_error`] for the measurement that retired it and for the
//!   one residual upstream still names (a `screen`-content RD class that is
//!   NOT dimension-conditioned). The alpha/gray **monochrome** streams keep
//!   a floor at SVT preset ≥ 6 (upstream `b6a1737a` + `1ed7db46` fixed the
//!   mono edge-leaf coding that mis-coded them at preset 6 — see CLAUDE.md
//!   Known Bugs; nothing measures mono partial SBs below it) and only at
//!   multiples of 8 (the mono path does no TRUE→ALIGNED padding yet); the
//!   alpha item must match the colour item's dimensions, so an RGBA encode
//!   inherits that rule. [`svt_rs_dims_error`] is the single gate both the
//!   encode path and [`crate::EncoderConfig::validate_for_input`] apply.
//! * No 12-bit, no RGB (identity) model, no limited range, no gain map, no
//!   animation. Each is rejected honestly at encode time (and by
//!   [`crate::EncoderConfig::validate`]). Coded-lossless (QP 0) is
//!   implemented upstream on the 8-bit 4:2:0 path, but this backend's
//!   quality mapping deliberately does not reach it — see
//!   [`quality_to_qp_gated`].
//!
//! # Payload shape
//!
//! `EncodePipeline::try_encode_frame_420` returns a temporal-delimiter +
//! sequence-header + frame OBU sequence. It is muxed **verbatim**: the
//! leading TD matches the zenravif payload convention (zenrav1e packet data
//! also begins with a TD OBU) and is byte-identical to the streams the
//! svtav1-rs decode-conformance suite validates under `aomdec` (525 mono +
//! 1575 4:2:0 cells, `tools/decode_conformance.sh` at the pinned rev).
//!
//! # Quality / speed mapping
//!
//! Deliberately svtav1-rs's own documented mappings, NOT zenravif's fitted
//! quality→quantizer curve (`src/encode_plan.rs` mirrors describe zenravif
//! only):
//!
//! * quality 1..=100 → QP 63..=0, linear
//!   ([`svtav1::avif::AvifEncoder::quality_to_qp_static`]), except QP is
//!   clamped to ≥ 1: QP 0 corrupts on the pinned rev (see
//!   [`quality_to_qp_gated`]).
//! * speed 1..=10 → SVT preset 0..=13, linear
//!   (same formula as `svtav1::avif::AvifEncoder`'s internal
//!   `speed_to_preset`; that helper is private upstream, so the formula is
//!   mirrored here with provenance).
//!
//! # C parity
//!
//! At the pinned tree, svtav1-rs emits **byte-identical bitstreams to the C
//! SVT-AV1 encoder (v4.2.0 baseline)** across its verified battery
//! (upstream `rust/README.md` gate table + `rust/STATUS.md`): the full-SB
//! identity matrix (`identity_full_8bit` 280/280, every preset 0–13, bd8
//! synthetic), bd10 (matrix 36/36 + non-flat 309/309 + native-source
//! 100/100), partial SBs and odd dims (`partial_sb_gate` 146/146 aarch64 /
//! 145/145 x86-64 CI, now including a presets-0–5 block), 10-bit at
//! non-64-aligned dims (159/159), SB128, and multi-tile (29/29).
//! Coded-lossless is no longer a refusal: `lossless_gate.sh` is 112/144
//! byte-identical (presets 4–13 all 96/96, incl. partial superblocks) with
//! 32 self-promoting pinned cells, and **144/144 decode to the source**.
//! Not byte-exact everywhere yet: screen-content low presets carry pinned
//! RD near-ties (upstream issue #71, which also fires on 64-aligned
//! frames), bd10 photo p4 is 13/15, and QP 0 stays typed-refused on
//! monochrome, 10-bit, HDR-fork, screen-content-tools, superres and inter
//! frames. The zenavif round-trip and cross-backend decode tests
//! (`tests/svt_rs_backend.rs`, `tests/cross_backend_decode.rs`) verify the
//! seam end-to-end.

use crate::Result;
use crate::encoder::{EncodeBitDepth, EncodeChromaSubsampling, EncodeColorModel, EncodePixelRange};
use crate::encoder::{EncodedImage, EncoderConfig};
use crate::error::Error;
use almost_enough::Stop;
use imgref::ImgRef;
use rgb::Rgb;
use whereat::at;

/// CICP defaults when the config sets none — same defaults the zenravif
/// backend uses (zenravif `av1encoder.rs`: BT.709 primaries + sRGB transfer).
const DEFAULT_COLOR_PRIMARIES: u8 = 1; // BT.709
const DEFAULT_TRANSFER_CHARACTERISTICS: u8 = 13; // sRGB
/// The matrix this backend always converts with and signals. Matches the
/// zenavif YCbCr convention (zenravif also derives BT.601 for YCbCr; the
/// `EncoderConfig::matrix_coefficients` CICP field is not consulted by any
/// available backend — see that method's docs).
const MATRIX_COEFFICIENTS_BT601: u8 = 6;

/// Map a fallible svtav1-rs pipeline failure onto the matching zenavif
/// [`Error`] variant so the failure category survives to
/// `CategorizedError::category()` (backend-seam obligation 1). This replaces
/// the old `is_empty()` heuristic on the infallible `encode_frame*` calls
/// (obligation 4: an out-of-envelope config now surfaces as a structured
/// refusal instead of a possibly-corrupt bitstream or a panic).
fn map_svt_encode_error(e: whereat::At<svtav1::types::EncodeError>) -> whereat::At<Error> {
    use svtav1::types::EncodeError as SvtError;
    let (err, _trace) = e.decompose();
    match err {
        SvtError::Cancelled(reason) => at!(Error::Cancelled(reason)),
        SvtError::AllocFailed { .. } => at!(Error::OutOfMemory),
        SvtError::InvalidDimensions {
            width,
            height,
            reason,
        } => at!(Error::Encode(format!(
            "svtav1-rs rejected dimensions {width}x{height}: {reason}"
        ))),
        SvtError::UnsupportedConfig(what) => at!(Error::Unsupported(what)),
        // `EncodeError` is #[non_exhaustive]; future variants degrade to the
        // generic encode bucket rather than failing the build.
        other => at!(Error::Encode(format!("svtav1-rs encode failed: {other}"))),
    }
}

/// Map zenavif quality 1..=100 to an svtav1-rs QP, clamped away from QP 0.
///
/// QP 0 is `base_qindex` 0 = **coded-lossless**, and its history upstream
/// runs corrupt → refused → implemented:
///
/// 1. rev 3e25f52b: syntactically-valid bitstreams decoding to garbage
///    (ssim2 ~= −700; `benchmarks/backend_sweep_2026-07-22.tsv`).
/// 2. `f0f0a70ca` (issue #5): a typed `EncodeError::UnsupportedConfig`.
/// 3. `aeb619cd8` + `75cf7b0f7` (issue #5 chunk 2) + `129d45494` (issue #9
///    items 6-7): coded-lossless ENCODES on the 8-bit 4:2:0 still path and
///    the capability refusal is RETIRED (upstream's inventory went 15 → 14
///    capability refusals). Still typed-refused there: monochrome, 10-bit,
///    HDR-fork mode, screen-content tools, superres, inter frames.
///
/// The clamp is therefore no longer a corruption guard, and it is no longer
/// working around a refusal — it is a **product** choice, kept deliberately:
///
/// * quality 100 must ENCODE, and mapping it to QP 0 would silently switch
///   coding modes (WHT/TX_4X4, no in-loop filters) and multiply file size,
///   which is not what a caller asking for "quality 100" of a lossy ladder
///   is asking for;
/// * this backend converts RGB → 4:2:0 YCbCr, so a coded-lossless AV1
///   frame is still not a lossless *image* round-trip — advertising it as
///   one through the quality dial would be a false claim;
/// * `EncoderConfig` has no lossless request for this backend to honour, so
///   there is no way for a caller to ask for it explicitly and no way to
///   distinguish "I want q100" from "I want lossless".
///
/// Composition is covered by `svt_rs_quality_100_does_not_corrupt` (clamp
/// side) and `svt_rs_direct_qp0_codes_lossless_420` +
/// `svt_rs_direct_qp0_typed_refusal_outside_420_8bit` (upstream-behaviour
/// side, driving the pipeline directly) in `tests/svt_rs_backend.rs`.
/// Removing the clamp is a deliberate product decision that needs a
/// lossless request on `EncoderConfig` to hang off, not a doc fix — tracked
/// as zenavif#42.
pub(crate) fn quality_to_qp_gated(quality: f32) -> u8 {
    svtav1::avif::AvifEncoder::quality_to_qp_static(quality).max(1)
}

/// Map speed 1..=10 to an SVT-AV1 preset 0..=9.
///
/// Provenance: mirrors the private `AvifEncoder::speed_to_preset` in
/// imazen/svtav1 `svtav1-rs/svtav1/src/avif.rs` (speed 1 → preset 0
/// slowest/best, speed 10 → preset 9 fastest; linear with rounding into
/// 0..=13, then clamped to M9).
///
/// # The M9 clamp (mirror repair, 2026-09-01)
///
/// This helper used to return the un-clamped `0..=13` value while
/// upstream's `speed_to_preset` clamps with `.min(9)`, because **C
/// remaps every all-intra preset above M9 down to M9**
/// (`enc_handle.c:4416-4419`) — a still encoded at "preset 13" IS an M9
/// encode. The drift was byte-neutral (upstream's
/// `speed_to_preset_boundaries` records presets 9, 10 and 13 as each
/// byte-identical to C's M9 output, hence to each other) but it made the
/// dial advertise a distinction the encoder does not have: zenavif
/// speeds 7, 8, 9 and 10 all encode identically. MEASURED 2026-09-01 on
/// the live AVIF subsample sweep (`zenmetrics
/// benchmarks/avif_sweep_permutation_retrofit_2026-09-01.md` §3): speeds
/// 7/8/9/10 produce identical encoded_bytes AND identical SSIMULACRA2 to
/// six decimals on 2 images × 4 quality points, while speed 6 (preset 7)
/// differs on every cell — the discriminating control.
///
/// Both preset floors this module gates on are unaffected: speeds 1..=6
/// map to 0/1/3/4/6/7 either way, and speeds 7..=10 clear
/// [`MONO_HBD_MIN_PRESET`] (9) both before and after the clamp.
pub(crate) fn speed_to_svt_preset(speed: u8) -> u8 {
    let clamped = speed.clamp(1, 10) as u32;
    ((((clamped - 1) * 13 + 4) / 9) as u8).min(9)
}

/// The RESOLVED encoder state an svt-rs cell actually encodes with:
/// `(preset, qp, alpha_qp)`.
///
/// This is the svt-rs half of the sweep planner's byte-identity
/// contract ([`crate::sweep::fingerprint`]). The planner's default
/// mediators are zenravif's (`quality_to_quantizer` + the
/// `speed_derived` search table) and the svt-rs backend reads NEITHER —
/// it resolves quality through [`quality_to_qp_gated`] and speed through
/// [`speed_to_svt_preset`]. Fingerprinting an svt-rs config with
/// zenravif's mediators would therefore both miss real aliases and risk
/// merging cells that differ, so the fingerprint routes here instead.
pub(crate) fn svt_resolved_identity(config: &crate::EncoderConfig) -> (u8, u8, u8) {
    (
        speed_to_svt_preset(config.speed_effective()),
        quality_to_qp_gated(config.quality),
        quality_to_qp_gated(crate::encoder::effective_alpha_quality(config)),
    )
}

/// Push [`crate::expert::SvtParams`] onto a colour `EncodePipeline`.
///
/// The seam's historical behaviour is `SvtParams::default()`, which is
/// SVT-AV1 v4.2.0's **mainline** default set (tune 1 = PSNR, QM off,
/// variance boost off, sharpness 0, `max_tx_size` 64, preset-derived
/// screen-content mode, no tiles) — so a caller that never touches
/// `with_svt_params` gets byte-identical output to before this function
/// existed. Gated by `svt_params_default_leaves_the_pipeline_at_mainline`.
///
/// Values are [`crate::expert::SvtParams::clamped`] first: the port guards
/// `variance_boost_strength` and `variance_octile` with `debug_assert` only
/// (`var_boost.rs` indexes a `[f64; 5]` and computes
/// `octile * SUBBLOCKS_IN_OCTILE - 1`), and every fleet worker is a release
/// build — an unclamped sweep value is a worker crash, not a measurement.
///
/// Chroma QM levels are set to the same window as luma, mirroring what the
/// port's own `apply_tune_overrides` does for tune IQ/MS-SSIM.
///
/// Not applied to the monochrome path ([`encode_mono_plane_svt`]): alpha and
/// grayscale items stay at the mainline defaults, which is also what libavif
/// does (it drives alpha with `tune=psnr`).
fn apply_svt_params(
    pipeline: &mut svtav1::encoder::pipeline::EncodePipeline,
    config: &crate::EncoderConfig,
) {
    let p = config.svt_params();
    pipeline.hdr.tune = p.tune;
    pipeline.hdr.enable_variance_boost = p.enable_variance_boost;
    pipeline.hdr.variance_boost_strength = p.variance_boost_strength;
    pipeline.hdr.variance_octile = p.variance_octile;
    pipeline.hdr.enable_qm = p.enable_qm;
    pipeline.hdr.min_qm_level = p.min_qm_level;
    pipeline.hdr.max_qm_level = p.max_qm_level;
    pipeline.hdr.min_chroma_qm_level = p.min_qm_level;
    pipeline.hdr.max_chroma_qm_level = p.max_qm_level;
    pipeline.hdr.sharpness = p.sharpness;
    pipeline.hdr.screen_content_mode = p.force_screen_content_mode;
    pipeline.hdr.ac_bias = p.ac_bias;
    pipeline.hdr.max_tx_size = p.max_tx_size;
    pipeline.tile_cols_log2 = p.tile_cols_log2;
    pipeline.tile_rows_log2 = p.tile_rows_log2;
}

/// Lowest SVT preset at which the **monochrome** (Cs400) path is verified
/// to code a partial superblock correctly — the alpha auxiliary item and
/// grayscale colour items.
///
/// This used to be `PARTIAL_SB_MIN_PRESET`, a floor the 4:2:0 colour path
/// shared. The colour half of that floor is GONE (see
/// [`svt_rs_dims_error`]); the mono half stays because nothing upstream
/// measures mono partial superblocks below preset 6: `partial_sb_gate` is
/// bd8 **4:2:0** by its own scope line, and the mono partial-SB evidence is
/// the preset-6 edge-leaf fix (zenav1-svt `b6a1737a` + `1ed7db46`) plus this
/// crate's `svt_rs_direct_mono_partial_sb_preset6_roundtrips`. A floor with
/// no measurement under it stays where the measurement stops.
pub(crate) const MONO_PARTIAL_SB_MIN_PRESET: u8 = 6;

/// The dimension envelope this backend accepts, as one predicate shared by
/// the encode path and [`crate::EncoderConfig::validate_for_input`] (issue
/// #32). Returns the reason a `width`x`height` image is refused at zenavif
/// `speed`, or `None` when it encodes.
///
/// * Any speed: multiples of 64 always encode.
/// * The 4:2:0 **colour** path codes arbitrary dimensions at **every**
///   speed (upstream pads TRUE→ALIGNED and signals the true size).
/// * `mono_plane` streams — the Cs400 alpha auxiliary item and grayscale
///   colour items — need SVT preset ≥ [`MONO_PARTIAL_SB_MIN_PRESET`]
///   (speed ≥ 5) AND multiples of 8, because the port's monochrome path
///   does no TRUE→ALIGNED padding (`try_encode_frame` rejects
///   `aligned != true`) and its partial-SB edge coding is measured only at
///   preset ≥ 6 (zenav1-svt `b6a1737a` + `1ed7db46`; the round-trip gate
///   `svt_rs_direct_mono_partial_sb_preset6_roundtrips` in
///   `tests/svt_rs_backend.rs` keeps that fixed). Below that preset a mono
///   stream is 64-multiples only.
///
/// # Why the colour preset floor was removed (2026-08-29)
///
/// It rested on a premise that upstream measurement has since retired.
/// Until 2026-08-04 upstream gated its C-faithful PD1 refinement walk on a
/// COMPLETE superblock (`refined = matches!(preset, 0..=5) && use_funnel &&
/// full_sb`), so a partial SB at presets 0–5 fell back to a plain PD0 fixed
/// tree — a search C never runs. That `full_sb` gate is gone: the walk is
/// edge-aware, and `tools/partial_sb_gate.sh` grew a 23-cell presets-0–5
/// block, every cell byte-identical to real SvtAv1EncApp v4.2.0 (gate total
/// 146/146 on aarch64, 145/145 on the x86-64 CI runner — the one-cell
/// difference is an ISA-scoped C-side divergence, upstream
/// `SUSPECTED-C-BUGS.md` #9, not a port variable). Anti-vacuity was
/// re-measured adversarially: restoring `&& full_sb` drops the gate to
/// 118/141 with all 23 failures inside that block.
///
/// The residual upstream names is **not** dimension-conditioned, which is
/// what makes a *dimension* gate the wrong tool for it. Measured over 36
/// cells per preset (9 non-64-aligned geometries × {gradient, screen} ×
/// {q20, q48}): every `gradient` cell byte-matches at p0..p3 and p5; the
/// misses are `screen` content at p0/p1/p2 (+4 cells at p4) — upstream
/// issue #71, the palette/IntraBC over-picking RD class, which fires on
/// **64-ALIGNED** 256/384/512 screen frames too. Those aligned frames this
/// gate has always accepted at every speed, so the 64-multiple rule never
/// protected anyone from that class; it only refused correct encodes.
///
/// And the residual is an RD divergence — different partition choices,
/// different bytes — not corruption: upstream
/// `tools/arbitrary_size_robustness.sh` is 128/128 panic-free-and-decodable
/// with **0 refused** across every preset. What this crate owes its callers
/// is correct pixels, not byte-identity to C, and the positive gate
/// `svt_rs_partial_sb_roundtrip_at_low_presets` pins exactly that.
pub(crate) fn svt_rs_dims_error(
    width: usize,
    height: usize,
    speed: u8,
    mono_plane: bool,
) -> Option<&'static str> {
    if width == 0 || height == 0 {
        return Some("cannot encode an empty image");
    }
    if width.is_multiple_of(64) && height.is_multiple_of(64) {
        return None;
    }
    let preset = speed_to_svt_preset(speed);
    if mono_plane && preset < MONO_PARTIAL_SB_MIN_PRESET {
        return Some(
            "Av1Backend::SvtRs codes alpha and grayscale (Cs400) dimensions that are not \
             multiples of 64 only at SVT preset >= 6 (speed >= 5): the svtav1-rs monochrome \
             partial-superblock edge coding is measured only from that preset. Use speed >= 5, \
             pad/crop to multiples of 64, use RGB input, or use the zenravif backend",
        );
    }
    if mono_plane && (!width.is_multiple_of(8) || !height.is_multiple_of(8)) {
        return Some(
            "Av1Backend::SvtRs alpha and grayscale (Cs400) streams need dimensions \
             that are multiples of 8 (the svtav1-rs monochrome path pads no partial \
             8x8 block yet), and the alpha item must match the colour item's size. \
             Use RGB input, pad/crop to multiples of 8, or use the zenravif backend",
        );
    }
    None
}

/// Reject configuration the svtav1-rs backend cannot honor.
///
/// Encode entry points clamp/reject independently of the opt-in
/// [`crate::EncoderConfig::validate`], so these checks run on the encode
/// path too — a config asking for something this backend cannot produce
/// must never be served silently different output.
fn reject_unsupported_config(config: &EncoderConfig) -> Result<()> {
    if config.chroma_subsampling != EncodeChromaSubsampling::Yuv420 {
        return Err(at!(Error::Unsupported(
            "Av1Backend::SvtRs encodes 4:2:0 only: set \
             .chroma_subsampling(EncodeChromaSubsampling::Yuv420) \
             (the 4:4:4 default is zenravif-only for now)"
        )));
    }
    if config.color_model != EncodeColorModel::YCbCr {
        return Err(at!(Error::Unsupported(
            "Av1Backend::SvtRs supports the YCbCr color model only \
             (identity/RGB has no defined 4:2:0 subsampling)"
        )));
    }
    if config.pixel_range == Some(EncodePixelRange::Limited) {
        return Err(at!(Error::Unsupported(
            "Av1Backend::SvtRs signals full pixel range only \
             (the svtav1-rs sequence header pins color_range=1)"
        )));
    }
    if config.gain_map.is_some() {
        return Err(at!(Error::Unsupported(
            "Av1Backend::SvtRs does not support gain maps yet \
             (use the zenravif backend)"
        )));
    }
    #[cfg(feature = "encode-imazen")]
    if config.lossless {
        return Err(at!(Error::Unsupported(
            "Av1Backend::SvtRs has no lossless mode (QP 0 is not mathematically \
             lossless); use the zenravif backend for lossless"
        )));
    }
    Ok(())
}

/// Map a raw CICP color-primaries code point to the muxer's enum.
///
/// Same mapping shape as zenravif's `map_color_primaries`; unmapped code
/// points degrade to `Unspecified` (readers fall back to the AVIF defaults).
fn cicp_to_serialize_primaries(cp: u8) -> zenavif_serialize::constants::ColorPrimaries {
    use zenavif_serialize::constants::ColorPrimaries as CP;
    match cp {
        1 => CP::Bt709,
        6 => CP::Bt601,
        9 => CP::Bt2020,
        11 => CP::DciP3,
        12 => CP::DisplayP3,
        _ => CP::Unspecified,
    }
}

/// Map a raw CICP transfer-characteristics code point to the muxer's enum.
fn cicp_to_serialize_transfer(tc: u8) -> zenavif_serialize::constants::TransferCharacteristics {
    use zenavif_serialize::constants::TransferCharacteristics as TC;
    match tc {
        1 => TC::Bt709,
        6 => TC::Bt601,
        8 => TC::Linear,
        13 => TC::Srgb,
        14 => TC::Bt2020_10,
        16 => TC::Smpte2084,
        18 => TC::Hlg,
        _ => TC::Unspecified,
    }
}

/// Reject dimensions outside this backend's envelope — the encode-time
/// twin of the `validate_for_input` check, both driven by
/// [`svt_rs_dims_error`]. `mono_plane` is true when the encode emits a
/// Cs400 stream (alpha auxiliary item or grayscale colour item).
fn reject_out_of_envelope_dims(
    width: usize,
    height: usize,
    config: &EncoderConfig,
    mono_plane: bool,
) -> Result<()> {
    match svt_rs_dims_error(width, height, config.speed, mono_plane) {
        None => Ok(()),
        Some(reason) => Err(at!(Error::Encode(format!(
            "{reason} (got {width}x{height} at speed {} = SVT preset {})",
            config.speed,
            speed_to_svt_preset(config.speed)
        )))),
    }
}

/// Bit depth this backend codes for a request — the same
/// [`EncodeBitDepth`] resolution the zenravif path applies
/// (`encoder::resolve_bit_depth`), minus its ravif type.
fn effective_bit_depth(config: &EncoderConfig, input_is_16bit: bool) -> u8 {
    match config.bit_depth {
        EncodeBitDepth::Eight => 8,
        EncodeBitDepth::Ten => 10,
        EncodeBitDepth::Auto => {
            if input_is_16bit {
                10
            } else {
                8
            }
        }
    }
}

/// Lowest SVT preset with a native-10-bit **monochrome** level producer.
/// The port's `bd10_levels_native` (pipeline.rs) approves mono only where
/// the level re-encode post-pass runs — the eff-M9 band — because the
/// full-RD bd10 funnel requires 4:2:0; `try_encode_frame_hbd` refuses
/// anything else rather than emit 8-bit-quantized levels under a 10-bit
/// sequence header. Colour (4:2:0) has a bd10 producer at every preset.
pub(crate) const MONO_HBD_MIN_PRESET: u8 = 9;

/// The bit-depth envelope this backend accepts, shared by the encode path
/// and [`crate::EncoderConfig::validate_for_input`] (issue #33). Returns
/// the reason a `bit_depth`-bit encode at zenavif `speed` is refused, or
/// `None` when it encodes. `mono_plane` is true when a Cs400 stream is
/// emitted (alpha auxiliary item, grayscale colour item).
pub(crate) fn svt_rs_depth_error(
    bit_depth: u8,
    speed: u8,
    mono_plane: bool,
) -> Option<&'static str> {
    if bit_depth == 10 && mono_plane && speed_to_svt_preset(speed) < MONO_HBD_MIN_PRESET {
        return Some(
            "Av1Backend::SvtRs codes 10-bit alpha and grayscale (Cs400) streams at SVT \
             preset >= 9 (speed >= 7) only: the svtav1-rs bd10 monochrome level pass runs \
             there and nowhere else, and an AVIF alpha item must match the colour item's \
             depth. Use speed >= 7, RGB input, 8-bit, or the zenravif backend",
        );
    }
    None
}

/// Encode-time twin of the `validate_for_input` depth check, both driven
/// by [`svt_rs_depth_error`].
fn reject_out_of_envelope_depth(
    bit_depth: u8,
    config: &EncoderConfig,
    mono_plane: bool,
) -> Result<()> {
    match svt_rs_depth_error(bit_depth, config.speed, mono_plane) {
        None => Ok(()),
        Some(reason) => Err(at!(Error::Unsupported(reason))),
    }
}

/// One monochrome plane at the depth the stream is coded at.
enum MonoPlane<'a> {
    Eight(&'a [u8]),
    Ten(&'a [u16]),
}

/// Run one still-frame monochrome encode through the svtav1-rs pipeline.
///
/// `plane` is `stride`-strided (`stride >= width`), `width`/`height` already
/// inside the mono envelope of [`svt_rs_dims_error`] and the depth inside
/// [`svt_rs_depth_error`]. Returns the TD + sequence header + frame OBU
/// payload. Used for grayscale color items and alpha auxiliary items (both
/// are Cs400 streams).
#[expect(clippy::too_many_arguments, reason = "internal plane-encode helper")]
fn encode_mono_plane_svt(
    plane: MonoPlane<'_>,
    width: usize,
    height: usize,
    stride: usize,
    preset: u8,
    qp: u8,
    threads: usize,
    color_description: svtav1::entropy::obu::ColorDescription,
    stop: &almost_enough::StopToken,
) -> Result<Vec<u8>> {
    let w = u32::try_from(width).map_err(|_| at!(Error::Encode("width exceeds u32".into())))?;
    let h = u32::try_from(height).map_err(|_| at!(Error::Encode("height exceeds u32".into())))?;
    let rc = svtav1::encoder::rate_control::RcConfig {
        mode: svtav1::encoder::rate_control::RcMode::Cqp,
        qp,
        ..svtav1::encoder::rate_control::RcConfig::default()
    };
    let mut pipeline = svtav1::encoder::pipeline::EncodePipeline::new(w, h, preset, rc, 0, 1);
    pipeline.bit_depth = match plane {
        MonoPlane::Eight(_) => 8,
        MonoPlane::Ten(_) => 10,
    };
    pipeline.color_description = color_description;
    // Cooperative cancellation inside the pipeline (SB-cadence polling) —
    // backend-seam obligation 3: a capability the backend accepts must be
    // threaded through in the same change.
    pipeline.stop = stop.clone();
    // Bounded tile-parallel threading (byte-inert — tiles reassemble in
    // order; inert on today's single-tile frames but wired so a future
    // tile knob inherits the caller's thread budget). 0 = auto.
    pipeline.thread_count = threads;

    let payload = match plane {
        // The u8 pipeline reads a tight `stride`-strided plane; make it
        // tight when the caller's buffer is padded.
        MonoPlane::Eight(plane) => {
            if stride == width {
                pipeline.try_encode_frame(plane, width)
            } else {
                let mut tight = Vec::with_capacity(width * height);
                for row in plane.chunks(stride).take(height) {
                    tight.extend_from_slice(&row[..width]);
                }
                pipeline.try_encode_frame(&tight, width)
            }
        }
        // The hbd entry point takes the stride itself.
        MonoPlane::Ten(plane) => pipeline.try_encode_frame_hbd(plane, stride),
    }
    .map_err(map_svt_encode_error)?;
    Ok(payload)
}

/// Tight 4:2:0 planes at the depth the colour stream is coded at.
enum Yuv420Planes {
    Eight {
        y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
    },
    Ten {
        y: Vec<u16>,
        u: Vec<u16>,
        v: Vec<u16>,
    },
}

impl Yuv420Planes {
    fn bit_depth(&self) -> u8 {
        match self {
            Yuv420Planes::Eight { .. } => 8,
            Yuv420Planes::Ten { .. } => 10,
        }
    }

    /// RGB(A) of any source depth -> 4:2:0 at `bit_depth` (8 or 10)
    /// through the depth-generic f32 recipe (BT.601 full range — the
    /// matrix/range this backend signals). At 8 bits from an 8-bit
    /// source the entry points use the dedicated `rgb8_to_yuv420` /
    /// `rgba8_to_yuv420` kernels instead (byte-identical output to the
    /// 8-bit-only seam); this path serves 10-bit output and 16-bit input.
    fn convert<P: crate::yuv_convert::ForwardPixel>(
        rgb: &[P],
        stride: usize,
        width: usize,
        height: usize,
        bit_depth: u8,
    ) -> Self {
        let cw = width.div_ceil(2);
        let ch = height.div_ceil(2);
        let mut y = vec![0u16; width * height];
        let mut u = vec![0u16; cw * ch];
        let mut v = vec![0u16; cw * ch];
        crate::yuv_convert::rgbx_to_yuv420_u16(
            rgb,
            stride,
            width,
            height,
            bit_depth,
            crate::yuv_convert::YuvRange::Full,
            crate::yuv_convert::YuvMatrix::Bt601,
            &mut y,
            &mut u,
            &mut v,
        );
        if bit_depth == 8 {
            // Quantized at 8 bits by the kernel; narrow the container.
            let narrow = |p: Vec<u16>| p.into_iter().map(|s| s as u8).collect();
            Yuv420Planes::Eight {
                y: narrow(y),
                u: narrow(u),
                v: narrow(v),
            }
        } else {
            Yuv420Planes::Ten { y, u, v }
        }
    }
}

/// Run one still-frame 4:2:0 colour encode through the svtav1-rs pipeline
/// at the planes' depth. Returns the TD + sequence header + frame OBU
/// payload.
fn encode_color_420_svt(
    planes: &Yuv420Planes,
    width: usize,
    height: usize,
    config: &EncoderConfig,
    color_primaries: u8,
    transfer_characteristics: u8,
    stop: &almost_enough::StopToken,
) -> Result<Vec<u8>> {
    let w = u32::try_from(width).map_err(|_| at!(Error::Encode("width exceeds u32".into())))?;
    let h = u32::try_from(height).map_err(|_| at!(Error::Encode("height exceeds u32".into())))?;
    let qp = quality_to_qp_gated(config.quality);
    let preset = speed_to_svt_preset(config.speed);
    let rc = svtav1::encoder::rate_control::RcConfig {
        mode: svtav1::encoder::rate_control::RcMode::Cqp,
        qp,
        ..svtav1::encoder::rate_control::RcConfig::default()
    };
    // hierarchical_levels 0 + intra_period 1: single still key frame with a
    // reduced still-picture sequence header (the AvifEncoder pattern).
    let mut pipeline = svtav1::encoder::pipeline::EncodePipeline::new(w, h, preset, rc, 0, 1)
        .with_chroma_420(true);
    apply_svt_params(&mut pipeline, config);
    pipeline.bit_depth = planes.bit_depth();
    pipeline.color_description = svtav1::entropy::obu::ColorDescription {
        color_primaries,
        transfer_characteristics,
        matrix_coefficients: MATRIX_COEFFICIENTS_BT601,
        // Note: the svtav1-rs sequence-header writer pins color_range=1
        // (full) regardless of this flag; kept coherent anyway.
        full_range: true,
    };
    pipeline.stop = stop.clone();
    // Caller's thread budget (see encode_mono_plane_svt for semantics).
    pipeline.thread_count = config.threads.unwrap_or(0);

    // TD + sequence header + frame OBUs, muxed verbatim (module docs).
    match planes {
        Yuv420Planes::Eight { y, u, v } => pipeline.try_encode_frame_420(y, u, v, width),
        Yuv420Planes::Ten { y, u, v } => pipeline.try_encode_frame_420_hbd(y, u, v, width),
    }
    .map_err(map_svt_encode_error)
}

/// Build the AVIF muxer with the config's container-level metadata applied
/// (EXIF/XMP/ICC, rotation/mirror, HDR metadata, CICP).
fn build_aviffy(
    config: &EncoderConfig,
    color_primaries: u8,
    transfer_characteristics: u8,
    matrix_coefficients: zenavif_serialize::constants::MatrixCoefficients,
    monochrome: bool,
) -> zenavif_serialize::Aviffy {
    let mut aviffy = zenavif_serialize::Aviffy::new();
    aviffy
        .set_seq_profile(0)
        .set_chroma_subsampling((true, true))
        .set_monochrome(monochrome)
        .set_full_color_range(true)
        .set_color_primaries(cicp_to_serialize_primaries(color_primaries))
        .set_transfer_characteristics(cicp_to_serialize_transfer(transfer_characteristics))
        .set_matrix_coefficients(matrix_coefficients);
    if let Some(ref exif) = config.exif {
        aviffy.set_exif(exif.clone());
    }
    if let Some(ref xmp) = config.xmp {
        aviffy.set_xmp(xmp.clone());
    }
    if let Some(ref icc) = config.icc_profile {
        aviffy.set_icc_profile(icc.clone());
    }
    if let Some(angle) = config.rotation {
        aviffy.set_rotation(angle);
    }
    if let Some(axis) = config.mirror {
        aviffy.set_mirror(axis);
    }
    if let Some((max_cll, max_fall)) = config.content_light_level {
        aviffy.set_content_light_level(max_cll, max_fall);
    }
    if let Some(md) = config.mastering_display {
        aviffy.set_mastering_display(
            md.primaries,
            md.white_point,
            md.max_luminance,
            md.min_luminance,
        );
    }
    aviffy
}

/// Mux a colour payload (and optional alpha payload) into an AVIF file
/// with the config's container-level metadata.
#[expect(clippy::too_many_arguments, reason = "internal mux helper")]
fn mux_svt(
    config: &EncoderConfig,
    color_payload: Vec<u8>,
    alpha_payload: Option<Vec<u8>>,
    width: usize,
    height: usize,
    bit_depth: u8,
    color_primaries: u8,
    transfer_characteristics: u8,
    monochrome: bool,
) -> Result<EncodedImage> {
    let w = u32::try_from(width).map_err(|_| at!(Error::Encode("width exceeds u32".into())))?;
    let h = u32::try_from(height).map_err(|_| at!(Error::Encode("height exceeds u32".into())))?;
    // The av1C written here must match the payload's sequence header
    // (Chrome cross-validates): profile 0, 8- or 10-bit, 4:2:0 (or mono),
    // full range.
    let aviffy = build_aviffy(
        config,
        color_primaries,
        transfer_characteristics,
        if monochrome {
            zenavif_serialize::constants::MatrixCoefficients::Unspecified
        } else {
            zenavif_serialize::constants::MatrixCoefficients::Bt601
        },
        monochrome,
    );
    let avif_file = aviffy
        .try_to_vec(&color_payload, alpha_payload.as_deref(), w, h, bit_depth)
        .map_err(|e| at!(Error::Encode(format!("AVIF serialization failed: {e}"))))?;
    Ok(EncodedImage {
        color_byte_size: color_payload.len(),
        alpha_byte_size: alpha_payload.map_or(0, |a| a.len()),
        avif_file,
    })
}

/// Exact 8 -> 10 bit sample scaling (`round(v * 1023 / 255)`) for alpha
/// and gray planes widened to a 10-bit Cs400 stream.
#[inline]
fn widen_8_to_10(v: u8) -> u16 {
    ((u32::from(v) * 1023 + 127) / 255) as u16
}

/// Encode an 8-bit RGB image to AVIF via the svtav1-rs backend.
///
/// See the module docs for scope and constraints. Cancellation is checked
/// at the seam's phase boundaries (pre-conversion, pre-encode, pre-mux)
/// AND inside the pipeline itself: the token handed to `pipeline.stop` is
/// polled at superblock cadence by the encode loops at the pinned rev.
pub(crate) fn encode_rgb8_svt_rs(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;

    let width = img.width();
    let height = img.height();
    reject_out_of_envelope_dims(width, height, config, false)?;
    let bit_depth = effective_bit_depth(config, false);
    reject_out_of_envelope_depth(bit_depth, config, false)?;

    // ---- RGB -> YUV 4:2:0, BT.601 full range ----------------------------
    // Full range matches what the svtav1-rs sequence header signals
    // (color_range is pinned to 1) and zenravif's full-range default;
    // BT.601 matches the zenavif YCbCr convention. The in-house forward
    // kernel is the exact inverse of the decode recipe (per-pixel f32
    // chroma, box-averaged before quantization).
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let planes = if bit_depth == 8 {
        let cw = width.div_ceil(2);
        let ch = height.div_ceil(2);
        let mut y = vec![0u8; width * height];
        let mut u = vec![0u8; cw * ch];
        let mut v = vec![0u8; cw * ch];
        crate::yuv_convert::rgb8_to_yuv420(
            img.buf(),
            img.stride(),
            width,
            height,
            crate::yuv_convert::YuvRange::Full,
            crate::yuv_convert::YuvMatrix::Bt601,
            &mut y,
            &mut u,
            &mut v,
        );
        Yuv420Planes::Eight { y, u, v }
    } else {
        Yuv420Planes::convert(img.buf(), img.stride(), width, height, bit_depth)
    };

    // ---- svtav1-rs still-frame encode -----------------------------------
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let color_primaries = config.color_primaries.unwrap_or(DEFAULT_COLOR_PRIMARIES);
    let transfer_characteristics = config
        .transfer_characteristics
        .unwrap_or(DEFAULT_TRANSFER_CHARACTERISTICS);
    let av1_payload = encode_color_420_svt(
        &planes,
        width,
        height,
        config,
        color_primaries,
        transfer_characteristics,
        &stop,
    )?;

    // ---- AVIF container --------------------------------------------------
    stop.check().map_err(|e| at!(Error::from(e)))?;
    mux_svt(
        config,
        av1_payload,
        None,
        width,
        height,
        bit_depth,
        color_primaries,
        transfer_characteristics,
        false,
    )
}

/// CICP "unspecified" code point — what the alpha auxiliary stream signals
/// (an alpha plane has no colorimetry; readers ignore its CICP per MIAF).
const CICP_UNSPECIFIED: u8 = 2;

/// Colour description for a Cs400 alpha stream (no colorimetry).
fn alpha_color_description() -> svtav1::entropy::obu::ColorDescription {
    svtav1::entropy::obu::ColorDescription {
        color_primaries: CICP_UNSPECIFIED,
        transfer_characteristics: CICP_UNSPECIFIED,
        matrix_coefficients: CICP_UNSPECIFIED,
        full_range: true,
    }
}

/// Encode a colour 4:2:0 item plus a Cs400 alpha item and mux both — the
/// shared tail of the RGBA entry points. `alpha` is a tight plane at
/// `bit_depth` (8 or 10).
fn encode_rgba_planes_svt(
    planes: &Yuv420Planes,
    alpha: MonoPlane<'_>,
    width: usize,
    height: usize,
    config: &EncoderConfig,
    stop: &almost_enough::StopToken,
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let alpha_qp = quality_to_qp_gated(crate::encoder::effective_alpha_quality(config));
    let preset = speed_to_svt_preset(config.speed);
    let color_primaries = config.color_primaries.unwrap_or(DEFAULT_COLOR_PRIMARIES);
    let transfer_characteristics = config
        .transfer_characteristics
        .unwrap_or(DEFAULT_TRANSFER_CHARACTERISTICS);
    let color_payload = encode_color_420_svt(
        planes,
        width,
        height,
        config,
        color_primaries,
        transfer_characteristics,
        stop,
    )?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let alpha_payload = encode_mono_plane_svt(
        alpha,
        width,
        height,
        width,
        preset,
        alpha_qp,
        config.threads.unwrap_or(0),
        alpha_color_description(),
        stop,
    )?;

    // ---- AVIF container (color item + auxl alpha item) -------------------
    stop.check().map_err(|e| at!(Error::from(e)))?;
    mux_svt(
        config,
        color_payload,
        Some(alpha_payload),
        width,
        height,
        planes.bit_depth(),
        color_primaries,
        transfer_characteristics,
        false,
    )
}

/// Encode an 8-bit RGBA image to AVIF via the svtav1-rs backend.
///
/// Color travels exactly like [`encode_rgb8_svt_rs`] (4:2:0 BT.601 full
/// range); the straight (non-premultiplied) alpha plane is encoded as a
/// separate monochrome (Cs400) still and muxed as an `auxl` auxiliary item.
/// Alpha quality follows the [`crate::EncoderConfig::alpha_quality`]
/// contract (falls back to the color quality).
pub(crate) fn encode_rgba8_svt_rs(
    img: ImgRef<'_, rgb::Rgba<u8>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;

    let width = img.width();
    let height = img.height();
    // The alpha plane is a Cs400 stream: the stricter mono envelope applies.
    reject_out_of_envelope_dims(width, height, config, true)?;
    let bit_depth = effective_bit_depth(config, false);
    reject_out_of_envelope_depth(bit_depth, config, true)?;

    // ---- RGBA -> YUV 4:2:0 color + tight alpha plane --------------------
    // Same forward kernel as the RGB path (alpha ignored here — it rides
    // as its own Cs400 stream below), so RGB and RGBA encodes of the same
    // pixels produce byte-identical color payloads by construction.
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let planes = if bit_depth == 8 {
        let cw = width.div_ceil(2);
        let ch = height.div_ceil(2);
        let mut y = vec![0u8; width * height];
        let mut u = vec![0u8; cw * ch];
        let mut v = vec![0u8; cw * ch];
        crate::yuv_convert::rgba8_to_yuv420(
            img.buf(),
            img.stride(),
            width,
            height,
            crate::yuv_convert::YuvRange::Full,
            crate::yuv_convert::YuvMatrix::Bt601,
            &mut y,
            &mut u,
            &mut v,
        );
        Yuv420Planes::Eight { y, u, v }
    } else {
        Yuv420Planes::convert(img.buf(), img.stride(), width, height, bit_depth)
    };
    if bit_depth == 8 {
        let mut alpha = Vec::with_capacity(width * height);
        for row in img.rows() {
            alpha.extend(row.iter().map(|px| px.a));
        }
        encode_rgba_planes_svt(
            &planes,
            MonoPlane::Eight(&alpha),
            width,
            height,
            config,
            &stop,
        )
    } else {
        let mut alpha = Vec::with_capacity(width * height);
        for row in img.rows() {
            alpha.extend(row.iter().map(|px| widen_8_to_10(px.a)));
        }
        encode_rgba_planes_svt(
            &planes,
            MonoPlane::Ten(&alpha),
            width,
            height,
            config,
            &stop,
        )
    }
}

/// Encode a 16-bit RGB image to a 10-bit (profile 0, 4:2:0) AVIF via the
/// svtav1-rs backend (issue #33).
///
/// Input values are full u16 range (0–65535) in the image's own transfer
/// function; the RGB → YCbCr conversion runs at 10-bit precision from the
/// 16-bit source and the u16 planes are handed to the port's native
/// `try_encode_frame_420_hbd`. [`crate::EncodeBitDepth::Eight`] codes an
/// 8-bit stream from the same conversion.
pub(crate) fn encode_rgb16_svt_rs(
    img: ImgRef<'_, Rgb<u16>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;

    let width = img.width();
    let height = img.height();
    reject_out_of_envelope_dims(width, height, config, false)?;
    let bit_depth = effective_bit_depth(config, true);
    reject_out_of_envelope_depth(bit_depth, config, false)?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let planes = Yuv420Planes::convert(img.buf(), img.stride(), width, height, bit_depth);

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let color_primaries = config.color_primaries.unwrap_or(DEFAULT_COLOR_PRIMARIES);
    let transfer_characteristics = config
        .transfer_characteristics
        .unwrap_or(DEFAULT_TRANSFER_CHARACTERISTICS);
    let av1_payload = encode_color_420_svt(
        &planes,
        width,
        height,
        config,
        color_primaries,
        transfer_characteristics,
        &stop,
    )?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    mux_svt(
        config,
        av1_payload,
        None,
        width,
        height,
        bit_depth,
        color_primaries,
        transfer_characteristics,
        false,
    )
}

/// Encode a 16-bit RGBA image to a 10-bit AVIF via the svtav1-rs backend
/// (issue #33): colour as [`encode_rgb16_svt_rs`], the alpha plane scaled
/// to 10 bits (`scale_from_u16`) as a Cs400 `auxl` item — which needs
/// speed ≥ 7 (see [`svt_rs_depth_error`]).
pub(crate) fn encode_rgba16_svt_rs(
    img: ImgRef<'_, rgb::Rgba<u16>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;

    let width = img.width();
    let height = img.height();
    reject_out_of_envelope_dims(width, height, config, true)?;
    let bit_depth = effective_bit_depth(config, true);
    reject_out_of_envelope_depth(bit_depth, config, true)?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let planes = Yuv420Planes::convert(img.buf(), img.stride(), width, height, bit_depth);
    if bit_depth == 8 {
        let mut alpha = Vec::with_capacity(width * height);
        for row in img.rows() {
            alpha.extend(row.iter().map(|px| (px.a >> 8) as u8));
        }
        encode_rgba_planes_svt(
            &planes,
            MonoPlane::Eight(&alpha),
            width,
            height,
            config,
            &stop,
        )
    } else {
        let mut alpha = Vec::with_capacity(width * height);
        for row in img.rows() {
            alpha.extend(
                row.iter()
                    .map(|px| crate::convert::scale_from_u16(px.a, 10)),
            );
        }
        encode_rgba_planes_svt(
            &planes,
            MonoPlane::Ten(&alpha),
            width,
            height,
            config,
            &stop,
        )
    }
}

/// Encode an 8-bit grayscale image to a monochrome (Cs400) AVIF via the
/// svtav1-rs backend — the same still-frame mono pipeline the alpha plane
/// uses, muxed as a monochrome color item. [`crate::EncodeBitDepth::Ten`]
/// widens to a 10-bit Cs400 stream (speed ≥ 7 only, see
/// [`svt_rs_depth_error`]).
#[cfg(feature = "encode-mono")]
pub(crate) fn encode_gray8_svt_rs(
    img: ImgRef<'_, u8>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;

    let width = img.width();
    let height = img.height();
    // Grayscale is a Cs400 stream: the stricter mono envelope applies.
    reject_out_of_envelope_dims(width, height, config, true)?;
    let bit_depth = effective_bit_depth(config, false);
    reject_out_of_envelope_depth(bit_depth, config, true)?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let qp = quality_to_qp_gated(config.quality);
    let preset = speed_to_svt_preset(config.speed);
    let color_primaries = config.color_primaries.unwrap_or(DEFAULT_COLOR_PRIMARIES);
    let transfer_characteristics = config
        .transfer_characteristics
        .unwrap_or(DEFAULT_TRANSFER_CHARACTERISTICS);
    let color_description = svtav1::entropy::obu::ColorDescription {
        color_primaries,
        transfer_characteristics,
        // Monochrome streams carry no chroma; matrix is unspecified.
        matrix_coefficients: CICP_UNSPECIFIED,
        full_range: true,
    };

    let av1_payload = if bit_depth == 8 {
        encode_mono_plane_svt(
            MonoPlane::Eight(img.buf()),
            width,
            height,
            img.stride(),
            preset,
            qp,
            config.threads.unwrap_or(0),
            color_description,
            &stop,
        )?
    } else {
        let mut wide = Vec::with_capacity(width * height);
        for row in img.rows() {
            wide.extend(row.iter().map(|&v| widen_8_to_10(v)));
        }
        encode_mono_plane_svt(
            MonoPlane::Ten(&wide),
            width,
            height,
            width,
            preset,
            qp,
            config.threads.unwrap_or(0),
            color_description,
            &stop,
        )?
    };

    stop.check().map_err(|e| at!(Error::from(e)))?;
    mux_svt(
        config,
        av1_payload,
        None,
        width,
        height,
        bit_depth,
        color_primaries,
        transfer_characteristics,
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::speed_to_svt_preset;

    /// Pin the mirrored speed→preset mapping to the upstream boundary
    /// values (svtav1 avif.rs `speed_to_preset_boundaries`).
    #[test]
    fn speed_to_preset_matches_upstream_boundaries() {
        assert_eq!(speed_to_svt_preset(1), 0);
        // 9, NOT 13: C remaps every all-intra preset above M9 down to M9
        // (enc_handle.c:4416-4419), so upstream's own
        // `speed_to_preset_boundaries` asserts 9 here. This assertion read
        // 13 until 2026-09-01 — a drifted mirror, byte-neutral but it made
        // speeds 7..=10 look distinct when they encode identically.
        assert_eq!(speed_to_svt_preset(10), 9);
        assert_eq!(speed_to_svt_preset(5), 6);
        // The whole M9 class, spelled out: these four speeds are ONE encode.
        for s in [7u8, 8, 9, 10] {
            assert_eq!(speed_to_svt_preset(s), 9, "speed {s} must resolve to M9");
        }
        // Monotonic across the whole range.
        let mut prev = 0u8;
        for s in 1..=10u8 {
            let p = speed_to_svt_preset(s);
            assert!(p >= prev, "not monotonic at speed {s}");
            prev = p;
        }
    }

    /// [`crate::expert::SvtParams::resolved`] transcribes the port's
    /// `HdrForkConfig::apply_tune_overrides`. The sweep planner uses the
    /// transcription (so a cell resolves without an encode), so the two
    /// must not drift: drive the REAL port config through the REAL
    /// override and compare field by field, at a qp on each side of the
    /// tune-IQ `max_tx_size` switch.
    // `SvtParams` is #[non_exhaustive], so Default + field assignment is the
    // only spelling available — same reason `KnobProbe::apply` carries this.
    #[allow(clippy::field_reassign_with_default)]
    #[test]
    fn resolved_matches_the_port_tune_overrides() {
        for tune in [0u8, 1, 2, 3, 4] {
            for qp in [10u8, 45, 46, 63] {
                let mut ours = crate::expert::SvtParams::default();
                ours.tune = tune;
                let ours = ours.resolved(qp);

                let mut theirs = svtav1::encoder::hdr_mode::HdrForkConfig::mainline();
                theirs.tune = tune;
                theirs.apply_tune_overrides(qp);

                assert_eq!(
                    ours.enable_qm, theirs.enable_qm,
                    "tune {tune} qp {qp}: enable_qm"
                );
                assert_eq!(
                    ours.min_qm_level, theirs.min_qm_level,
                    "tune {tune} qp {qp}: qm_min"
                );
                assert_eq!(
                    ours.max_qm_level, theirs.max_qm_level,
                    "tune {tune} qp {qp}: qm_max"
                );
                assert_eq!(
                    ours.sharpness, theirs.sharpness,
                    "tune {tune} qp {qp}: sharpness"
                );
                assert_eq!(
                    ours.enable_variance_boost, theirs.enable_variance_boost,
                    "tune {tune} qp {qp}: variance boost"
                );
                assert_eq!(
                    ours.variance_boost_strength, theirs.variance_boost_strength,
                    "tune {tune} qp {qp}: vb strength"
                );
                assert_eq!(
                    ours.max_tx_size, theirs.max_tx_size,
                    "tune {tune} qp {qp}: max_tx_size"
                );
                assert_eq!(
                    ours.force_screen_content_mode, theirs.screen_content_mode,
                    "tune {tune} qp {qp}: screen_content_mode"
                );
            }
        }
    }

    /// The seam's historical behaviour IS `SvtParams::default()`: applying
    /// the default set must leave the pipeline's `hdr` at the port's own
    /// mainline default and its tiles at 0/0, so a caller that never
    /// touches `with_svt_params` encodes byte-identically to before the
    /// knob axis existed.
    #[test]
    fn svt_params_default_leaves_the_pipeline_at_mainline() {
        let rc = svtav1::encoder::rate_control::RcConfig {
            mode: svtav1::encoder::rate_control::RcMode::Cqp,
            qp: 35,
            ..svtav1::encoder::rate_control::RcConfig::default()
        };
        let mut pipeline = svtav1::encoder::pipeline::EncodePipeline::new(64, 64, 6, rc, 0, 1)
            .with_chroma_420(true);
        let before = pipeline.hdr.clone();
        super::apply_svt_params(&mut pipeline, &crate::EncoderConfig::new());
        assert_eq!(pipeline.hdr, before, "default SvtParams must be inert");
        assert_eq!(
            pipeline.hdr,
            svtav1::encoder::hdr_mode::HdrForkConfig::mainline()
        );
        assert_eq!((pipeline.tile_cols_log2, pipeline.tile_rows_log2), (0, 0));
    }
}
