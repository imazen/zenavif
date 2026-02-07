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
//! - **`managed`** (default): 100% safe managed API - no unsafe code!
//! - **`asm`**: Hand-written assembly (fastest, uses C FFI)
//!
//! The default `managed` feature uses rav1d-safe's managed API which is
//! completely safe Rust with zero unsafe code in the entire decode path.
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
#[cfg(feature = "asm")]
mod decoder;
#[cfg(feature = "managed")]
mod decoder_managed;
mod error;
mod image;
pub mod simd;

pub use config::DecoderConfig;
#[cfg(feature = "asm")]
pub use decoder::AvifDecoder;
#[cfg(feature = "managed")]
pub use decoder_managed::ManagedAvifDecoder;
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
    #[cfg(feature = "managed")]
    {
        let mut decoder = ManagedAvifDecoder::new(data, config)?;
        decoder.decode(stop)
    }
    
    #[cfg(all(not(feature = "managed"), feature = "asm"))]
    {
        let mut decoder = AvifDecoder::new(data, config)?;
        decoder.decode(stop)
    }
    
    #[cfg(not(any(feature = "managed", feature = "asm")))]
    {
        compile_error!("At least one feature must be enabled: managed or asm");
    }
}
