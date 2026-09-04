//! # zenavif
//!
//! Pure Rust AVIF image codec powered by [rav1d-safe](https://github.com/memorysafety/rav1d)
//! and [zenravif](https://lib.rs/crates/zenravif).
//!
//! Decodes and encodes AVIF images using the pure Rust rav1d AV1 decoder
//! and zenavif-parse container parser.
//!
//! ## Quick Start
//!
//! ```no_run
//! use zenavif::decode;
//!
//! let avif_data = std::fs::read("image.avif").unwrap();
//! let image = decode(&avif_data).unwrap();
//! println!("{}x{}", image.width(), image.height());
//! ```
//!
//! ## Features
//!
//! - **`unsafe-asm`**: Hand-written assembly decoder via C FFI (fastest) — overrides the default safe decoder
//! - **`encode`**: AVIF encoding via zenravif
//! - **`target-quality`**: [`encode_rgb8_with_target`] — converge on a requested SSIMULACRA2/zensim score
//! - **`zencodec`**: Integration with [`zencodec`](https://crates.io/crates/zencodec) traits
//!
//! The default decoder uses rav1d-safe's managed API — completely safe Rust
//! with zero unsafe code in the entire decode path.
//!
//! ## Configuration
//!
//! For more control over decoding, use [`decode_with`] with a [`DecoderConfig`]:
//!
//! ```no_run
//! use zenavif::{decode_with, DecoderConfig};
//! use enough::Unstoppable;
//!
//! let config = DecoderConfig::new()
//!     .threads(4)
//!     .apply_grain(true)
//!     .frame_size_limit(8192 * 8192);
//!
//! let avif_data = std::fs::read("image.avif").unwrap();
//! let image = decode_with(&avif_data, &config, &Unstoppable).unwrap();
//! ```

#![cfg_attr(
    not(any(feature = "unsafe-asm", feature = "_dev")),
    forbid(unsafe_code)
)]
#![cfg_attr(feature = "_dev", deny(unsafe_code))]

// Crate info for whereat error tracing (enables at!() macro with GitHub links)
whereat::define_at_crate_info!();

mod alloc_util;
mod cancel;

#[cfg(feature = "auto-tune")]
mod auto_tune;
#[cfg(feature = "auto-tune")]
pub use auto_tune::{AutoTuneError, AutoTuneOptions, QualityTarget};

/// Backend + knob tuning — pick an [`Av1Backend`] *and* its knobs for one
/// image, from a bake the **caller** supplies (never bundled weights).
///
/// The [`auto_tune`] family's backend-choosing sibling; see the module
/// docs for the contract and for [`backend_tuner::StubTuner`], the
/// measured-default implementation a consumer can integrate against
/// before a bake exists.
#[cfg(all(feature = "auto-tune", feature = "encode"))]
pub mod backend_tuner;
#[cfg(all(feature = "auto-tune", feature = "encode"))]
pub use backend_tuner::{
    AllowedBackends, AvifTune, AvifTuner, AvifTuning, StubTuner, TuneRequest, TuneSource,
};

