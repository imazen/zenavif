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
//! ## Calibration (2026-06-14 time curve; 2026-06-23 ENCODE memory re-measure)
//!
//! Measured marginal working set (`VmHWM` delta around the codec call —
//! excludes the binary floor + input buffer) plus wall and user/sys CPU
//! (`/proc/self/stat`, threads=1), one process per op.
//!
//! ### ENCODE peak memory — re-measured 2026-06-23 (VmHWM + heaptrack)
//!
//! The earlier calibration over-predicted peak memory: the fixed overhead was
//! set to 8 MiB and the slope to 40 B/px, but a fresh sweep
//! (`examples/mem_probe_encode`, RGB8, threads=1, sizes 256–2048 px × speed
//! {6,8,10} × content {photo, screenshot} × q {50,85}, 2 reps/cell) measured a
//! **fixed intercept ≈ 4.6 MiB and a worst-case slope ≈ 30–38 B/px**. The
//! old model was 2.0× over the measured typical at 256² (fixed-dominated),
//! 1.25× at 1 MP, and 1.35× at 4 MP. Tightened to **5.5 MiB fixed + 37 B/px**,
//! which keeps the TYP ≥ measured worst-case + 10 % at every swept size while
//! cutting the over-prediction to 1.11–1.49×. Provenance:
//! `benchmarks/zenavif_encode_mem_2026-06-23.tsv`.
//!
//! Measured ENCODE marginal (worst-case over content × speed × q, VmHWM):
//!
//! | size  | worst marginal | B/px (incl. intercept) |
//! |-------|----------------|------------------------|
//! | 256²  | 5.4 MiB        | 84                     |
//! | 512²  | 13 MiB         | 51                     |
//! | 1 MP  | 39 MiB         | 38                     |
//! | 4 MP  | 124 MiB        | 31                     |
//!
//! heaptrack peak heap (requested, sets the MAX tier): 1 MP 47.9 MiB, 4 MP
//! 155.6 MiB (includes the input buffer). Dependence: **quality** is the
//! biggest knob (q85/q50 ≈ 1.28× — high quality holds more coefficients);
//! **content** ≈ 1.26× (photo > screenshot); **speed** only ≈ 1.09× (speed-10
//! is heaviest, so the model's speed-independence is conservatively fine —
//! the constants are fit to the speed-10/q85/photo worst case). The MAX (1.8×)
//! tier clears the heaptrack-requested heap by 1.6–1.9×.
//!
//! ### ENCODE time + alpha/depth (2026-06-14, unchanged)
//!
//! Time-vs-speed and the alpha/10-bit deltas are from the earlier grid
//! (`benchmarks/avif_resource_main_2026-06-14.tsv` +
//! `avif_resource_alphadepth_2026-06-14.tsv`; harness
//! `scripts/avif_resource_calibrate.py`); the 2026-06-23 sweep covered only
//! the RGB8 8-bit path, so the alpha (+7 B/px) and 10-bit (×1.55) memory
//! factors are carried forward unchanged rather than extrapolated.
//!
//! | speed | encode us/px |
//! |-------|--------------|
//! | 4     | 7.6          |
//! | 6     | 2.0          |
//! | 8     | 1.2          |
//! | 10    | 0.55         |
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

// ── Calibrated constants (encode mem: 2026-06-23 VmHWM+heaptrack re-measure;
//    encode time + alpha/depth: 2026-06-14) ──

/// Encoder fixed overhead (size-independent): AV1 plans, tables, tiles.
/// Re-measured 2026-06-23: the VmHWM intercept of the worst-case (photo,
/// speed-10, q85) marginal fit is ~4.6 MiB; rounded up to 5.5 MiB so the TYP
/// clears the measured worst case + 10 % at the smallest swept sizes.
const ENCODE_FIXED_OVERHEAD: u64 = (5.5 * (1 << 20) as f64) as u64;
/// Typical encoder marginal working set, bytes/pixel, 8-bit (sub-linear in
/// size: ~38 B/px effective at 1 MP, ~31 at 4 MP including the intercept).
/// Re-measured 2026-06-23: 37 B/px is the tightest slope that, with the 5.5 MiB
/// fixed term, keeps the TYP ≥ the measured worst case + 10 % across 256–2048 px
/// (the old 40 over-predicted by up to 2.0× at small sizes).
const ENCODE_BPP: f64 = 37.0;

