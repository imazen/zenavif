//! zenav1-svt AVIF encode backend (`zenav1-svt` feature, EXPERIMENTAL).
//!
//! Drives the pure Rust SVT-AV1 `EncodePipeline` and muxes the coded items
//! through `zenavif-serialize`.
//!
//! # Wired scope
//!
//! Still RGB/RGBA images use 4:2:0 BT.601 full-range YCbCr; grayscale uses
//! Cs400. Straight alpha is a separate Cs400 `auxl` item and honors the
//! configured alpha quality fallback. Color, grayscale and alpha support
//! odd dimensions and partial superblocks at every public speed, at both
//! 8 and 10 bits. Sixteen-bit RGB/RGBA input is quantized to native 10-bit
//! planes; monochrome uses native 10-bit coefficient coding, although its
//! mode decisions currently use the upper eight source bits upstream.
//! CICP is signaled in the sequence header and container, with `clli` and
//! `mdcv` written container-side.
//!
//! This adapter still rejects 12-bit, 4:2:2/4:4:4, identity/RGB, limited
//! range and gain maps. RGB/RGBA animation at 8/10 bits uses full sequence
//! headers and independent sync samples, sharing the still pixel-coding path.
//! Explicit coded-lossless remains unwired; the quality dial retains QP >= 1
//! (see [`quality_to_qp_gated`]).
//!
//! # Quality and speed
//!
//! Quality uses SVT's linear 1..=100 to QP 63..=0 mapping with the QP >= 1
//! clamp. Speed uses the upstream rounded 0..=13 mapping, clamped to M9 for
//! all-intra coding, so speeds 7..=10 are aliases. Sweep fingerprints use
//! these same resolved values.
//!
//! # Verification
//!
//! `tests/svt_rs_backend.rs` checks real AVIF round trips, unchanged pixel
//! quality floors, supported geometry/depth combinations, and direct QP-0
//! source reconstruction with independent raw decoders. The pinned upstream
//! merge's C regression and native-lossless results are recorded in
//! `rust/benchmarks/main_merge_2026-09-07.md` in zenav1-svt. These are scoped
//! measurements, not a claim that every C feature or coding case is exact.

mod animation;
pub(crate) use animation::{
    encode_animation_rgb8, encode_animation_rgb16, encode_animation_rgba8, encode_animation_rgba16,
};

use crate::Result;
use crate::encoder::{EncodeChromaSubsampling, EncodeColorModel, EncodePixelRange};
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

/// Map a fallible zenav1-svt pipeline failure onto the matching zenavif
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
            "zenav1-svt rejected dimensions {width}x{height}: {reason}"
        ))),
        SvtError::UnsupportedConfig(what) => at!(Error::Unsupported(what)),
        // `EncodeError` is #[non_exhaustive]; future variants degrade to the
        // generic encode bucket rather than failing the build.
        other => at!(Error::Encode(format!("zenav1-svt encode failed: {other}"))),
    }
}

/// Map the lossy quality ladder to SVT QP >= 1.
///
/// Upstream implements coded-lossless QP 0 for 8/10-bit color and mono.
/// This adapter retains its existing quality policy: RGB conversion and
/// 4:2:0 subsampling cannot promise image-lossless reconstruction. The
/// explicit lossless request is still rejected here pending public wiring.
/// `svt_rs_quality_100_does_not_corrupt` covers the ladder; the direct QP-0
/// tests separately require exact reconstruction of every coded plane.
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
/// Speeds 1..=6 map to 0/1/3/4/6/7; speeds 7..=10 map to 9.
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
// Only `src/sweep.rs` (behind `__expert`) calls this.
#[cfg_attr(not(feature = "__expert"), allow(dead_code))]
pub(crate) fn svt_resolved_identity(config: &crate::EncoderConfig) -> (i8, u8, u8) {
    (
        config
            .svt_route_preset
            .map(|p| p.value())
            .unwrap_or_else(|| speed_to_svt_preset(config.speed_effective()) as i8),
        quality_to_qp_gated(config.quality),
        quality_to_qp_gated(crate::encoder::effective_alpha_quality(config)),
    )
}

