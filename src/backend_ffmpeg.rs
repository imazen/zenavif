//! ffmpeg-shellout backend for the AV1 decode perf spike.
//!
//! Spawns `ffmpeg -hwaccel <method>` to decode AVIF to raw RGBA. Used to
//! measure HW-accelerated AV1 decode (VA-API / D3D11VA / DXVA2 / CUDA)
//! against the in-process rav1d-safe decoder before investing in native
//! libva / Media Foundation FFI.
//!
//! ## What this measures
//!
//! Each call:
//! 1. Probes the AVIF container (zenavif-parse) for dimensions — cheap.
//! 2. Spawns `ffmpeg ... -benchmark` writing rawvideo (rgba) to stdout.
//! 3. Reads pixel bytes + stderr in parallel.
//! 4. Parses `bench: utime=...s rtime=...s` from ffmpeg's stderr for the
//!    "library-native" decode time (subprocess startup excluded).
//!
//! Reports two numbers per decode:
//! - **total_ms** — Rust-side wall clock around the whole call.
//! - **bench_rtime_ms** — ffmpeg's internal real time (excludes subprocess startup).
//!
//! The gap between them is the subprocess-spawn overhead users would
//! avoid by linking the HW decoder directly. Useful to estimate
//! whether to invest in native FFI: if `bench_rtime_ms` already loses
//! to rav1d-safe, the HW path is dead in the water.

#![cfg_attr(not(feature = "backend-ffmpeg"), allow(dead_code))]

use crate::backend::{Av1DecoderBackend, BackendError};
use crate::config::DecoderConfig;
use enough::Stop;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;
use zenpixels::{PixelBuffer, PixelDescriptor};

/// Last-call timing breakdown produced by the ffmpeg backend.
///
/// Filled by `decode()` so the bench harness can pull it out via
/// `last_timing()` after a successful decode.
#[derive(Debug, Clone, Copy, Default)]
pub struct FfmpegTiming {
    /// Wall-clock from spawn to readback completion.
    pub total: Duration,
    /// `bench: rtime=...` parsed from ffmpeg stderr. `None` if not
    /// emitted (e.g. ffmpeg < 4.0 or the option silently dropped).
    pub bench_rtime: Option<Duration>,
}

/// Backend that shells out to `ffmpeg` for HW (or CPU) AV1 decode.
pub struct FfmpegBackend {
    /// Value passed to `-hwaccel` (e.g. `"vaapi"`, `"d3d11va"`,
    /// `"dxva2"`, `"cuda"`, `"none"` for CPU).
    hwaccel: &'static str,
    last_timing: FfmpegTiming,
    /// Cached availability: `None` = not yet probed.
    available: Option<bool>,
}

impl FfmpegBackend {
    /// Construct a backend pinned to one hwaccel method.
    #[must_use]
    pub fn new(hwaccel: &'static str) -> Self {
        Self {
            hwaccel,
            last_timing: FfmpegTiming::default(),
            available: None,
        }
    }

    /// Timing from the most recent successful `decode()` call.
    #[must_use]
    pub fn last_timing(&self) -> FfmpegTiming {
        self.last_timing
    }

    /// Probe `ffmpeg -hwaccels` for this method. Cached after first call.
    fn probe_available(&mut self) -> bool {
        if let Some(v) = self.available {
            return v;
        }
        let v = ffmpeg_supports_hwaccel(self.hwaccel);
        self.available = Some(v);
        v
    }
}

impl Av1DecoderBackend for FfmpegBackend {
    fn name(&self) -> &'static str {
        self.hwaccel
    }

    fn is_available(&self) -> bool {
        // Method-on-trait can't &mut self; do a fresh probe. Bench harness
        // uses the concrete type so it gets the cached path.
        ffmpeg_supports_hwaccel(self.hwaccel)
    }

    fn decode(
        &mut self,
        avif_data: &[u8],
        _config: &DecoderConfig,
        stop: &dyn Stop,
    ) -> Result<PixelBuffer, BackendError> {
        if !self.probe_available() {
            return Err(BackendError::Unavailable(format!(
                "ffmpeg does not advertise hwaccel '{}'",
                self.hwaccel
            )));
        }
        stop.check().map_err(|_| BackendError::Cancelled)?;

        // Parse the AVIF container ourselves and pipe the raw AV1 OBU
        // payload to ffmpeg's `-f obu` demuxer. This is portable across
        // ffmpeg versions that don't ship the `avif` demuxer (Ubuntu
        // 22's ffmpeg 4.4 lacks it) and measures the actual AV1 decode
        // cost — container parsing happens in zenavif-parse, same as
        // the rust path.
        let (width, height, av1_obu) = extract_av1_obu(avif_data)
            .map_err(|m| BackendError::Decode(format!("avif → obu extract: {m}")))?;

        // Build ffmpeg command.
        // - `-hide_banner -loglevel info` keeps stderr small.
        // - `-f obu -i -` reads raw AV1 OBU bitstream from stdin.
        // - `-hwaccel <method>` (skipped when method == "none").
        // - `-an -sn -map 0:v:0` decode just the video track.
        // - `-f rawvideo -pix_fmt rgba -` write raw RGBA to stdout.
        // - `-benchmark` so we can parse internal rtime from stderr.
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "info"]);
        if self.hwaccel != "none" {
            cmd.args(["-hwaccel", self.hwaccel]);
        }
        cmd.args([
            "-f",
            "obu",
            "-i",
            "-",
            "-an",
            "-sn",
            "-map",
            "0:v:0",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-benchmark",
            "-",
        ]);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let started = std::time::Instant::now();
        let mut child = cmd
            .spawn()
            .map_err(|e| BackendError::Decode(format!("ffmpeg spawn: {e}")))?;

        // Write OBU payload to stdin in a thread so we don't deadlock
        // against a full pipe; ffmpeg may read lazily.
        let mut stdin = child.stdin.take().expect("piped stdin");
        let avif_owned = av1_obu;
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(&avif_owned);
            drop(stdin);
        });

        // Read stdout (raw RGBA) and stderr in parallel.
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");

        let stdout_thread = std::thread::spawn(move || {
            let mut buf = Vec::with_capacity(1 << 20);
            let _ = stdout.read_to_end(&mut buf);
            buf
        });
        let stderr_thread = std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = stderr.read_to_string(&mut buf);
            buf
        });

        let status = child
            .wait()
            .map_err(|e| BackendError::Decode(format!("ffmpeg wait: {e}")))?;
        let _ = writer.join();
        let pixels = stdout_thread.join().unwrap_or_default();
        let stderr_text = stderr_thread.join().unwrap_or_default();
        let total = started.elapsed();

        if !status.success() {
            return Err(BackendError::Decode(format!(
                "ffmpeg exit {:?}; stderr tail:\n{}",
                status.code(),
                tail(&stderr_text, 600)
            )));
        }

        let expected = (width as usize) * (height as usize) * 4;
        if pixels.len() != expected {
            return Err(BackendError::Decode(format!(
                "rawvideo length mismatch: got {}, expected {} ({}x{} rgba); stderr tail:\n{}",
                pixels.len(),
                expected,
                width,
                height,
                tail(&stderr_text, 400)
            )));
        }

        self.last_timing = FfmpegTiming {
            total,
            bench_rtime: parse_bench_rtime(&stderr_text),
        };

        // Allocate an untyped RGBA8 PixelBuffer and memcpy the raw
        // rgba rows in. aligned_stride(width) == width * 4 for RGBA8,
        // so a single bulk copy works.
        let mut buf = PixelBuffer::new(width, height, PixelDescriptor::RGBA8);
        buf.rows_mut(0, height)
            .as_strided_bytes_mut()
            .copy_from_slice(&pixels);
        Ok(buf)
    }
}

