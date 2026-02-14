//! # zenavif
//!
//! Pure Rust AVIF image codec powered by [rav1d](https://github.com/memorysafety/rav1d)
//! and [ravif](https://lib.rs/crates/ravif).
//!
//! This crate provides a safe, ergonomic API for decoding and encoding AVIF images
//! using the pure Rust rav1d AV1 decoder and avif-parse container parser.
//!
//! ## Quick Start
//!
//! ```no_run
//! use zenavif::{decode, PixelData};
//!
//! let avif_data = std::fs::read("image.avif").unwrap();
//! let image = decode(&avif_data).unwrap();
//!
//! match image {
//!     PixelData::Rgb8(img) => {
//!         println!("RGB8 image: {}x{}", img.width(), img.height());
//!     }
//!     PixelData::Rgba8(img) => {
//!         println!("RGBA8 image: {}x{}", img.width(), img.height());
//!     }
//!     _ => {}
//! }
//! ```
//!
//! ## Features
//!
//! - **`asm`**: Hand-written assembly (fastest, uses C FFI) — overrides the default managed decoder
//! - **`encode`**: AVIF encoding via ravif
//!
//! The default decoder uses rav1d-safe's managed API which is completely safe Rust
//! with zero unsafe code in the entire decode path.
//!
//! ## Configuration
//!
//! For more control over decoding, use `decode_with` with a `DecoderConfig`:
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

mod config;
mod convert;
#[cfg(feature = "unsafe-asm")]
mod decoder;
mod decoder_managed;
#[cfg(feature = "encode")]
mod encoder;
mod error;
mod image;
pub mod simd;
#[doc(hidden)]
pub mod yuv_convert;
#[doc(hidden)]
pub mod yuv_convert_fast;
pub mod yuv_convert_libyuv;
pub mod yuv_convert_libyuv_16bit;
pub mod yuv_convert_libyuv_autovec;
pub mod yuv_convert_libyuv_simd;
mod zencodec;

pub use config::DecoderConfig;
#[cfg(feature = "unsafe-asm")]
pub use decoder::AvifDecoder;
pub use decoder_managed::ManagedAvifDecoder;
#[cfg(feature = "encode")]
pub use encoder::{
    AnimationFrame, AnimationFrameRgba, EncodeAlphaMode, EncodeBitDepth, EncodeColorModel,
    EncodedAnimation, EncodedImage, EncoderConfig, MasteringDisplayConfig,
    encode_animation_rgb8, encode_animation_rgba8,
    encode_rgb8, encode_rgb16, encode_rgba8, encode_rgba16,
};
pub use enough::{Stop, StopReason, Unstoppable};
pub use error::{Error, Result};
pub use image::{
    ChromaSampling, CleanAperture, ColorPrimaries, ColorRange, ContentLightLevel,
    DecodedAnimation, DecodedAnimationInfo, DecodedFrame, ImageInfo, ImageMirror, ImageRotation,
    MasteringDisplayColourVolume, MatrixCoefficients, PixelAspectRatio, TransferCharacteristics,
};
pub use zencodec::{AvifDecodeJob, AvifDecoding};
#[cfg(feature = "encode")]
pub use zencodec::{AvifEncodeJob, AvifEncoding};
pub use zencodec_types::PixelData;

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
pub fn decode(data: &[u8]) -> Result<PixelData> {
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
) -> Result<PixelData> {
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
/// Supports Rgb8, Rgba8, Rgb16, and Rgba16 variants. Returns
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
pub fn encode(image: &PixelData) -> Result<EncodedImage> {
    encode_with(image, &EncoderConfig::default(), &Unstoppable)
}

/// Encode a decoded image to AVIF with custom settings and cancellation
///
/// Supports Rgb8, Rgba8, Rgb16, and Rgba16 variants. Returns
/// [`Error::Unsupported`] for grayscale inputs.
#[cfg(feature = "encode")]
pub fn encode_with(
    image: &PixelData,
    config: &EncoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<EncodedImage> {
    match image {
        PixelData::Rgb8(img) => encode_rgb8(img.as_ref(), config, stop),
        PixelData::Rgba8(img) => encode_rgba8(img.as_ref(), config, stop),
        PixelData::Rgb16(img) => encode_rgb16(img.as_ref(), config, stop),
        PixelData::Rgba16(img) => encode_rgba16(img.as_ref(), config, stop),
        _ => Err(whereat::at(Error::Unsupported(
            "only RGB/RGBA 8/16-bit encoding is supported",
        ))),
    }
}
