//! Error types for zenavif

use enough::StopReason;

/// Error type for zenavif decoding operations
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// AVIF container parsing error
    #[error("AVIF parse error: {0}")]
    Parse(#[from] zenavif_parse::Error),

    /// The bytes are invalid or internally inconsistent — a corrupt
    /// bitstream, or a decoded frame whose actual structure (planes,
    /// format) doesn't match what its own signaled matrix/chroma mode or
    /// grid metadata requires (e.g. a grid box whose declared tile count
    /// doesn't match the tiles actually present).
    #[error("Malformed AVIF: {0}")]
    Malformed(&'static str),

    /// Input ended before a complete image could be read — truncated or
    /// insufficient data, including a degenerate empty input.
    #[error("Unexpected end of input: {0}")]
    UnexpectedEof(&'static str),

    /// AV1 decode error from rav1d — an opaque failure in zenavif's own
    /// decode pipeline (a post-decode expectation about planes, bit
    /// depth, grid config, or buffer descriptor that didn't hold, or a
    /// `rav1d-safe` setup/init failure) not attributable to the input
    /// bitstream. Genuinely input-caused decode failures are
    /// [`Malformed`](Self::Malformed) / [`UnexpectedEof`](Self::UnexpectedEof)
    /// instead — see [`error_from_rav1d`].
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

    /// AV1 encode error — an opaque failure in the encode pipeline (a
    /// foreign encoder/metric dependency such as `ravif`/`butteraugli`/
    /// `fast_ssim2`/`zensim`, or zenavif's own encode-side assumption) not
    /// attributable to caller-supplied parameters, buffer, or call
    /// sequence. Those are [`InvalidParameters`](Self::InvalidParameters) /
    /// [`InvalidBuffer`](Self::InvalidBuffer) /
    /// [`InvalidState`](Self::InvalidState) instead.
    #[error("AV1 encode error: {0}")]
    Encode(String),

    /// A caller-supplied output sink ([`zencodec::decode::DecodeRowSink`]'s
    /// `begin`/`provide_next_buffer`/`finish`) failed while streaming
    /// decoded rows. `SinkError` is an opaque `Box<dyn Error>` from the
    /// caller's own sink implementation, so only the message survives.
    #[error("Sink I/O error: {0}")]
    Io(String),

    /// The bytes are a well-formed image using a feature this codec
    /// doesn't (or doesn't yet) implement — e.g. a signaled
    /// matrix-coefficients/chroma-subsampling combination with no defined
    /// reconstruction, an unimplemented bit depth, or a `ReconstructHdr`
    /// request against an image shape (alpha, bit depth) the gain-map
    /// apply path doesn't yet cover.
    #[error("Unsupported: {0}")]
    Unsupported(&'static str),

    /// Caller-supplied configuration or parameters were invalid — not the
    /// image's fault (bad knobs/quality/target-quality search range, an
    /// unrecognized enum value such as a future `GainMapRender` mode, or
    /// an API entry point invoked against input it doesn't apply to, e.g.
    /// `AnimationDecoder` on a non-animated file).
    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),

    /// A caller-supplied pixel buffer has an invalid layout for the
    /// operation — wrong size, stride, or pixel format/descriptor.
    #[error("Invalid buffer: {0}")]
    InvalidBuffer(String),

    /// The operation was invoked in an invalid state or out of sequence —
    /// e.g. finishing an animation encode with no frames pushed, or a
    /// later frame whose dimensions/pixel-format are inconsistent with
    /// the first.
    #[error("Invalid state: {0}")]
    InvalidState(String),

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

/// Classify a `rav1d-safe` managed-API decode failure into the right
/// zenavif [`Error`], instead of discarding it into a blanket `Internal`
/// bucket the way the pre-reshape code did (`.map_err(|_e| ...)`).
/// `context` supplies the message for the cases with no more specific
/// signal (`InvalidSettings`/`InitFailed`/`Other`).
///
/// `rav1d_safe::src::managed::Error::OutOfMemory` routes to zenavif's own
/// dedicated [`Error::OutOfMemory`] variant rather than staying
/// `Decode`-shaped, since `Resource` is a better fit than "the decoder
/// itself failed". The published `rav1d-safe` (0.5.x on crates.io) has no
/// cooperative-cancellation variant on this enum — zenavif's own explicit
/// `stop.check()` calls around each decode step are the cancellation path
/// today; a future rav1d-safe release adding one should gain its own arm
/// here routing to `Error::Cancelled`.
pub(crate) fn error_from_rav1d(e: rav1d_safe::src::managed::Error, context: &'static str) -> Error {
    use rav1d_safe::src::managed::Error as R;
    match e {
        R::InvalidData => Error::Malformed(context),
        R::NeedMoreData => Error::UnexpectedEof(context),
        R::OutOfMemory => Error::OutOfMemory,
        // `InvalidSettings`/`InitFailed`/`Other` are opaque rav1d-safe setup
        // faults (the settings here are zenavif's own, not caller-facing) —
        // not attributable to the input bitstream, so they stay the
        // "decoder itself failed" `Decode` bucket.
        R::InvalidSettings(_) | R::InitFailed | R::Other(_) => Error::Decode {
            code: -1,
            msg: context,
        },
    }
}

/// Codec-agnostic error taxonomy (zencodec's origin-first, two-level
/// `ErrorCategory` — PR #116, `caterr-reshape`). Maps every [`Error`]
/// variant to exactly one coarse [`zencodec::ErrorCategory`] so a consumer
/// can route on the category (HTTP status, retry policy, logging) without
/// naming this enum. `zencodec` is a hard dependency of this crate, so the
/// impl is unconditional.
impl zencodec::CategorizedError for Error {
    fn codec_name(&self) -> Option<&'static str> {
        Some("zenavif")
    }

    fn category(&self) -> zencodec::ErrorCategory {
        use zencodec::ErrorCategory as C;
        use zencodec::ImageError as Img;
        use zencodec::InternalKind as Internal;
        use zencodec::InvalidKind as Invalid;
        use zencodec::LimitKind as L;
        use zencodec::RequestError as Req;
        use zencodec::ResourceError as Res;
        use zencodec::UnsupportedImageKind as UImg;
        match self {
            // Delegate to the container parser's own taxonomy — the whole
            // point of zenavif-parse adopting `CategorizedError`: a
            // malformed container stays `Malformed`, a truncated one
            // `UnexpectedEof`, a parser cap `Resource(Limits(_))`, etc.,
            // without re-classifying here.
            Self::Parse(e) => e.category(),

            // The AV1 bitstream itself is invalid — or a decoded frame's
            // structure disagrees with its own signaled format (a grid
            // whose tile count doesn't match its declared dimensions, a
            // signaled Identity/GBR matrix missing a required plane).
            Self::Malformed(_) => C::Image(Img::Malformed),

            // Truncated / insufficient input (including a degenerate
            // empty buffer).
            Self::UnexpectedEof(_) => C::Image(Img::UnexpectedEof),

            // `Decode` is now reserved for opaque decode-pipeline failures
            // not attributable to the input: zenavif's own post-decode
            // invariant checks (plane bit depth, grid config, buffer
            // descriptor assumptions) and rav1d-safe setup/init faults
            // (`error_from_rav1d`'s `InvalidSettings`/`InitFailed`/`Other`
            // arm). The dominant case by far is our own invariant, so this
            // maps to `Bug` rather than `Dependency` — the rare rav1d-safe
            // setup-fault sub-case is a deliberate, documented
            // simplification (see `error_from_rav1d`), not a fact that
            // every site here is definitely zenavif's own bug.
            Self::Decode { .. } => C::Internal(Internal::Bug),

            // `yuv` is a foreign crate whose error we cannot `impl
            // CategorizedError` on. Every `ColorConversion` call site in
            // this crate converts AV1-decoded YUV planes into an RGB
            // buffer zenavif itself allocated (from the same dimensions)
            // — never a raw caller-supplied buffer — so a mismatch here
            // reflects an internal inconsistency between our own
            // dimension bookkeeping and the yuv crate's expectations, not
            // something a caller could fix by changing their request.
            // Unclassified further.
            Self::ColorConversion(_) => C::Internal(Internal::Dependency),

            // An opaque encode-pipeline failure: a foreign encoder/metric
            // dependency (`ravif`/`butteraugli`/`fast_ssim2`/`zensim`) or
            // zenavif's own encode-side assumption, neither of which this
            // call site can classify further. Caller-attributable encode
            // faults are `InvalidParameters`/`InvalidBuffer`/`InvalidState`
            // instead (see those variants' construction sites).
            Self::Encode(_) => C::Internal(Internal::Dependency),

            // A caller-supplied `DecodeRowSink` failed. `SinkError` is an
            // opaque `Box<dyn Error>` from the caller's own sink
            // implementation (no `std::io::ErrorKind` to extract), so this
            // is the portable no-`std` `opaque()` form — same choice
            // zenavif-parse makes for its own `Io` arm.
            Self::Io(_) => C::Io(zencodec::CodecIoKind::opaque()),

            // The format is handled, but the image/bitstream (or a
            // caller-requested transform over it) uses a combination this
            // codec hasn't — or hasn't yet — implemented.
            Self::Unsupported(_) => C::Image(Img::Unsupported(UImg::Feature)),

            // Caller-request-origin faults: the request is well-formed at
            // the API level but its content is wrong (config, buffer
            // geometry, or call sequence). The caller can fix these.
            Self::InvalidParameters(_) => C::Request(Req::Invalid(Invalid::Parameters)),
            Self::InvalidBuffer(_) => C::Request(Req::Invalid(Invalid::Buffer)),
            Self::InvalidState(_) => C::Request(Req::Invalid(Invalid::State)),

            // A configured image-dimensions cap was hit.
            Self::ImageTooLarge { .. } => C::Resource(Res::Limits(L::Pixels)),

            // A configured resource cap was hit. The variant carries only
            // a `String`, not a structured kind, and is a catch-all over
            // allocation guards / input-size checks / animation
            // sink-writes, so we report a single representative kind —
            // `Memory`, the dominant allocation-guard axis. The precise
            // limit stays in `Display`.
            Self::ResourceLimit(_) => C::Resource(Res::Limits(L::Memory)),

            // Allocation failed (distinct from a configured resource limit).
            Self::OutOfMemory => C::Resource(Res::OutOfMemory),

            // Cooperative cancellation / deadline.
            Self::Cancelled(reason) => C::Stopped(*reason),

            // Delegate to the zencodec cause type (the operation axis,
            // including `PixelFormat`).
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
    use zencodec::{
        CategorizedError, ErrorCategory as C, ImageError as Img, InternalKind as Internal,
        InvalidKind as Invalid, LimitKind as L, RequestError as Req, ResourceError as Res,
        UnsupportedImageKind as UImg,
    };

    #[test]
    fn error_category_mapping() {
        assert_eq!(Error::Unsupported("x").codec_name(), Some("zenavif"));

        // Parse errors delegate to zenavif-parse's CategorizedError.
        assert_eq!(
            Error::Parse(zenavif_parse::Error::InvalidData("bad box")).category(),
            C::Image(Img::Malformed)
        );
        assert_eq!(
            Error::Parse(zenavif_parse::Error::UnexpectedEOF).category(),
            C::Image(Img::UnexpectedEof)
        );
        assert_eq!(
            Error::Parse(zenavif_parse::Error::ResourceLimitExceeded("megapixels")).category(),
            C::Resource(Res::Limits(L::Pixels))
        );

        // Genuine input-caused decode failures, recovered from the real
        // rav1d-safe cause (or zenavif's own bitstream-consistency checks)
        // instead of being discarded into `Internal`.
        assert_eq!(
            Error::Malformed("grid tile count doesn't match grid dimensions").category(),
            C::Image(Img::Malformed)
        );
        assert_eq!(
            Error::UnexpectedEof("empty AV1 OBU data").category(),
            C::Image(Img::UnexpectedEof)
        );

        // Opaque decoder/encoder/color-conversion pipeline faults not
        // attributable to the input -> Internal, split Bug (our own
        // invariant, the `Decode` bucket) vs Dependency (a foreign
        // library we can't classify further: `Encode`/`ColorConversion`).
        assert_eq!(
            Error::Decode {
                code: -1,
                msg: "expected 8-bit planes"
            }
            .category(),
            C::Internal(Internal::Bug)
        );
        assert_eq!(
            Error::Encode("ravif failed".into()).category(),
            C::Internal(Internal::Dependency)
        );
        assert_eq!(
            Error::ColorConversion(yuv::YuvError::ZeroBaseSize).category(),
            C::Internal(Internal::Dependency)
        );

        // A caller-supplied DecodeRowSink failure routes through the
        // dedicated Io category, not Internal.
        assert_eq!(
            Error::Io("sink write failed".into()).category(),
            C::Io(zencodec::CodecIoKind::opaque())
        );

        // Format handled, feature not (yet) implemented.
        assert_eq!(
            Error::Unsupported("monochrome alpha").category(),
            C::Image(Img::Unsupported(UImg::Feature))
        );

        // Caller-request-origin faults (finding 4: previously absent).
        assert_eq!(
            Error::InvalidParameters("bad quality".into()).category(),
            C::Request(Req::Invalid(Invalid::Parameters))
        );
        assert_eq!(
            Error::InvalidBuffer("alpha size mismatch".into()).category(),
            C::Request(Req::Invalid(Invalid::Buffer))
        );
        assert_eq!(
            Error::InvalidState("no frames to encode".into()).category(),
            C::Request(Req::Invalid(Invalid::State))
        );

        // Limits.
        assert_eq!(
            Error::ImageTooLarge {
                width: 99999,
                height: 99999
            }
            .category(),
            C::Resource(Res::Limits(L::Pixels))
        );
        assert_eq!(
            Error::ResourceLimit("peak memory".into()).category(),
            C::Resource(Res::Limits(L::Memory))
        );

        // Allocation.
        assert_eq!(Error::OutOfMemory.category(), C::Resource(Res::OutOfMemory));

        // Cancellation / deadline delegate to StopReason via Stopped.
        assert_eq!(
            Error::Cancelled(enough::StopReason::Cancelled).category(),
            C::Stopped(enough::StopReason::Cancelled)
        );
        assert_eq!(
            Error::Cancelled(enough::StopReason::TimedOut).category(),
            C::Stopped(enough::StopReason::TimedOut)
        );

        // UnsupportedOperation delegates to the zencodec cause type.
        assert_eq!(
            Error::UnsupportedOperation(zencodec::UnsupportedOperation::AnimationEncode).category(),
            C::Request(Req::Unsupported(
                zencodec::UnsupportedOperation::AnimationEncode
            ))
        );

        // The blanket `impl CategorizedError for At<E>` forwards both axes.
        let at_err = whereat::At::<Error>::from(Error::Unsupported("x"));
        assert_eq!(at_err.category(), C::Image(Img::Unsupported(UImg::Feature)));
        assert_eq!(at_err.codec_name(), Some("zenavif"));
    }

    #[test]
    fn error_from_rav1d_classifies_real_cause() {
        use super::error_from_rav1d;
        use rav1d_safe::src::managed::Error as R;

        assert_eq!(
            error_from_rav1d(R::InvalidData, "decode failed").category(),
            C::Image(Img::Malformed)
        );
        assert_eq!(
            error_from_rav1d(R::NeedMoreData, "decode failed").category(),
            C::Image(Img::UnexpectedEof)
        );
        assert_eq!(
            error_from_rav1d(R::OutOfMemory, "decode failed").category(),
            C::Resource(Res::OutOfMemory)
        );
        assert_eq!(
            error_from_rav1d(R::InitFailed, "decode failed").category(),
            C::Internal(Internal::Bug)
        );
        assert_eq!(
            error_from_rav1d(R::InvalidSettings("bad threads"), "decode failed").category(),
            C::Internal(Internal::Bug)
        );
        assert_eq!(
            error_from_rav1d(R::Other("mystery".into()), "decode failed").category(),
            C::Internal(Internal::Bug)
        );
    }
}
