// Copyright (c) Imazen LLC and the JPEG XL Project Authors.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing

//! AVIF encode/decode resource estimation (peak memory + time).
//!
//! Mirrors the zen per-codec estimation pattern (cf. `zenwebp::heuristics`)
//! with separate [`EncodeEstimate`] (min / typical / max peak memory, time,
//! output size) and [`DecodeEstimate`] (peak memory, output size, time).
//!
//! Unlike a CPU-bound transform codec, AVIF's dominant cost is **encode
//! time, which is governed by the AV1 `speed` preset** — far more than by
//! resolution alone. The model therefore carries a per-speed time curve.
//! AVIF is comparatively *light on memory* (the AV1 encoder is tile-bounded:
//! ~24–40 B/px vs hundreds for a full VarDCT), and *very* cheap to decode.
//!
//! ## Model
//!
//! ```text
//! encode_peak = input + ENCODE_FIXED + bytes_per_pixel · pixels
//! encode_time = encode_us_per_px(speed) · pixels   (× alpha/10-bit factors)
//! decode_peak = DECODE_FIXED + DECODE_BPP · pixels
//! decode_time = DECODE_US_PER_PX · pixels
//! ```
//!
//! All times are **single-thread CPU** (the calibration pinned encoder
//! threads to 1; wall ≈ user there). With N encoder threads, wall latency
//! is roughly `time_ms / N` (AV1 tile/row threading parallelizes well); the
//! decoder self-threads, so its wall can already be below `time_ms` on
//! large images.
//!
//! ## Calibration (2026-06-14)
//!
//! Measured marginal working set (`avif_probe` `VmHWM` delta around the
//! codec call — excludes the binary floor + input buffer) plus wall and
//! user/sys CPU (`/proc/self/stat`, threads=1), one process per op, over a
//! 5-class × 256–2048 px × speed 4/6/8/10 × rgb/rgba × 8/10-bit grid.
//! Provenance: `benchmarks/avif_resource_main_2026-06-14.tsv` (mem +
//! time-vs-speed) and `benchmarks/avif_resource_alphadepth_2026-06-14.tsv`
//! (alpha + depth deltas); harness `scripts/avif_resource_calibrate.py`.
//!
//! Measured (8-bit, single-thread, working set is sub-linear in size so the
//! per-pixel numbers are mid-range anchors, not the 12 MP limit):
//!
//! | speed | encode us/px | encode mem B/px |
//! |-------|--------------|-----------------|
//! | 4     | 7.6          | ~38             |
//! | 6     | 2.0          | ~43             |
//! | 8     | 1.2          | ~43             |
//! | 10    | 0.55         | ~46             |
//!
//! Decode: ~18 B/px, ~0.06 us/px (≈ 30× cheaper than even fast encode).
//! Alpha (separate plane): +7 B/px, +30 % encode time. 10-bit: ×1.55 mem,
//! +40 % time. Speeds 1–3 (slower than 4) were not measured — the curve is
//! clamped to the speed-4 value below 4 (a lower bound there).

/// Resource estimate for an AVIF encode. `#[non_exhaustive]` so fields can
/// be added without a breaking change.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct EncodeEstimate {
    /// Best-case peak memory in bytes (simple / low-entropy content).
    pub peak_memory_bytes_min: u64,
    /// Typical (≈ p50) peak memory in bytes for natural content.
    pub peak_memory_bytes: u64,
    /// Conservative upper-bound peak memory in bytes (worst content + margin).
    pub peak_memory_bytes_max: u64,
    /// Rough single-thread encode time in ms. Divide by encoder thread
    /// count for an approximate wall-latency estimate.
    pub time_ms: f32,
    /// Rough estimated output size in bytes (lossy, ~q75).
    pub output_bytes: u64,
}

/// Resource estimate for an AVIF decode. `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct DecodeEstimate {
    /// Typical peak memory in bytes.
    pub peak_memory_bytes: u64,
    /// Decoded pixel-buffer size in bytes.
    pub output_bytes: u64,
    /// Rough decode time in ms (the decoder self-threads on large images,
    /// so real wall can be below this).
    pub time_ms: f32,
}

// ── Calibrated constants (2026-06-14, avif_probe marginal working set) ──

