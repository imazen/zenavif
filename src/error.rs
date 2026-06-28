//! Error types for zenavif

use enough::StopReason;

/// Error type for zenavif decoding operations
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// AVIF container parsing error
    #[error("AVIF parse error: {0}")]
    Parse(#[from] zenavif_parse::Error),

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

    /// AV1 encode error
    #[error("AV1 encode error: {0}")]
    Encode(String),

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

    /// A resource limit was exceeded (memory, input size, output size, etc.)
    #[error("Resource limit exceeded: {0}")]
    ResourceLimit(String),

    /// Memory allocation failed
    #[error("Out of memory")]
    OutOfMemory,

    /// Operation was cancelled via Stop trait
    #[error("Operation cancelled: {0:?}")]
    Cancelled(StopReason),

    /// Unsupported codec operation
    #[error(transparent)]
    UnsupportedOperation(#[from] zencodec::UnsupportedOperation),
}

impl From<StopReason> for Error {
    fn from(reason: StopReason) -> Self {
        Error::Cancelled(reason)
    }
}

/// Codec-agnostic error taxonomy (zencodec PR #103). Maps every [`Error`]
/// variant to exactly one coarse [`zencodec::ErrorCategory`] so a consumer can
/// route on the category (HTTP status, retry policy, logging) without naming
/// this enum. `zencodec` is a hard dependency of this crate, so the impl is
/// unconditional.
impl zencodec::CategorizedError for Error {
    fn codec_name(&self) -> Option<&'static str> {
        Some("zenavif")
    }

    fn category(&self) -> zencodec::ErrorCategory {
        use zencodec::ErrorCategory as C;
        use zencodec::LimitKind as L;
        match self {
            // Delegate to the container parser's own taxonomy — the whole point
            // of zenavif-parse adopting `CategorizedError`: a malformed container
            // stays `MalformedImage`, a truncated one `UnexpectedEof`, a parser
            // cap `LimitsExceeded`, etc., without re-classifying here.
            Self::Parse(e) => e.category(),

            // `Decode` is a catch-all the decode pipeline reuses for many kinds
            // of failure: rav1d's opaque `code`/`msg`, decoder setup/flush faults
            // ("failed to create decoder", "failed to flush decoder"), and the
            // crate's own internal invariant checks ("expected 8-bit planes",
            // "monochrome should not reach chroma conversion", grid-stitch buffer
            // failures), alongside a few genuinely-malformed cases (mismatched
            // grid tile count, missing planes). The plurality are
            // internal/invariant faults, and the variant carries no structural
            // signal to split them, so map the whole variant to the conservative
            // `Internal` rather than blame the input.
            Self::Decode { .. } => C::Internal,

            // `yuv` is a foreign crate whose error we cannot `impl
            // CategorizedError` on; a colour-conversion failure here is an
            // internal pipeline fault, not attributable to the input image.
            Self::ColorConversion(_) => C::Internal,

            // Encoder (`ravif`) errors are stringly-typed and span config /
            // limits / internal faults; the wrapping site can't tell which, so
            // map to `Internal`. The detail string is preserved for diagnostics.
            Self::Encode(_) => C::Internal,

            // The format is handled, but uses a feature this codec hasn't built.
            Self::Unsupported(_) => C::UnsupportedImageFeature,

            // A configured image-dimensions cap was hit.
            Self::ImageTooLarge { .. } => C::LimitsExceeded(L::Pixels),

            // A configured resource cap was hit. The variant carries only a
            // `String`, not a structured kind, and is a catch-all over
            // allocation guards / input-size checks / animation sink-writes, so
            // we report a single representative kind — `Memory`, the dominant
            // allocation-guard axis. The precise limit stays in `Display`.
            Self::ResourceLimit(_) => C::LimitsExceeded(L::Memory),

            // Allocation failed (distinct from a configured resource limit).
            Self::OutOfMemory => C::OutOfMemory,

            // Cooperative cancellation / deadline — delegate to the zencodec
            // `StopReason` arm (`Cancelled` vs `TimedOut`).
            Self::Cancelled(reason) => reason.category(),

            // Delegate to the zencodec cause type (`UnsupportedOperation` /
            // `UnsupportedPixelFormat`).
            Self::UnsupportedOperation(op) => op.category(),
        }
    }
}

