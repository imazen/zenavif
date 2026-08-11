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
//! * 8-bit still images only: RGB/RGBA → 4:2:0 YCbCr (BT.601, full range),
//!   plus grayscale → monochrome (Cs400). RGBA's straight alpha plane is a
//!   separate Cs400 encode muxed as an `auxl` auxiliary item, honoring the
//!   [`crate::EncoderConfig::alpha_quality`] fallback contract.
//! * Width and height must be **multiples of 64** — a zenavif-side
//!   verified-envelope restriction, no longer an upstream limit. At the
//!   pinned rev the pipeline pads TRUE→ALIGNED internally, signals the
//!   TRUE dimensions in the sequence header, and codes partial
//!   superblocks (byte-matching C at presets ≥ 6; panic-free +
//!   aomdec-decodable at presets 0–5 — upstream task #95). zenavif keeps
//!   the 64-multiple gate until its own cross-backend decode validation
//!   covers non-64-aligned cells; the alpha/gray (mono) path upstream
//!   also still requires 8-aligned dims and 64-aligned below preset 6.
//! * No 10-bit, no RGB (identity) model, no limited range, no lossless, no
//!   gain map, no animation. Each is rejected honestly at encode time (and
//!   by [`crate::EncoderConfig::validate`]).
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
//! At the pinned rev, svtav1-rs emits **byte-identical bitstreams to the C
//! SVT-AV1 encoder (v4.2.0 baseline)** across its verified battery
//! (upstream `rust/STATUS.md`): the full-SB identity matrix (54/54, all
//! presets, bd8 synthetic), bd10 (matrix 36/36 + non-flat 309/309),
//! real-photo p0 bd8 (135/135) and bd10 p0–p3 (187/187), partial SBs at
//! presets ≥ 6 incl. odd dims (101/101), SB128, and multi-tile (29/29).
//! Not byte-exact everywhere yet: screen-content low presets carry pinned
//! RD near-ties, bd10 photo p4 is 13/15, and QP 0 / lossless is rejected
//! upstream (typed `UnsupportedConfig`, issue #5). The zenavif round-trip
//! and cross-backend decode tests (`tests/svt_rs_backend.rs`,
//! `tests/cross_backend_decode.rs`) verify the seam end-to-end.

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
/// QP 0 (base_qindex 0 = coded-lossless) is unimplemented upstream. It
/// used to CORRUPT (measured 2026-07-22, benchmarks/backend_sweep_2026-07-22
/// .tsv: syntactically-valid bitstreams decoding to garbage, ssim2 ~= -700);
/// since upstream `f0f0a70ca` (issue #5) `try_encode_frame*` REJECTS it with
/// a typed `EncodeError::UnsupportedConfig` instead. The clamp stays for a
/// different reason now: quality 100 maps to QP 0 linearly, and it must
/// ENCODE (at QP 1, the verified floor), not fail. Composition is covered by
/// `svt_rs_quality_100_does_not_corrupt` (clamp side) and
/// `svt_rs_direct_qp0_rejected_typed` (upstream-gate side) in
/// `tests/svt_rs_backend.rs`. Remove the clamp only if the quality→QP
/// mapping stops touching 0 or upstream implements coded-lossless.
fn quality_to_qp_gated(quality: f32) -> u8 {
    svtav1::avif::AvifEncoder::quality_to_qp_static(quality).max(1)
}

