//! # zenavif
//!
//! Pure Rust AVIF image decoder powered by [rav1d](https://github.com/memorysafety/rav1d).
//!
//! This crate provides a safe, ergonomic API for decoding AVIF images using
//! the pure Rust rav1d AV1 decoder and avif-parse container parser.
//!
//! ## Quick Start
//!
//! ```no_run
//! use zenavif::{decode, DecodedImage};
//!
//! let avif_data = std::fs::read("image.avif").unwrap();
//! let image = decode(&avif_data).unwrap();
//!
//! match image {
//!     DecodedImage::Rgb8(img) => {
//!         println!("RGB8 image: {}x{}", img.width(), img.height());
//!     }
//!     DecodedImage::Rgba8(img) => {
//!         println!("RGBA8 image: {}x{}", img.width(), img.height());
//!     }
//!     _ => {}
//! }
//! ```
//!
//! ## Features
//!
//! - Pure Rust implementation (via rav1d)
//! - 8-bit and 10/12-bit depth support
//! - Alpha channel support with premultiplied alpha handling
//! - Film grain synthesis
//! - Cooperative cancellation via `enough::Stop`
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

mod chroma;
mod config;
mod convert;
mod decoder;
mod error;
mod image;

pub use config::DecoderConfig;
pub use decoder::AvifDecoder;
pub use enough::{Stop, StopReason, Unstoppable};
pub use error::{Error, Result};
pub use image::{
    ChromaSampling, ColorPrimaries, ColorRange, DecodedImage, ImageInfo, MatrixCoefficients,
    TransferCharacteristics,
};

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
pub fn decode(data: &[u8]) -> Result<DecodedImage> {
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
pub fn decode_with(data: &[u8], config: &DecoderConfig, stop: &impl Stop) -> Result<DecodedImage> {
    let mut decoder = AvifDecoder::new(data, config)?;
    decoder.decode(stop)
}