/// Parse the AVIF container with zenavif-parse and return
/// `(width, height, av1_obu_payload)` for the primary item.
///
/// `av1_obu_payload` is the raw AV1 OBU bytestream as stored in the
/// AVIF container (low-overhead OBU form, no Annex-B start codes) —
/// suitable for ffmpeg's `-f obu` demuxer.
///
/// Grid items return the merged-output dimensions; the OBU payload is
/// the first tile only (good enough for the spike — the perf
/// hypothesis is about per-image decode cost, and most test vectors
/// are non-grid).
fn extract_av1_obu(avif_data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let parser = zenavif_parse::AvifParser::from_owned_with_config(
        avif_data.to_vec(),
        &zenavif_parse::DecodeConfig::default().lenient(true),
        &enough::Unstoppable,
    )
    .map_err(|e| format!("{e}"))?;

    let primary_data = parser
        .primary_data()
        .map_err(|e| format!("primary_data: {e}"))?;
    // AVIF stores AV1 in low-overhead OBU form without a Temporal
    // Delimiter. ffmpeg's `obu` demuxer requires a TD to start each
    // frame, so inject one (OBU_TEMPORAL_DELIMITER, has_size=1,
    // size=0 → bytes 0x12 0x00).
    let mut obu = Vec::with_capacity(primary_data.len() + 2);
    obu.extend_from_slice(&[0x12, 0x00]);
    obu.extend_from_slice(&primary_data);

    let (w, h) = if let Some(grid) = parser.grid_config() {
        (grid.output_width, grid.output_height)
    } else {
        let meta = parser
            .primary_metadata()
            .map_err(|e| format!("primary_metadata: {e}"))?;
        (meta.max_frame_width.get(), meta.max_frame_height.get())
    };
    Ok((w, h, obu))
}

/// Parse `bench: utime=0.123s stime=0.045s rtime=0.234s` from ffmpeg
/// stderr. Returns the rtime value as Duration.
fn parse_bench_rtime(stderr: &str) -> Option<Duration> {
    // Expect a line like: `bench: utime=0.012s stime=0.004s rtime=0.018s`
    for line in stderr.lines() {
        let line = line.trim_start();
        if !line.starts_with("bench:") {
            continue;
        }
        if let Some(rest) = line.split_once("rtime=") {
            let v = rest.1.trim_start_matches('=');
            let v = v.split('s').next().unwrap_or("");
            if let Ok(secs) = v.parse::<f64>() {
                return Some(Duration::from_secs_f64(secs));
            }
        }
    }
    None
}

/// Check `ffmpeg -hwaccels` for the requested method. Returns true if
/// method is `"none"` (CPU path is always available when ffmpeg exists).
fn ffmpeg_supports_hwaccel(method: &str) -> bool {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-hwaccels"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else { return false };
    if !out.status.success() {
        return false;
    }
    if method == "none" {
        return true;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().any(|l| l.trim() == method)
}

fn tail(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let start = s.len() - n;
    // step forward to a char boundary
    let mut i = start;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    &s[i..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bench_rtime_extracts_value() {
        let s = "Some other line\nbench: utime=0.012s stime=0.004s rtime=0.018s\nfooter";
        let v = parse_bench_rtime(s).expect("should parse");
        assert!((v.as_secs_f64() - 0.018).abs() < 1e-6);
    }

    #[test]
    fn parse_bench_rtime_missing_returns_none() {
        assert!(parse_bench_rtime("no bench line here").is_none());
    }
}