/// Push `expert::SvtParams` onto a colour `EncodePipeline`.
///
/// The seam's historical behaviour is `SvtParams::default()`, which is
/// SVT-AV1 v4.2.0's **mainline** default set (tune 1 = PSNR, QM off,
/// variance boost off, sharpness 0, `max_tx_size` 64, preset-derived
/// screen-content mode, no tiles) — so a caller that never touches
/// `with_svt_params` gets byte-identical output to before this function
/// existed. Gated by `svt_params_default_leaves_the_pipeline_at_mainline`.
///
/// Values are `SvtParams::clamped` first: the port guards
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
    let p = config.svt_params_resolved();
    pipeline.hdr = resolved_svt_hdr(config);
    pipeline.tile_cols_log2 = p.tile_cols_log2;
    pipeline.tile_rows_log2 = p.tile_rows_log2;
}

/// Shared query/encode validation for routing controls.
fn resolved_svt_hdr(config: &EncoderConfig) -> svtav1::encoder::hdr_mode::HdrForkConfig {
    let p = config.svt_params_resolved();
    svtav1::encoder::hdr_mode::HdrForkConfig {
        tune: p.tune,
        enable_variance_boost: p.enable_variance_boost,
        variance_boost_strength: p.variance_boost_strength,
        variance_octile: p.variance_octile,
        enable_qm: p.enable_qm,
        min_qm_level: p.min_qm_level,
        max_qm_level: p.max_qm_level,
        min_chroma_qm_level: p.min_qm_level,
        max_chroma_qm_level: p.max_qm_level,
        sharpness: p.sharpness,
        screen_content_mode: p.force_screen_content_mode,
        ac_bias: p.ac_bias,
        max_tx_size: p.max_tx_size,
        ..Default::default()
    }
}

pub(crate) fn validate_still_controls(
    config: &EncoderConfig,
    monochrome: bool,
) -> core::result::Result<(), String> {
    if config.svt.tune > 5 {
        return Err(
            "SVT adapter tune must be 0..=5; its legacy slot 5 means film grain, not C VMAF".into(),
        );
    }
    if config.svt_route_policy.is_some() && config.svt.tune == 5 {
        return Err("SvtParity cannot use the adapter's legacy film-grain tune slot 5: C tune 5 is VMAF and rejects all-intra".into());
    }
    if !config.svt.ac_bias.is_finite() || !(0.0..=8.0).contains(&config.svt.ac_bias) {
        return Err("SVT AC bias must be finite and in 0..=8".into());
    }
    if config.svt.force_screen_content_mode.is_some_and(|m| m > 3) {
        return Err("SVT screen-content mode must be 0..=3".into());
    }
    config.svt_film_grain.validate().map_err(str::to_owned)?;
    if monochrome && config.svt_film_grain.enabled() {
        return Err("C film grain requires 8/10-bit 4:2:0".into());
    }
    let hdr = resolved_svt_hdr(config);
    if monochrome
        && (config.svt_route_policy.is_some()
            || !config.svt_route_enhancements.is_empty()
            || config.svt != crate::svt_params::SvtParams::default())
    {
        return Err("SVT monochrome uses the Rust extension with default coding tools; parity, color-tool overrides and enhancements are unsupported".into());
    }
    if let Some(svtav1::avif::EncodingPolicy::SvtParity(reference)) = config.svt_route_policy {
        reference.validate_hdr_config(&hdr).map_err(str::to_owned)?;
    }
    config
        .svt_route_enhancements
        .validate(
            config
                .svt_route_preset
                .map(|p| p.value())
                .unwrap_or_else(|| speed_to_svt_preset(config.speed) as i8),
            true,
            !monochrome,
        )
        .map_err(str::to_owned)?;
    Ok(())
}

/// All public speeds support partial and odd-size color, mono and alpha.
/// Keep validation and encode-time checks together; the pinned port owns
/// detailed geometry validation and returns its dimensions and reason.
pub(crate) fn svt_rs_dims_error(
    width: usize,
    height: usize,
    _speed: u8,
    _mono_plane: bool,
) -> Option<&'static str> {
    if width == 0 || height == 0 {
        Some("cannot encode an empty image")
    } else {
        None
    }
}

