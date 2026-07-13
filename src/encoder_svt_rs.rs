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
//! * 8-bit RGB input → 4:2:0 YCbCr (BT.601, full range) still images only.
//! * Width and height must be **multiples of 64**: the svtav1-rs pipeline
//!   codes full 64×64 superblocks and signals the coded dimensions in the
//!   sequence header (no partial-SB coding yet). Padding here would leak
//!   padded dimensions into decoded output (zenavif's decoder sizes the
//!   image from the sequence header, not `ispe`), so unaligned input is
//!   rejected instead of silently padded.
//! * No alpha, no 10-bit, no RGB (identity) model, no limited range, no
//!   lossless, no gain map, no animation. Each is rejected honestly at
//!   encode time (and by [`crate::EncoderConfig::validate`]).
//!
//! # Payload shape
//!
//! `EncodePipeline::encode_frame_420` returns a temporal-delimiter +
//! sequence-header + frame OBU sequence. It is muxed **verbatim**: the
//! leading TD matches the zenravif payload convention (zenrav1e packet data
//! also begins with a TD OBU) and is byte-identical to the streams the
//! svtav1-rs decode-conformance suite validates under `aomdec` (525/525
//! mono + 700/700 4:2:0 cells at the pinned rev).
//!
//! # Quality / speed mapping
//!
//! Deliberately svtav1-rs's own documented mappings, NOT zenravif's fitted
//! quality→quantizer curve (`src/encode_plan.rs` mirrors describe zenravif
//! only):
//!
//! * quality 1..=100 → QP 63..=0, linear
//!   ([`svtav1::avif::AvifEncoder::quality_to_qp_static`]).
//! * speed 1..=10 → SVT preset 0..=13, linear
//!   (same formula as `svtav1::avif::AvifEncoder`'s internal
//!   `speed_to_preset`; that helper is private upstream, so the formula is
//!   mirrored here with provenance).
//!
//! # C parity
//!
//! Bitstream identity vs the C SVT-AV1 encoder is NOT yet asserted — the
//! gate's parity test lands when svtav1-rs reaches decision-layer bitstream
//! identity. What is verified today is decode conformance (aomdec, upstream)
//! plus the zenavif round-trip tests in `tests/svt_rs_backend.rs`.

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

/// Encode an 8-bit RGB image to AVIF via the svtav1-rs backend.
///
/// See the module docs for scope and constraints. Cancellation is checked
/// at phase boundaries (pre-conversion, pre-encode, pre-mux) — the
/// svtav1-rs pipeline has no per-superblock stop hook yet.
pub(crate) fn encode_rgb8_svt_rs(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;

    let width = img.width();
    let height = img.height();
    if width == 0 || height == 0 {
        return Err(at!(Error::Encode(format!(
            "cannot encode empty image ({width}x{height})"
        ))));
    }
    if !width.is_multiple_of(64) || !height.is_multiple_of(64) {
        return Err(at!(Error::Encode(format!(
            "Av1Backend::SvtRs requires dimensions that are multiples of 64 \
             (got {width}x{height}): the svtav1-rs pipeline codes full 64x64 \
             superblocks and signals coded dimensions in the sequence header. \
             Pad/crop upstream or use the zenravif backend for arbitrary sizes"
        ))));
    }
    let w = u32::try_from(width).map_err(|_| at!(Error::Encode("width exceeds u32".into())))?;
    let h = u32::try_from(height).map_err(|_| at!(Error::Encode("height exceeds u32".into())))?;

    // ---- RGB -> YUV 4:2:0, BT.601 full range ----------------------------
    // Full range matches what the svtav1-rs sequence header signals
    // (color_range is pinned to 1) and zenravif's full-range default;
    // BT.601 matches the zenavif YCbCr convention. The `yuv` crate is the
    // same engine the decode path uses in the other direction.
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let mut planar = yuv::YuvPlanarImageMut::<u8>::alloc(w, h, yuv::YuvChromaSubsampling::Yuv420);
    let rgb_bytes: &[u8] = bytemuck::cast_slice(img.buf());
    let rgb_stride_components = u32::try_from(img.stride() * 3)
        .map_err(|_| at!(Error::Encode("row stride exceeds u32".into())))?;
    yuv::rgb_to_yuv420(
        &mut planar,
        rgb_bytes,
        rgb_stride_components,
        yuv::YuvRange::Full,
        yuv::YuvStandardMatrix::Bt601,
        yuv::YuvConversionMode::Balanced,
    )
    .map_err(|e| at!(Error::Encode(format!("RGB->YUV420 conversion failed: {e}"))))?;

    // ---- svtav1-rs still-frame encode -----------------------------------
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let qp = svtav1::avif::AvifEncoder::quality_to_qp_static(config.quality);
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

    // TD + sequence header + frame OBUs, muxed verbatim (module docs).
    let av1_payload = pipeline.encode_frame_420(
        planar.y_plane.borrow(),
        planar.u_plane.borrow(),
        planar.v_plane.borrow(),
        width,
    );
    if av1_payload.is_empty() {
        return Err(at!(Error::Encode(
            "svtav1-rs pipeline returned an empty bitstream".into()
        )));
    }

    // ---- AVIF container --------------------------------------------------
    // The av1C written here must match the payload's sequence header
    // (Chrome cross-validates): profile 0, 8-bit, 4:2:0, full range.
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let mut aviffy = zenavif_serialize::Aviffy::new();
    aviffy
        .set_seq_profile(0)
        .set_chroma_subsampling((true, true))
        .set_monochrome(false)
        .set_full_color_range(true)
        .set_color_primaries(cicp_to_serialize_primaries(color_primaries))
        .set_transfer_characteristics(cicp_to_serialize_transfer(transfer_characteristics))
        .set_matrix_coefficients(zenavif_serialize::constants::MatrixCoefficients::Bt601);
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