mod cicp_resolve;
mod codec;
mod config;
mod convert;
mod decode_av1;
#[cfg(feature = "unsafe-asm")]
mod decoder;
mod decoder_managed;
/// AVIF quality estimation and re-encoding recommendations.
pub mod detect;
#[cfg(feature = "encode")]
mod encode_plan;
#[cfg(feature = "encode")]
mod encoder;
#[cfg(feature = "zenav1-aom-encode")]
mod encoder_aom;
#[cfg(feature = "zenav1-svt")]
mod encoder_svt_rs;
mod error;
/// Deterministic zenanalyze palette gate (FEATURE_HINTS §E rule 1).
#[cfg(feature = "encode")]
pub mod palette_gate;
#[cfg(feature = "encode")]
pub use palette_gate::PalettePreference;
/// Per-image fast-tier budget heads (FEATURE_HINTS §E heads 2+3 — the
/// FAST_TIER_PARITY P2 tx/partition budget rules).
#[cfg(feature = "encode")]
pub mod fast_heads;
#[cfg(feature = "encode")]
pub use fast_heads::{FastTierBudgets, PartitionBudget, TxBudget};
#[cfg(feature = "__expert")]
pub mod expert;
/// Calibrated encode/decode resource estimation (peak memory + time).
pub mod heuristics;
mod image;
/// q0-prediction head for target-quality mode (starting-quality seed for
/// the ssim2-targeted search; fitted constants, fast_heads pattern).
#[cfg(feature = "auto-tune")]
pub mod q0_head;
#[cfg(feature = "_dev")]
pub mod simd;
#[cfg(not(feature = "_dev"))]
pub(crate) mod simd;
mod strip_convert;
/// SVT-AV1 still-image knobs. Private module; its `SvtParams` is
/// re-exported as `expert::SvtParams` behind `__expert`. Gated on
/// the two features that consume it: the svt-rs encode seam and the
/// `__expert` sweep planner. Both imply `encode`.
#[cfg(any(feature = "zenav1-svt", feature = "__expert"))]
mod svt_params;
/// Budgeted sweep-plan construction over the encoder knob space
/// (calibration tooling; unstable like everything behind `__expert`).
#[cfg(feature = "__expert")]
pub mod sweep;
#[cfg(feature = "target-quality")]
mod target_quality;
#[cfg(feature = "target-quality")]
pub use target_quality::{
    TargetMetric, TargetOptions, TargetedEncode, encode_rgb8_with_target, encode_rgb16_with_target,
    encode_rgba8_with_target,
};
/// Superblock pooling primitives shared by the diffmap-guided closed loops.
#[cfg(any(feature = "two-pass-butteraugli", feature = "target-quality"))]
mod sb_pool;
/// Butteraugli-diffmap-guided two-pass encoding (spatial closed loop).
#[cfg(feature = "two-pass-butteraugli")]
pub mod two_pass;
/// Generation-C zensim scoring (`ZensimProfile::C`) and its attribution
/// steering map — the 944-feature regime `Zensim::compute` cannot produce.
#[cfg(feature = "target-quality")]
pub mod zensim_c;
#[cfg(feature = "two-pass-butteraugli")]
pub use two_pass::{
    FRAME_HINTS_LIVE, TwoPassEncode, TwoPassMetric, TwoPassOptions, encode_rgb8_two_pass,
};
/// zensim-diffmap-driven closed loop (global score correction + spatial
/// per-superblock quantizer scaling from one metric call per pass).
#[cfg(feature = "two-pass-zensim")]
pub mod two_pass_zensim;
#[cfg(feature = "two-pass-zensim")]
pub use two_pass_zensim::{
    LatticePolicy, SPATIAL_HINTS_LIVE, TwoShotOptions, TwoShotResult, ZensimLoopOptions,
    ZensimLoopResult, anchor_quality_for_zensim, anchor_quantizer_for_zensim,
    anchor_zensim_for_quantizer, encode_rgb8_zensim_loop, encode_rgb8_zensim_two_shot,
};
mod validation;
#[cfg(feature = "_dev")]
pub mod yuv_convert;
#[cfg(not(feature = "_dev"))]
pub(crate) mod yuv_convert;
#[cfg(all(any(target_arch = "x86_64", target_arch = "aarch64"), feature = "_dev"))]
#[allow(unsafe_code)]
pub mod yuv_convert_fast;
#[cfg(feature = "_dev")]
pub mod yuv_convert_libyuv;
#[cfg(not(feature = "_dev"))]
pub(crate) mod yuv_convert_libyuv;
#[cfg(feature = "_dev")]
pub mod yuv_convert_libyuv_autovec;
#[cfg(not(feature = "_dev"))]
pub(crate) mod yuv_convert_libyuv_autovec;
#[cfg(all(any(target_arch = "x86_64", target_arch = "aarch64"), feature = "_dev"))]
pub mod yuv_convert_libyuv_simd;
#[cfg(all(
    any(target_arch = "x86_64", target_arch = "aarch64"),
    not(feature = "_dev")
))]
pub(crate) mod yuv_convert_libyuv_simd;
// #[cfg(feature = "zennode")]
// pub mod zennode_defs;