/// Encoder fixed overhead (size-independent): AV1 plans, tables, tiles.
const ENCODE_FIXED_OVERHEAD: u64 = 8 << 20;
/// Typical encoder marginal working set, bytes/pixel, 8-bit (sub-linear in
/// size; ~43 at 1 MP, ~24 at 12 MP — 40 is a mid-range anchor).
const ENCODE_BPP: f64 = 40.0;
/// Extra working set for an alpha plane (encoded as a second AV1 image).
const ENCODE_ALPHA_BPP: f64 = 7.0;
/// 10-bit (16-bit input) inflates the encoder working set ~×1.55.
const ENCODE_DEPTH10_MEM_FACTOR: f64 = 1.55;
/// Alpha adds ~30 % encode time; 10-bit adds ~40 %.
const ENCODE_ALPHA_TIME_FACTOR: f64 = 1.3;
const ENCODE_DEPTH10_TIME_FACTOR: f64 = 1.4;
/// Content-spread multipliers on the working set (zenwebp parity).
const MULT_MIN: f64 = 0.8;
const MULT_MAX: f64 = 1.8;
/// Lossy ~q75 AVIF lands near this fraction of the raw input bytes.
const ENCODE_OUTPUT_RATIO: f64 = 0.06;

const DECODE_FIXED_OVERHEAD: u64 = 6 << 20;
/// Decoder marginal working set, bytes/pixel (sub-linear; ~18 at 1 MP).
const DECODE_BPP: f64 = 15.0;
const DECODE_DEPTH10_MEM_FACTOR: f64 = 1.4;
/// Decoder throughput: ~0.07 us/px (8-bit). 10-bit ~×1.4.
const DECODE_US_PER_PX: f64 = 0.07;
const DECODE_DEPTH10_TIME_FACTOR: f64 = 1.4;

/// Single-thread encode microseconds/pixel for an AV1 `speed` preset (1–10,
/// lower = slower/denser search). Linearly interpolates the measured
/// anchors {4: 7.6, 6: 2.0, 8: 1.2, 10: 0.55}; clamped to [4, 10] (speeds
/// 1–3 unmeasured — the speed-4 value is a lower bound there).
fn encode_us_per_px(speed: u8) -> f64 {
    const ANCHORS: [(f64, f64); 4] = [(4.0, 7.6), (6.0, 2.0), (8.0, 1.2), (10.0, 0.55)];
    let s = (speed as f64).clamp(ANCHORS[0].0, ANCHORS[3].0);
    for w in ANCHORS.windows(2) {
        let (s0, u0) = w[0];
        let (s1, u1) = w[1];
        if s <= s1 {
            return u0 + (u1 - u0) * (s - s0) / (s1 - s0);
        }
    }
    ANCHORS[3].1
}

/// Estimate peak memory / time / output for an AVIF encode.
///
/// * `width`, `height` — image dimensions in pixels.
/// * `input_bpp` — input bytes per pixel; also selects the stratum:
///   3 = RGB8, 4 = RGBA8, 6 = RGB16→10-bit, 8 = RGBA16→10-bit. Alpha (bpp
///   4/8) and 10-bit (bpp 6/8) cost extra memory and time.
/// * `speed` — AV1 speed preset 1–10 (lower = slower, denser, larger). The
///   dominant time knob.
///
/// Returns `None` only on dimension overflow.
#[must_use]
pub fn estimate_encode(
    width: u32,
    height: u32,
    input_bpp: u8,
    speed: u8,
) -> Option<EncodeEstimate> {
    let pixels = (width as u64).checked_mul(height as u64)?;
    let input = pixels.checked_mul(input_bpp as u64)?;
    let has_alpha = input_bpp == 4 || input_bpp == 8;
    let high_depth = input_bpp >= 6;

    let mut bpp = ENCODE_BPP;
    if high_depth {
        bpp *= ENCODE_DEPTH10_MEM_FACTOR;
    }
    if has_alpha {
        bpp += ENCODE_ALPHA_BPP;
    }
    let working = (pixels as f64 * bpp) as u64;
    let base = ENCODE_FIXED_OVERHEAD.checked_add(input)?;
    let typical = base.checked_add(working)?;
    let min = base + (working as f64 * MULT_MIN) as u64;
    let max = base + (working as f64 * MULT_MAX) as u64;

    let mut us_px = encode_us_per_px(speed);
    if has_alpha {
        us_px *= ENCODE_ALPHA_TIME_FACTOR;
    }
    if high_depth {
        us_px *= ENCODE_DEPTH10_TIME_FACTOR;
    }
    let time_ms = (pixels as f64 * us_px / 1000.0) as f32;
    let output_bytes = (input as f64 * ENCODE_OUTPUT_RATIO) as u64;

    Some(EncodeEstimate {
        peak_memory_bytes_min: min,
        peak_memory_bytes: typical,
        peak_memory_bytes_max: max,
        time_ms,
        output_bytes,
    })
}