/// Per-pixel working-set multiplier for the (backend, subsampling) arm,
/// relative to the zenravif 4:4:4 arm [`ENCODE_BPP`] was calibrated on.
///
/// Measured slope ratios from `benchmarks/avif_backend_calib_2026-08-13.tsv`
/// (five sizes 0.066-8.356 MP, worst case over quality x speed x reps):
///   zenravif 4:4:4  29.3 B/px  -> 1.00 (the calibration arm)
///   zenravif 4:2:0  22.3 B/px  -> 0.76  (quarter-resolution chroma planes)
///   Zenav1Svt    4:2:0  52.7 B/px  -> 1.80  (relative to 4:4:4)
///
/// Zenav1Svt costs ~2.4x zenravif at EQUAL subsampling, and its peak is flat
/// across speed, quality and thread count — the pipeline allocates its frame
/// working set up front rather than scaling with the search. Rounded UP to the
/// nearest 0.01 so the estimate never lands under the measurement.
fn backend_subsampling_mem_factor(arm: EstimateArm) -> f64 {
    match arm {
        EstimateArm::Zenravif444 => 1.00,
        EstimateArm::Zenravif420 => 0.76,
        EstimateArm::Zenav1Svt420 => 1.80,
    }
}

impl EstimateArm {
    /// Pick the arm an [`crate::EncoderConfig`] will actually encode on.
    ///
    /// Encode-gated because `Av1Backend` / `EncodeChromaSubsampling` are — a
    /// decode-only build still gets the rest of this module.
    #[cfg(feature = "encode")]
    pub fn for_config(config: &crate::EncoderConfig) -> Self {
        #[cfg(feature = "zenav1-svt")]
        if config.backend == crate::Av1Backend::Zenav1Svt {
            // Zenav1Svt is 4:2:0-only; validate() rejects anything else.
            return EstimateArm::Zenav1Svt420;
        }
        match config.chroma_subsampling {
            crate::EncodeChromaSubsampling::Yuv444 => EstimateArm::Zenravif444,
            crate::EncodeChromaSubsampling::Yuv420 => EstimateArm::Zenravif420,
        }
    }
}

/// The measured (backend x subsampling) arms of the encode memory model.
///
/// Deliberately NOT `Av1Backend` + `EncodeChromaSubsampling`: those live behind
/// the `encode` feature, while `heuristics` is always built (a caller sizing a
/// job queue should not have to compile the encoder). The arms here are exactly
/// the combinations that were measured; `Zenav1Svt` encodes 4:2:0 only, so there is
/// no 4:4:4 variant of it to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EstimateArm {
    /// zenrav1e backend at the shipped default 4:4:4 — the calibration arm.
    #[default]
    Zenravif444,
    /// zenrav1e backend at 4:2:0 (quarter-resolution chroma).
    Zenravif420,
    /// `Av1Backend::Zenav1Svt`, which is 4:2:0-only.
    Zenav1Svt420,
}

impl EstimateArm {
    /// Deprecated spelling of [`EstimateArm::Zenav1Svt420`].
    ///
    /// Renamed with the [`crate::Av1Backend`] variant it names. Kept as an
    /// associated constant so existing consumers keep compiling in both
    /// expression and pattern position.
    #[deprecated(
        since = "0.1.8",
        note = "renamed to `EstimateArm::Zenav1Svt420` to match the zenav1-svt crate; \
                the alias is removed in 0.2"
    )]
    #[allow(non_upper_case_globals)]
    pub const SvtRs420: Self = Self::Zenav1Svt420;
}
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
    estimate_encode_for(width, height, input_bpp, speed, EstimateArm::Zenravif444)
}

