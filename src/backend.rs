//! AV1 decode backend dispatch — pluggable Rust / native-HW backends.
//!
//! Modeled on `heic/src/backend.rs`. Users pick an ordered allowlist with
//! [`DecoderConfig::with_backends`]; the dispatcher tries each in order
//! and falls through on recoverable errors.
//!
//! ## Spike status (2026-05-23)
//!
//! Experimental scaffold to measure HW vs SW perf for still AVIF decode.
//! Native variants currently shell out to `ffmpeg` with `-hwaccel vaapi`
//! / `-hwaccel d3d11va` etc.; replacement with native `libva` / Media
//! Foundation FFI lands after perf data confirms HW is worth the effort.
//!
//! ## Allowlist semantics
//!
//! - Empty allowlist → `Error::Backend("no backend selected")`.
//! - Variants whose feature isn't compiled in are silently skipped.
//! - Recoverable errors ([`BackendError::Unavailable`] / [`BackendError::Decode`])
//!   fall through. Terminal errors ([`BackendError::LimitsExceeded`] /
//!   [`BackendError::Cancelled`]) short-circuit.

#![cfg_attr(not(feature = "backend-ffmpeg"), allow(dead_code))]

use crate::config::DecoderConfig;
use crate::error::{Error, Result};
use enough::Stop;
use whereat::at;
use zenpixels::PixelBuffer;

/// AV1 decode backend.
///
/// Native variants are gated by `feature + target_os` — only enabled
/// variants are constructible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecodeBackend {
    /// Pure-Rust rav1d-safe (always available).
    Rust,
    /// VA-API AV1 decode via libva (Linux). Currently routed through
    /// `ffmpeg -hwaccel vaapi` for the spike.
    #[cfg(all(feature = "backend-ffmpeg", target_os = "linux"))]
    Vaapi,
    /// Direct3D 11 Video Acceleration (Windows). Currently routed
    /// through `ffmpeg -hwaccel d3d11va`.
    #[cfg(all(feature = "backend-ffmpeg", target_os = "windows"))]
    D3d11va,
    /// DXVA2 fallback (Windows, older). Currently routed through
    /// `ffmpeg -hwaccel dxva2`.
    #[cfg(all(feature = "backend-ffmpeg", target_os = "windows"))]
    Dxva2,
    /// CUDA NVDEC AV1 decode (NVIDIA only). Routed through
    /// `ffmpeg -hwaccel cuda`.
    #[cfg(feature = "backend-ffmpeg")]
    Cuda,
    /// ffmpeg CPU path (libdav1d via ffmpeg). Useful as a third
    /// data point against rav1d-safe + HW.
    #[cfg(feature = "backend-ffmpeg")]
    FfmpegCpu,
}

impl DecodeBackend {
    /// Stable name used in logs and TSV columns.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            #[cfg(all(feature = "backend-ffmpeg", target_os = "linux"))]
            Self::Vaapi => "vaapi",
            #[cfg(all(feature = "backend-ffmpeg", target_os = "windows"))]
            Self::D3d11va => "d3d11va",
            #[cfg(all(feature = "backend-ffmpeg", target_os = "windows"))]
            Self::Dxva2 => "dxva2",
            #[cfg(feature = "backend-ffmpeg")]
            Self::Cuda => "cuda",
            #[cfg(feature = "backend-ffmpeg")]
            Self::FfmpegCpu => "ffmpeg-cpu",
        }
    }
}

/// Errors a backend can return from [`Av1DecoderBackend::decode`].
#[derive(Debug)]
#[non_exhaustive]
pub enum BackendError {
    /// Backend not available right now (driver/device/store package missing).
    Unavailable(String),
    /// Backend was reached but rejected the bitstream / container.
    Decode(String),
    /// Configured resource limit exceeded — terminal.
    LimitsExceeded(&'static str),
    /// Stop token fired — terminal.
    Cancelled,
}

impl core::fmt::Display for BackendError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "backend unavailable: {m}"),
            Self::Decode(m) => write!(f, "decode failed: {m}"),
            Self::LimitsExceeded(m) => write!(f, "limit exceeded: {m}"),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for BackendError {}

/// AV1-on-AVIF backend implementation.
///
/// The trait is intentionally coarse for spike simplicity (input = full
/// AVIF file blob; output = `PixelBuffer`). A future revision will split
/// into (parse → av1C extract) + (decode-tile-to-YUV) once perf data
/// justifies that complexity.
pub trait Av1DecoderBackend: Send {
    /// Stable identifier (`"rust"`, `"vaapi"`, `"d3d11va"`, …).
    fn name(&self) -> &'static str;

    /// True if the backend can actually run on this host right now.
    fn is_available(&self) -> bool;

