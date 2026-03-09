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
//! - **`zencodec`**: Integration with [`zencodec-types`](https://crates.io/crates/zencodec-types) traits
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

mod config;
mod convert;
#[cfg(feature = "unsafe-asm")]
mod decoder;
mod decoder_managed;
/// AVIF quality estimation and re-encoding recommendations.
pub mod detect;
#[cfg(feature = "encode")]
mod encoder;
mod error;
mod image;
#[cfg(feature = "_dev")]
pub mod simd;
#[cfg(not(feature = "_dev"))]
pub(crate) mod simd;
mod strip_convert;
#[cfg(feature = "_dev")]
pub mod yuv_convert;
#[cfg(not(feature = "_dev"))]
pub(crate) mod yuv_convert;
#[cfg(all(target_arch = "x86_64", feature = "_dev"))]
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
#[cfg(all(target_arch = "x86_64", feature = "_dev"))]
pub mod yuv_convert_libyuv_simd;
#[cfg(all(target_arch = "x86_64", not(feature = "_dev")))]
pub(crate) mod yuv_convert_libyuv_simd;
#[cfg(feature = "zencodec")]
mod zencodec;

use whereat::at;

pub use config::DecoderConfig;
#[cfg(feature = "unsafe-asm")]
pub use decoder::AvifDecoder;
pub use decoder_managed::{AnimationDecoder, ManagedAvifDecoder};
#[cfg(feature = "encode")]
pub use encoder::{
    AnimationFrame, AnimationFrame16, AnimationFrameRgba, AnimationFrameRgba16, EncodeAlphaMode,
    EncodeBitDepth, EncodeColorModel, EncodePixelRange, EncodedAnimation, EncodedImage,
    EncoderConfig, MasteringDisplayConfig, encode_animation_rgb8, encode_animation_rgb16,
    encode_animation_rgba8, encode_animation_rgba16, encode_rgb8, encode_rgb16, encode_rgba8,
    encode_rgba16,
};
pub use enough::{Stop, StopReason, Unstoppable};
pub use error::{Error, Result};
pub use image::{
    ChromaSampling, CleanAperture, ColorPrimaries, ColorRange, ContentLightLevel, DecodedAnimation,
    DecodedAnimationInfo, DecodedFrame, ImageInfo, ImageMirror, ImageRotation,
    MasteringDisplayColourVolume, MatrixCoefficients, PixelAspectRatio, TransferCharacteristics,
};
#[cfg(feature = "zencodec")]
pub use zencodec::{
    AvifDecodeJob, AvifDecoder as AvifZenDecoder, AvifDecoderConfig, AvifFullFrameDecoder,
};
#[cfg(all(feature = "zencodec", feature = "encode"))]
pub use zencodec::{AvifEncodeJob, AvifEncoder, AvifEncoderConfig};
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
    encode_with(image, &EncoderConfig::default(), &Unstoppable)
}

/// Encode a decoded image to AVIF with custom settings and cancellation
///
/// Supports Rgb8, Rgba8, Rgb16, and Rgba16 pixel formats. Returns
/// [`Error::Unsupported`] for grayscale inputs.
#[cfg(feature = "encode")]
pub fn encode_with(
    image: &PixelBuffer,
    config: &EncoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<EncodedImage> {
    use zenpixels::PixelDescriptor;

    let desc = image.descriptor();
    if desc.layout_compatible(PixelDescriptor::RGB8) {
        let img = image.try_as_imgref::<rgb::Rgb<u8>>().unwrap();
        encode_rgb8(img, config, stop)
    } else if desc.layout_compatible(PixelDescriptor::RGBA8) {
        let img = image.try_as_imgref::<rgb::Rgba<u8>>().unwrap();
        encode_rgba8(img, config, stop)
    } else if desc.layout_compatible(PixelDescriptor::RGB16) {
        let img = image.try_as_imgref::<rgb::Rgb<u16>>().unwrap();
        encode_rgb16(img, config, stop)
    } else if desc.layout_compatible(PixelDescriptor::RGBA16) {
        let img = image.try_as_imgref::<rgb::Rgba<u16>>().unwrap();
        encode_rgba16(img, config, stop)
    } else {
        Err(at!(Error::Unsupported(
            "only RGB/RGBA 8/16-bit encoding is supported",
        )))
    }
}