/// Backend- and subsampling-aware peak-memory / time estimate.
///
/// [`estimate_encode`] was calibrated on ONE configuration — the zenravif
/// backend at its default 4:4:4 — and had no notion of either axis, so it
/// mis-sized every other arm. Measured at 4K, its typical tier under-predicted
/// `Av1Backend::Zenav1Svt` by 1.34x (a cap sized from it would be too small, the
/// unsafe direction) while over-predicting zenravif 4:2:0 by 1.48x.
///
/// The multipliers below are measured SLOPE RATIOS from a five-size sweep
/// (0.066 / 0.262 / 1.049 / 2.097 / 8.356 MP, all multiples of 64 so Zenav1Svt can
/// encode every cell; worst case over quality {50,85} x speed {6,10} x 2 reps;
/// `benchmarks/avif_backend_calib_2026-08-13.tsv`). Least-squares
/// `alpha + beta*px` fits of total peak RSS:
///
/// | arm              | fit                     | R^2    |
/// |------------------|-------------------------|--------|
/// | zenravif 4:4:4   | 8.88 MiB + 29.3 B/px    | 0.9977 |
/// | zenravif 4:2:0   | 7.58 MiB + 22.3 B/px    | 0.9982 |
/// | Zenav1Svt   4:2:0    | 3.58 MiB + 52.7 B/px    | 1.0000 |
///
/// Ratios are applied rather than the absolute fits because the shipped
/// constants were calibrated on a different platform and on the MARGINAL
/// working set (WSL2 x86-64, `VmHWM` delta) while this sweep measured total
/// peak RSS on macOS arm64. Slope ratios are the part that transfers; the
/// absolute intercept is not re-fit here.
pub fn estimate_encode_for(
    width: u32,
    height: u32,
    input_bpp: u8,
    speed: u8,
    arm: EstimateArm,
) -> Option<EncodeEstimate> {
    let pixels = (width as u64).checked_mul(height as u64)?;
    let input = pixels.checked_mul(input_bpp as u64)?;
    let has_alpha = input_bpp == 4 || input_bpp == 8;
    let high_depth = input_bpp >= 6;

    let mut bpp = ENCODE_BPP * backend_subsampling_mem_factor(arm);
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

/// How an encode scales across CPU cores (measured, single-photo sparse fit,
/// `benchmarks/vcpu_resource_sweep_2026-06-20.tsv`). zenavif wraps the
/// tile-parallel AV1 encoder, so it is the best-scaling zen codec — but wall
/// time still does NOT scale as `1/cores`: the useful thread count is bounded
/// by the AV1 tile count (which grows with image size), and Amdahl saturation
/// applies. Use [`estimate_encode_threaded`].
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ThreadingInfo {
    /// Whether the encode uses more than one core at all.
    pub parallel: bool,
    /// Threads beyond this yield no further speedup (the AV1 tile count caps
    /// it; scales with image size). 1 = serial.
    pub max_useful_threads: u32,
    /// Amdahl parallel fraction `p` fitted from measurement; peak speedup is
    /// `1/(1-p)`. 0 = serial.
    pub parallel_fraction: f32,
    /// Extra peak working-set per added worker thread, bytes (the γ term —
    /// small for peak RSS: tiles are views into one shared FrameState, though
    /// per-tile contexts churn the allocator).
    pub mem_bytes_per_thread: u64,
}

impl ThreadingInfo {
    /// Threads that actually do work given `cores` available (clamped to
    /// `max_useful_threads`).
    #[must_use]
    pub fn effective_threads(&self, cores: usize) -> u64 {
        (cores.max(1) as u64).min(self.max_useful_threads.max(1) as u64)
    }
    /// Achieved wall-time speedup at `cores` (Amdahl, clamped). 1.0 = serial.
    #[must_use]
    pub fn speedup(&self, cores: usize) -> f32 {
        let n = self.effective_threads(cores);
        if !self.parallel || n <= 1 {
            return 1.0;
        }
        let p = self.parallel_fraction as f64;
        (1.0 / ((1.0 - p) + p / n as f64)) as f32
    }
}

/// Threading characterisation for a zenavif (AV1) encode. Tile-parallel:
/// `parallel_fraction` ≈ 0.93, and the useful thread count scales with the
/// tile count (≈ `pixels / 65536`, clamped to `[4, 32]`) — measured ~3.3×
/// at 256², ~9× at 1024², ~10× at 2048² (28 cores).
#[must_use]
pub fn encode_threading_info(pixels: u64) -> ThreadingInfo {
    let tiles = (pixels / 65_536).clamp(4, 32) as u32;
    ThreadingInfo {
        parallel: true,
        max_useful_threads: tiles,
        parallel_fraction: 0.93,
        mem_bytes_per_thread: 1_000_000,
    }
}

/// Threading characterisation for a zenavif (AV1) decode.
///
/// AVIF decode is only *partly* parallel: rav1d-safe decodes AV1 tiles in
/// parallel (zenavif runs it with `max_frame_delay=1` — tile parallelism, no
/// frame threading), but zenavif's own YUV→RGB(A) conversion is a single
/// auto-vectorised pass, and most still images carry a single tile. So the
/// useful-thread knee is far lower than the encode side and grows slowly with
/// size. Unlike [`encode_threading_info`], whose `parallel_fraction` is fitted
/// to measurement, this is a conservative model (decode threading is not
/// separately benchmarked); the knee `pixels / 262144` clamped to `[1, 8]` errs
/// toward serial so a scheduler does not over-promise speedup.
#[must_use]
pub fn decode_threading_info(pixels: u64) -> ThreadingInfo {
    let knee = (pixels / 262_144).clamp(1, 8) as u32;
    ThreadingInfo {
        parallel: knee > 1,
        max_useful_threads: knee,
        // Modest parallel fraction: tile-decode parallelises, the conversion
        // pass does not.
        parallel_fraction: 0.5,
        mem_bytes_per_thread: 0,
    }
}

/// [`estimate_encode`] adjusted for `cores` available CPU cores: `time_ms` is
/// divided by the measured (saturating, tile-bounded) speedup and the peak
/// terms gain the per-tile working-set. Returns `None` only on dimension
/// overflow.
#[must_use]
pub fn estimate_encode_threaded(
    width: u32,
    height: u32,
    input_bpp: u8,
    speed: u8,
    cores: usize,
) -> Option<EncodeEstimate> {
    estimate_encode_threaded_for(
        width,
        height,
        input_bpp,
        speed,
        cores,
        EstimateArm::default(),
    )
}

/// Arm-aware [`estimate_encode_threaded`]. See [`estimate_encode_for`] for why
/// the arm matters: one estimate for every backend under-predicted Zenav1Svt by
/// 1.34x, and a cap sized from an under-prediction is the unsafe direction.
pub fn estimate_encode_threaded_for(
    width: u32,
    height: u32,
    input_bpp: u8,
    speed: u8,
    cores: usize,
    arm: EstimateArm,
) -> Option<EncodeEstimate> {
    let mut e = estimate_encode_for(width, height, input_bpp, speed, arm)?;
    let pixels = (width as u64).saturating_mul(height as u64);
    let ti = encode_threading_info(pixels);
    e.time_ms = (e.time_ms as f64 / ti.speedup(cores) as f64) as f32;
    let extra = ti
        .mem_bytes_per_thread
        .saturating_mul(ti.effective_threads(cores).saturating_sub(1));
    e.peak_memory_bytes_min = e.peak_memory_bytes_min.saturating_add(extra);
    e.peak_memory_bytes = e.peak_memory_bytes.saturating_add(extra);
    e.peak_memory_bytes_max = e.peak_memory_bytes_max.saturating_add(extra);
    Some(e)
}

// ── Memory-adaptive encode concurrency (crate-private) ──────────────────────
//
// Helpers for fitting the encoder thread count to a memory budget so an
// encode reduces its own parallelism instead of blowing past
// `ResourceLimits::max_memory_bytes` (or the machine's available RAM). Used
// by the zencodec adapter pre-flight in `src/codec.rs`; gated so decode-only
// builds don't carry dead code.

/// The thread count the encoder would use absent an explicit request:
/// `std::thread::available_parallelism()` (1 when unknown). An explicit
/// `Some(n > 0)` request wins; `Some(0)` means "auto" (matches the native
/// `EncoderConfig::threads` semantics where `None` = default pool).
#[cfg(any(test, feature = "encode"))]
pub(crate) fn requested_or_default_threads(requested: Option<usize>) -> usize {
    match requested {
        Some(n) if n > 0 => n,
        _ => std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1),
    }
}