#[cfg(feature = "encode")]
use whereat::at;

pub use codec::{
    AvifAnimationFrameDecoder, AvifDecodeJob, AvifDecoder as AvifZenDecoder, AvifDecoderConfig,
};
#[cfg(feature = "encode")]
pub use codec::{AvifAnimationFrameEncoder, AvifEncodeJob, AvifEncoder, AvifEncoderConfig};
pub use config::DecoderConfig;
pub use decode_av1::decode_av1_obu;
// DECODE-BENCH FORK: raw-OBU decode-to-YUV seam + second backend (zenav1-aom).
pub use decode_av1::{DecodeBackend, DecodedYuv, decode_av1_obu_yuv, decode_av1_obu_yuv_with};
#[cfg(feature = "unsafe-asm")]
pub use decoder::AvifDecoder;
pub use decoder_managed::{AnimationDecoder, ManagedAvifDecoder};
#[cfg(feature = "encode")]
pub use encode_plan::{
    EncodePlan, PlanInput, SpeedDerived, TilesResolution, quality_for_quantizer,
};
#[cfg(feature = "encode-mono")]
pub use encoder::encode_gray8;
#[cfg(feature = "encode")]
pub use encoder::{
    AnimationFrame, AnimationFrame16, AnimationFrameRgba, AnimationFrameRgba16, Av1Backend,
    EncodeAlphaMode, EncodeBitDepth, EncodeChromaSubsampling, EncodeColorModel, EncodePixelRange,
    EncodedAnimation, EncodedImage, EncoderConfig, GainMapConfig, MasteringDisplayConfig,
    encode_animation_rgb8, encode_animation_rgb16, encode_animation_rgba8, encode_animation_rgba16,
    encode_rgb8, encode_rgb16, encode_rgba8, encode_rgba16,
};
pub use enough::{Stop, StopReason, Unstoppable};
pub use error::{Error, Result};
pub use image::{
    AvifDepthMap, AvifGainMap, ChromaSampling, CleanAperture, ColorPrimaries, ColorRange,
    ContentLightLevel, DecodedAnimation, DecodedAnimationInfo, DecodedFrame, GainMapChannel,
    GainMapMetadata, ImageInfo, ImageMirror, ImageRotation, MasteringDisplayColourVolume,
    MatrixCoefficients, PixelAspectRatio, TransferCharacteristics,
};
pub use validation::ValidationError;
pub use zenpixels::PixelBuffer;

/// Decode an AVIF image with default settings
///
/// This is a convenience function that uses default decoder settings
/// and no cancellation support.
///
/// # Example
///
/// ```no_run
/// let avif_data = std::fs::read("image.avif").unwrap();
/// let image = zenavif::decode(&avif_data).unwrap();
/// ```
pub fn decode(data: &[u8]) -> Result<PixelBuffer> {
    decode_with(data, &DecoderConfig::default(), &Unstoppable)
}