/// Reject configuration the zenav1-svt backend cannot honor.
///
/// Encode entry points clamp/reject independently of the opt-in
/// [`crate::EncoderConfig::validate`], so these checks run on the encode
/// path too — a config asking for something this backend cannot produce
/// must never be served silently different output.
fn reject_unsupported_config(config: &EncoderConfig) -> Result<()> {
    config
        .validate()
        .map_err(|e| at!(Error::InvalidParameters(e.to_string())))?;
    validate_still_controls(config, false).map_err(|e| at!(Error::InvalidParameters(e)))?;
    if config.chroma_subsampling != EncodeChromaSubsampling::Yuv420 {
        return Err(at!(Error::Unsupported(
            "Av1Backend::Zenav1Svt encodes 4:2:0 only: set \
             .chroma_subsampling(EncodeChromaSubsampling::Yuv420) \
             (the 4:4:4 default is zenravif-only for now)"
        )));
    }
    if config.color_model != EncodeColorModel::YCbCr {
        return Err(at!(Error::Unsupported(
            "Av1Backend::Zenav1Svt supports the YCbCr color model only \
             (identity/RGB has no defined 4:2:0 subsampling)"
        )));
    }
    if config.pixel_range == Some(EncodePixelRange::Limited) {
        return Err(at!(Error::Unsupported(
            "Av1Backend::Zenav1Svt signals full pixel range only \
             (the zenav1-svt sequence header pins color_range=1)"
        )));
    }
    if config.gain_map.is_some() {
        return Err(at!(Error::Unsupported(
            "Av1Backend::Zenav1Svt does not support gain maps yet \
             (use the zenravif backend)"
        )));
    }
    #[cfg(feature = "encode-imazen")]
    if config.lossless {
        return Err(at!(Error::Unsupported(
            "Av1Backend::Zenav1Svt does not expose image-lossless encoding; \
             use the zenravif backend for lossless"
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
#[allow(deprecated)]
fn cicp_to_serialize_transfer(tc: u8) -> zenavif_serialize::constants::TransferCharacteristics {
    use zenavif_serialize::constants::TransferCharacteristics as TC;
    match tc {
        1 => TC::Bt709,
        4 => TC::Bt470M,
        5 => TC::Bt470BG,
        6 => TC::Bt601,
        7 => TC::Smpte240,
        9 => TC::Log,
        10 => TC::LogSqrt,
        11 => TC::Iec61966,
        12 => TC::Bt1361,
        15 => TC::Bt2020_12,
        17 => TC::Smpte428,
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

/// Bit depth this backend codes for a request, from the one shared resolver
/// ([`crate::EncoderConfig::coded_bit_depth_bits`]) -- so a depth this port
/// cannot code reaches its own gate below rather than being silently ignored.
fn effective_bit_depth(config: &EncoderConfig, input_is_16bit: bool) -> u8 {
    config.coded_bit_depth_bits(input_is_16bit)
}

/// The pinned SVT port supports native 8/10-bit color, monochrome and
/// alpha at every public speed. Twelve-bit remains an unimplemented depth.
pub(crate) fn svt_rs_depth_error(
    bit_depth: u8,
    _speed: u8,
    _mono_plane: bool,
) -> Option<&'static str> {
    if !matches!(bit_depth, 8 | 10) {
        return Some(
            "Av1Backend::Zenav1Svt codes 8- and 10-bit only; zenav1-svt has no 12-bit \
             encode. Only Av1Backend::Zenav1Aom codes EncodeBitDepth::Twelve",
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

/// Run one still-frame monochrome encode through the zenav1-svt pipeline.
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
    preset: i8,
    qp: u8,
    threads: usize,
    color_description: svtav1::entropy::obu::ColorDescription,
    stop: &almost_enough::StopToken,
    mode: FrameMode,
) -> Result<Vec<u8>> {
    let w = u32::try_from(width).map_err(|_| at!(Error::Encode("width exceeds u32".into())))?;
    let h = u32::try_from(height).map_err(|_| at!(Error::Encode("height exceeds u32".into())))?;
    let rc = svtav1::encoder::rate_control::RcConfig {
        mode: svtav1::encoder::rate_control::RcMode::Cqp,
        qp,
        ..svtav1::encoder::rate_control::RcConfig::default()
    };
    let mut pipeline = svtav1::encoder::pipeline::EncodePipeline::new_with_preset(
        w,
        h,
        svtav1::avif::NativePreset::new(preset)
            .ok_or_else(|| at!(Error::InvalidParameters("invalid native preset".into())))?,
        rc,
        0,
        1,
    );
    pipeline.bit_depth = match plane {
        MonoPlane::Eight(_) => 8,
        MonoPlane::Ten(_) => 10,
    };
    pipeline.color_description = color_description;
    // Cooperative cancellation inside the pipeline (SB-cadence polling) —
    // backend-seam obligation 3: a capability the backend accepts must be
    // threaded through in the same change.
    pipeline = mode.configure(pipeline);
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

/// Run one still-frame 4:2:0 colour encode through the zenav1-svt pipeline
/// at the planes' depth. Returns the TD + sequence header + frame OBU
/// payload.
#[expect(
    clippy::too_many_arguments,
    reason = "shared color pipeline configuration"
)]
fn encode_color_420_svt(
    planes: &Yuv420Planes,
    width: usize,
    height: usize,
    config: &EncoderConfig,
    color_primaries: u8,
    transfer_characteristics: u8,
    stop: &almost_enough::StopToken,
    mode: FrameMode,
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
    let native = config
        .svt_route_preset
        .unwrap_or_else(|| svtav1::avif::NativePreset::new(preset as i8).unwrap());
    let mut pipeline =
        svtav1::encoder::pipeline::EncodePipeline::new_with_preset(w, h, native, rc, 0, 1)
            .with_chroma_420(true);
    apply_svt_params(&mut pipeline, config);
    pipeline.enhancements = config.svt_route_enhancements;
    pipeline.film_grain = config.svt_film_grain.clone();
    if let Some(svtav1::avif::EncodingPolicy::SvtParity(reference)) = config.svt_route_policy {
        pipeline.reference = reference;
        reference
            .validate_hdr_config(&pipeline.hdr)
            .map_err(|e| at!(Error::InvalidParameters(e.into())))?;
    }
    pipeline.bit_depth = planes.bit_depth();
    pipeline.color_description = svtav1::entropy::obu::ColorDescription {
        color_primaries,
        transfer_characteristics,
        matrix_coefficients: MATRIX_COEFFICIENTS_BT601,
        // Note: the zenav1-svt sequence-header writer pins color_range=1
        // (full) regardless of this flag; kept coherent anyway.
        full_range: true,
    };
    pipeline = mode.configure(pipeline);
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
/// Coded pixels shared by still-item and animation-track serialization.
struct CodedSvtFrame {
    color: Vec<u8>,
    alpha: Option<Vec<u8>>,
    width: u32,
    height: u32,
    bit_depth: u8,
    color_primaries: u8,
    transfer_characteristics: u8,
    monochrome: bool,
}

impl CodedSvtFrame {
    fn mux_still(self, config: &EncoderConfig) -> Result<EncodedImage> {
        let mut aviffy = build_aviffy(
            config,
            self.color_primaries,
            self.transfer_characteristics,
            if self.monochrome {
                zenavif_serialize::constants::MatrixCoefficients::Unspecified
            } else {
                zenavif_serialize::constants::MatrixCoefficients::Bt601
            },
            self.monochrome,
        );
        aviffy.set_premultiplied_alpha(
            self.alpha.is_some()
                && config.alpha_color_mode == crate::EncodeAlphaMode::Premultiplied,
        );
        let avif_file = aviffy
            .try_to_vec(
                &self.color,
                self.alpha.as_deref(),
                self.width,
                self.height,
                self.bit_depth,
            )
            .map_err(|e| at!(Error::Encode(format!("AVIF serialization failed: {e}"))))?;
        Ok(EncodedImage {
            color_byte_size: self.color.len(),
            alpha_byte_size: self.alpha.as_ref().map_or(0, Vec::len),
            avif_file,
        })
    }
}

#[derive(Clone, Copy)]
enum FrameMode {
    Still,
    Sequence { framerate: f64 },
}
impl FrameMode {
    fn configure(
        self,
        mut pipeline: svtav1::encoder::pipeline::EncodePipeline,
    ) -> svtav1::encoder::pipeline::EncodePipeline {
        if let Self::Sequence { framerate } = self {
            pipeline = pipeline.with_image_sequence();
            pipeline.rc_config.framerate = framerate;
        }
        pipeline
    }
}

#[expect(clippy::too_many_arguments, reason = "coded frame construction")]
fn coded_frame(
    color: Vec<u8>,
    alpha: Option<Vec<u8>>,
    width: usize,
    height: usize,
    bit_depth: u8,
    color_primaries: u8,
    transfer_characteristics: u8,
    monochrome: bool,
) -> Result<CodedSvtFrame> {
    let width = u32::try_from(width).map_err(|_| at!(Error::Encode("width exceeds u32".into())))?;
    let height =
        u32::try_from(height).map_err(|_| at!(Error::Encode("height exceeds u32".into())))?;
    Ok(CodedSvtFrame {
        color,
        alpha,
        width,
        height,
        bit_depth,
        color_primaries,
        transfer_characteristics,
        monochrome,
    })
}

/// Exact 8 -> 10 bit sample scaling (`round(v * 1023 / 255)`) for alpha
/// and gray planes widened to a 10-bit Cs400 stream.
#[inline]
fn widen_8_to_10(v: u8) -> u16 {
    ((u32::from(v) * 1023 + 127) / 255) as u16
}

/// Encode an 8-bit RGB image to AVIF via the zenav1-svt backend.
///
/// See the module docs for scope and constraints. Cancellation is checked
/// at the seam's phase boundaries (pre-conversion, pre-encode, pre-mux)
/// AND inside the pipeline itself: the token handed to `pipeline.stop` is
/// polled at superblock cadence by the encode loops at the pinned rev.
fn encode_rgb8_frame(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
    mode: FrameMode,
) -> Result<CodedSvtFrame> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;

    let width = img.width();
    let height = img.height();
    reject_out_of_envelope_dims(width, height, config, false)?;
    let bit_depth = effective_bit_depth(config, false);
    reject_out_of_envelope_depth(bit_depth, config, false)?;

    // ---- RGB -> YUV 4:2:0, BT.601 full range ----------------------------
    // Full range matches what the zenav1-svt sequence header signals
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

    // ---- zenav1-svt still-frame encode -----------------------------------
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
        mode,
    )?;

    // ---- AVIF container --------------------------------------------------
    stop.check().map_err(|e| at!(Error::from(e)))?;
    coded_frame(
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

/// Encode a colour 4:2:0 payload plus a Cs400 alpha payload — the
/// shared tail of the RGBA entry points. `alpha` is a tight plane at
/// `bit_depth` (8 or 10).
fn encode_rgba_planes_svt(
    planes: &Yuv420Planes,
    alpha: MonoPlane<'_>,
    width: usize,
    height: usize,
    config: &EncoderConfig,
    stop: &almost_enough::StopToken,
    mode: FrameMode,
) -> Result<CodedSvtFrame> {
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
        mode,
    )?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let alpha_payload = encode_mono_plane_svt(
        alpha,
        width,
        height,
        width,
        preset as i8,
        alpha_qp,
        config.threads.unwrap_or(0),
        alpha_color_description(),
        stop,
        mode,
    )?;

    // ---- AVIF container (color item + auxl alpha item) -------------------
    stop.check().map_err(|e| at!(Error::from(e)))?;
    coded_frame(
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

/// Encode an 8-bit RGBA image to AVIF via the zenav1-svt backend.
///
/// Color travels exactly like [`encode_rgb8_svt_rs`] (4:2:0 BT.601 full
/// range); the straight (non-premultiplied) alpha plane is encoded as a
/// separate monochrome (Cs400) still and muxed as an `auxl` auxiliary item.
/// Alpha quality follows the [`crate::EncoderConfig::alpha_quality`]
/// contract (falls back to the color quality).
fn encode_rgba8_frame(
    img: ImgRef<'_, rgb::Rgba<u8>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
    mode: FrameMode,
) -> Result<CodedSvtFrame> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;

    let width = img.width();
    let height = img.height();
    // The alpha plane is a Cs400 stream with matching depth and dimensions.
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
            mode,
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
            mode,
        )
    }
}

/// Encode a 16-bit RGB image to a 10-bit (profile 0, 4:2:0) AVIF via the
/// zenav1-svt backend (issue #33).
///
/// Input values are full u16 range (0–65535) in the image's own transfer
/// function; the RGB → YCbCr conversion runs at 10-bit precision from the
/// 16-bit source and the u16 planes are handed to the port's native
/// `try_encode_frame_420_hbd`. [`crate::EncodeBitDepth::Eight`] codes an
/// 8-bit stream from the same conversion.
fn encode_rgb16_frame(
    img: ImgRef<'_, Rgb<u16>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
    mode: FrameMode,
) -> Result<CodedSvtFrame> {
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
        mode,
    )?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    coded_frame(
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

/// Encode a 16-bit RGBA image to a 10-bit AVIF via the zenav1-svt backend
/// (issue #33): colour as [`encode_rgb16_svt_rs`], the alpha plane scaled
/// to 10 bits (`scale_from_u16`) as a Cs400 `auxl` item at every speed.
fn encode_rgba16_frame(
    img: ImgRef<'_, rgb::Rgba<u16>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
    mode: FrameMode,
) -> Result<CodedSvtFrame> {
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
            mode,
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
            mode,
        )
    }
}

/// Encode an 8-bit grayscale image to a monochrome (Cs400) AVIF via the
/// zenav1-svt backend — the same still-frame mono pipeline the alpha plane
/// uses, muxed as a monochrome color item. [`crate::EncodeBitDepth::Ten`]
/// widens to a 10-bit Cs400 stream at every speed.
#[cfg(feature = "encode-mono")]
fn encode_gray8_frame(
    img: ImgRef<'_, u8>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
    mode: FrameMode,
) -> Result<CodedSvtFrame> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;
    validate_still_controls(config, true).map_err(|e| at!(Error::InvalidParameters(e)))?;

    let width = img.width();
    let height = img.height();
    // Grayscale uses the shared Cs400 depth and dimension checks.
    reject_out_of_envelope_dims(width, height, config, true)?;
    let bit_depth = effective_bit_depth(config, false);
    reject_out_of_envelope_depth(bit_depth, config, true)?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let qp = quality_to_qp_gated(config.quality);
    let preset = config
        .svt_route_preset
        .map(|p| p.value())
        .unwrap_or_else(|| speed_to_svt_preset(config.speed) as i8);
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
            mode,
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
            mode,
        )?
    };

    stop.check().map_err(|e| at!(Error::from(e)))?;
    coded_frame(
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

pub(crate) fn encode_rgb8_svt_rs(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    encode_rgb8_frame(img, config, stop, FrameMode::Still)?.mux_still(config)
}

pub(crate) fn encode_rgba8_svt_rs(
    img: ImgRef<'_, rgb::Rgba<u8>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    encode_rgba8_frame(img, config, stop, FrameMode::Still)?.mux_still(config)
}

pub(crate) fn encode_rgb16_svt_rs(
    img: ImgRef<'_, Rgb<u16>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    encode_rgb16_frame(img, config, stop, FrameMode::Still)?.mux_still(config)
}

pub(crate) fn encode_rgba16_svt_rs(
    img: ImgRef<'_, rgb::Rgba<u16>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    encode_rgba16_frame(img, config, stop, FrameMode::Still)?.mux_still(config)
}

#[cfg(feature = "encode-mono")]
pub(crate) fn encode_gray8_svt_rs(
    img: ImgRef<'_, u8>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    encode_gray8_frame(img, config, stop, FrameMode::Still)?.mux_still(config)
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

    /// `SvtParams::resolved` transcribes the port's
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
                let mut ours = crate::svt_params::SvtParams::default();
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