/// Implicit memory budget when the caller set no `max_memory_bytes`:
/// detected available RAM × 0.8 (Linux `/proc/meminfo` `MemAvailable`).
/// `None` on other platforms or when detection fails — no implicit cap
/// there. The 20 % headroom leaves room for the caller's own buffers and
/// the rest of the process.
// `test` alone over-includes: the only test caller is Linux-gated (it reads
// /proc/meminfo), so on macOS/Windows a `cargo clippy` lib-test build compiled
// these and then failed `-D dead_code` on them. Gate on the conditions that
// actually have a caller.
#[cfg(any(feature = "encode", all(test, target_os = "linux")))]
pub(crate) fn implicit_memory_budget() -> Option<u64> {
    detected_available_ram().map(|bytes| (bytes as f64 * 0.8) as u64)
}

/// Available RAM in bytes: Linux `MemAvailable` (kernel's estimate of memory
/// usable without swapping). `None` elsewhere — macOS/Windows have no
/// equally cheap equivalent, and a wrong guess is worse than no cap.
#[cfg(any(feature = "encode", all(test, target_os = "linux")))]
fn detected_available_ram() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        for line in meminfo.lines() {
            if let Some(rest) = line.strip_prefix("MemAvailable:") {
                let kib: u64 = rest.trim().trim_end_matches("kB").trim().parse().ok()?;
                return Some(kib.saturating_mul(1024));
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Fit the encoder thread count to a memory budget: the largest count whose
/// thread-aware conservative peak ([`EncodeEstimate::peak_memory_bytes_max`]
/// via [`estimate_encode_threaded`]) fits `budget`, floored at 1.
///
/// * `requested` — explicit thread request already on the config
///   (`Some(n > 0)`); `None`/`Some(0)` start from
///   [`requested_or_default_threads`] (the machine parallelism the encoder
///   would otherwise use).
/// * `budget` — memory budget in bytes (explicit `max_memory_bytes` or the
///   implicit available-RAM budget). `None` = unlimited: no fit, no pin.
///
/// Returns `(pin, note)`:
/// * `pin = Some(n)` — pin the encoder to `n` threads; returned only when
///   the budget requires fewer threads than would otherwise run. At the
///   floor (`n == 1`) the budget may STILL be exceeded — the caller must
///   re-check the estimate at the pinned count and raise its memory-limit
///   error there (thread reduction cannot shrink the single-thread peak).
/// * `pin = None` — no reduction needed (or no budget); leave the codec
///   default.
/// * `note` — human-readable record of any reduction (reductions are never
///   silent).
#[cfg(any(test, feature = "encode"))]
pub(crate) fn fit_threads_to_budget(
    width: u32,
    height: u32,
    input_bpp: u8,
    speed: u8,
    requested: Option<usize>,
    budget: Option<u64>,
) -> (Option<usize>, Option<String>) {
    let Some(budget) = budget else {
        return (None, None);
    };
    let start = requested_or_default_threads(requested);
    // `effective_threads` clamps to the tile count, so estimates are flat
    // above it — the walk from `start` terminates against the same values
    // and only begins moving once `n` drops below the tile bound.
    let mut n = start;
    loop {
        match estimate_encode_threaded(width, height, input_bpp, speed, n) {
            // Dimension overflow: nothing to fit (the caller's own estimate
            // check sees the same `None` and skips; dimension caps handle it).
            None => return (None, None),
            Some(e) if e.peak_memory_bytes_max <= budget => break,
            Some(_) => {}
        }
        if n == 1 {
            break;
        }
        n -= 1;
    }
    if n == start {
        return (None, None);
    }
    let over = estimate_encode_threaded(width, height, input_bpp, speed, n)
        .is_some_and(|e| e.peak_memory_bytes_max > budget);
    let note = if over {
        format!(
            "AVIF encode threads reduced {start} -> {n} (floor) for the \
             {budget}-byte memory budget, which the single-threaded \
             conservative peak estimate still exceeds"
        )
    } else {
        format!(
            "AVIF encode threads reduced {start} -> {n} to fit the \
             {budget}-byte memory budget (conservative peak estimate)"
        )
    };
    (Some(n), Some(note))
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

    /// The typical tier must NEVER land under a measured peak — a cap sized
    /// from an under-prediction is the unsafe direction (it rejects or OOMs a
    /// job that would have fit).
    ///
    /// Cells are the worst measured total peak RSS per arm from
    /// `benchmarks/avif_backend_calib_2026-08-13.tsv` (macOS arm64, RGB8,
    /// threads=1, worst case over quality {50,85} x speed {6,10} x 2 reps).
    /// This is the regression gate for the 2026-08-13 backend-awareness fix:
    /// before it, one estimate served every arm and under-predicted Zenav1Svt by
    /// 1.34x.
    #[test]
    fn estimate_typical_covers_measured_peak_on_every_arm() {
        // (arm, w, h, measured worst-case peak RSS in bytes)
        let cells: &[(EstimateArm, u32, u32, u64)] = &[
            (EstimateArm::Zenravif444, 1024, 1024, 39_949_000),
            (EstimateArm::Zenravif444, 3840, 2176, 251_900_000),
            (EstimateArm::Zenravif420, 1024, 1024, 32_190_000),
            (EstimateArm::Zenravif420, 3840, 2176, 192_900_000),
            (EstimateArm::Zenav1Svt420, 1024, 1024, 59_240_000),
            (EstimateArm::Zenav1Svt420, 3840, 2176, 444_100_000),
        ];
        for &(arm, w, h, measured) in cells {
            let est = estimate_encode_for(w, h, 3, 10, arm).expect("estimate");
            assert!(
                est.peak_memory_bytes >= measured,
                "{arm:?} {w}x{h}: typical {} < measured {measured}",
                est.peak_memory_bytes,
            );
            assert!(
                est.peak_memory_bytes_max >= est.peak_memory_bytes,
                "{arm:?}: max tier below typical",
            );
        }
    }

    /// The arms must stay ORDERED the way they measure: at equal size, Zenav1Svt
    /// is the heaviest and zenravif 4:2:0 the lightest. A refactor that
    /// collapsed the arms back to one number would pass the coverage test
    /// above (it is one-sided) but fail this one.
    #[test]
    fn estimate_arms_ordered_as_measured() {
        let (w, h) = (3840, 2176);
        let p = |arm| {
            estimate_encode_for(w, h, 3, 10, arm)
                .unwrap()
                .peak_memory_bytes
        };
        let (svt, y444, y420) = (
            p(EstimateArm::Zenav1Svt420),
            p(EstimateArm::Zenravif444),
            p(EstimateArm::Zenravif420),
        );
        assert!(
            svt > y444,
            "Zenav1Svt {svt} should exceed zenravif 4:4:4 {y444}"
        );
        assert!(y444 > y420, "4:4:4 {y444} should exceed 4:2:0 {y420}");
    }

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
    /// re-measured 2048² encode mem (≈ 37 B/px → ~155 MB working at 8-bit).
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
        // 8-bit RGB working set ~37 B/px (re-measured 2026-06-23); typical peak
        // = input + ENCODE_FIXED_OVERHEAD + 37·px must sit in a sane band. Use
        // the constant (not a literal) so the band tracks the calibrated fixed
        // term; the worst-case measured slope is ~30–38 B/px, so [33, 45] frames
        // the 37 anchor with margin on both sides.
        let working = rgb.peak_memory_bytes - ENCODE_FIXED_OVERHEAD - px * 3;
        assert!(
            working >= px * 33 && working <= px * 45,
            "encode working {working} not ~37 B/px of {px}px"
        );
    }

    #[test]
    fn overflow_returns_none() {
        assert!(estimate_encode(u32::MAX, u32::MAX, 8, 6).is_none());
        assert!(estimate_decode(u32::MAX, u32::MAX, 8).is_none());
    }

    // ── fit_threads_to_budget (memory-adaptive encode concurrency) ──────

    /// 2048² has 64 tiles (clamped to 32), so every thread count 1..=32 is
    /// effective and each added thread costs exactly `mem_bytes_per_thread`
    /// on the peak tiers — the walk-down is fully deterministic when the
    /// start is an explicit request (no machine parallelism involved).
    #[test]
    fn fit_walks_down_to_largest_fitting_count() {
        let (w, h) = (2048, 2048);
        let e1 = estimate_encode_threaded(w, h, 3, 6, 1).unwrap();
        let e3 = estimate_encode_threaded(w, h, 3, 6, 3).unwrap();
        let e4 = estimate_encode_threaded(w, h, 3, 6, 4).unwrap();
        // Sanity: the per-thread term is strictly monotone in this range.
        assert!(e3.peak_memory_bytes_max > e1.peak_memory_bytes_max);
        assert!(e4.peak_memory_bytes_max > e3.peak_memory_bytes_max);
        // Budget admits exactly 3 threads: e3 fits, e4 does not.
        let budget = e3.peak_memory_bytes_max;
        let (pin, note) = fit_threads_to_budget(w, h, 3, 6, Some(32), Some(budget));
        assert_eq!(pin, Some(3), "expected walk-down 32 -> 3");
        let note = note.expect("reduction must be recorded, never silent");
        assert!(
            note.contains("32") && note.contains("3"),
            "note should record the reduction: {note}"
        );
    }

    /// A budget that only the single-threaded estimate fits floors at 1.
    #[test]
    fn fit_floors_at_one_thread() {
        let (w, h) = (2048, 2048);
        let e1 = estimate_encode_threaded(w, h, 3, 6, 1).unwrap();
        let (pin, note) =
            fit_threads_to_budget(w, h, 3, 6, Some(32), Some(e1.peak_memory_bytes_max));
        assert_eq!(pin, Some(1));
        assert!(note.is_some());
    }

    /// When even threads=1 exceeds the budget, the fit still returns the
    /// floor (best effort) and the note says so — the CALLER holds the
    /// error path (checked against the estimate at the pinned count).
    #[test]
    fn fit_at_floor_still_over_budget_reports_floor() {
        let (w, h) = (2048, 2048);
        let e1 = estimate_encode_threaded(w, h, 3, 6, 1).unwrap();
        let (pin, note) =
            fit_threads_to_budget(w, h, 3, 6, Some(32), Some(e1.peak_memory_bytes_max - 1));
        assert_eq!(pin, Some(1));
        assert!(
            note.unwrap().contains("floor"),
            "floor-exceeded note must be explicit"
        );
        // The caller's re-check at the pinned count must then fail:
        assert!(e1.peak_memory_bytes_max > e1.peak_memory_bytes_max - 1);
    }

    /// No budget → no fit, no pin, no note.
    #[test]
    fn fit_without_budget_is_inert() {
        assert_eq!(
            fit_threads_to_budget(2048, 2048, 3, 6, Some(32), None),
            (None, None)
        );
    }

    /// A budget the requested count already fits → no pin (leave the codec
    /// default / explicit request in place).
    #[test]
    fn fit_no_reduction_when_request_fits() {
        let (w, h) = (2048, 2048);
        let e4 = estimate_encode_threaded(w, h, 3, 6, 4).unwrap();
        assert_eq!(
            fit_threads_to_budget(w, h, 3, 6, Some(4), Some(e4.peak_memory_bytes_max)),
            (None, None)
        );
    }

    /// Explicit request wins over machine parallelism; 0 means "auto".
    #[test]
    fn requested_or_default_semantics() {
        assert_eq!(requested_or_default_threads(Some(7)), 7);
        let auto = requested_or_default_threads(None);
        assert!(auto >= 1);
        assert_eq!(requested_or_default_threads(Some(0)), auto);
    }

    /// The plan-of-record worked example (CODEC-MEMORY-PLAN wave 2): 20 MP
    /// RGB8 at speed 6 under a 2 GiB budget. The AV1 tile bound (32) caps
    /// the per-thread term, so even the 32-thread conservative peak
    /// (1,428,767,168 B ≈ 1.33 GiB) fits 2 GiB with ~0.7 GiB headroom — no
    /// reduction. The walk engages only when the budget squeezes into the
    /// [peak_max(1), peak_max(32)] band, and floors at 1 below it. Exact
    /// byte values track the 2026-06-23 memory calibration — update them
    /// alongside the constants if recalibrated.
    #[test]
    fn fit_twenty_megapixel_two_gib_example() {
        let (w, h) = (5000, 4000); // 20 MP
        let e1 = estimate_encode_threaded(w, h, 3, 6, 1).unwrap();
        let e32 = estimate_encode_threaded(w, h, 3, 6, 32).unwrap();
        assert_eq!(e1.peak_memory_bytes_max, 1_397_767_168);
        assert_eq!(e32.peak_memory_bytes_max, 1_428_767_168);

        // 2 GiB budget: fits at every thread count — no pin, no note.
        let two_gib = 2u64 << 30;
        assert!(e32.peak_memory_bytes_max < two_gib);
        assert_eq!(
            fit_threads_to_budget(w, h, 3, 6, Some(32), Some(two_gib)),
            (None, None)
        );

        // Squeezed budget (peak_max(1) + 5 per-thread increments): admits
        // exactly 6 threads.
        let (pin, note) = fit_threads_to_budget(
            w,
            h,
            3,
            6,
            Some(32),
            Some(e1.peak_memory_bytes_max + 5_000_000),
        );
        assert_eq!(pin, Some(6));
        assert!(note.is_some());

        // Below the single-thread peak: floors at 1 and says so — the codec
        // pre-flight then raises the memory-limit error.
        let (pin, note) =
            fit_threads_to_budget(w, h, 3, 6, Some(32), Some(e1.peak_memory_bytes_max - 1));
        assert_eq!(pin, Some(1));
        assert!(note.unwrap().contains("floor"));
    }

    /// On Linux the implicit budget is detected and sane (nonzero, below
    /// the raw MemAvailable it derives from).
    #[cfg(target_os = "linux")]
    #[test]
    fn implicit_budget_detects_on_linux() {
        let raw = detected_available_ram().expect("/proc/meminfo MemAvailable");
        let budget = implicit_memory_budget().expect("implicit budget");
        assert!(budget > 0);
        assert!(budget < raw, "80% headroom must reduce the raw figure");
    }
}