    /// Decode an AVIF file blob to a `PixelBuffer`.
    fn decode(
        &mut self,
        avif_data: &[u8],
        config: &DecoderConfig,
        stop: &dyn Stop,
    ) -> core::result::Result<PixelBuffer, BackendError>;
}

impl DecodeBackend {
    /// Construct a backend instance.
    #[must_use]
    pub fn instance(self) -> Box<dyn Av1DecoderBackend> {
        match self {
            Self::Rust => Box::new(RustBackend),
            #[cfg(all(feature = "backend-ffmpeg", target_os = "linux"))]
            Self::Vaapi => Box::new(crate::backend_ffmpeg::FfmpegBackend::new("vaapi")),
            #[cfg(all(feature = "backend-ffmpeg", target_os = "windows"))]
            Self::D3d11va => Box::new(crate::backend_ffmpeg::FfmpegBackend::new("d3d11va")),
            #[cfg(all(feature = "backend-ffmpeg", target_os = "windows"))]
            Self::Dxva2 => Box::new(crate::backend_ffmpeg::FfmpegBackend::new("dxva2")),
            #[cfg(feature = "backend-ffmpeg")]
            Self::Cuda => Box::new(crate::backend_ffmpeg::FfmpegBackend::new("cuda")),
            #[cfg(feature = "backend-ffmpeg")]
            Self::FfmpegCpu => Box::new(crate::backend_ffmpeg::FfmpegBackend::new("none")),
        }
    }
}

/// Recommended default allowlist for the current build & target.
///
/// Order: native HW first (when feature + target_os matches), then
/// [`DecodeBackend::Rust`] as the always-available fallback.
#[must_use]
pub fn recommended_backends() -> Vec<DecodeBackend> {
    let mut out: Vec<DecodeBackend> = Vec::new();
    #[cfg(all(feature = "backend-ffmpeg", target_os = "linux"))]
    out.push(DecodeBackend::Vaapi);
    #[cfg(all(feature = "backend-ffmpeg", target_os = "windows"))]
    {
        out.push(DecodeBackend::D3d11va);
        out.push(DecodeBackend::Dxva2);
    }
    #[cfg(feature = "backend-ffmpeg")]
    out.push(DecodeBackend::Cuda);
    out.push(DecodeBackend::Rust);
    out
}

/// Dispatch a decode through an ordered allowlist.
pub fn decode_with_backends(
    backends: &[DecodeBackend],
    avif_data: &[u8],
    config: &DecoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<PixelBuffer> {
    if backends.is_empty() {
        return Err(at!(Error::Backend("no backend selected".into())));
    }

    let mut last_err = String::new();
    for &b in backends {
        // Fast path: skip Box dispatch for the Rust backend.
        if b == DecodeBackend::Rust {
            return crate::decode_with(avif_data, config, stop);
        }
        let mut inst = b.instance();
        if !inst.is_available() {
            last_err = format!("{}: unavailable", b.name());
            continue;
        }
        match inst.decode(avif_data, config, &stop) {
            Ok(pixels) => return Ok(pixels),
            Err(BackendError::LimitsExceeded(_)) => {
                return Err(at!(Error::ImageTooLarge {
                    width: 0,
                    height: 0
                }));
            }
            Err(BackendError::Cancelled) => {
                return Err(at!(Error::Cancelled(enough::StopReason::Cancelled)));
            }
            Err(BackendError::Unavailable(m) | BackendError::Decode(m)) => {
                last_err = format!("{}: {m}", b.name());
            }
        }
    }
    Err(at!(Error::Backend(format!(
        "all backends failed: {last_err}"
    ))))
}

/// Pure-Rust backend wrapping `crate::decode_with`.
pub struct RustBackend;

impl Av1DecoderBackend for RustBackend {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn decode(
        &mut self,
        avif_data: &[u8],
        config: &DecoderConfig,
        stop: &dyn Stop,
    ) -> core::result::Result<PixelBuffer, BackendError> {
        crate::decode_with(avif_data, config, stop).map_err(|e| match e.error() {
            Error::Cancelled(_) => BackendError::Cancelled,
            Error::ImageTooLarge { .. } => {
                BackendError::LimitsExceeded("image dimensions exceed configured limit")
            }
            _ => BackendError::Decode(format!("{e}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_backend_name() {
        assert_eq!(DecodeBackend::Rust.name(), "rust");
    }

    #[test]
    fn recommended_includes_rust() {
        let order = recommended_backends();
        assert!(order.contains(&DecodeBackend::Rust));
    }

    #[test]
    fn empty_allowlist_errors() {
        let cfg = DecoderConfig::default();
        let res = decode_with_backends(&[], b"not-an-avif", &cfg, &enough::Unstoppable);
        assert!(res.is_err());
    }
}