/// Decode an AVIF image with custom settings and cancellation support
///
/// # Arguments
///
/// * `data` - Raw AVIF file data
/// * `config` - Decoder configuration
/// * `stop` - Cancellation token (use `Unstoppable` if not needed)
///
/// # Example
///
/// ```no_run
/// use zenavif::{decode_with, DecoderConfig};
/// use enough::Unstoppable;
///
/// let config = DecoderConfig::new().threads(4);
/// let avif_data = std::fs::read("image.avif").unwrap();
/// let image = decode_with(&avif_data, &config, &Unstoppable).unwrap();
/// ```
pub fn decode_with(
    data: &[u8],
    config: &DecoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<PixelBuffer> {
    #[cfg(feature = "unsafe-asm")]
    {
        let mut decoder = AvifDecoder::new(data, config)?;
        decoder.decode(stop)
    }

    #[cfg(not(feature = "unsafe-asm"))]
    {
        let mut decoder = ManagedAvifDecoder::new(data, config)?;
        decoder.decode(stop)
    }
}

/// Decode an animated AVIF with default settings
///
/// Returns all frames with timing info, or [`Error::Unsupported`] if the
/// file is not animated.
///
/// # Example
///
/// ```no_run
/// let avif_data = std::fs::read("animation.avif").unwrap();
/// let animation = zenavif::decode_animation(&avif_data).unwrap();
/// for frame in &animation.frames {
///     println!("{}x{} frame, {}ms", frame.pixels.width(), frame.pixels.height(), frame.duration_ms);
/// }
/// ```
pub fn decode_animation(data: &[u8]) -> Result<DecodedAnimation> {
    decode_animation_with(data, &DecoderConfig::default(), &Unstoppable)
}

/// Decode an animated AVIF with custom settings and cancellation support
///
/// Returns all frames with timing info, or [`Error::Unsupported`] if the
/// file is not animated.
pub fn decode_animation_with(
    data: &[u8],
    config: &DecoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<DecodedAnimation> {
    let mut decoder = ManagedAvifDecoder::new(data, config)?;
    decoder.decode_animation(stop)
}

/// Encode a decoded image to AVIF with default settings
///
/// Supports Rgb8, Rgba8, Rgb16, and Rgba16 pixel formats. Returns
/// [`Error::Unsupported`] for grayscale inputs.
///
/// # Example
///
/// ```no_run
/// let avif_data = std::fs::read("image.avif").unwrap();
/// let image = zenavif::decode(&avif_data).unwrap();
/// let encoded = zenavif::encode(&image).unwrap();
/// std::fs::write("output.avif", &encoded.avif_file).unwrap();
/// ```
#[cfg(feature = "encode")]
pub fn encode(image: &PixelBuffer) -> Result<EncodedImage> {
    encode_with(
        image,
        &EncoderConfig::default(),
        almost_enough::StopToken::new(Unstoppable),
    )
}

/// Encode a decoded image to AVIF with custom settings and cancellation
///
/// Supports Rgb8, Rgba8, Rgb16, and Rgba16 pixel formats. Returns
/// [`Error::UnsupportedOperation`] (with [`zencodec::UnsupportedOperation::PixelFormat`])
/// for grayscale (and any other unhandled) inputs.
#[cfg(feature = "encode")]
pub fn encode_with(
    image: &PixelBuffer,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    use zenpixels::PixelDescriptor;

    let desc = image.descriptor();

    // `layout_compatible` is necessary but NOT sufficient for a typed view:
    // `PixelBuffer::try_as_imgref` additionally requires the row stride to be
    // a whole number of pixels. A caller-supplied buffer whose stride is not
    // divisible by the pixel size (externally allocated frames, sub-region
    // views onto an odd-stride parent) passes the first check and fails the
    // second — so this is a caller-fixable input error, not an invariant.
    macro_rules! view_as {
        ($t:ty) => {
            image.try_as_imgref::<$t>().ok_or_else(|| {
                at!(Error::InvalidBuffer(format!(
                    "pixel buffer row stride is {} bytes, which is not a whole number of \
                     {}-byte {} pixels; re-pack the buffer with a stride divisible by the \
                     pixel size before encoding",
                    image.stride(),
                    core::mem::size_of::<$t>(),
                    stringify!($t),
                )))
            })?
        };
    }

    if desc.layout_compatible(PixelDescriptor::RGB8) {
        encode_rgb8(view_as!(rgb::Rgb<u8>), config, stop)
    } else if desc.layout_compatible(PixelDescriptor::RGBA8) {
        encode_rgba8(view_as!(rgb::Rgba<u8>), config, stop)
    } else if desc.layout_compatible(PixelDescriptor::RGB16) {
        encode_rgb16(view_as!(rgb::Rgb<u16>), config, stop)
    } else if desc.layout_compatible(PixelDescriptor::RGBA16) {
        encode_rgba16(view_as!(rgb::Rgba<u16>), config, stop)
    } else {
        Err(at!(Error::UnsupportedOperation(
            zencodec::UnsupportedOperation::PixelFormat,
        )))
    }
}