/// Map speed 1..=10 to an SVT-AV1 preset 0..=13.
///
/// Provenance: mirrors the private `AvifEncoder::speed_to_preset` in
/// imazen/svtav1 `svtav1-rs/svtav1/src/avif.rs` (speed 1 → preset 0
/// slowest/best, speed 10 → preset 13 fastest; linear with rounding).
fn speed_to_svt_preset(speed: u8) -> u8 {
    let clamped = speed.clamp(1, 10) as u32;
    (((clamped - 1) * 13 + 4) / 9) as u8
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
    if config.bit_depth == EncodeBitDepth::Ten {
        return Err(at!(Error::Unsupported(
            "Av1Backend::SvtRs is 8-bit only for now (bit_depth Ten is zenravif-only)"
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

/// Reject dimensions outside this backend's verified envelope (module docs:
/// 64-multiples only — a zenavif-side restriction; upstream pads and codes
/// partial SBs, but zenavif's cross-backend validation does not cover
/// non-64-aligned cells yet, and the mono alpha/gray path upstream is
/// stricter than the 4:2:0 path).
fn reject_unaligned_dims(width: usize, height: usize) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(at!(Error::Encode(format!(
            "cannot encode empty image ({width}x{height})"
        ))));
    }
    if !width.is_multiple_of(64) || !height.is_multiple_of(64) {
        return Err(at!(Error::Encode(format!(
            "Av1Backend::SvtRs currently accepts dimensions that are \
             multiples of 64 only (got {width}x{height}) — the envelope \
             zenavif has cross-validated. Pad/crop upstream or use the \
             zenravif backend for arbitrary sizes"
        ))));
    }
    Ok(())
}

/// Run one still-frame monochrome encode through the svtav1-rs pipeline.
///
/// `plane` is `stride`-strided (`stride >= width`), `width`/`height` already
/// 64-aligned. Returns the TD + sequence header + frame OBU payload. Used for
/// grayscale color items and alpha auxiliary items (both are Cs400 streams).
#[expect(clippy::too_many_arguments, reason = "internal plane-encode helper")]
fn encode_mono_plane_svt(
    plane: &[u8],
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
    pipeline.bit_depth = 8;
    pipeline.color_description = color_description;
    // Cooperative cancellation inside the pipeline (SB-cadence polling) —
    // backend-seam obligation 3: a capability the backend accepts must be
    // threaded through in the same change.
    pipeline.stop = stop.clone();
    // Bounded tile-parallel threading (byte-inert — tiles reassemble in
    // order; inert on today's single-tile frames but wired so a future
    // tile knob inherits the caller's thread budget). 0 = auto.
    pipeline.thread_count = threads;

    // The pipeline reads a tight `stride`-strided plane; make it tight when
    // the caller's buffer is padded.
    let payload = if stride == width {
        pipeline.try_encode_frame(plane, width)
    } else {
        let mut tight = Vec::with_capacity(width * height);
        for row in plane.chunks(stride).take(height) {
            tight.extend_from_slice(&row[..width]);
        }
        pipeline.try_encode_frame(&tight, width)
    }
    .map_err(map_svt_encode_error)?;
    Ok(payload)
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
    reject_unaligned_dims(width, height)?;
    let w = u32::try_from(width).map_err(|_| at!(Error::Encode("width exceeds u32".into())))?;
    let h = u32::try_from(height).map_err(|_| at!(Error::Encode("height exceeds u32".into())))?;

    // ---- RGB -> YUV 4:2:0, BT.601 full range ----------------------------
    // Full range matches what the svtav1-rs sequence header signals
    // (color_range is pinned to 1) and zenravif's full-range default;
    // BT.601 matches the zenavif YCbCr convention. The in-house forward
    // kernel is the exact inverse of the decode recipe (per-pixel f32
    // chroma, box-averaged before quantization).
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let mut y_plane = vec![0u8; width * height];
    let mut u_plane = vec![0u8; cw * ch];
    let mut v_plane = vec![0u8; cw * ch];
    crate::yuv_convert::rgb8_to_yuv420(
        img.buf(),
        img.stride(),
        width,
        height,
        crate::yuv_convert::YuvRange::Full,
        crate::yuv_convert::YuvMatrix::Bt601,
        &mut y_plane,
        &mut u_plane,
        &mut v_plane,
    );

    // ---- svtav1-rs still-frame encode -----------------------------------
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let qp = quality_to_qp_gated(config.quality);
    let preset = speed_to_svt_preset(config.speed);
    let color_primaries = config.color_primaries.unwrap_or(DEFAULT_COLOR_PRIMARIES);
    let transfer_characteristics = config
        .transfer_characteristics
        .unwrap_or(DEFAULT_TRANSFER_CHARACTERISTICS);

    let rc = svtav1::encoder::rate_control::RcConfig {
        mode: svtav1::encoder::rate_control::RcMode::Cqp,
        qp,
        ..svtav1::encoder::rate_control::RcConfig::default()
    };
    // hierarchical_levels 0 + intra_period 1: single still key frame with a
    // reduced still-picture sequence header (the AvifEncoder pattern).
    let mut pipeline = svtav1::encoder::pipeline::EncodePipeline::new(w, h, preset, rc, 0, 1)
        .with_chroma_420(true);
    pipeline.bit_depth = 8;
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
    let av1_payload = pipeline
        .try_encode_frame_420(&y_plane, &u_plane, &v_plane, width)
        .map_err(map_svt_encode_error)?;

    // ---- AVIF container --------------------------------------------------
    // The av1C written here must match the payload's sequence header
    // (Chrome cross-validates): profile 0, 8-bit, 4:2:0, full range.
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let aviffy = build_aviffy(
        config,
        color_primaries,
        transfer_characteristics,
        zenavif_serialize::constants::MatrixCoefficients::Bt601,
        false,
    );

    let avif_file = aviffy
        .try_to_vec(&av1_payload, None, w, h, 8)
        .map_err(|e| at!(Error::Encode(format!("AVIF serialization failed: {e}"))))?;

    Ok(EncodedImage {
        color_byte_size: av1_payload.len(),
        alpha_byte_size: 0,
        avif_file,
    })
}

/// CICP "unspecified" code point — what the alpha auxiliary stream signals
/// (an alpha plane has no colorimetry; readers ignore its CICP per MIAF).
const CICP_UNSPECIFIED: u8 = 2;

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
    reject_unaligned_dims(width, height)?;
    let w = u32::try_from(width).map_err(|_| at!(Error::Encode("width exceeds u32".into())))?;
    let h = u32::try_from(height).map_err(|_| at!(Error::Encode("height exceeds u32".into())))?;

    // ---- RGBA -> YUV 4:2:0 color + tight alpha plane --------------------
    // Same forward kernel as the RGB path (alpha ignored here — it rides
    // as its own Cs400 stream below), so RGB and RGBA encodes of the same
    // pixels produce byte-identical color payloads by construction.
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let mut y_plane = vec![0u8; width * height];
    let mut u_plane = vec![0u8; cw * ch];
    let mut v_plane = vec![0u8; cw * ch];
    crate::yuv_convert::rgba8_to_yuv420(
        img.buf(),
        img.stride(),
        width,
        height,
        crate::yuv_convert::YuvRange::Full,
        crate::yuv_convert::YuvMatrix::Bt601,
        &mut y_plane,
        &mut u_plane,
        &mut v_plane,
    );
    let mut alpha_plane = Vec::with_capacity(width * height);
    for row in img.rows() {
        alpha_plane.extend(row.iter().map(|px| px.a));
    }

    // ---- svtav1-rs still-frame encodes: color, then alpha ---------------
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let qp = quality_to_qp_gated(config.quality);
    let alpha_qp = quality_to_qp_gated(crate::encoder::effective_alpha_quality(config));
    let preset = speed_to_svt_preset(config.speed);
    let color_primaries = config.color_primaries.unwrap_or(DEFAULT_COLOR_PRIMARIES);
    let transfer_characteristics = config
        .transfer_characteristics
        .unwrap_or(DEFAULT_TRANSFER_CHARACTERISTICS);

    let rc = svtav1::encoder::rate_control::RcConfig {
        mode: svtav1::encoder::rate_control::RcMode::Cqp,
        qp,
        ..svtav1::encoder::rate_control::RcConfig::default()
    };
    let mut pipeline = svtav1::encoder::pipeline::EncodePipeline::new(w, h, preset, rc, 0, 1)
        .with_chroma_420(true);
    pipeline.bit_depth = 8;
    pipeline.color_description = svtav1::entropy::obu::ColorDescription {
        color_primaries,
        transfer_characteristics,
        matrix_coefficients: MATRIX_COEFFICIENTS_BT601,
        full_range: true,
    };
    pipeline.stop = stop.clone();
    // Caller's thread budget (see encode_mono_plane_svt for semantics).
    pipeline.thread_count = config.threads.unwrap_or(0);
    let color_payload = pipeline
        .try_encode_frame_420(&y_plane, &u_plane, &v_plane, width)
        .map_err(map_svt_encode_error)?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let alpha_payload = encode_mono_plane_svt(
        &alpha_plane,
        width,
        height,
        width,
        preset,
        alpha_qp,
        config.threads.unwrap_or(0),
        svtav1::entropy::obu::ColorDescription {
            color_primaries: CICP_UNSPECIFIED,
            transfer_characteristics: CICP_UNSPECIFIED,
            matrix_coefficients: CICP_UNSPECIFIED,
            full_range: true,
        },
        &stop,
    )?;

    // ---- AVIF container (color item + auxl alpha item) -------------------
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let aviffy = build_aviffy(
        config,
        color_primaries,
        transfer_characteristics,
        zenavif_serialize::constants::MatrixCoefficients::Bt601,
        false,
    );
    let avif_file = aviffy
        .try_to_vec(&color_payload, Some(&alpha_payload), w, h, 8)
        .map_err(|e| at!(Error::Encode(format!("AVIF serialization failed: {e}"))))?;

    Ok(EncodedImage {
        color_byte_size: color_payload.len(),
        alpha_byte_size: alpha_payload.len(),
        avif_file,
    })
}

/// Encode an 8-bit grayscale image to a monochrome (Cs400) AVIF via the
/// svtav1-rs backend — the same still-frame mono pipeline the alpha plane
/// uses, muxed as a monochrome color item.
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
    reject_unaligned_dims(width, height)?;
    let w = u32::try_from(width).map_err(|_| at!(Error::Encode("width exceeds u32".into())))?;
    let h = u32::try_from(height).map_err(|_| at!(Error::Encode("height exceeds u32".into())))?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let qp = quality_to_qp_gated(config.quality);
    let preset = speed_to_svt_preset(config.speed);
    let color_primaries = config.color_primaries.unwrap_or(DEFAULT_COLOR_PRIMARIES);
    let transfer_characteristics = config
        .transfer_characteristics
        .unwrap_or(DEFAULT_TRANSFER_CHARACTERISTICS);

    let av1_payload = encode_mono_plane_svt(
        img.buf(),
        width,
        height,
        img.stride(),
        preset,
        qp,
        config.threads.unwrap_or(0),
        svtav1::entropy::obu::ColorDescription {
            color_primaries,
            transfer_characteristics,
            // Monochrome streams carry no chroma; matrix is unspecified.
            matrix_coefficients: CICP_UNSPECIFIED,
            full_range: true,
        },
        &stop,
    )?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let aviffy = build_aviffy(
        config,
        color_primaries,
        transfer_characteristics,
        zenavif_serialize::constants::MatrixCoefficients::Unspecified,
        true,
    );
    let avif_file = aviffy
        .try_to_vec(&av1_payload, None, w, h, 8)
        .map_err(|e| at!(Error::Encode(format!("AVIF serialization failed: {e}"))))?;

    Ok(EncodedImage {
        color_byte_size: av1_payload.len(),
        alpha_byte_size: 0,
        avif_file,
    })
}

#[cfg(test)]
mod tests {
    use super::speed_to_svt_preset;

    /// Pin the mirrored speed→preset mapping to the upstream boundary
    /// values (svtav1 avif.rs `speed_to_preset_boundaries`).
    #[test]
    fn speed_to_preset_matches_upstream_boundaries() {
        assert_eq!(speed_to_svt_preset(1), 0);
        assert_eq!(speed_to_svt_preset(10), 13);
        // Monotonic across the whole range.
        let mut prev = 0u8;
        for s in 1..=10u8 {
            let p = speed_to_svt_preset(s);
            assert!(p >= prev, "not monotonic at speed {s}");
            prev = p;
        }
    }
}
