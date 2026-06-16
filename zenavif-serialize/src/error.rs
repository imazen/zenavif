//! Error type for AVIF serialization.

use std::fmt;
use std::io;

/// An error produced while muxing AV1 data into an AVIF container.
///
/// Returned wrapped in [`whereat::At`] (see the crate [`Result`] alias) so it
/// carries the source location where it originated — useful for server-side
/// logs. Get the inner error with [`At::error()`](whereat::At::error) (borrow)
/// or [`At::decompose().0`](whereat::At::decompose) (owned).
#[derive(Debug)]
#[non_exhaustive]
pub enum SerializeError {
    /// Invalid muxing parameters — e.g. an unsupported bit depth, or color and
    /// alpha dimensions that don't match. These are caller-supplied-argument
    /// errors, not malformed untrusted input.
    InvalidInput(&'static str),
    /// The output sink (`io::Write`) failed while writing the container.
    Io(io::Error),
    /// Allocation failed (the `Vec` writer backend ran out of capacity).
    Oom,
}

impl fmt::Display for SerializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(msg) => write!(f, "invalid AVIF muxing input: {msg}"),
            Self::Io(e) => write!(f, "AVIF write failed: {e}"),
            Self::Oom => f.write_str("AVIF serialization ran out of memory"),
        }
    }
}

impl std::error::Error for SerializeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for SerializeError {
    #[inline]
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<crate::writer::OOM> for SerializeError {
    #[inline]
    fn from(_: crate::writer::OOM) -> Self {
        Self::Oom
    }
}

/// A `Result` whose error is a [`SerializeError`] wrapped in [`whereat::At`],
/// so it records the `file:line` where the error originated.
pub type Result<T> = core::result::Result<T, whereat::At<SerializeError>>;
