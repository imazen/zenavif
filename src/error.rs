//! Error types for zenavif

use enough::StopReason;

/// Error type for zenavif decoding operations
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// AVIF container parsing error
    #[error("AVIF parse error: {0}")]
    Parse(#[from] avif_parse::Error),

    /// AV1 decode error from rav1d
    #[error("AV1 decode error {code}: {msg}")]
    Decode {
        /// rav1d error code
        code: i32,
        /// Error description
        msg: &'static str,
    },

    /// YUV to RGB color conversion error
    #[error("Color conversion error: {0}")]
    ColorConversion(#[from] yuv::YuvError),

    /// Unsupported feature
    #[error("Unsupported: {0}")]
    Unsupported(&'static str),

    /// Image dimensions exceed configured limit
    #[error("Image too large: {width}x{height}")]
    ImageTooLarge {
        /// Image width
        width: u32,
        /// Image height
        height: u32,
    },

    /// Memory allocation failed
    #[error("Out of memory")]
    OutOfMemory,

    /// Operation was cancelled via Stop trait
    #[error("Operation cancelled: {0:?}")]
    Cancelled(StopReason),
}

impl From<StopReason> for Error {
    fn from(reason: StopReason) -> Self {
        Error::Cancelled(reason)
    }
}

/// Result type for zenavif operations with location tracking
pub type Result<T, E = whereat::At<Error>> = core::result::Result<T, E>;