/// Bridge a bare native [`Error`] into the shared envelope as
/// `At<zencodec::CodecError>` — the Pattern-B (envelope) error type the zencodec
/// trait impls return. Used at the trait boundary for errors constructed *in
/// place* (e.g. `Encoder::reject`, `AnimationFrameDecoder::wrap_sink_error`, the
/// `push_decoder` sink-error wrap): `.into()` starts the location trace and wraps
/// the categorized value in one step. An *already-located* `At<Error>` is
/// converted with [`zencodec::CodecError::of`] instead (it keeps the existing
/// trace; `From<At<Error>> for At<CodecError>` is impossible under the orphan
/// rule). `#[track_caller]` records the call site as the trace origin, mirroring
/// `at!`. The codec name (`"zenavif"`) and category come from this type's
/// [`zencodec::CategorizedError`] impl, so the envelope is fully populated.
impl From<Error> for whereat::At<zencodec::CodecError> {
    #[track_caller]
    fn from(e: Error) -> Self {
        use whereat::ErrorAtExt;
        zencodec::CodecError::of(e.start_at())
    }
}

/// Result type for zenavif operations with location tracking
pub type Result<T, E = whereat::At<Error>> = core::result::Result<T, E>;

#[cfg(test)]
mod error_category_tests {
    use super::Error;
    use zencodec::{CategorizedError, ErrorCategory as C, LimitKind as L};

    #[test]
    fn error_category_mapping() {
        assert_eq!(Error::Unsupported("x").codec_name(), Some("zenavif"));

        // Parse errors delegate to zenavif-parse's CategorizedError (PR #17):
        // a malformed container stays MalformedImage, a truncated one
        // UnexpectedEof, a parser cap LimitsExceeded.
        assert_eq!(
            Error::Parse(zenavif_parse::Error::InvalidData("bad box")).category(),
            C::MalformedImage
        );
        assert_eq!(
            Error::Parse(zenavif_parse::Error::UnexpectedEOF).category(),
            C::UnexpectedEof
        );
        assert_eq!(
            Error::Parse(zenavif_parse::Error::ResourceLimitExceeded("megapixels")).category(),
            C::LimitsExceeded(L::Pixels)
        );

        // Opaque decoder / color-conversion / encoder faults -> Internal.
        assert_eq!(
            Error::Decode {
                code: -1,
                msg: "failed to decode"
            }
            .category(),
            C::Internal
        );
        assert_eq!(Error::Encode("ravif failed".into()).category(), C::Internal);

        // Format handled, feature not built.
        assert_eq!(
            Error::Unsupported("monochrome alpha").category(),
            C::UnsupportedImageFeature
        );

        // Limits.
        assert_eq!(
            Error::ImageTooLarge {
                width: 99999,
                height: 99999
            }
            .category(),
            C::LimitsExceeded(L::Pixels)
        );
        assert_eq!(
            Error::ResourceLimit("peak memory".into()).category(),
            C::LimitsExceeded(L::Memory)
        );

        // Allocation.
        assert_eq!(Error::OutOfMemory.category(), C::OutOfMemory);

        // Cancellation / deadline delegate to StopReason.
        assert_eq!(
            Error::Cancelled(enough::StopReason::Cancelled).category(),
            C::Cancelled
        );
        assert_eq!(
            Error::Cancelled(enough::StopReason::TimedOut).category(),
            C::TimedOut
        );

        // UnsupportedOperation delegates to the zencodec cause type.
        assert_eq!(
            Error::UnsupportedOperation(zencodec::UnsupportedOperation::AnimationEncode).category(),
            C::UnsupportedOperation
        );

        // The blanket `impl CategorizedError for At<E>` forwards both axes.
        let at_err = whereat::At::<Error>::from(Error::Unsupported("x"));
        assert_eq!(at_err.category(), C::UnsupportedImageFeature);
        assert_eq!(at_err.codec_name(), Some("zenavif"));
    }
}