/// Estimate peak memory / time for an AVIF decode.
///
/// * `width`, `height` — image dimensions in pixels.
/// * `output_bpp` — bytes per pixel of the decoded buffer (3 = RGB8,
///   4 = RGBA8, 6/8 = 16-bit). bpp ≥ 6 selects the 10-bit stratum.
///
/// Returns `None` only on dimension overflow.
#[must_use]
pub fn estimate_decode(width: u32, height: u32, output_bpp: u8) -> Option<DecodeEstimate> {
    let pixels = (width as u64).checked_mul(height as u64)?;
    let output_bytes = pixels.checked_mul(output_bpp as u64)?;
    let high_depth = output_bpp >= 6;

    let mut bpp = DECODE_BPP;
    let mut us_px = DECODE_US_PER_PX;
    if high_depth {
        bpp *= DECODE_DEPTH10_MEM_FACTOR;
        us_px *= DECODE_DEPTH10_TIME_FACTOR;
    }
    let peak = DECODE_FIXED_OVERHEAD + (pixels as f64 * bpp) as u64;
    let time_ms = (pixels as f64 * us_px / 1000.0) as f32;

    Some(DecodeEstimate {
        peak_memory_bytes: peak,
        output_bytes,
        time_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode time must follow the measured speed curve: slower preset →
    /// strictly more time, with the big speed-4 vs speed-10 gap (~14×).
    #[test]
    fn encode_time_scales_with_speed() {
        let (w, h) = (2048, 2048);
        let t4 = estimate_encode(w, h, 3, 4).unwrap().time_ms;
        let t6 = estimate_encode(w, h, 3, 6).unwrap().time_ms;
        let t10 = estimate_encode(w, h, 3, 10).unwrap().time_ms;
        assert!(t4 > t6 && t6 > t10, "slower preset = more time");
        assert!(
            t4 > t10 * 8.0,
            "speed-4 vs speed-10 gap should be ~14x, got {}",
            t4 / t10
        );
        // speeds below 4 clamp to the speed-4 value (unmeasured lower bound).
        assert_eq!(estimate_encode(w, h, 3, 2).unwrap().time_ms, t4);
    }

    /// Decode is far cheaper than encode in both memory and time.
    #[test]
    fn decode_cheaper_than_encode() {
        let (w, h) = (2048, 2048);
        let enc = estimate_encode(w, h, 3, 6).unwrap();
        let dec = estimate_decode(w, h, 3).unwrap();
        assert!(dec.time_ms < enc.time_ms / 10.0, "decode << encode time");
        assert!(
            dec.peak_memory_bytes < enc.peak_memory_bytes,
            "decode < encode mem"
        );
    }

    /// Alpha and 10-bit each add measurable encode cost; bracket the
    /// measured 2048² encode mem (≈ 40 B/px → ~168 MB working at 8-bit).
    #[test]
    fn alpha_and_depth_add_cost() {
        let (w, h) = (2048, 2048);
        let px = (w as u64) * (h as u64);
        let rgb = estimate_encode(w, h, 3, 6).unwrap();
        let rgba = estimate_encode(w, h, 4, 6).unwrap();
        let rgb10 = estimate_encode(w, h, 6, 6).unwrap();
        assert!(
            rgba.peak_memory_bytes > rgb.peak_memory_bytes,
            "alpha adds mem"
        );
        assert!(rgba.time_ms > rgb.time_ms, "alpha adds time");
        assert!(
            rgb10.peak_memory_bytes > rgb.peak_memory_bytes,
            "10-bit adds mem"
        );
        assert!(rgb10.time_ms > rgb.time_ms, "10-bit adds time");
        // 8-bit RGB working set ~40 B/px; typical peak = input + fixed +
        // ~40·px must sit in a sane band around the measured ~168 MB working.
        let working = rgb.peak_memory_bytes - (8 << 20) - px * 3;
        assert!(
            working >= px * 30 && working <= px * 55,
            "encode working {working} not ~40 B/px of {px}px"
        );
    }

    #[test]
    fn overflow_returns_none() {
        assert!(estimate_encode(u32::MAX, u32::MAX, 8, 6).is_none());
        assert!(estimate_decode(u32::MAX, u32::MAX, 8).is_none());
    }
}
