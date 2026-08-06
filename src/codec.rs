//! zencodec trait implementations for zenavif.
//!
//! Provides [`AvifEncoderConfig`] and [`AvifDecoderConfig`] types that implement
//! the trait hierarchy from zencodec, wrapping the native zenavif API.
//!
//! # Trait mapping
//!
//! | zencodec | zenavif adapter |
//! |----------------|-----------------|
//! | `EncoderConfig` | [`AvifEncoderConfig`] |
//! | `EncodeJob` | [`AvifEncodeJob`] |
//! | `Encoder` | [`AvifEncoder`] |
//! | `DecoderConfig` | [`AvifDecoderConfig`] |
//! | `DecodeJob<'a>` | [`AvifDecodeJob`] |
//! | `Decode` | [`AvifDecoder`] |
//! | `AnimationFrameEncoder` | [`AvifAnimationFrameEncoder`] |
//! | `AnimationFrameDecoder` | [`AvifAnimationFrameDecoder`] |

use std::borrow::Cow;
use std::sync::Arc;

use enough::Stop;
use rgb::{Rgb, Rgba};
use zencodec::AnimationFrame;
#[cfg(feature = "encode")]
use zencodec::Metadata;
use zencodec::decode::DecodeOutput;
#[cfg(feature = "encode")]
use zencodec::encode::EncodeOutput;
use zencodec::{
    CodecError, GainMapPresence, ImageFormat, ImageInfo, ImageSequence, ResourceLimits, Supplements,
};
use zenpixels::{ChannelType, ColorAuthority, PixelBuffer, PixelDescriptor, PixelSlice};
use zenpixels_convert::PixelBufferConvertTypedExt as _;

use crate::error::Error;
use whereat::{At, ResultAtExt as _, at};

/// Convert a [`zencodec::ThreadingPolicy`] to a concrete thread count.
///
/// Returns the thread count to pass to rav1e/ravif (encode) or dav1d/rav1d (decode).
/// - `0` means "auto" (let the library pick based on available parallelism).
/// - `1` means single-threaded.
/// - Any other value is the requested thread count.
fn policy_to_threads(policy: zencodec::ThreadingPolicy) -> u32 {
    match policy {
        zencodec::ThreadingPolicy::Sequential => 1,
        zencodec::ThreadingPolicy::Parallel => 0, // 0 = auto
        // The enum is #[non_exhaustive] and includes deprecated legacy variants
        // (SingleThread, LimitOrSingle, LimitOrAny, Balanced, Unlimited). 0
        // (auto) is the safe default for any of those — the deprecated arms
        // emit warnings at the construction site, which is where they should
        // be fixed.
        _ => 0,
    }
}

/// Memory-adaptive concurrency pre-flight shared by the still and animation
/// encode paths: fit the encoder thread count to the memory budget, verify
/// the calibrated thread-aware estimate at the chosen count, and return the
/// thread pin (`Some(n)` only when a reduction is needed) plus the reduction
/// note (never silent).
///
/// Budget semantics (see `crate::heuristics::fit_threads_to_budget`):
/// * explicit `ResourceLimits::max_memory_bytes` is a hard budget — when even
///   the single-threaded conservative peak
///   (`EncodeEstimate::peak_memory_bytes_max`) exceeds it, this errors with
///   the memory-limit error (thread reduction cannot shrink a single-thread
///   peak);
/// * with no explicit limit, 80 % of detected available RAM (Linux
///   `MemAvailable`; no implicit cap elsewhere) bounds the thread choice, and
///   an encode that cannot fit even single-threaded errors with a hint to set
///   `max_memory_bytes` — a clean error beats the kernel OOM-killing the
///   process (measured on 32 GB boxes).
///
/// `bpp` is the caller's input-buffer bytes-per-pixel, which is also the
/// calibrated estimate stratum (3/4/6/8). The f32 paths pass 12/16, which the
/// model treats as ≥ 6 (10-bit stratum) — an over-estimate of their actual
/// 8-bit re-encode, i.e. conservative in the safe direction. The gray path
/// passes 1, a slight under-estimate of its RGB expansion (working-set term
/// dominates; the 2 B/px difference is ~5 %).
#[cfg(feature = "encode")]
fn fit_encode_threads_to_memory(
    limits: &ResourceLimits,
    config: &crate::EncoderConfig,
    w: u32,
    h: u32,
    bpp: u8,
) -> Result<(Option<usize>, Option<String>), At<Error>> {
    use crate::heuristics as hx;
    let speed = config.speed_value();
    let requested = config.threads;
    let explicit = limits.max_memory_bytes;
    let budget = explicit.or_else(hx::implicit_memory_budget);
    let (pin, note) = hx::fit_threads_to_budget(w, h, bpp, speed, requested, budget);
    let chosen = pin.unwrap_or_else(|| hx::requested_or_default_threads(requested));
    if let (Some(budget_bytes), Some(est)) = (
        budget,
        hx::estimate_encode_threaded(w, h, bpp, speed, chosen),
    ) && est.peak_memory_bytes_max > budget_bytes
    {
        // The fit already walked to the floor: `chosen` is 1 (or the
        // caller explicitly requested 1), so this encode does not fit
        // the budget at ANY thread count.
        return Err(match limits.check_memory(est.peak_memory_bytes_max) {
            // Explicit limit: the standard `LimitExceeded::Memory`
            // actual/max figures, with context appended.
            Err(e) => at!(Error::ResourceLimit(format!(
                "{e} (calibrated AVIF encode peak estimate: exceeds \
                 max_memory_bytes even single-threaded; reduce dimensions \
                 or raise the limit)"
            ))),
            // No explicit limit (check_memory passes vacuously): the
            // implicit available-RAM budget, with the override hint.
            Ok(()) => at!(Error::ResourceLimit(format!(
                "calibrated AVIF encode peak estimate {est_max} B exceeds the \
                 implicit memory budget {budget_bytes} B (80% of detected \
                 available RAM) even single-threaded; set \
                 ResourceLimits::max_memory_bytes to choose the budget explicitly",
                est_max = est.peak_memory_bytes_max,
            ))),
        });
    }
    Ok((pin, note))
}

// ── Encoding ────────────────────────────────────────────────────────────────

/// AVIF encoder configuration implementing [`zencodec::encode::EncoderConfig`].
///
/// Wraps [`crate::EncoderConfig`] and tracks universal quality/effort/lossless
/// settings for the trait interface.
///
/// # Examples
///
/// ```rust,ignore
/// use zencodec::encode::EncoderConfig;
/// use zenavif::AvifEncoderConfig;
///
/// let enc = AvifEncoderConfig::new()
///     .with_quality(80.0)
///     .with_effort_u32(6);
/// ```
#[cfg(feature = "encode")]
#[derive(Clone, Debug)]
pub struct AvifEncoderConfig {
    inner: crate::EncoderConfig,
    /// Trait-level effort (0-10 signed scale). Inverted to AVIF speed.
    trait_effort: Option<i32>,
    /// Trait-level calibrated quality (0.0-100.0).
    trait_quality: Option<f32>,
    /// Whether lossless is explicitly enabled.
    lossless: bool,
}

#[cfg(feature = "encode")]
impl AvifEncoderConfig {
    /// Create a default AVIF encoder config (quality 75, speed 4).
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: crate::EncoderConfig::new(),
            trait_effort: None,
            trait_quality: None,
            lossless: false,
        }
    }

    /// Access the underlying [`crate::EncoderConfig`].
    #[must_use]
    pub fn inner(&self) -> &crate::EncoderConfig {
        &self.inner
    }

    /// Mutable access to the underlying [`crate::EncoderConfig`].
    pub fn inner_mut(&mut self) -> &mut crate::EncoderConfig {
        &mut self.inner
    }

    /// Set encode quality (0.0 = worst, 100.0 = lossless).
    #[must_use]
    pub fn with_quality(mut self, quality: f32) -> Self {
        self.inner = self.inner.quality(quality);
        self
    }

    /// Set encode effort/speed (0 = slowest/best, 10 = fastest).
    #[must_use]
    pub fn with_effort_u32(mut self, effort: u32) -> Self {
        self.inner = self.inner.speed(effort.min(10) as u8);
        self
    }

    /// Enable or disable lossless encoding (inherent method).
    #[must_use]
    pub fn with_lossless_mode(mut self, lossless: bool) -> Self {
        self.lossless = lossless;
        if lossless {
            self.inner = self.inner.quality(100.0);
            #[cfg(feature = "encode-imazen")]
            {
                self.inner = self.inner.with_lossless(true);
            }
        }
        self
    }

    /// Set alpha channel quality (0.0 = worst, 100.0 = lossless) (inherent method).
    #[must_use]
    pub fn with_alpha_quality_value(mut self, quality: f32) -> Self {
        self.inner = self.inner.alpha_quality(quality);
        self
    }

    /// Embed a pre-encoded gain map for UltraHDR / ISO 21496-1.
    ///
    /// See [`crate::EncoderConfig::with_gain_map`] for details.
    #[must_use]
    pub fn with_gain_map(
        mut self,
        av1_data: Vec<u8>,
        width: u32,
        height: u32,
        bit_depth: u8,
        metadata: Vec<u8>,
    ) -> Self {
        self.inner = self
            .inner
            .with_gain_map(av1_data, width, height, bit_depth, metadata);
        self
    }

    /// Convenience: encode RGB8 pixels with this config.
    pub fn encode_rgb8(
        &self,
        img: imgref::ImgRef<'_, Rgb<u8>>,
    ) -> Result<EncodeOutput, At<CodecError>> {
        use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
        self.clone()
            .job()
            .encoder()?
            .encode(PixelSlice::from(img).erase())
    }

    /// Convenience: encode RGBA8 pixels with this config.
    pub fn encode_rgba8(
        &self,
        img: imgref::ImgRef<'_, Rgba<u8>>,
    ) -> Result<EncodeOutput, At<CodecError>> {
        use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
        self.clone()
            .job()
            .encoder()?
            .encode(PixelSlice::from(img).erase())
    }

    /// Convenience: encode Gray8 pixels with this config.
    pub fn encode_gray8(
        &self,
        img: imgref::ImgRef<'_, rgb::Gray<u8>>,
    ) -> Result<EncodeOutput, At<CodecError>> {
        use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
        self.clone()
            .job()
            .encoder()?
            .encode(PixelSlice::from(img).erase())
    }

    /// Convenience: encode RGB f32 pixels with this config.
    pub fn encode_rgb_f32(
        &self,
        img: imgref::ImgRef<'_, Rgb<f32>>,
    ) -> Result<EncodeOutput, At<CodecError>> {
        use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
        self.clone()
            .job()
            .encoder()?
            .encode(PixelSlice::from(img).erase())
    }

    /// Convenience: encode RGBA f32 pixels with this config.
    pub fn encode_rgba_f32(
        &self,
        img: imgref::ImgRef<'_, Rgba<f32>>,
    ) -> Result<EncodeOutput, At<CodecError>> {
        use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
        self.clone()
            .job()
            .encoder()?
            .encode(PixelSlice::from(img).erase())
    }

    /// Convenience: encode Gray f32 pixels with this config.
    pub fn encode_gray_f32(
        &self,
        img: imgref::ImgRef<'_, rgb::Gray<f32>>,
    ) -> Result<EncodeOutput, At<CodecError>> {
        use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
        self.clone()
            .job()
            .encoder()?
            .encode(PixelSlice::from(img).erase())
    }
}

#[cfg(feature = "encode")]
impl Default for AvifEncoderConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "encode")]
static ENCODE_DESCRIPTORS: &[PixelDescriptor] = &[
    // SDR
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::BGRA8_SRGB,
    PixelDescriptor::RGBX8_SRGB,
    PixelDescriptor::BGRX8_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::GRAYF32_LINEAR,
    // f32 PQ BT.2020 (HDR)
    PixelDescriptor::RGBF32_LINEAR
        .with_transfer(zenpixels::TransferFunction::Pq)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    PixelDescriptor::RGBAF32_LINEAR
        .with_transfer(zenpixels::TransferFunction::Pq)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    // f32 HLG BT.2020 (HDR)
    PixelDescriptor::RGBF32_LINEAR
        .with_transfer(zenpixels::TransferFunction::Hlg)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    PixelDescriptor::RGBAF32_LINEAR
        .with_transfer(zenpixels::TransferFunction::Hlg)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    // HDR — 16-bit with PQ/HLG transfer and BT.2020 primaries
    PixelDescriptor::RGB16_SRGB,
    PixelDescriptor::RGBA16_SRGB,
    // 16-bit PQ BT.2020
    PixelDescriptor::RGB16_SRGB
        .with_transfer(zenpixels::TransferFunction::Pq)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    PixelDescriptor::RGBA16_SRGB
        .with_transfer(zenpixels::TransferFunction::Pq)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    // 16-bit HLG BT.2020
    PixelDescriptor::RGB16_SRGB
        .with_transfer(zenpixels::TransferFunction::Hlg)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    PixelDescriptor::RGBA16_SRGB
        .with_transfer(zenpixels::TransferFunction::Hlg)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    // 16-bit Display P3 sRGB transfer
    PixelDescriptor::RGB16_SRGB.with_primaries(zenpixels::ColorPrimaries::DisplayP3),
    PixelDescriptor::RGBA16_SRGB.with_primaries(zenpixels::ColorPrimaries::DisplayP3),
    // 16-bit PQ BT.2020 narrow range (broadcast HDR10)
    PixelDescriptor::RGB16_SRGB
        .with_transfer(zenpixels::TransferFunction::Pq)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020)
        .with_signal_range(zenpixels::SignalRange::Narrow),
    PixelDescriptor::RGBA16_SRGB
        .with_transfer(zenpixels::TransferFunction::Pq)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020)
        .with_signal_range(zenpixels::SignalRange::Narrow),
    // 16-bit HLG BT.2020 narrow range (broadcast HLG)
    PixelDescriptor::RGB16_SRGB
        .with_transfer(zenpixels::TransferFunction::Hlg)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020)
        .with_signal_range(zenpixels::SignalRange::Narrow),
    PixelDescriptor::RGBA16_SRGB
        .with_transfer(zenpixels::TransferFunction::Hlg)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020)
        .with_signal_range(zenpixels::SignalRange::Narrow),
];

#[cfg(feature = "encode")]
static AVIF_ENCODE_CAPABILITIES: zencodec::encode::EncodeCapabilities =
    zencodec::encode::EncodeCapabilities::new()
        .with_icc(true)
        .with_exif(true)
        .with_xmp(true)
        .with_cicp(true)
        // AVIF's nclx (CICP) has been spec-mandated since the first MIAF/HEIF
        // editions: the colour-information property is reader-authoritative — it
        // overrides any in-bitstream colour and a default is always assumed — so a
        // conforming reader honors nclx. CICP is therefore safe as the *sole* color
        // carrier here; drop a redundant ICC, like JXL. (Contrast PNG's cICP, a far
        // newer optional chunk that is not yet sole-safe.)
        .with_cicp_is_valid_carrier(true)
        .with_cicp_safe_sole_carrier(true)
        .with_stop(true)
        .with_lossy(true)
        .with_lossless(cfg!(feature = "encode-imazen"))
        .with_hdr(true)
        .with_gain_map(true)
        .with_animation(true)
        .with_native_gray(false)
        .with_native_16bit(true)
        .with_native_f32(false)
        .with_native_alpha(true)
        .with_enforces_max_pixels(true)
        .with_enforces_max_memory(true)
        .with_effort_range(0, 10)
        .with_quality_range(0.0, 100.0)
        .with_threads_supported_range(1, 256);

/// Map generic quality (libjpeg-turbo scale) to AVIF native quality.
///
/// Calibrated on CID22-512 corpus (209 images) to produce the same median
/// SSIMULACRA2 as libjpeg-turbo at each quality level.
#[cfg(feature = "encode")]
fn calibrated_avif_quality(generic_q: f32) -> f32 {
    const TABLE: &[(f32, f32)] = &[
        (5.0, 5.0),
        (10.0, 13.9),
        (15.0, 23.9),
        (20.0, 31.0),
        (25.0, 36.1),
        (30.0, 40.1),
        (35.0, 43.4),
        (40.0, 45.7),
        (45.0, 48.0),
        (50.0, 50.0),
        (55.0, 52.1),
        (60.0, 54.1),
        (65.0, 56.6),
        (70.0, 59.2),
        (72.0, 60.7),
        (75.0, 62.8),
        (78.0, 65.1),
        (80.0, 66.6),
        (82.0, 68.5),
        (85.0, 71.1),
        (87.0, 72.6),
        (90.0, 75.8),
        (92.0, 78.3),
        (95.0, 82.8),
        (97.0, 85.5),
        (99.0, 87.0),
    ];
    interp_quality(TABLE, generic_q)
}

/// Piecewise linear interpolation with clamping at table bounds.
#[cfg(feature = "encode")]
fn interp_quality(table: &[(f32, f32)], x: f32) -> f32 {
    if x <= table[0].0 {
        return table[0].1;
    }
    if x >= table[table.len() - 1].0 {
        return table[table.len() - 1].1;
    }
    for i in 1..table.len() {
        if x <= table[i].0 {
            let (x0, y0) = table[i - 1];
            let (x1, y1) = table[i];
            let t = (x - x0) / (x1 - x0);
            return y0 + t * (y1 - y0);
        }
    }
    table[table.len() - 1].1
}

#[cfg(feature = "encode")]
impl zencodec::encode::EncoderConfig for AvifEncoderConfig {
    type Error = At<CodecError>;
    type Job = AvifEncodeJob;

    fn format() -> ImageFormat {
        ImageFormat::Avif
    }

    fn supported_descriptors() -> &'static [PixelDescriptor] {
        ENCODE_DESCRIPTORS
    }

    fn capabilities() -> &'static zencodec::encode::EncodeCapabilities {
        &AVIF_ENCODE_CAPABILITIES
    }

    fn with_generic_effort(mut self, effort: i32) -> Self {
        let clamped = effort.clamp(0, 10);
        self.trait_effort = Some(clamped);
        // Invert: effort 0 = fastest (speed 10), effort 10 = slowest (speed 1)
        // rav1e requires speed in 1..=10, so clamp the result
        let speed = (10 - clamped).clamp(1, 10) as u8;
        self.inner = self.inner.speed(speed);
        self
    }

    fn generic_effort(&self) -> Option<i32> {
        self.trait_effort
    }

    fn with_generic_quality(mut self, quality: f32) -> Self {
        let clamped = quality.clamp(0.0, 100.0);
        self.trait_quality = Some(clamped);
        let native = calibrated_avif_quality(clamped);
        self.inner = self.inner.quality(native);
        self
    }

    fn generic_quality(&self) -> Option<f32> {
        self.trait_quality
    }

    fn with_lossless(mut self, lossless: bool) -> Self {
        self.lossless = lossless;
        if lossless {
            self.inner = self.inner.quality(100.0);
            #[cfg(feature = "encode-imazen")]
            {
                self.inner = self.inner.with_lossless(true);
            }
        }
        self
    }

    fn is_lossless(&self) -> Option<bool> {
        Some(self.lossless)
    }

    /// Honor a [`Fidelity`](zencodec::encode::Fidelity) target as natively as
    /// AVIF allows.
    ///
    /// - `Lossless` → true qindex-0 lossless **only** under `encode-imazen`
    ///   (the only build where it is real, matching `capabilities().lossless()`).
    ///   Otherwise it would be q100 lossy, so it degrades to top quality and is
    ///   reported as `codec_quality`, never falsely as `Lossless`.
    /// - `Lossy(CodecSpecificQuality(q))` → the AVIF quality dial.
    /// - `Lossy(ApproxSsim2(s))` / `Lossy(ApproxButteraugli(d))` → AVIF has no
    ///   native metric loop, so these map coarsely onto the quality dial and are
    ///   reported as `codec_quality`, honest that no convergence happened.
    fn with_fidelity(self, fidelity: zencodec::encode::Fidelity) -> Self {
        use zencodec::encode::{Fidelity, LossyTarget};
        match fidelity {
            Fidelity::Lossless => {
                // `with_lossless(true)` is only truly lossless under
                // `encode-imazen`; elsewhere it sets `self.lossless = true` over
                // a q100 lossy encode, which would make the report lie. Keep the
                // report honest by not claiming lossless on those builds.
                if cfg!(feature = "encode-imazen") {
                    self.with_lossless(true)
                } else {
                    self.with_lossless(false).with_generic_quality(100.0)
                }
            }
            Fidelity::Lossy(LossyTarget::CodecSpecificQuality(q)) => {
                self.with_lossless(false).with_generic_quality(q)
            }
            Fidelity::Lossy(LossyTarget::ApproxSsim2(s)) => {
                self.with_lossless(false).with_generic_quality(s)
            }
            Fidelity::Lossy(LossyTarget::ApproxButteraugli(d)) => {
                let q = (96.0 - d * 12.0).clamp(0.0, 100.0);
                self.with_lossless(false).with_generic_quality(q)
            }
            // `Fidelity` / `LossyTarget` are `#[non_exhaustive]`.
            _ => self.with_lossless(false),
        }
    }

    fn resolved_target_fidelity(&self) -> Option<zencodec::encode::Fidelity> {
        use zencodec::encode::Fidelity;
        if self.lossless {
            Some(Fidelity::Lossless)
        } else {
            self.trait_quality.map(Fidelity::codec_quality)
        }
    }

    fn with_alpha_quality(mut self, quality: f32) -> Self {
        self.inner = self.inner.alpha_quality(quality);
        self
    }

    fn alpha_quality(&self) -> Option<f32> {
        self.inner.alpha_quality
    }

    /// Core-adjusted resource estimate via zencodec's unified `estimate` API.
    ///
    /// Delegates to the crate's calibrated
    /// [`crate::heuristics::estimate_encode_threaded`] (memory / time /
    /// output, keyed on the AV1 `speed` preset and the input bytes-per-pixel
    /// stratum, adjusted for `compute.cores()`): wall time is divided by the
    /// measured Amdahl speedup — better than the linear `at_cores` division,
    /// which is therefore NOT applied on top (it would divide a second time)
    /// — and the peak tiers carry the measured per-thread working-set term
    /// (`ThreadingInfo::mem_bytes_per_thread`), so a many-core environment
    /// sees the higher parallel peak, not the single-thread one. The local
    /// [`crate::heuristics::ThreadingInfo`] maps onto the shared
    /// [`zencodec::estimate::ThreadingInformation`] only here at the boundary —
    /// the codec's local heuristics stay decoupled from the optional `zencodec`
    /// dependency, so a decode-only build still compiles.
    fn estimate_encode_resources(
        &self,
        image: &zencodec::estimate::ImageCharacteristics,
        compute: &zencodec::estimate::ComputeEnvironment,
    ) -> zencodec::estimate::ResourceEstimate {
        use zencodec::estimate::{ResourceEstimate, ThreadingInformation};
        let speed = self.inner.speed_value();
        let bpp = image.descriptor().bytes_per_pixel() as u8;
        let lti = crate::heuristics::encode_threading_info(image.pixels());
        let ti = if lti.parallel {
            // The AV1 encode saturates at `max_useful_threads` (the tile count,
            // which scales with image size). The local `parallel_fraction` /
            // `mem_bytes_per_thread` are not carried by the published
            // ThreadingInformation, which models only the saturation knee.
            ThreadingInformation::parallel(lti.max_useful_threads)
        } else {
            ThreadingInformation::SERIAL
        };
        let cores = compute.cores();
        let (w, h) = (image.width(), image.height());
        match (
            crate::heuristics::estimate_encode(w, h, bpp, speed),
            crate::heuristics::estimate_encode_threaded(w, h, bpp, speed, cores),
        ) {
            (Some(single), Some(threaded)) => {
                // wall = Amdahl-adjusted; cpu = total work ≈ the calibrated
                // single-thread time (threads=1 calibration, wall ≈ user).
                ResourceEstimate::new(threaded.peak_memory_bytes, threaded.time_ms as u64)
                    .with_peak_max(threaded.peak_memory_bytes_max)
                    .with_cpu_ms(single.time_ms as u64)
                    .with_threading(ti)
            }
            _ => ResourceEstimate::conservative(image).at_cores(cores),
        }
    }

    fn job(self) -> AvifEncodeJob {
        AvifEncodeJob {
            config: self,
            stop: None,
            exif: None,
            icc_profile: None,
            xmp: None,
            limits: ResourceLimits::none(),
            cicp: None,
            content_light_level: None,
            mastering_display: None,
            rotation: None,
            mirror: None,
            policy: None,
            canvas_size: None,
            loop_count: None,
        }
    }
}

// ── Encode Job ──────────────────────────────────────────────────────────────

/// Per-operation AVIF encode job.
#[cfg(feature = "encode")]
pub struct AvifEncodeJob {
    config: AvifEncoderConfig,
    stop: Option<zencodec::StopToken>,
    exif: Option<Arc<[u8]>>,
    icc_profile: Option<Arc<[u8]>>,
    xmp: Option<Arc<[u8]>>,
    limits: ResourceLimits,
    cicp: Option<zencodec::Cicp>,
    content_light_level: Option<zencodec::ContentLightLevel>,
    mastering_display: Option<zencodec::MasteringDisplay>,
    rotation: Option<u8>,
    mirror: Option<u8>,
    policy: Option<zencodec::encode::EncodePolicy>,
    canvas_size: Option<(u32, u32)>,
    loop_count: Option<Option<u32>>,
}

#[cfg(feature = "encode")]
impl AvifEncodeJob {
    /// Set EXIF metadata to embed in the encoded AVIF.
    #[must_use]
    pub fn with_exif(mut self, exif: impl Into<Arc<[u8]>>) -> Self {
        self.exif = Some(exif.into());
        self
    }
}

#[cfg(feature = "encode")]
impl zencodec::encode::EncodeJob for AvifEncodeJob {
    type Error = At<CodecError>;
    type Enc = AvifEncoder;
    type AnimationFrameEnc = AvifAnimationFrameEncoder;

    fn with_stop(mut self, stop: zencodec::StopToken) -> Self {
        self.stop = Some(stop);
        self
    }

    #[allow(deprecated)] // required trait method; callers use with_metadata_policy
    fn with_metadata(mut self, meta: Metadata) -> Self {
        if let Some(exif) = meta.exif {
            self.exif = Some(exif);
        }
        if let Some(icc) = meta.icc_profile {
            self.icc_profile = Some(icc);
        }
        if let Some(xmp) = meta.xmp {
            self.xmp = Some(xmp);
        }
        if let Some(cicp) = meta.cicp {
            self.cicp = Some(cicp);
        }
        if let Some(cll) = meta.content_light_level {
            self.content_light_level = Some(cll);
        }
        if let Some(mdcv) = meta.mastering_display {
            self.mastering_display = Some(mdcv);
        }
        // Map EXIF-style orientation to AVIF rotation/mirror boxes
        let (rotation, mirror) = orientation_to_avif(meta.orientation);
        self.rotation = rotation;
        self.mirror = mirror;
        self
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn with_policy(mut self, policy: zencodec::encode::EncodePolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    fn encoder(self) -> Result<AvifEncoder, At<CodecError>> {
        let mut config = self.config.inner.clone();
        // Resolve the color *description* (ICC vs CICP code points) through
        // zencodec's `resolve_color_emit` — the single source of truth for which
        // color carrier we emit. It reconciles a caller-supplied CICP / ICC
        // against AVIF's capabilities under the job's ColorEmitPolicy. The
        // returned CICP (if any) is lowered to AVIF's nclx carrier (all three
        // axes, so the matrix stays consistent); the ICC disposition picks the
        // bytes to embed. This subsumes the old "apply self.cicp verbatim".
        let (plan_cicp, plan_icc) =
            resolve_avif_color(self.cicp, self.icc_profile, self.policy.as_ref());
        if let Some(cicp) = plan_cicp {
            config = apply_cicp_to_config(config, cicp);
        }
        // Apply HDR metadata from Metadata
        if let Some(cll) = self.content_light_level {
            config = config.content_light_level(
                cll.max_content_light_level,
                cll.max_frame_average_light_level,
            );
        }
        if let Some(mdcv) = self.mastering_display {
            // ST 2086 (the mdcv box): f32 CIE xy (0.0–1.0) → 0.00002 units
            // (×50000), u16, stored verbatim by ravif/the box; the decoder
            // reads ×0.00002. (Was ×65535 — off by 1.31× vs spec, and broke
            // round-trip against our own decoder.)
            let xy_to_u16 = |v: f32| (v * 50000.0 + 0.5) as u16;
            config = config.mastering_display(crate::MasteringDisplayConfig {
                primaries: [
                    (
                        xy_to_u16(mdcv.primaries_xy[0][0]),
                        xy_to_u16(mdcv.primaries_xy[0][1]),
                    ),
                    (
                        xy_to_u16(mdcv.primaries_xy[1][0]),
                        xy_to_u16(mdcv.primaries_xy[1][1]),
                    ),
                    (
                        xy_to_u16(mdcv.primaries_xy[2][0]),
                        xy_to_u16(mdcv.primaries_xy[2][1]),
                    ),
                ],
                white_point: (
                    xy_to_u16(mdcv.white_point_xy[0]),
                    xy_to_u16(mdcv.white_point_xy[1]),
                ),
                // ST 2086: cd/m² → 0.0001 units (×10000). (Was ×256.)
                max_luminance: (mdcv.max_luminance * 10000.0 + 0.5) as u32,
                // ST 2086: cd/m² → 0.0001 units (×10000). (Was ×16384.)
                min_luminance: (mdcv.min_luminance * 10000.0 + 0.5) as u32,
            });
        }
        // Apply rotation/mirror from orientation metadata
        if let Some(rot) = self.rotation {
            config = config.rotation(rot);
        }
        if let Some(mir) = self.mirror {
            config = config.mirror(mir);
        }
        // Apply threading policy from ResourceLimits.
        // Skip Parallel — it means "use the ambient pool", so keep the codec's
        // own default rather than pinning a thread count. Dimensions are not
        // known yet here, so this only lowers the POLICY; the memory-budget
        // thread fit runs at encode time (`checked_config`), where it may pin
        // a lower count — including under Parallel — when the calibrated
        // estimate would exceed `max_memory_bytes` (or available RAM).
        if !matches!(self.limits.threading(), zencodec::ThreadingPolicy::Parallel) {
            let threads = policy_to_threads(self.limits.threading());
            if threads > 0 {
                config = config.threads(Some(threads as usize));
            }
            // threads == 0 only from future unknown variants; leave default
        }
        // Apply encode policy: suppress metadata the policy disallows.
        let exif = match self.policy {
            Some(ref p) if !p.resolve_exif(true) => None,
            _ => self.exif,
        };
        // `plan_icc` already encodes the keep/synthesize/drop decision from
        // resolve_color_emit. The coarse `embed_icc: Some(false)` gate is an
        // explicit caller override that can still suppress an otherwise-kept ICC.
        let icc_profile = match self.policy {
            Some(ref p) if !p.resolve_icc(true) => None,
            _ => plan_icc,
        };
        let xmp = match self.policy {
            Some(ref p) if !p.resolve_xmp(true) => None,
            _ => self.xmp,
        };
        Ok(AvifEncoder {
            config,
            stop: self.stop,
            exif,
            icc_profile,
            xmp,
            limits: self.limits,
            caller_cicp: plan_cicp,
            threads_note: None,
        })
    }

    fn with_canvas_size(mut self, width: u32, height: u32) -> Self {
        self.canvas_size = Some((width, height));
        self
    }

    fn with_loop_count(mut self, count: Option<u32>) -> Self {
        self.loop_count = Some(count);
        self
    }

    fn animation_frame_encoder(self) -> Result<AvifAnimationFrameEncoder, At<CodecError>> {
        let mut config = self.config.inner.clone();
        // Resolve color description the same way as the still path (single source
        // of truth): lower the resolved CICP to nclx and carry the resolved ICC.
        let (plan_cicp, plan_icc) =
            resolve_avif_color(self.cicp, self.icc_profile, self.policy.as_ref());
        if let Some(cicp) = plan_cicp {
            config = apply_cicp_to_config(config, cicp);
        }
        // Apply HDR metadata
        if let Some(cll) = self.content_light_level {
            config = config.content_light_level(
                cll.max_content_light_level,
                cll.max_frame_average_light_level,
            );
        }
        if let Some(mdcv) = self.mastering_display {
            let xy_to_u16 = |v: f32| (v * 65535.0 + 0.5) as u16;
            config = config.mastering_display(crate::MasteringDisplayConfig {
                primaries: [
                    (
                        xy_to_u16(mdcv.primaries_xy[0][0]),
                        xy_to_u16(mdcv.primaries_xy[0][1]),
                    ),
                    (
                        xy_to_u16(mdcv.primaries_xy[1][0]),
                        xy_to_u16(mdcv.primaries_xy[1][1]),
                    ),
                    (
                        xy_to_u16(mdcv.primaries_xy[2][0]),
                        xy_to_u16(mdcv.primaries_xy[2][1]),
                    ),
                ],
                white_point: (
                    xy_to_u16(mdcv.white_point_xy[0]),
                    xy_to_u16(mdcv.white_point_xy[1]),
                ),
                max_luminance: (mdcv.max_luminance * 256.0 + 0.5) as u32,
                min_luminance: (mdcv.min_luminance * 16384.0 + 0.5) as u32,
            });
        }
        if let Some(rot) = self.rotation {
            config = config.rotation(rot);
        }
        if let Some(mir) = self.mirror {
            config = config.mirror(mir);
        }
        // Apply threading policy (canvas dimensions are not known yet here;
        // the memory-budget thread fit runs in `finish_inner`, where it may
        // pin a lower count — including under Parallel).
        if !matches!(self.limits.threading(), zencodec::ThreadingPolicy::Parallel) {
            let threads = policy_to_threads(self.limits.threading());
            if threads > 0 {
                config = config.threads(Some(threads as usize));
            }
        }
        // Apply metadata
        let policy = self.policy.as_ref();
        if let Some(exif) = self.exif
            && policy.is_none_or(|p| p.resolve_exif(true))
        {
            config = config.exif(exif.to_vec());
        }
        if let Some(icc) = plan_icc
            && policy.is_none_or(|p| p.resolve_icc(true))
        {
            config = config.icc_profile(icc.to_vec());
        }
        if let Some(xmp) = self.xmp
            && policy.is_none_or(|p| p.resolve_xmp(true))
        {
            config = config.xmp(xmp.to_vec());
        }

        let (canvas_w, canvas_h) = match self.canvas_size {
            Some((w, h)) => (Some(w), Some(h)),
            None => (None, None),
        };

        Ok(AvifAnimationFrameEncoder {
            config,
            stop: self.stop,
            frames: Vec::new(),
            pixel_format: None,
            canvas_width: canvas_w,
            canvas_height: canvas_h,
            limits: self.limits,
            frame_count: 0,
        })
    }
}

/// Lower a [`zencodec::Cicp`] onto the native AVIF encoder config, writing all
/// three nclx axes (primaries, transfer, matrix) so the config carries a
/// coherent triple rather than a partial/stale one (the prior bug set only some
/// axes). Note: the *emitted* nclx matrix is determined by ravif's own YCbCr
/// conversion (BT.601), so `config.matrix_coefficients` is informational —
/// no available backend consults it (its only reader was the deprecated
/// svtav1 path); the coherent triple is kept for introspection and any
/// future backend.
#[cfg(feature = "encode")]
fn apply_cicp_to_config(
    config: crate::EncoderConfig,
    cicp: zencodec::Cicp,
) -> crate::EncoderConfig {
    config
        .color_primaries(cicp.color_primaries)
        .transfer_characteristics(cicp.transfer_characteristics)
        .matrix_coefficients(cicp.matrix_coefficients)
}

/// Resolve which color description to emit for an AVIF encode, the single source
/// of truth for the color carrier.
///
/// Feeds a [`zencodec::SourceColor`] (built from the caller's CICP and/or ICC)
/// and AVIF's `AVIF_ENCODE_CAPABILITIES`
/// through [`zencodec::resolve_color_emit`] under the job's
/// [`ColorEmitPolicy`](zencodec::ColorEmitPolicy) (defaulting to
/// [`Balanced`](zencodec::ColorEmitPolicy::Balanced)). Returns:
///
/// - the CICP to write to nclx (`None` ⇒ leave the descriptor / encoder default),
/// - the ICC bytes to embed, materialized from the plan's
///   [`IccDisposition`](zencodec::IccDisposition):
///   [`KeepSource`](zencodec::IccDisposition::KeepSource) keeps the caller's
///   bytes, [`SynthesizeFrom`](zencodec::IccDisposition::SynthesizeFrom) fetches
///   a bundled profile for the primaries (sRGB ⇒ `None`, so nothing is embedded),
///   and [`Drop`](zencodec::IccDisposition::Drop) emits no ICC.
///
/// Channel count is left unset here: pixels aren't known yet at job-build time,
/// and the resolver's grayscale path still fires off an ICC that declares gray.
#[cfg(feature = "encode")]
fn resolve_avif_color(
    cicp: Option<zencodec::Cicp>,
    icc: Option<Arc<[u8]>>,
    policy: Option<&zencodec::encode::EncodePolicy>,
) -> (Option<zencodec::Cicp>, Option<Arc<[u8]>>) {
    let mut src = zencodec::SourceColor::default();
    if let Some(c) = cicp {
        src = src.with_cicp(c).with_color_authority(ColorAuthority::Cicp);
    }
    if let Some(ref bytes) = icc {
        src = src
            .with_icc_profile(bytes.clone())
            .with_color_authority(ColorAuthority::Icc);
    }

    let emit_policy = policy
        .map(|p| p.resolve_color(zencodec::ColorEmitPolicy::Balanced))
        .unwrap_or(zencodec::ColorEmitPolicy::Balanced);

    let plan = zencodec::resolve_color_emit(&src, &AVIF_ENCODE_CAPABILITIES, emit_policy);

    let icc_out = match plan.icc {
        zencodec::IccDisposition::KeepSource => icc,
        zencodec::IccDisposition::SynthesizeFrom(c) => {
            // Transfer-aware lowering: `synthesize_icc_for_cicp` matches the TRC, so a
            // BT.2020-PQ source never gets the SDR-TRC Rec.2020 profile. `Profile`
            // → own a copy; `NotNeeded`/`NeedsCms`/`CmsUnsupported` → no ICC (nclx
            // still carries the color, and AVIF nclx is a sole-safe carrier).
            use zenpixels_convert::icc_profiles::SynthesizedIcc;
            match zenpixels_convert::icc_profiles::synthesize_icc_for_cicp(c) {
                SynthesizedIcc::Profile(bytes) => Some(Arc::<[u8]>::from(bytes.as_ref())),
                _ => None,
            }
        }
        zencodec::IccDisposition::Drop => None,
        // IccDisposition is #[non_exhaustive]; a future variant defaults to not
        // embedding an ICC (safe — nclx still carries the color).
        _ => None,
    };

    (plan.cicp, icc_out)
}

// ── Encoder ─────────────────────────────────────────────────────────────────

/// Single-image AVIF encoder.
#[cfg(feature = "encode")]
pub struct AvifEncoder {
    config: crate::EncoderConfig,
    stop: Option<zencodec::StopToken>,
    exif: Option<Arc<[u8]>>,
    icc_profile: Option<Arc<[u8]>>,
    xmp: Option<Arc<[u8]>>,
    limits: ResourceLimits,
    /// CICP resolved by [`resolve_color_emit`] in `encoder()` (caller-supplied
    /// CICP, possibly derived from an ICC). When set, it is the authority — the
    /// pixel-descriptor color in `apply_descriptor_color` only fills axes this
    /// leaves *unspecified*, so a caller's CICP is never clobbered.
    caller_cicp: Option<zencodec::Cicp>,
    /// Record of a memory-budget thread reduction made by `checked_config`
    /// (reductions are never silent). Attached to the [`EncodeOutput`] as a
    /// `String` extra in `make_output`.
    threads_note: Option<String>,
}

#[cfg(feature = "encode")]
impl AvifEncoder {
    fn build_config(&self) -> crate::EncoderConfig {
        let mut cfg = self.config.clone();
        if let Some(ref exif) = self.exif {
            cfg = cfg.exif(exif.to_vec());
        }
        if let Some(ref icc) = self.icc_profile {
            cfg = cfg.icc_profile(icc.to_vec());
        }
        if let Some(ref xmp) = self.xmp {
            cfg = cfg.xmp(xmp.to_vec());
        }
        cfg
    }

    /// Pre-flight resource checks + memory-adaptive thread fit, returning the
    /// final native config for this encode (any budget-driven thread pin
    /// applied on top of [`build_config`](Self::build_config)).
    ///
    /// Checks, in order: dimension caps; the raw input-buffer size against
    /// `max_memory_bytes` (cheap floor, kept from the pre-calibrated era);
    /// then the CALIBRATED thread-aware peak estimate at the fitted thread
    /// count via [`fit_encode_threads_to_memory`] — which reduces the thread
    /// count to fit the budget and errors only when even the single-threaded
    /// estimate does not fit. Any reduction is recorded in
    /// [`threads_note`](Self::threads_note) (surfaced by `make_output`).
    fn checked_config(
        &mut self,
        w: usize,
        h: usize,
        bpp: u64,
    ) -> Result<crate::EncoderConfig, At<Error>> {
        self.limits
            .check_dimensions(w as u32, h as u32)
            .map_err(|_| {
                at!(Error::ImageTooLarge {
                    width: w as u32,
                    height: h as u32,
                })
            })?;
        let estimated_mem = w as u64 * h as u64 * bpp;
        self.limits
            .check_memory(estimated_mem)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;
        let (pin, note) = fit_encode_threads_to_memory(
            &self.limits,
            &self.config,
            w as u32,
            h as u32,
            bpp as u8,
        )?;
        self.threads_note = note;
        let mut cfg = self.build_config();
        if let Some(n) = pin {
            cfg = cfg.threads(Some(n));
        }
        Ok(cfg)
    }

    fn make_output(&self, data: Vec<u8>) -> Result<EncodeOutput, At<Error>> {
        self.limits
            .check_output_size(data.len() as u64)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;
        let mut out = EncodeOutput::new(data, ImageFormat::Avif);
        if let Some(ref note) = self.threads_note {
            // Reductions are never silent: readable by the caller via
            // `output.extras::<String>()` (no zenavif-specific type — this
            // crate adds no public API for it).
            out = out.with_extras(note.clone());
        }
        Ok(out)
    }

    fn stop_token(&self) -> almost_enough::StopToken {
        match &self.stop {
            Some(s) => s.clone(),
            None => almost_enough::StopToken::new(enough::Unstoppable),
        }
    }

    /// Fill in CICP color axes from the pixel descriptor for the axes a
    /// caller-supplied CICP left *unspecified*. A `Metadata`-set CICP (resolved
    /// in `encoder()` into [`caller_cicp`](Self::caller_cicp) and already lowered
    /// onto the config) is the authority and is never overwritten here — fixing
    /// the prior bug where the descriptor unconditionally clobbered the caller's
    /// primaries/transfer (and never set a matching matrix). For HDR transfers
    /// (PQ/HLG), also switches to 10-bit encoding depth.
    fn apply_descriptor_color(&mut self, desc: PixelDescriptor) {
        use zenpixels::{ColorPrimaries, TransferFunction};

        let transfer = desc.transfer;
        let primaries = desc.primaries;

        // Map transfer function to CICP transfer_characteristics
        let tc = match transfer {
            TransferFunction::Pq => Some(16u8),
            TransferFunction::Hlg => Some(18),
            TransferFunction::Bt709 => Some(1),
            TransferFunction::Srgb => Some(13),
            TransferFunction::Linear => Some(8),
            _ => None,
        };

        // Map color primaries to CICP color_primaries
        let cp = match primaries {
            ColorPrimaries::Bt2020 => Some(9u8),
            ColorPrimaries::DisplayP3 => Some(12),
            ColorPrimaries::Bt709 => Some(1),
            _ => None,
        };

        // Which axes did the caller's CICP already pin? An axis is "specified"
        // only if caller_cicp is present AND the code point is not the H.273
        // unspecified/reserved sentinel (primaries/transfer: 0 reserved, 2
        // unspecified). Matrix 0 is Identity — a real value, not unspecified.
        let caller = self.caller_cicp;
        let cp_specified = caller.is_some_and(|c| !matches!(c.color_primaries, 0 | 2));
        let tc_specified = caller.is_some_and(|c| !matches!(c.transfer_characteristics, 0 | 2));

        // Fill only the unspecified axes from the descriptor, so the caller's
        // CICP wins on the axes it pinned.
        if let Some(tc_val) = tc
            && !tc_specified
        {
            self.config = self.config.clone().transfer_characteristics(tc_val);
        }
        if let Some(cp_val) = cp
            && !cp_specified
        {
            self.config = self.config.clone().color_primaries(cp_val);
        }

        // Keep the matrix consistent with the primaries/transfer we just wrote.
        // When the caller supplied no CICP at all, its matrix wasn't applied in
        // encoder(), so derive one from the descriptor here (RGB content ⇒
        // Identity/0, matching `Cicp::from_descriptor`) so the config carries a
        // coherent triple (informational — no available backend reads the
        // matrix field). When the caller DID supply a CICP, its matrix is
        // already on the config; leave it alone.
        if caller.is_none() && (cp.is_some() || tc.is_some()) {
            let mc = zenpixels::Cicp::from_descriptor(&desc)
                .map(|c| c.matrix_coefficients)
                .unwrap_or(0);
            self.config = self.config.clone().matrix_coefficients(mc);
        }

        // For PQ/HLG, switch to 10-bit depth (the native HDR depth for AV1)
        if matches!(transfer, TransferFunction::Pq | TransferFunction::Hlg) {
            self.config = self.config.clone().bit_depth(crate::EncodeBitDepth::Ten);
        }

        // Map narrow signal range to limited pixel range
        if desc.signal_range == zenpixels::SignalRange::Narrow {
            self.config = self
                .config
                .clone()
                .pixel_range(crate::EncodePixelRange::Limited);
        }
    }

    /// Convert f32 RGB pixels to u16 and encode via the 16-bit path.
    /// Used for HDR (PQ/HLG) f32 data that would be corrupted by linear_to_srgb_u8().
    fn encode_f32_as_u16_rgb(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 6)?; // 6 bytes per u16 RGB pixel
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgb: Vec<Rgb<u16>> = raw
            .chunks_exact(12)
            .map(|c| {
                let r = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let g = f32::from_le_bytes([c[4], c[5], c[6], c[7]]);
                let b = f32::from_le_bytes([c[8], c[9], c[10], c[11]]);
                Rgb {
                    r: (r.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                    g: (g.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                    b: (b.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(rgb, w, h);
        let result = crate::encode_rgb16(img.as_ref(), &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    /// Convert f32 RGBA pixels to u16 and encode via the 16-bit path.
    /// Used for HDR (PQ/HLG) f32 data that would be corrupted by linear_to_srgb_u8().
    fn encode_f32_as_u16_rgba(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 8)?; // 8 bytes per u16 RGBA pixel
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgba: Vec<Rgba<u16>> = raw
            .chunks_exact(16)
            .map(|c| {
                let r = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let g = f32::from_le_bytes([c[4], c[5], c[6], c[7]]);
                let b = f32::from_le_bytes([c[8], c[9], c[10], c[11]]);
                let a = f32::from_le_bytes([c[12], c[13], c[14], c[15]]);
                Rgba {
                    r: (r.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                    g: (g.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                    b: (b.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                    a: (a.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(rgba, w, h);
        let result = crate::encode_rgba16(img.as_ref(), &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    // ── Per-format encode helpers ──────────────────────────────────────

    fn do_encode_rgb8(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 3)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgb: &[Rgb<u8>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgb, w, h);
        let result = crate::encode_rgb8(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgba8(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 4)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgba: &[Rgba<u8>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgba, w, h);
        let result = crate::encode_rgba8(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_gray8(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 1)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        #[cfg(feature = "encode-mono")]
        {
            // True monochrome AV1 (Cs400): luma-only bitstream, no chroma
            // RDO — measured 2-3x faster at output-byte parity vs the RGB
            // expansion (imazen/zenavif#6).
            let img = imgref::ImgRef::new(&raw, w, h);
            let result = crate::encode_gray8(img, &cfg, stop)?;
            self.make_output(result.avif_file)
        }
        #[cfg(not(feature = "encode-mono"))]
        {
            // Gray → RGB for encoding. RGB→YCbCr of R=G=B is exactly
            // neutral chroma, so this is pixel-safe — just slower (chroma
            // RDO still runs). The `encode-mono` feature routes through
            // zenravif's Cs400 path instead.
            let rgb: Vec<Rgb<u8>> = raw.iter().map(|&g| Rgb { r: g, g, b: g }).collect();
            let img = imgref::ImgVec::new(rgb, w, h);
            let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
            self.make_output(result.avif_file)
        }
    }

    fn do_encode_rgb_f32(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        use linear_srgb::default::linear_to_srgb_u8;
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 12)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgb: Vec<Rgb<u8>> = raw
            .chunks_exact(12)
            .map(|c| {
                let r = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let g = f32::from_le_bytes([c[4], c[5], c[6], c[7]]);
                let b = f32::from_le_bytes([c[8], c[9], c[10], c[11]]);
                Rgb {
                    r: linear_to_srgb_u8(r.clamp(0.0, 1.0)),
                    g: linear_to_srgb_u8(g.clamp(0.0, 1.0)),
                    b: linear_to_srgb_u8(b.clamp(0.0, 1.0)),
                }
            })
            .collect();
        let img = imgref::ImgVec::new(rgb, w, h);
        let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgba_f32(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        use linear_srgb::default::linear_to_srgb_u8;
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 16)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgba: Vec<Rgba<u8>> = raw
            .chunks_exact(16)
            .map(|c| {
                let r = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let g = f32::from_le_bytes([c[4], c[5], c[6], c[7]]);
                let b = f32::from_le_bytes([c[8], c[9], c[10], c[11]]);
                let a = f32::from_le_bytes([c[12], c[13], c[14], c[15]]);
                Rgba {
                    r: linear_to_srgb_u8(r.clamp(0.0, 1.0)),
                    g: linear_to_srgb_u8(g.clamp(0.0, 1.0)),
                    b: linear_to_srgb_u8(b.clamp(0.0, 1.0)),
                    a: (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(rgba, w, h);
        let result = crate::encode_rgba8(img.as_ref(), &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_gray_f32(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        use linear_srgb::default::linear_to_srgb_u8;
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 4)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgb: Vec<Rgb<u8>> = raw
            .chunks_exact(4)
            .map(|c| {
                let v = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let s = linear_to_srgb_u8(v.clamp(0.0, 1.0));
                Rgb { r: s, g: s, b: s }
            })
            .collect();
        let img = imgref::ImgVec::new(rgb, w, h);
        let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgb16(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 6)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgb: &[Rgb<u16>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgb, w, h);
        let result = crate::encode_rgb16(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgba16(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 8)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgba: &[Rgba<u16>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgba, w, h);
        let result = crate::encode_rgba16(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }
}

#[cfg(feature = "encode")]
impl zencodec::encode::Encoder for AvifEncoder {
    type Error = At<CodecError>;

    fn reject(op: zencodec::UnsupportedOperation) -> At<CodecError> {
        // Bare native error exiting the trait boundary: `.into()` routes through
        // `From<Error> for At<CodecError>`, locating + enveloping in one step.
        Error::UnsupportedOperation(op).into()
    }

    fn encode_srgba8(
        self,
        data: &mut [u8],
        make_opaque: bool,
        width: u32,
        height: u32,
        stride_pixels: u32,
    ) -> Result<EncodeOutput, At<CodecError>> {
        self.encode_srgba8_inner(data, make_opaque, width, height, stride_pixels)
            .map_err(zencodec::CodecError::of)
    }

    fn encode(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<CodecError>> {
        self.encode_inner(pixels).map_err(zencodec::CodecError::of)
    }
}

// ── Animation Frame Encoder ──────────────────────────────────────────────────

/// Buffered frame for animation encoding.
#[cfg(feature = "encode")]
enum BufferedFrame {
    Rgb8 {
        pixels: imgref::ImgVec<Rgb<u8>>,
        duration_ms: u32,
    },
    Rgba8 {
        pixels: imgref::ImgVec<Rgba<u8>>,
        duration_ms: u32,
    },
    Rgb16 {
        pixels: imgref::ImgVec<Rgb<u16>>,
        duration_ms: u32,
    },
    Rgba16 {
        pixels: imgref::ImgVec<Rgba<u16>>,
        duration_ms: u32,
    },
}

/// Full-frame animation encoder for AVIF.
///
/// Buffers frames and encodes them all on [`finish()`](zencodec::encode::AnimationFrameEncoder::finish).
/// All frames must have the same dimensions and pixel format.
#[cfg(feature = "encode")]
pub struct AvifAnimationFrameEncoder {
    config: crate::EncoderConfig,
    stop: Option<zencodec::StopToken>,
    frames: Vec<BufferedFrame>,
    pixel_format: Option<zenpixels::PixelFormat>,
    canvas_width: Option<u32>,
    canvas_height: Option<u32>,
    limits: ResourceLimits,
    /// Number of frames pushed so far, for max_frames enforcement.
    frame_count: u32,
}

#[cfg(feature = "encode")]
impl AvifAnimationFrameEncoder {
    fn stop_token(&self) -> almost_enough::StopToken {
        match &self.stop {
            Some(s) => s.clone(),
            None => almost_enough::StopToken::new(enough::Unstoppable),
        }
    }
}

#[cfg(feature = "encode")]
impl zencodec::encode::AnimationFrameEncoder for AvifAnimationFrameEncoder {
    type Error = At<CodecError>;

    fn reject(op: zencodec::UnsupportedOperation) -> At<CodecError> {
        // Bare native error exiting the trait boundary: `.into()` routes through
        // `From<Error> for At<CodecError>`, locating + enveloping in one step.
        Error::UnsupportedOperation(op).into()
    }

    fn push_frame(
        &mut self,
        pixels: PixelSlice<'_>,
        duration_ms: u32,
        stop: Option<&dyn Stop>,
    ) -> Result<(), At<CodecError>> {
        self.push_frame_inner(pixels, duration_ms, stop)
            .map_err(zencodec::CodecError::of)
    }

    fn finish(self, stop: Option<&dyn Stop>) -> Result<EncodeOutput, At<CodecError>> {
        self.finish_inner(stop).map_err(zencodec::CodecError::of)
    }
}

// ── Decoding ────────────────────────────────────────────────────────────────

/// AVIF decoder configuration implementing [`zencodec::decode::DecoderConfig`].
#[derive(Clone, Debug)]
pub struct AvifDecoderConfig {
    inner: crate::DecoderConfig,
    /// When true, gain map and depth map data will be attached to
    /// `DecodeOutput` extras. Default: false.
    extract_gain_map: bool,
    /// How to handle the image's stored orientation (`irot`/`imir`).
    ///
    /// Default: [`OrientationHint::Preserve`](zencodec::OrientationHint::Preserve)
    /// — the decoder does **not** bake the orientation into the pixels: decode
    /// returns pixels in stored orientation and [`ImageInfo`] reports the
    /// stored dims + the intrinsic [`Orientation`](zencodec::Orientation) tag.
    /// With [`Correct`](zencodec::OrientationHint::Correct) the decoder applies
    /// the orientation and reports display dimensions with
    /// [`Orientation::Identity`](zencodec::Orientation::Identity).
    ///
    /// Set via [`with_orientation`](Self::with_orientation) or
    /// [`DecodeJob::with_orientation`](zencodec::decode::DecodeJob::with_orientation).
    orientation: zencodec::OrientationHint,
}

impl AvifDecoderConfig {
    /// Create a default AVIF decoder config.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: crate::DecoderConfig::new(),
            extract_gain_map: false,
            orientation: zencodec::OrientationHint::Preserve,
        }
    }

    /// Set how the decoder handles the image's stored orientation
    /// (`irot`/`imir`). See [`orientation`](Self::orientation) for semantics.
    /// Default [`OrientationHint::Preserve`](zencodec::OrientationHint::Preserve).
    #[must_use]
    pub fn with_orientation(mut self, hint: zencodec::OrientationHint) -> Self {
        self.orientation = hint;
        self
    }

    /// Set resource limits.
    #[must_use]
    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        if let Some(max_pixels) = limits.max_pixels {
            self.inner = self
                .inner
                .frame_size_limit(max_pixels.min(u32::MAX as u64) as u32);
        }
        if let Some(max_w) = limits.max_width
            && let Some(max_h) = limits.max_height
        {
            let max = max_w as u64 * max_h as u64;
            self.inner = self.inner.frame_size_limit(max.min(u32::MAX as u64) as u32);
        }
        self
    }

    /// Access the underlying [`crate::DecoderConfig`].
    /// Set the number of decode threads (0 = auto).
    #[must_use]
    pub fn with_threads(mut self, threads: u32) -> Self {
        self.inner = self.inner.threads(threads);
        self
    }

    /// Apply film grain synthesis during decode.
    #[must_use]
    pub fn with_film_grain(mut self, apply: bool) -> Self {
        self.inner = self.inner.apply_grain(apply);
        self
    }

    /// Enable extraction of gain map and depth map data into `DecodeOutput`
    /// extras.
    ///
    /// When enabled, the gain map is converted to a normalized
    /// [`GainMapSource`](zencodec::gainmap::GainMapSource) and attached to
    /// decode output so callers can retrieve it via
    /// `output.extras::<zencodec::gainmap::GainMapSource>()`.
    /// Depth maps (when available) are attached as
    /// [`AvifDepthMap`](crate::AvifDepthMap). Default: **false**.
    ///
    /// `ImageInfo` metadata (`supplements`, `GainMapPresence`) is always
    /// populated from container probe data regardless of this flag.
    #[must_use]
    pub fn with_extract_gain_map(mut self, extract: bool) -> Self {
        self.extract_gain_map = extract;
        self
    }

    /// Access the underlying [`crate::DecoderConfig`].
    #[must_use]
    pub fn inner(&self) -> &crate::DecoderConfig {
        &self.inner
    }

    /// Mutable access to the underlying [`crate::DecoderConfig`].
    pub fn inner_mut(&mut self) -> &mut crate::DecoderConfig {
        &mut self.inner
    }

    /// Convenience: decode image with this config.
    pub fn decode(&self, data: &[u8]) -> Result<DecodeOutput, At<CodecError>> {
        use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
        self.clone()
            .job()
            .decoder(Cow::Borrowed(data), &[])?
            .decode()
    }

    /// Convenience: probe image header with this config.
    pub fn probe_header(&self, data: &[u8]) -> Result<ImageInfo, At<CodecError>> {
        use zencodec::decode::{DecodeJob as _, DecoderConfig as _};
        self.clone().job().probe(data)
    }

    /// Convenience: probe full image metadata (may be expensive).
    pub fn probe_full(&self, data: &[u8]) -> Result<ImageInfo, At<CodecError>> {
        use zencodec::decode::{DecodeJob as _, DecoderConfig as _};
        self.clone().job().probe_full(data)
    }

    /// Convenience: decode into a pre-allocated RGB8 buffer.
    pub fn decode_into_rgb8(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgb<u8>>,
    ) -> Result<ImageInfo, At<CodecError>> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        map_rgb8_rows(&output.into_buffer(), &mut dst, |s, d| {
            d.copy_from_slice(s);
        });
        Ok(info)
    }

    /// Convenience: decode into a pre-allocated RGBA8 buffer.
    pub fn decode_into_rgba8(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgba<u8>>,
    ) -> Result<ImageInfo, At<CodecError>> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        map_rgba8_rows(&output.into_buffer(), &mut dst, |s, d| {
            d.copy_from_slice(s);
        });
        Ok(info)
    }

    /// Convenience: decode into a pre-allocated RGB f32 buffer.
    pub fn decode_into_rgb_f32(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgb<f32>>,
    ) -> Result<ImageInfo, At<CodecError>> {
        use linear_srgb::default::srgb_u8_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        map_rgb8_rows(&output.into_buffer(), &mut dst, |s, d| {
            for (px, out) in s.iter().zip(d.iter_mut()) {
                *out = Rgb {
                    r: srgb_u8_to_linear(px.r),
                    g: srgb_u8_to_linear(px.g),
                    b: srgb_u8_to_linear(px.b),
                };
            }
        });
        Ok(info)
    }

    /// Convenience: decode into a pre-allocated RGBA f32 buffer.
    pub fn decode_into_rgba_f32(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgba<f32>>,
    ) -> Result<ImageInfo, At<CodecError>> {
        use linear_srgb::default::srgb_u8_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        map_rgba8_rows(&output.into_buffer(), &mut dst, |s, d| {
            for (px, out) in s.iter().zip(d.iter_mut()) {
                *out = Rgba {
                    r: srgb_u8_to_linear(px.r),
                    g: srgb_u8_to_linear(px.g),
                    b: srgb_u8_to_linear(px.b),
                    a: px.a as f32 / 255.0,
                };
            }
        });
        Ok(info)
    }

    /// Convenience: decode into a pre-allocated Gray f32 buffer.
    pub fn decode_into_gray_f32(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, rgb::Gray<f32>>,
    ) -> Result<ImageInfo, At<CodecError>> {
        use linear_srgb::default::srgb_u8_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        // BT.709 luma coefficients in linear light
        let (kr, kb) =
            crate::yuv_convert::matrix_coefficients(crate::yuv_convert::YuvMatrix::Bt709);
        let kg = 1.0 - kr - kb;
        map_rgb8_rows(&output.into_buffer(), &mut dst, |s, d| {
            for (px, out) in s.iter().zip(d.iter_mut()) {
                let r = srgb_u8_to_linear(px.r);
                let g = srgb_u8_to_linear(px.g);
                let b = srgb_u8_to_linear(px.b);
                *out = rgb::Gray(kr * r + kg * g + kb * b);
            }
        });
        Ok(info)
    }
}

/// Borrow a decoded buffer as `ImgRef<Rgb<u8>>`, converting only when it isn't
/// already RGB8 sRGB — this skips the redundant identity convert + full-image
/// allocation an unconditional `to_rgb8()` would do. Rows are walked in
/// O(height) (the prior `nth(y)` per-row lookup was O(height²)). `f` writes
/// each output row over the overlapping `min(width)`×`min(height)` region.
fn map_rgb8_rows<D>(
    buffer: &PixelBuffer,
    dst: &mut imgref::ImgRefMut<'_, D>,
    mut f: impl FnMut(&[Rgb<u8>], &mut [D]),
) {
    let converted;
    // The borrow-free fast path needs BOTH an exact descriptor match and a
    // stride that is a whole number of pixels (`try_as_imgref`'s second
    // condition). An odd-stride RGB8 buffer satisfies the first and not the
    // second, so fall back to the owning conversion instead of panicking.
    let src = if let Some(view) = buffer
        .try_as_imgref::<Rgb<u8>>()
        .filter(|_| buffer.descriptor() == PixelDescriptor::RGB8_SRGB)
    {
        view
    } else {
        converted = buffer.to_rgb8();
        converted.as_imgref()
    };
    let w = dst.width().min(src.width());
    let h = dst.height().min(src.height());
    for (src_row, dst_row) in src.rows().zip(dst.rows_mut()).take(h) {
        f(&src_row[..w], &mut dst_row[..w]);
    }
}

/// Like [`map_rgb8_rows`] but views the buffer as RGBA8 (preserving alpha).
fn map_rgba8_rows<D>(
    buffer: &PixelBuffer,
    dst: &mut imgref::ImgRefMut<'_, D>,
    mut f: impl FnMut(&[Rgba<u8>], &mut [D]),
) {
    let converted;
    // Same fast-path/fallback split as `map_rgb8_rows` — see the note there.
    let src = if let Some(view) = buffer
        .try_as_imgref::<Rgba<u8>>()
        .filter(|_| buffer.descriptor() == PixelDescriptor::RGBA8_SRGB)
    {
        view
    } else {
        converted = buffer.to_rgba8();
        converted.as_imgref()
    };
    let w = dst.width().min(src.width());
    let h = dst.height().min(src.height());
    for (src_row, dst_row) in src.rows().zip(dst.rows_mut()).take(h) {
        f(&src_row[..w], &mut dst_row[..w]);
    }
}

impl Default for AvifDecoderConfig {
    fn default() -> Self {
        Self::new()
    }
}

static DECODE_DESCRIPTORS: &[PixelDescriptor] = &[
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::RGB16_SRGB,
    PixelDescriptor::RGBA16_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::GRAY16_SRGB,
];

static AVIF_DECODE_CAPABILITIES: zencodec::decode::DecodeCapabilities =
    zencodec::decode::DecodeCapabilities::new()
        .with_icc(true)
        .with_exif(true)
        .with_xmp(true)
        .with_cicp(true)
        .with_stop(true)
        .with_animation(true)
        .with_cheap_probe(true)
        .with_streaming(true)
        .with_hdr(true)
        .with_gain_map(true)
        .with_reconstructs_hdr(true)
        .with_native_gray(true)
        .with_native_16bit(true)
        .with_native_alpha(true)
        .with_enforces_max_pixels(true)
        .with_enforces_max_memory(true)
        .with_enforces_max_input_bytes(true)
        .with_threads_supported_range(1, 256);

impl zencodec::decode::DecoderConfig for AvifDecoderConfig {
    type Error = At<CodecError>;
    type Job<'a> = AvifDecodeJob;

    fn formats() -> &'static [ImageFormat] {
        &[ImageFormat::Avif]
    }

    fn supported_descriptors() -> &'static [PixelDescriptor] {
        DECODE_DESCRIPTORS
    }

    fn capabilities() -> &'static zencodec::decode::DecodeCapabilities {
        &AVIF_DECODE_CAPABILITIES
    }

    /// Core-adjusted resource estimate via zencodec's unified `estimate` API.
    ///
    /// Mirrors [`estimate_encode_resources`](AvifEncoderConfig::estimate_encode_resources):
    /// delegates to the crate's calibrated [`crate::heuristics::estimate_decode`]
    /// (peak memory / time, keyed on the decoded output bytes-per-pixel) and maps
    /// the local [`crate::heuristics::ThreadingInfo`] onto the shared
    /// [`zencodec::estimate::ThreadingInformation`] only here at the boundary, so
    /// the local heuristics stay decoupled from the optional `zencodec`
    /// dependency.
    ///
    /// The estimate covers the *whole* decode: the AV1 frame/tile working set
    /// (owned by `rav1d-safe`) plus zenavif's own RGB(A) output buffer. AVIF
    /// decode is only partly parallel (tile decode parallelises; the YUV→RGB
    /// conversion does not), so the [`ThreadingInformation`] knee is
    /// conservative — see [`crate::heuristics::decode_threading_info`].
    fn estimate_decode_resources(
        &self,
        image: &zencodec::estimate::ImageCharacteristics,
        compute: &zencodec::estimate::ComputeEnvironment,
    ) -> zencodec::estimate::ResourceEstimate {
        use zencodec::estimate::{ResourceEstimate, ThreadingInformation};
        let bpp = image.descriptor().bytes_per_pixel() as u8;
        let lti = crate::heuristics::decode_threading_info(image.pixels());
        let ti = if lti.parallel {
            ThreadingInformation::parallel(lti.max_useful_threads)
        } else {
            ThreadingInformation::SERIAL
        };
        match crate::heuristics::estimate_decode(image.width(), image.height(), bpp) {
            Some(e) => ResourceEstimate::new(e.peak_memory_bytes, e.time_ms as u64)
                .with_threading(ti)
                .at_cores(compute.cores()),
            None => ResourceEstimate::conservative(image).at_cores(compute.cores()),
        }
    }

    fn job<'a>(self) -> Self::Job<'a> {
        let extract_gain_map = self.extract_gain_map;
        let orientation = self.orientation;
        AvifDecodeJob {
            config: self,
            stop: None,
            limits: ResourceLimits::none(),
            start_frame_index: 0,
            policy: None,
            extract_gain_map,
            gain_map_render: zencodec::GainMapRender::default(),
            orientation,
        }
    }
}

// ── Decode Job ──────────────────────────────────────────────────────────────

/// Per-operation AVIF decode job.
pub struct AvifDecodeJob {
    config: AvifDecoderConfig,
    stop: Option<zencodec::StopToken>,
    limits: ResourceLimits,
    start_frame_index: u32,
    policy: Option<zencodec::decode::DecodePolicy>,
    /// When true, attach gain map and depth map data to `DecodeOutput` extras.
    /// Default: false. Metadata (supplements, `GainMapPresence`) is always
    /// populated regardless of this flag.
    extract_gain_map: bool,
    /// Gain-map rendition intent (zencodec 0.1.21). `Components` decodes
    /// the gain-map AV1 payload into a
    /// [`zencodec::decode::DecodedGainMap`]; `ReconstructHdr` additionally
    /// applies it (ultrahdr-core) producing linear f32 HDR pixels — see
    /// `with_gain_map_render`. Default `BaseOnly`.
    gain_map_render: zencodec::GainMapRender,
    /// How to handle the image's stored orientation (`irot`/`imir`).
    /// Default [`OrientationHint::Preserve`](zencodec::OrientationHint::Preserve).
    orientation: zencodec::OrientationHint,
}

impl AvifDecodeJob {
    fn effective_config(&self) -> crate::DecoderConfig {
        let mut cfg = self.config.inner.clone();
        if let Some(max_pixels) = self.limits.max_pixels {
            cfg = cfg.frame_size_limit(max_pixels.min(u32::MAX as u64) as u32);
        }
        // Apply threading policy from ResourceLimits.
        // Skip Parallel — it means "use the ambient pool", so keep the codec's
        // own default (which is 1 thread to avoid the rav1d-safe DisjointMut
        // race condition).
        if !matches!(self.limits.threading(), zencodec::ThreadingPolicy::Parallel) {
            let threads = policy_to_threads(self.limits.threading());
            cfg = cfg.threads(threads);
        }
        // Forward resource limits to the container parser.
        if let Some(mem) = self.limits.max_memory_bytes {
            cfg.parser_peak_memory_limit = Some(mem);
        }
        if let Some(px) = self.limits.max_pixels {
            // Convert pixels to megapixels (round up to avoid zero).
            let mp = px.div_ceil(1_000_000).min(u32::MAX as u64) as u32;
            cfg.parser_total_megapixels_limit = Some(mp);
        }
        if let Some(frames) = self.limits.max_frames {
            cfg.parser_max_animation_frames = Some(frames);
        }
        // Honor the allocation-fallibility preference for zenavif's own decode
        // buffers (output / grid / crop / per-row scratch). `CodecDefault` (the
        // default) keeps each site's own default fallibility. The AV1 frame/tile
        // buffers are owned by `rav1d-safe` and are not affected.
        cfg.alloc_pref = self.limits.prefer_fallible_allocations.into();
        cfg
    }

    /// Enable extraction of gain map and depth map data into `DecodeOutput`
    /// extras.
    ///
    /// When enabled, the gain map is converted to a normalized
    /// [`GainMapSource`](zencodec::gainmap::GainMapSource) and attached to
    /// decode output so callers can retrieve it via
    /// `output.extras::<zencodec::gainmap::GainMapSource>()`.
    /// Depth maps (when available) are attached as
    /// [`AvifDepthMap`](crate::AvifDepthMap). Default: **false**.
    ///
    /// `ImageInfo` metadata (`supplements`, `GainMapPresence`) is always
    /// populated from container probe data regardless of this flag.
    #[must_use]
    pub fn with_extract_gain_map(mut self, extract: bool) -> Self {
        self.extract_gain_map = extract;
        self
    }

    /// Check input data size against limits.
    fn check_input_size(&self, data: &[u8]) -> Result<(), At<Error>> {
        self.limits
            .check_input_size(data.len() as u64)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;
        Ok(())
    }

    /// Check decoded image dimensions and estimated memory against limits.
    fn check_decode_limits(&self, info: &crate::image::ImageInfo) -> Result<(), At<Error>> {
        self.limits
            .check_dimensions(info.width, info.height)
            .map_err(|_| {
                at!(Error::ImageTooLarge {
                    width: info.width,
                    height: info.height,
                })
            })?;
        // Estimate output memory: width * height * max_bpp (4 bytes for RGBA8, 8 for RGBA16)
        let bpp: u64 = if info.bit_depth > 8 {
            if info.has_alpha { 8 } else { 6 }
        } else if info.has_alpha {
            4
        } else {
            3
        };
        let estimated_mem = info.width as u64 * info.height as u64 * bpp;
        self.limits
            .check_memory(estimated_mem)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;
        Ok(())
    }
}

impl<'a> zencodec::decode::DecodeJob<'a> for AvifDecodeJob {
    type Error = At<CodecError>;
    type Dec = AvifDecoder<'a>;
    type StreamDec = AvifStreamingDecoder;
    type AnimationFrameDec = AvifAnimationFrameDecoder;

    fn with_stop(mut self, stop: zencodec::StopToken) -> Self {
        self.stop = Some(stop);
        self
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn with_start_frame_index(mut self, index: u32) -> Self {
        self.start_frame_index = index;
        self
    }

    fn with_policy(mut self, policy: zencodec::decode::DecodePolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// `ReconstructHdr` applies the gain map to the SDR base via
    /// ultrahdr-core (`DecodeCapabilities::reconstructs_hdr()` is `true`):
    /// the output is linear f32 RGBA (1.0 = SDR white / 203 nits) in the
    /// base image's primaries, MaxCLL/MaxFALL measured from the
    /// reconstructed pixels, and the gain-map components still surfaced
    /// for transcode use. Files without a gain map decode as honest SDR.
    /// Alpha-carrying or >8-bit bases are refused loudly — use
    /// [`GainMapRender::Components`] and apply downstream for those.
    fn with_gain_map_render(mut self, render: zencodec::GainMapRender) -> Self {
        self.gain_map_render = render;
        self
    }

    fn with_orientation(mut self, hint: zencodec::OrientationHint) -> Self {
        self.orientation = hint;
        self.config.orientation = hint;
        self
    }

    fn probe(&self, data: &[u8]) -> Result<ImageInfo, At<CodecError>> {
        self.probe_inner(data).map_err(zencodec::CodecError::of)
    }

    fn output_info(&self, data: &[u8]) -> Result<zencodec::decode::OutputInfo, At<CodecError>> {
        self.output_info_inner(data)
            .map_err(zencodec::CodecError::of)
    }

    fn push_decoder(
        self,
        data: Cow<'a, [u8]>,
        sink: &mut dyn zencodec::decode::DecodeRowSink,
        preferred: &[PixelDescriptor],
    ) -> Result<zencodec::decode::OutputInfo, At<CodecError>> {
        // Bake path: orientation isn't row-local, so a true row-streamed sink
        // would emit pixels in stored orientation. Route through a full decode
        // (which bakes the buffer upright and reports display dims) and copy the
        // baked rows to the sink. `Preserve` (default) keeps the native, low-
        // memory streaming sink unchanged.
        //
        // `copy_decode_to_sink` is generic over `Self`, so it already returns
        // `Self::Error` (= `At<CodecError>`); the sink-error wrap bridges a bare
        // native `Error` into the envelope via `.into()`. The non-bake path keeps
        // its verbatim `At<Error>` body in `push_decoder_inner` and is re-wrapped
        // once at this boundary.
        if will_auto_orient(self.orientation) {
            return zencodec::helpers::copy_decode_to_sink(self, data, sink, preferred, |e| {
                Error::Io(e.to_string()).into()
            });
        }
        self.push_decoder_inner(data, sink)
            .map_err(zencodec::CodecError::of)
    }

    fn decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifDecoder<'a>, At<CodecError>> {
        self.decoder_inner(data, preferred)
            .map_err(zencodec::CodecError::of)
    }

    fn streaming_decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifStreamingDecoder, At<CodecError>> {
        self.streaming_decoder_inner(data, preferred)
            .map_err(zencodec::CodecError::of)
    }

    fn animation_frame_decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifAnimationFrameDecoder, At<CodecError>> {
        self.animation_frame_decoder_inner(data, preferred)
            .map_err(zencodec::CodecError::of)
    }
}

// ── Native → trait metadata conversion ──────────────────────────────────────

/// Convert AVIF rotation + mirror properties to EXIF orientation.
///
/// AVIF uses separate `irot` (rotation) and `imir` (mirror) boxes.
/// The display pipeline applies: mirror first, then rotate (both CCW).
fn avif_to_orientation(
    rotation: Option<&zenavif_parse::ImageRotation>,
    mirror: Option<&zenavif_parse::ImageMirror>,
) -> zencodec::Orientation {
    use zencodec::Orientation;
    let angle = rotation.map(|r| r.angle).unwrap_or(0);
    match (mirror.map(|m| m.axis), angle) {
        (None, 0) => Orientation::Identity,
        (None, 90) => Orientation::Rotate270,
        (None, 180) => Orientation::Rotate180,
        (None, 270) => Orientation::Rotate90,
        (Some(0), 0) => Orientation::FlipH,
        (Some(0), 90) => Orientation::Transpose,
        (Some(0), 180) => Orientation::FlipV,
        (Some(0), 270) => Orientation::Transverse,
        (Some(1), 0) => Orientation::FlipV,
        (Some(1), 90) => Orientation::Transverse,
        (Some(1), 180) => Orientation::FlipH,
        (Some(1), 270) => Orientation::Transpose,
        _ => Orientation::Identity,
    }
}

// ── Orientation hint: Preserve (default) vs bake ────────────────────────────
//
// The zencodec adapter honors `OrientationHint` the same way zenjpeg and heic
// do, so the codecs report orientation consistently. zenavif's orientation
// source is the container's `irot`/`imir` transform boxes (NOT EXIF); the
// native decoder leaves pixels in stored orientation, so the adapter is what
// applies the transform when the caller asks for it.

/// Whether the orientation hint requests baking the image's orientation into
/// the decoded pixels. `Correct`/`CorrectAndTransform` bake; `Preserve`,
/// `ExactTransform`, and any future variant do not (the safe default — keep
/// pixels in stored orientation and report the orientation on `ImageInfo`).
/// Mirrors heic's and zenjpeg's policy so the codecs agree.
fn will_auto_orient(hint: zencodec::OrientationHint) -> bool {
    use zencodec::OrientationHint;
    matches!(
        hint,
        OrientationHint::Correct | OrientationHint::CorrectAndTransform(_)
    )
}

/// The image's intrinsic orientation from its `irot`/`imir` container boxes —
/// the net transform that, applied to the stored pixels, yields the upright
/// (display) image. Equals what [`avif_to_orientation`] computes.
fn intrinsic_orientation(native: &crate::image::ImageInfo) -> zencodec::Orientation {
    avif_to_orientation(native.rotation.as_ref(), native.mirror.as_ref())
}

/// Bake the resolved orientation into a decoded buffer when the hint is on the
/// bake path, and report the resulting `(orientation, width, height)` to put on
/// `ImageInfo` / `OutputInfo`.
///
/// - `Preserve` (default): pixels are untouched; report the stored dims + the
///   intrinsic orientation tag (callers apply it via `display_width/height`).
/// - bake path (`Correct`/`CorrectAndTransform`): physically apply the
///   intrinsic orientation (zenavif resolves the net transform to the intrinsic,
///   matching heic); report the upright display dims + `Orientation::Identity`.
///   A no-op bake (already-upright image) still reports `Identity` per the
///   `OrientationHint::bakes()` contract.
fn bake_orientation(
    pixels: PixelBuffer,
    native: &crate::image::ImageInfo,
    hint: zencodec::OrientationHint,
) -> (PixelBuffer, zencodec::Orientation, u32, u32) {
    let intrinsic = intrinsic_orientation(native);
    if !will_auto_orient(hint) {
        let (w, h) = (pixels.width(), pixels.height());
        return (pixels, intrinsic, w, h);
    }
    let baked = if intrinsic.is_identity() {
        pixels
    } else {
        zenpixels_convert::orient::apply_orientation(pixels.as_slice(), intrinsic)
    };
    let (w, h) = (baked.width(), baked.height());
    (baked, zencodec::Orientation::Identity, w, h)
}

/// Resolve the dims + orientation tag to report on `ImageInfo`/`OutputInfo`
/// **without** a decoded buffer (probe paths). `native.width`/`height` are the
/// stored (coded) dims. Mirrors [`bake_orientation`]'s reporting.
fn reported_dims_and_orientation(
    native: &crate::image::ImageInfo,
    hint: zencodec::OrientationHint,
) -> (u32, u32, zencodec::Orientation) {
    let intrinsic = intrinsic_orientation(native);
    if !will_auto_orient(hint) {
        return (native.width, native.height, intrinsic);
    }
    let (w, h) = intrinsic.output_dimensions(native.width, native.height);
    (w, h, zencodec::Orientation::Identity)
}

/// Rewrite an `ImageInfo` (built by [`convert_native_info`], which reports the
/// `Preserve` view: stored dims + intrinsic tag) into the resolved reporting for
/// `hint`. A no-op when the hint preserves; on the bake path it swaps to display
/// dims + `Orientation::Identity`.
fn apply_reported_orientation(
    mut info: ImageInfo,
    native: &crate::image::ImageInfo,
    hint: zencodec::OrientationHint,
) -> ImageInfo {
    if !will_auto_orient(hint) {
        return info;
    }
    let (w, h, orientation) = reported_dims_and_orientation(native, hint);
    info.width = w;
    info.height = h;
    info.with_orientation(orientation)
}

/// Convert EXIF orientation to AVIF rotation raw code + mirror axis.
///
/// Inverse of [`avif_to_orientation`]. Returns `(rotation_code, mirror_axis)`.
/// Rotation codes: 0=0°, 1=90°CCW, 2=180°, 3=270°CCW.
#[cfg(feature = "encode")]
fn orientation_to_avif(orientation: zencodec::Orientation) -> (Option<u8>, Option<u8>) {
    use zencodec::Orientation;
    match orientation {
        Orientation::Identity => (None, None),
        Orientation::FlipH => (Some(0), Some(0)), // mirror=0, no rotation
        Orientation::Rotate180 => (Some(2), None), // 180° CCW
        Orientation::FlipV => (Some(2), Some(0)), // mirror=0, 180° CCW
        Orientation::Transpose => (Some(1), Some(0)), // mirror=0, 90° CCW
        Orientation::Rotate90 => (Some(3), None), // 270° CCW = 90° CW
        Orientation::Transverse => (Some(3), Some(0)), // mirror=0, 270° CCW
        Orientation::Rotate270 => (Some(1), None), // 90° CCW = 270° CW
        _ => (None, None),
    }
}

/// Set transfer function and color primaries from native CICP on the pixel buffer.
/// Whether an ICC profile's device class (header bytes 16..20) is valid
/// for the buffer's channel layout: GRAY-class on Gray/GrayAlpha,
/// RGB-class on Rgb/Rgba/Bgra. Pairing them crosswise is invalid
/// signaling (libpng, among others, rejects it).
fn icc_class_matches_layout(icc: &[u8], layout: zenpixels::ChannelLayout) -> bool {
    if icc.len() < 132 {
        return false;
    }
    let class = &icc[16..20];
    let buffer_gray = matches!(
        layout,
        zenpixels::ChannelLayout::Gray | zenpixels::ChannelLayout::GrayAlpha
    );
    if buffer_gray {
        class == b"GRAY"
    } else {
        class == b"RGB "
    }
}

/// Attach the authoritative source color to a decoded buffer as a
/// [`zenpixels::ColorContext`], making the pixels self-describing for
/// downstream stages (CMS, load-bearing reduction, re-encode).
///
/// The selection runs through zencodec's drop-dupe rules
/// ([`zencodec::decode::SourceColor::to_color_context`]: the
/// non-authoritative field is dropped — ICC > nclx per MIAF). The ICC is
/// then class-gated against the buffer layout: an RGB-class profile
/// never rides a Gray buffer (the raw CICP stays as the fallback signal
/// — it carries the raw H.273 code points the descriptor enums fold
/// away). The conversion/orientation/reduction stages all propagate the
/// context; the load-bearing gray collapse swaps or suppresses per its
/// own ICC rules.
fn attach_color_context_class_gated(
    pixels: PixelBuffer,
    source_color: &zencodec::decode::SourceColor,
) -> PixelBuffer {
    match color_context_for_layout(source_color, pixels.descriptor().layout()) {
        Some(ctx) => pixels.with_color_context(ctx),
        None => pixels,
    }
}

/// The class-gated context [`attach_color_context_class_gated`] attaches,
/// computed for a known layout — shared with the streaming decoder, whose
/// strip scratch buffers are rebuilt per batch and need the context
/// re-applied on every emitted slice.
fn color_context_for_layout(
    source_color: &zencodec::decode::SourceColor,
    layout: zenpixels::ChannelLayout,
) -> Option<Arc<zenpixels::ColorContext>> {
    let mut ctx = source_color.to_color_context();
    if let Some(icc) = ctx
        .icc
        .take_if(|icc| !icc_class_matches_layout(icc, layout))
    {
        // Class mismatch: the profile cannot ride this layout. Its
        // DERIVED CICP is the authoritative description (the profile
        // outranked the signaled nclx per MIAF), so prefer it over both
        // the drop-dupe survivor and the signaled fallback.
        ctx.cicp = derived_cicp_from_icc(&icc)
            .or(ctx.cicp)
            .or(source_color.cicp);
    }
    if ctx.icc.is_none() && ctx.cicp.is_none() {
        return None;
    }
    Some(Arc::new(ctx))
}

/// Build the [`zencodec::decode::SourceColor`] for the native info the
/// way [`convert_native_info`] does (raw CICP + full-range; ICC
/// authority when ICC bytes are present, per MIAF).
fn native_source_color(native: &crate::image::ImageInfo) -> zencodec::decode::SourceColor {
    let mut sc = zencodec::decode::SourceColor::default();
    sc.cicp = Some(zencodec::Cicp::new(
        native.color_primaries.0,
        native.transfer_characteristics.0,
        native.matrix_coefficients.0,
        native.color_range == crate::image::ColorRange::Full,
    ));
    if let Some(ref icc) = native.icc_profile {
        sc.icc_profile = Some(Arc::<[u8]>::from(icc.as_slice()));
        // authority stays Icc (SourceColor's default) — ICC > nclx per MIAF
    } else {
        sc.color_authority = ColorAuthority::Cicp;
    }
    sc
}

/// Derive an ICC profile's CICP description: an explicit embedded
/// `cICP` tag first, then normalized-hash identification of well-known
/// profiles. This is the same chain zenpixels-convert's load-bearing
/// reduction uses to decide whether a gray collapse keeps accurate
/// color signaling.
fn derived_cicp_from_icc(icc: &[u8]) -> Option<zencodec::Cicp> {
    zenpixels::icc::extract_cicp(icc)
        .or_else(|| zenpixels::icc::identify_common(icc).and_then(|id| id.to_cicp()))
}

/// Whether native-gray output keeps accurate color for this file.
///
/// Gray files carrying RGB-class ICC profiles are common in the wild.
/// When the profile's CICP is derivable, native gray is fine: the gray
/// pixels get a CICP-only context (white point + transfer remain fully
/// meaningful for single-channel data), so there is no need to expand
/// to RGB just to honor the profile. Only an underivable RGB-class (or
/// unclassifiable) profile declines native gray — the profile is then
/// the sole accurate description and must stay on a layout it
/// describes; a gray preference resolves through the load-bearing
/// reduction's ICC rules instead.
fn icc_allows_native_gray(native: &crate::image::ImageInfo) -> bool {
    match &native.icc_profile {
        None => true,
        Some(icc) => {
            (icc.len() >= 132 && &icc[16..20] == b"GRAY") || derived_cicp_from_icc(icc).is_some()
        }
    }
}

/// [`attach_color_context_class_gated`] from zenavif's native info: build
/// the [`zencodec::decode::SourceColor`] exactly the way
/// [`convert_native_info`] does (raw CICP code points + full-range flag;
/// ICC authority when ICC bytes are present, per MIAF).
fn attach_source_color_context(
    pixels: PixelBuffer,
    native: &crate::image::ImageInfo,
) -> PixelBuffer {
    attach_color_context_class_gated(pixels, &native_source_color(native))
}

fn set_cicp_on_pixels(pixels: PixelBuffer, info: &crate::image::ImageInfo) -> PixelBuffer {
    let mut desc = pixels.descriptor();
    if let Some(tf) = zenpixels::TransferFunction::from_cicp(info.transfer_characteristics.0) {
        desc = desc.with_transfer(tf);
    }
    if let Some(p) = zenpixels::ColorPrimaries::from_cicp(info.color_primaries.0) {
        desc = desc.with_primaries(p);
    }
    pixels.with_descriptor(desc)
}

/// Convert zenavif's native `ImageInfo` to `zencodec::ImageInfo`.
fn convert_native_info(native: &crate::image::ImageInfo) -> ImageInfo {
    let orientation = avif_to_orientation(native.rotation.as_ref(), native.mirror.as_ref());

    let cicp = zencodec::Cicp::new(
        native.color_primaries.0,
        native.transfer_characteristics.0,
        native.matrix_coefficients.0,
        native.color_range == crate::image::ColorRange::Full,
    );

    let channels: u8 = if native.monochrome {
        if native.has_alpha { 2 } else { 1 }
    } else if native.has_alpha {
        4
    } else {
        3
    };

    let mut info = ImageInfo::new(native.width, native.height, ImageFormat::Avif)
        .with_alpha(native.has_alpha)
        .with_bit_depth(native.bit_depth)
        .with_channel_count(channels)
        .with_cicp(cicp)
        .with_orientation(orientation);

    if let Some(ref icc) = native.icc_profile {
        info = info.with_icc_profile(icc.clone());
        // authority stays Icc (default) — ICC > nclx per MIAF spec
    } else {
        // No ICC → CICP (from nclx or AV1 SPS) is authoritative
        info = info.with_color_authority(ColorAuthority::Cicp);
    }
    if let Some(ref exif) = native.exif {
        info = info.with_exif(exif.clone());
    }
    if let Some(ref xmp) = native.xmp {
        info = info.with_xmp(xmp.clone());
    }
    if let Some(ref cll) = native.content_light_level {
        info = info.with_content_light_level(zencodec::ContentLightLevel::new(
            cll.max_content_light_level,
            cll.max_pic_average_light_level,
        ));
    }
    if let Some(ref mdcv) = native.mastering_display {
        // Convert from 0.00002 units (u16) to CIE 1931 xy (f32), and 0.0001 cd/m² (u32) to f32
        let xy = |v: u16| v as f32 * 0.00002;
        info = info.with_mastering_display(zencodec::MasteringDisplay::new(
            [
                [xy(mdcv.primaries[0].0), xy(mdcv.primaries[0].1)],
                [xy(mdcv.primaries[1].0), xy(mdcv.primaries[1].1)],
                [xy(mdcv.primaries[2].0), xy(mdcv.primaries[2].1)],
            ],
            [xy(mdcv.white_point.0), xy(mdcv.white_point.1)],
            mdcv.max_luminance as f32 * 0.0001,
            mdcv.min_luminance as f32 * 0.0001,
        ));
    }

    // Supplemental content flags: gain map, depth map.
    let has_gain_map = native.gain_map.is_some();
    let has_depth_map = native.depth_map.is_some();
    if has_gain_map || has_depth_map {
        let mut supplements = Supplements::default();
        supplements.gain_map = has_gain_map;
        supplements.depth_map = has_depth_map;
        info = info.with_supplements(supplements);
    }

    // Gain map presence: Absent when definitively none, Available when metadata
    // can be converted, Unknown otherwise (default).
    if native.gain_map.is_some() {
        info = info.with_gain_map(convert_gain_map_presence(native));
    } else {
        info = info.with_gain_map(GainMapPresence::Absent);
    }

    info
}

/// Convert native AVIF gain map metadata to zencodec's `GainMapPresence`.
///
/// Parses the AV1 sequence header from the gain map data to extract dimensions,
/// then converts the ISO 21496-1 metadata to zencodec's canonical representation.
fn convert_gain_map_presence(native: &crate::image::ImageInfo) -> GainMapPresence {
    let gm = match native.gain_map.as_ref() {
        Some(gm) => gm,
        None => return GainMapPresence::Absent,
    };

    match convert_gain_map_info(gm) {
        Some(info) => GainMapPresence::Available(Box::new(info)),
        // If we can't parse the OBU, we know a gain map exists but can't
        // extract its dimensions — report as Unknown rather than lying.
        None => GainMapPresence::Unknown,
    }
}

/// Convert an [`AvifGainMap`](crate::image::AvifGainMap) to zencodec's
/// [`GainMapInfo`](zencodec::GainMapInfo).
///
/// Parses the AV1 sequence header to extract dimensions, converts the
/// ISO 21496-1 metadata fields, and optionally converts alt color info
/// to a [`Cicp`](zencodec::Cicp). Returns `None` if the AV1 bitstream
/// cannot be parsed.
/// Apply the gain map to a decoded SDR base, producing linear f32 RGBA
/// HDR pixels (1.0 = SDR white / 203 nits, base image's primaries) plus
/// the measured (MaxCLL, MaxFALL). Shared by the buffered and streaming
/// decode paths so both honor [`zencodec::GainMapRender::ReconstructHdr`]
/// identically. Call only when `native_info.gain_map` is `Some`.
fn reconstruct_hdr_pixels(
    pixels: zenpixels::PixelBuffer,
    native_info: &crate::image::ImageInfo,
    target_headroom: Option<f32>,
    decode_config: &crate::DecoderConfig,
    stop: &dyn Stop,
) -> Result<(zenpixels::PixelBuffer, (u16, u16)), At<Error>> {
    let gm = native_info
        .gain_map
        .as_ref()
        .expect("reconstruct_hdr_pixels: gain map presence checked by caller");
    // Honest-capability gates: the apply kernels read 8-bit RGB(A) bases
    // and emit constant alpha = 1.0, so a real alpha channel or a >8-bit
    // base cannot be reconstructed without corruption. The zencodec
    // contract demands a loud refusal over silent degradation (use
    // Components + apply downstream for those).
    if native_info.has_alpha {
        return Err(at!(Error::Unsupported(
            "ReconstructHdr with an alpha channel is unsupported \
                  (apply emits opaque); use GainMapRender::Components",
        )));
    }
    match pixels.descriptor().pixel_format() {
        zenpixels::PixelFormat::Rgb8 | zenpixels::PixelFormat::Rgba8 => {}
        _ => {
            return Err(at!(Error::Unsupported(
                "ReconstructHdr requires an 8-bit base (10/12-bit not yet \
                      supported); use GainMapRender::Components",
            )));
        }
    }
    let metadata = convert_gain_map_info(gm).ok_or_else(|| {
        at!(Error::Malformed(
            "gain map present but its ISO 21496-1 metadata failed to parse"
        ))
    })?;
    let (gpx, gw, gh, gch) =
        crate::decode_av1::decode_av1_obu_with_config(&gm.gain_map_data, decode_config)?;
    let gainmap = ultrahdr_core::GainMap {
        width: gw,
        height: gh,
        channels: gch,
        data: gpx,
    };
    let params = &metadata.params;
    // None = full reconstruction at the gain map's encoded maximum
    // headroom; Some(h) renders for a display with h× SDR-white
    // capability (clamped inside ultrahdr-core's weight calculation).
    let boost = target_headroom.unwrap_or_else(|| {
        (params.alternate_hdr_headroom.max(params.base_hdr_headroom) as f32).exp2()
    });
    let hdr = ultrahdr_core::gainmap::apply_gainmap(
        &pixels,
        &gainmap,
        params,
        boost,
        ultrahdr_core::HdrOutputFormat::LinearFloat,
        stop,
    )
    .map_err(|_e| {
        at!(Error::Malformed(
            "gain-map apply failed (see ultrahdr-core validation rules)"
        ))
    })?;
    // The apply kernels emit constant alpha = 1.0 (structural, not
    // scanned) — tag it Opaque so downstream encoders know the lane is
    // not load-bearing without rescanning.
    let desc = hdr
        .descriptor()
        .with_alpha(Some(zenpixels::AlphaMode::Opaque));
    let hdr = hdr.with_descriptor(desc);
    // The linear output IS describable: source primaries (raw code
    // point), H.273 transfer 8 (linear), identity matrix (RGB data),
    // full range. No SDR ICC or transfer may carry over, but a linear
    // CICP is strictly more self-describing than nothing — the enum
    // descriptor folds primaries the raw code point keeps.
    let linear_cicp = zencodec::Cicp::new(native_info.color_primaries.0, 8, 0, true);
    let hdr = hdr.with_color_context(Arc::new(zenpixels::ColorContext::from_cicp(linear_cicp)));
    let cll = measure_cll_linear(&hdr);
    Ok((hdr, cll))
}

/// Measure (MaxCLL, MaxFALL) in nits from linear f32 RGBA pixels where
/// 1.0 = SDR white (203 nits): MaxCLL = peak of per-pixel max(R,G,B),
/// MaxFALL = frame average of the same, both scaled by 203.
fn measure_cll_linear(pixels: &zenpixels::PixelBuffer) -> (u16, u16) {
    const SDR_WHITE_NITS: f32 = 203.0;
    let slice = pixels.as_slice();
    let bytes = slice.as_strided_bytes();
    let stride = slice.stride();
    let (w, h) = (slice.width() as usize, slice.rows() as usize);
    let mut peak = 0.0f32;
    let mut sum = 0.0f64;
    for y in 0..h {
        let row = &bytes[y * stride..][..w * 16];
        let row_f32: &[f32] = rgb::bytemuck::cast_slice(row);
        for px in row_f32.chunks_exact(4) {
            let m = px[0].max(px[1]).max(px[2]).max(0.0);
            peak = peak.max(m);
            sum += f64::from(m);
        }
    }
    let fall = if w * h > 0 {
        (sum / (w * h) as f64) as f32
    } else {
        0.0
    };
    let to_nits = |v: f32| ((v * SDR_WHITE_NITS).round().clamp(0.0, 65535.0)) as u16;
    (to_nits(peak), to_nits(fall))
}

fn convert_gain_map_info(gm: &crate::image::AvifGainMap) -> Option<zencodec::GainMapInfo> {
    // Parse AV1 sequence header to get gain map image dimensions.
    let (width, height, gm_channels_from_av1) =
        match zenavif_parse::AV1Metadata::parse_av1_bitstream(&gm.gain_map_data) {
            Ok(meta) => (
                meta.max_frame_width.get(),
                meta.max_frame_height.get(),
                if meta.monochrome { 1u8 } else { 3u8 },
            ),
            Err(_) => return None,
        };

    let md = &gm.metadata;
    let channels = if md.is_multichannel {
        3u8
    } else {
        gm_channels_from_av1.min(1)
    };

    let params = zencodec::GainMapParams::from(md);

    let mut gm_info = zencodec::GainMapInfo::new(params, width, height, channels);

    // Convert alternate rendition color info to CICP / ICC.
    match &gm.alt_color_info {
        Some(zenavif_parse::ColorInformation::Nclx {
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            full_range,
        }) => {
            gm_info = gm_info.with_alternate_cicp(zencodec::Cicp::new(
                *color_primaries as u8,
                *transfer_characteristics as u8,
                *matrix_coefficients as u8,
                *full_range,
            ));
        }
        Some(zenavif_parse::ColorInformation::IccProfile(icc)) => {
            gm_info = gm_info.with_alternate_icc(icc.clone());
        }
        None => {}
    }

    Some(gm_info)
}

/// Strip metadata from [`ImageInfo`] according to a [`DecodePolicy`](zencodec::decode::DecodePolicy).
///
/// When a policy flag resolves to `false` (default is `true` = allow), the
/// corresponding metadata field is cleared so callers never see it.
fn apply_decode_policy(info: &mut ImageInfo, policy: &zencodec::decode::DecodePolicy) {
    if !policy.resolve_icc(true) {
        info.source_color.icc_profile = None;
    }
    if !policy.resolve_exif(true) {
        info.embedded_metadata.exif = None;
    }
    if !policy.resolve_xmp(true) {
        info.embedded_metadata.xmp = None;
    }
}

// ── Pixel conversion helpers ────────────────────────────────────────────────

/// Check if two descriptors match on pixel format (channel type + alpha),
/// ignoring transfer function, primaries, and signal range metadata.
fn format_matches(a: PixelDescriptor, b: PixelDescriptor) -> bool {
    a.pixel_format() == b.pixel_format()
}

/// Apply preferred format negotiation to decoder output.
///
/// If `preferred` is empty, returns `pixels` unchanged (native format).
/// If `preferred` is non-empty, finds the first descriptor we can satisfy:
/// - Same or lower bit depth: downconvert (caller explicitly asked for it)
/// - Higher bit depth than native: skip (can't upscale losslessly)
///
/// Transfer function and color primaries on the native descriptor are preserved
/// (set from CICP metadata). Negotiation only considers channel type and alpha.
/// Whether negotiation selects native grayscale output for an alpha-free
/// monochrome source: yes with no preference (gray IS the native format,
/// per the `native_gray` capability), or when the caller's first
/// preference is a Gray layout. A leading RGB preference keeps the
/// classic expanded decode.
fn wants_gray_output(preferred: &[PixelDescriptor]) -> bool {
    match preferred.first() {
        None => true,
        Some(p) => p.layout() == zenpixels::ChannelLayout::Gray,
    }
}

/// `source_is_gray`: the *coded* image is alpha-free monochrome, so a
/// gray preference can be satisfied exactly (an RGB-expanded mono buffer
/// is R=G=B; luma of equal channels is the channel).
fn negotiate_format(
    mut pixels: PixelBuffer,
    preferred: &[PixelDescriptor],
    source_is_gray: bool,
) -> PixelBuffer {
    if preferred.is_empty() {
        return pixels;
    }

    let native = pixels.descriptor();

    // If the native pixel format matches any preferred descriptor, return as-is.
    // We compare pixel format only (ignoring transfer/primaries/signal range),
    // because CICP metadata enriches the descriptor but doesn't change the data.
    if preferred.iter().any(|p| format_matches(*p, native)) {
        return pixels;
    }

    // Find first preferred descriptor we can produce.
    for pref in preferred {
        // Can't upscale bit depth losslessly.
        if pref.channel_type().byte_size() > native.channel_type().byte_size() {
            continue;
        }

        // Grayscale preferences: satisfiable exactly only for monochrome
        // sources (never synthesize luma for color images here — that is
        // a CMS decision, not format negotiation). The collapse goes
        // through the load-bearing reduction, which VERIFIES R==G==B at
        // the byte level (instead of trusting container metadata),
        // rewrites in place with no allocation, and handles color
        // signaling (an RGB-class ICC profile cannot describe a Gray
        // layout — a gray-class variant is swapped in when derivable,
        // otherwise the collapse is suppressed and we fall through
        // honestly).
        if pref.layout() == zenpixels::ChannelLayout::Gray {
            if source_is_gray && pref.channel_type() == ChannelType::U8 {
                use zenpixels_convert::PixelBufferLoadBearingExt as _;
                pixels.reduce_to_load_bearing_format_in_place(true);
                if pixels.descriptor().layout() == zenpixels::ChannelLayout::Gray {
                    // 10/12-bit mono reduces to Gray16; honor the U8 ask.
                    if pixels.descriptor().channel_type() == ChannelType::U16 {
                        return crate::convert::downscale_to_8bit(pixels);
                    }
                    return pixels;
                }
                // Scan disagreed with the metadata, or an underivable
                // RGB-class ICC suppressed the collapse: never fake
                // gray — let the remaining preferences have their shot.
            }
            continue;
        }

        // Gray native, color preference at the same depth: expand.
        if native.layout() == zenpixels::ChannelLayout::Gray
            && pref.channel_type() == native.channel_type()
            && native.channel_type() == ChannelType::U8
        {
            if pref.layout().has_alpha() {
                return pixels.to_rgba8().into();
            }
            return pixels.to_rgb8().into();
        }

        // If caller wants 8-bit and we have 16-bit, downconvert.
        if pref.channel_type() == ChannelType::U8 && native.channel_type() == ChannelType::U16 {
            if pref.layout().has_alpha() {
                return pixels.to_rgba8().into();
            }
            return pixels.to_rgb8().into();
        }

        // Same bit depth but different layout (e.g., RGB vs RGBA).
        if pref.channel_type() == native.channel_type() {
            if pref.layout().has_alpha() && !native.layout().has_alpha() {
                if native.channel_type() == ChannelType::U8 {
                    return pixels.to_rgba8().into();
                }
                continue;
            }
            if !pref.layout().has_alpha() && native.layout().has_alpha() {
                if native.channel_type() == ChannelType::U8 {
                    return pixels.to_rgb8().into();
                }
                continue;
            }
        }
    }

    // No preferred descriptor matched — return native format.
    pixels
}

// ── Decoder ─────────────────────────────────────────────────────────────────

/// Single-image AVIF decoder.
pub struct AvifDecoder<'a> {
    config: crate::DecoderConfig,
    stop: Option<zencodec::StopToken>,
    data: Cow<'a, [u8]>,
    preferred: Vec<PixelDescriptor>,
    limits: ResourceLimits,
    policy: Option<zencodec::decode::DecodePolicy>,
    extract_gain_map: bool,
    gain_map_render: zencodec::GainMapRender,
    /// How to handle the image's stored orientation (`irot`/`imir`).
    /// Default [`OrientationHint::Preserve`](zencodec::OrientationHint::Preserve).
    orientation: zencodec::OrientationHint,
}

impl zencodec::decode::Decode for AvifDecoder<'_> {
    type Error = At<CodecError>;

    fn decode(self) -> Result<DecodeOutput, At<CodecError>> {
        self.decode_inner().map_err(zencodec::CodecError::of)
    }
}

/// Streaming AVIF decoder with real tile-row streaming for grid images.
///
/// For grid (tiled) images, each [`next_batch`](zencodec::decode::StreamingDecode::next_batch)
/// call decodes one tile-row of AV1 tiles, color-converts them, and stitches
/// them into a strip. Peak memory is proportional to one tile-row instead of
/// the full image.
///
/// For non-grid 8-bit color images, the decoded YUV frame is held in memory
/// and converted strip-by-strip on demand. This eliminates the full RGB
/// allocation and keeps the working set in L2 cache.
///
/// For non-grid 16-bit or monochrome images, falls back to full-frame
/// conversion and emits fixed-height strips.
pub struct AvifStreamingDecoder {
    info: ImageInfo,
    y_offset: u32,
    output_width: u32,
    output_height: u32,
    /// Grid path: managed decoder for tile-row streaming.
    decoder: Option<crate::ManagedAvifDecoder>,
    /// Stop token for cancellable grid decoding.
    stop: zencodec::StopToken,
    grid_rows: u32,
    grid_cols: u32,
    current_grid_row: u32,
    /// Pixel descriptor with CICP metadata for strip buffers.
    strip_descriptor: PixelDescriptor,
    /// Reusable strip buffer for the current tile-row or strip conversion.
    strip_buffer: Option<PixelBuffer>,
    /// Non-grid strip conversion: holds decoded YUV frames, converts on demand.
    strip_converter: Option<crate::strip_convert::StripConverter>,
    /// Optimal strip height for the strip converter path.
    strip_height: u32,
    /// Class-gated color context applied to every emitted strip: the
    /// scratch `strip_buffer` is rebuilt per batch without one, so the
    /// context is re-attached at emission. `None` when the source
    /// carries no color signaling (or the HDR/bake source buffer had
    /// none).
    strip_color_context: Option<Arc<zenpixels::ColorContext>>,
    /// Bake path (`OrientationHint::bakes()`): the fully-decoded, orientation-
    /// baked buffer. Orientation is not strip-local (transposes need the whole
    /// image), so the bake path materializes upright once and emits it in
    /// fixed-height strips. `None` on the preserve path (the default), where the
    /// grid / strip-converter fields drive low-memory streaming unchanged.
    baked: Option<PixelBuffer>,
}

impl AvifStreamingDecoder {
    /// Stitch decoded tiles horizontally into `self.strip_buffer`.
    fn stitch_tiles(&mut self, tiles: &[PixelBuffer], strip_h: u32) {
        let bpp = self.strip_descriptor.bytes_per_pixel();
        let mut strip = PixelBuffer::new(self.output_width, strip_h, self.strip_descriptor);
        {
            let mut sm = strip.as_slice_mut();
            for py in 0..strip_h {
                let dst_row = sm.row_mut(py);
                let mut x_offset = 0usize;
                for tile in tiles {
                    let tile_w = tile.width() as usize;
                    let actual_w =
                        tile_w.min((self.output_width as usize).saturating_sub(x_offset));
                    // Guard the source row by each tile's own height: grid tiles in
                    // a row may decode to different heights, and `strip_h` is taken
                    // from the first tile, so a shorter tile would make
                    // `tile.row(py)` panic. A tile that is off-canvas (actual_w ==
                    // 0) or too short for this row contributes nothing; still
                    // advance x_offset so later tiles in the row line up.
                    if actual_w != 0 && py < tile.height() {
                        let tile_slice = tile.as_slice();
                        let src = tile_slice.row(py);
                        let copy_bytes = actual_w * bpp;
                        let dst_start = x_offset * bpp;
                        dst_row[dst_start..dst_start + copy_bytes]
                            .copy_from_slice(&src[..copy_bytes]);
                    }
                    x_offset += tile_w;
                }
            }
        }
        self.strip_buffer = Some(strip);
    }
}

impl zencodec::decode::StreamingDecode for AvifStreamingDecoder {
    type Error = At<CodecError>;

    fn next_batch(&mut self) -> Result<Option<(u32, PixelSlice<'_>)>, At<CodecError>> {
        self.next_batch_inner().map_err(zencodec::CodecError::of)
    }

    fn info(&self) -> &ImageInfo {
        &self.info
    }
}

// ── Frame Decoder ───────────────────────────────────────────────────────────

/// Animation AVIF full-frame decoder.
///
/// Lazily decodes frames on demand. The `AnimationFrameDecoder` trait doesn't pass
/// a stop token per-call, so per-frame cancellation is not available
/// through this interface (use the native `AnimationDecoder` API for that).
pub struct AvifAnimationFrameDecoder {
    anim_decoder: crate::AnimationDecoder,
    index: usize,
    /// Number of frames decoded so far (including skipped ones).
    frames_decoded: u32,
    /// Skip frames before this index. Frames are still decoded to maintain
    /// correct compositing state, but not yielded to the caller.
    start_frame_index: u32,
    info: Arc<ImageInfo>,
    total_frames: u32,
    /// Animation loop count (0 = infinite, n = play n times).
    loop_count: u32,
    preferred: Vec<PixelDescriptor>,
    /// Holds the current frame's pixels so `render_next_frame` can return
    /// a borrowing `AnimationFrame<'_>`.
    current_frame: Option<PixelBuffer>,
    /// Resource limits for frame count and animation duration enforcement.
    limits: ResourceLimits,
    /// Accumulated animation duration in milliseconds across all decoded frames.
    accumulated_ms: u64,
    /// Orientation to bake into every frame: the intrinsic `irot`/`imir`
    /// transform on the bake path (`OrientationHint::bakes()`), or `Identity`
    /// (no-op) on the preserve path (the default). Applied after format
    /// negotiation, before the frame is yielded.
    bake_to: zencodec::Orientation,
}

impl zencodec::decode::AnimationFrameDecoder for AvifAnimationFrameDecoder {
    type Error = At<CodecError>;

    fn wrap_sink_error(err: zencodec::decode::SinkError) -> Self::Error {
        // Bare native error -> envelope via the `From<Error> for At<CodecError>` bridge.
        Error::ResourceLimit(err.to_string()).into()
    }

    fn info(&self) -> &ImageInfo {
        &self.info
    }

    fn frame_count(&self) -> Option<u32> {
        Some(self.total_frames)
    }

    fn loop_count(&self) -> Option<u32> {
        Some(self.loop_count)
    }

    fn render_next_frame(
        &mut self,
        stop: Option<&dyn zencodec::enough::Stop>,
    ) -> Result<Option<AnimationFrame<'_>>, At<CodecError>> {
        self.render_next_frame_inner(stop)
            .map_err(zencodec::CodecError::of)
    }

    fn render_next_frame_to_sink(
        &mut self,
        stop: Option<&dyn zencodec::enough::Stop>,
        sink: &mut dyn zencodec::decode::DecodeRowSink,
    ) -> Result<Option<zencodec::decode::OutputInfo>, Self::Error> {
        zencodec::helpers::copy_frame_to_sink(self, stop, sink)
    }
}

// ── Pattern B envelope: the original At<Error> method bodies, moved to private
// inherent `*_inner` helpers. Each zencodec trait method above is now a thin
// `self.*_inner(..).map_err(zencodec::CodecError::of)` boundary that re-wraps the
// located native error into the shared `At<CodecError>` envelope (Pattern B),
// preserving the whereat trace. The inherent helpers keep the verbatim logic.

#[cfg(feature = "encode")]
impl AvifEncoder {
    fn encode_srgba8_inner(
        mut self,
        data: &mut [u8],
        make_opaque: bool,
        width: u32,
        height: u32,
        stride_pixels: u32,
    ) -> Result<EncodeOutput, At<Error>> {
        let w = width as usize;
        let h = height as usize;
        let stride = stride_pixels as usize;
        let cfg = self.checked_config(w, h, 4)?;
        let stop = self.stop_token();

        if make_opaque {
            // Strip alpha: RGBA → RGB in-place, then encode RGB
            let mut rgb = Vec::with_capacity(w * h);
            for y in 0..h {
                let row_start = y * stride * 4;
                let row = &data[row_start..row_start + w * 4];
                for px in row.chunks_exact(4) {
                    rgb.push(Rgb {
                        r: px[0],
                        g: px[1],
                        b: px[2],
                    });
                }
            }
            let img = imgref::ImgVec::new(rgb, w, h);
            let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
            self.make_output(result.avif_file)
        } else {
            // Zero-copy RGBA path — bytemuck cast the contiguous region
            if stride == w {
                let pixel_bytes = &data[..w * h * 4];
                let rgba: &[Rgba<u8>] = bytemuck::cast_slice(pixel_bytes);
                let img = imgref::Img::new(rgba, w, h);
                let result = crate::encode_rgba8(img, &cfg, stop)?;
                self.make_output(result.avif_file)
            } else {
                // Strided: use ImgRef with stride
                let total_pixels = (h - 1) * stride + w;
                let pixel_bytes = &data[..total_pixels * 4];
                let rgba: &[Rgba<u8>] = bytemuck::cast_slice(pixel_bytes);
                let img = imgref::Img::new_stride(rgba, w, h, stride);
                let result = crate::encode_rgba8(img, &cfg, stop)?;
                self.make_output(result.avif_file)
            }
        }
    }

    fn encode_inner(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        use zenpixels::PixelFormat;

        // Propagate HDR color metadata from pixel descriptor to encoder config
        let desc = pixels.descriptor();
        self.apply_descriptor_color(desc);

        // For f32 pixels with HDR transfer (PQ/HLG), convert to u16 and use 16-bit
        // path to preserve HDR data. The default f32 path uses linear_to_srgb_u8()
        // which would silently destroy HDR values.
        let is_hdr_transfer = matches!(
            desc.transfer,
            zenpixels::TransferFunction::Pq | zenpixels::TransferFunction::Hlg
        );

        match desc.pixel_format() {
            PixelFormat::RgbF32 if is_hdr_transfer => {
                return self.encode_f32_as_u16_rgb(pixels);
            }
            PixelFormat::RgbaF32 if is_hdr_transfer => {
                return self.encode_f32_as_u16_rgba(pixels);
            }
            _ => {}
        }

        match desc.pixel_format() {
            PixelFormat::Rgb8 => self.do_encode_rgb8(pixels),
            PixelFormat::Rgba8 => self.do_encode_rgba8(pixels),
            PixelFormat::Gray8 => self.do_encode_gray8(pixels),
            PixelFormat::Rgb16 => self.do_encode_rgb16(pixels),
            PixelFormat::Rgba16 => self.do_encode_rgba16(pixels),
            PixelFormat::RgbF32 => self.do_encode_rgb_f32(pixels),
            PixelFormat::RgbaF32 => self.do_encode_rgba_f32(pixels),
            PixelFormat::GrayF32 => self.do_encode_gray_f32(pixels),
            PixelFormat::Bgra8 => {
                // Swizzle BGRA → RGBA and encode
                let raw = pixels.contiguous_bytes();
                let w = pixels.width() as usize;
                let h = pixels.rows() as usize;
                let cfg = self.checked_config(w, h, 4)?;
                let stop = self.stop_token();
                let rgba: Vec<Rgba<u8>> = raw
                    .chunks_exact(4)
                    .map(|c| Rgba {
                        r: c[2],
                        g: c[1],
                        b: c[0],
                        a: c[3],
                    })
                    .collect();
                let img = imgref::ImgVec::new(rgba, w, h);
                let result = crate::encode_rgba8(img.as_ref(), &cfg, stop)?;
                self.make_output(result.avif_file)
            }
            PixelFormat::Rgbx8 => {
                // Byte 3 is padding — strip to 3-channel RGB.
                let raw = pixels.contiguous_bytes();
                let w = pixels.width() as usize;
                let h = pixels.rows() as usize;
                let cfg = self.checked_config(w, h, 3)?;
                let stop = self.stop_token();
                let rgb: Vec<Rgb<u8>> = raw
                    .chunks_exact(4)
                    .map(|c| Rgb {
                        r: c[0],
                        g: c[1],
                        b: c[2],
                    })
                    .collect();
                let img = imgref::ImgVec::new(rgb, w, h);
                let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
                self.make_output(result.avif_file)
            }
            PixelFormat::Bgrx8 => {
                // BGRA layout with padding byte — swap B↔R and strip to RGB.
                let raw = pixels.contiguous_bytes();
                let w = pixels.width() as usize;
                let h = pixels.rows() as usize;
                let cfg = self.checked_config(w, h, 3)?;
                let stop = self.stop_token();
                let rgb: Vec<Rgb<u8>> = raw
                    .chunks_exact(4)
                    .map(|c| Rgb {
                        r: c[2],
                        g: c[1],
                        b: c[0],
                    })
                    .collect();
                let img = imgref::ImgVec::new(rgb, w, h);
                let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
                self.make_output(result.avif_file)
            }
            _ => Err(at!(Error::UnsupportedOperation(
                zencodec::UnsupportedOperation::PixelFormat,
            ))),
        }
    }
}

#[cfg(feature = "encode")]
impl AvifAnimationFrameEncoder {
    fn push_frame_inner(
        &mut self,
        pixels: PixelSlice<'_>,
        duration_ms: u32,
        stop: Option<&dyn Stop>,
    ) -> Result<(), At<Error>> {
        // Check cancellation (combine per-call + owned stop)
        if let Some(s) = stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }
        if let Some(ref s) = self.stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }

        let w = pixels.width();
        let h = pixels.rows();

        // Validate canvas dimensions match
        match (self.canvas_width, self.canvas_height) {
            (Some(cw), Some(ch)) if cw != w || ch != h => {
                return Err(at!(Error::InvalidState(format!(
                    "frame dimensions {}x{} don't match canvas {}x{}",
                    w, h, cw, ch
                ))));
            }
            (None, None) => {
                self.canvas_width = Some(w);
                self.canvas_height = Some(h);
            }
            _ => {}
        }

        // Check resource limits
        let desc = pixels.descriptor();
        let bpp = desc.bytes_per_pixel() as u64;
        self.limits.check_dimensions(w, h).map_err(|_| {
            at!(Error::ImageTooLarge {
                width: w,
                height: h,
            })
        })?;
        self.limits
            .check_memory(w as u64 * h as u64 * bpp)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;

        // Enforce max_frames limit.
        self.frame_count += 1;
        self.limits
            .check_frames(self.frame_count)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;

        let fmt = desc.pixel_format();

        // Validate consistent pixel format across frames
        if let Some(expected) = self.pixel_format {
            if fmt != expected {
                return Err(at!(Error::InvalidState(format!(
                    "pixel format mismatch: first frame was {expected:?}, this frame is {fmt:?}"
                ))));
            }
        } else {
            self.pixel_format = Some(fmt);
        }

        let raw = pixels.contiguous_bytes();
        let wu = w as usize;
        let hu = h as usize;

        let frame = match fmt {
            zenpixels::PixelFormat::Rgb8 => {
                let rgb: Vec<Rgb<u8>> = bytemuck::cast_slice(&raw).to_vec();
                BufferedFrame::Rgb8 {
                    pixels: imgref::ImgVec::new(rgb, wu, hu),
                    duration_ms,
                }
            }
            zenpixels::PixelFormat::Rgba8 => {
                let rgba: Vec<Rgba<u8>> = bytemuck::cast_slice(&raw).to_vec();
                BufferedFrame::Rgba8 {
                    pixels: imgref::ImgVec::new(rgba, wu, hu),
                    duration_ms,
                }
            }
            zenpixels::PixelFormat::Rgb16 => {
                let rgb: Vec<Rgb<u16>> = bytemuck::cast_slice(&raw).to_vec();
                BufferedFrame::Rgb16 {
                    pixels: imgref::ImgVec::new(rgb, wu, hu),
                    duration_ms,
                }
            }
            zenpixels::PixelFormat::Rgba16 => {
                let rgba: Vec<Rgba<u16>> = bytemuck::cast_slice(&raw).to_vec();
                BufferedFrame::Rgba16 {
                    pixels: imgref::ImgVec::new(rgba, wu, hu),
                    duration_ms,
                }
            }
            _ => {
                return Err(at!(Error::UnsupportedOperation(
                    zencodec::UnsupportedOperation::PixelFormat,
                )));
            }
        };

        self.frames.push(frame);
        Ok(())
    }

    fn finish_inner(mut self, stop: Option<&dyn Stop>) -> Result<EncodeOutput, At<Error>> {
        if let Some(s) = stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }
        if let Some(ref s) = self.stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }

        if self.frames.is_empty() {
            return Err(at!(Error::InvalidState("no frames to encode".into())));
        }

        // Memory-adaptive concurrency: canvas dimensions and pixel format are
        // known once frames exist, so fit the encoder thread count to the
        // memory budget here (the animation-job counterpart of the still
        // path's `checked_config`). The estimate covers the per-frame AV1
        // encoder working set — the dominant cost; the buffered input frames
        // were checked per push against the raw-size limit above. Errors when
        // even the single-threaded estimate does not fit the budget.
        let mut threads_note = None;
        if let (Some(w), Some(h), Some(fmt)) =
            (self.canvas_width, self.canvas_height, self.pixel_format)
        {
            let bpp: u8 = match fmt {
                zenpixels::PixelFormat::Rgb8 => 3,
                zenpixels::PixelFormat::Rgba8 => 4,
                zenpixels::PixelFormat::Rgb16 => 6,
                // Rgba16 (push_frame_inner rejects everything else).
                _ => 8,
            };
            let (pin, note) = fit_encode_threads_to_memory(&self.limits, &self.config, w, h, bpp)?;
            if let Some(n) = pin {
                self.config = self.config.clone().threads(Some(n));
            }
            threads_note = note;
        }

        let stop_token = self.stop_token();

        let avif_file = match self.frames[0] {
            BufferedFrame::Rgb8 { .. } => {
                let anim_frames: Vec<crate::AnimationFrame> = self
                    .frames
                    .into_iter()
                    .map(|f| match f {
                        BufferedFrame::Rgb8 {
                            pixels,
                            duration_ms,
                        } => crate::AnimationFrame {
                            pixels,
                            duration_ms,
                        },
                        _ => unreachable!(),
                    })
                    .collect();
                let result =
                    crate::encode_animation_rgb8(&anim_frames, &self.config, stop_token.clone())?;
                result.avif_file
            }
            BufferedFrame::Rgba8 { .. } => {
                let anim_frames: Vec<crate::AnimationFrameRgba> = self
                    .frames
                    .into_iter()
                    .map(|f| match f {
                        BufferedFrame::Rgba8 {
                            pixels,
                            duration_ms,
                        } => crate::AnimationFrameRgba {
                            pixels,
                            duration_ms,
                        },
                        _ => unreachable!(),
                    })
                    .collect();
                let result =
                    crate::encode_animation_rgba8(&anim_frames, &self.config, stop_token.clone())?;
                result.avif_file
            }
            BufferedFrame::Rgb16 { .. } => {
                let anim_frames: Vec<crate::AnimationFrame16> = self
                    .frames
                    .into_iter()
                    .map(|f| match f {
                        BufferedFrame::Rgb16 {
                            pixels,
                            duration_ms,
                        } => crate::AnimationFrame16 {
                            pixels,
                            duration_ms,
                        },
                        _ => unreachable!(),
                    })
                    .collect();
                let result =
                    crate::encode_animation_rgb16(&anim_frames, &self.config, stop_token.clone())?;
                result.avif_file
            }
            BufferedFrame::Rgba16 { .. } => {
                let anim_frames: Vec<crate::AnimationFrameRgba16> = self
                    .frames
                    .into_iter()
                    .map(|f| match f {
                        BufferedFrame::Rgba16 {
                            pixels,
                            duration_ms,
                        } => crate::AnimationFrameRgba16 {
                            pixels,
                            duration_ms,
                        },
                        _ => unreachable!(),
                    })
                    .collect();
                let result =
                    crate::encode_animation_rgba16(&anim_frames, &self.config, stop_token.clone())?;
                result.avif_file
            }
        };

        self.limits
            .check_output_size(avif_file.len() as u64)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;

        let mut out = EncodeOutput::new(avif_file, ImageFormat::Avif);
        if let Some(note) = threads_note {
            // Reductions are never silent: readable via `extras::<String>()`.
            out = out.with_extras(note);
        }
        Ok(out)
    }
}

impl AvifDecodeJob {
    fn probe_inner(&self, data: &[u8]) -> Result<ImageInfo, At<Error>> {
        let decoder = crate::ManagedAvifDecoder::new(data, &self.config.inner)?;
        let native_info = decoder.probe_info()?;
        // `convert_native_info` reports the Preserve view (stored dims +
        // intrinsic tag); rewrite to display dims + Identity on the bake path.
        let mut info = apply_reported_orientation(
            convert_native_info(&native_info),
            &native_info,
            self.orientation,
        );
        // Detect animation from the container's track structure.
        if let Some(anim) = decoder.animation_info() {
            info = info.with_sequence(ImageSequence::Animation {
                frame_count: Some(anim.frame_count as u32),
                loop_count: Some(anim.loop_count),
                random_access: true,
            });
        }
        if let Ok(probe) = crate::detect::probe(data) {
            info = info.with_source_encoding_details(probe);
        }
        if let Some(ref policy) = self.policy {
            apply_decode_policy(&mut info, policy);
        }
        Ok(info)
    }

    fn output_info_inner(&self, data: &[u8]) -> Result<zencodec::decode::OutputInfo, At<Error>> {
        let decoder = crate::ManagedAvifDecoder::new(data, &self.config.inner)?;
        let native_info = decoder.probe_info()?;
        let mut desc = if native_info.bit_depth > 8 {
            if native_info.has_alpha {
                PixelDescriptor::RGBA16_SRGB
            } else {
                PixelDescriptor::RGB16_SRGB
            }
        } else if native_info.has_alpha {
            PixelDescriptor::RGBA8_SRGB
        } else {
            PixelDescriptor::RGB8_SRGB
        };
        // Override TF and primaries from CICP if available.
        if let Some(tf) =
            zenpixels::TransferFunction::from_cicp(native_info.transfer_characteristics.0)
        {
            desc = desc.with_transfer(tf);
        }
        if let Some(p) = zenpixels::ColorPrimaries::from_cicp(native_info.color_primaries.0) {
            desc = desc.with_primaries(p);
        }
        // Report the post-orientation output dims + what the decoder bakes:
        // `Correct` bakes the intrinsic orientation (output = display dims,
        // `orientation_applied` = intrinsic); `Preserve` bakes nothing
        // (output = stored dims, `orientation_applied` = Identity, caller orients).
        let (w, h, _) = reported_dims_and_orientation(&native_info, self.orientation);
        let orientation_applied = if will_auto_orient(self.orientation) {
            intrinsic_orientation(&native_info)
        } else {
            zencodec::Orientation::Identity
        };
        Ok(zencodec::decode::OutputInfo::full_decode(w, h, desc)
            .with_orientation_applied(orientation_applied))
    }

    fn decoder_inner<'a>(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifDecoder<'a>, At<Error>> {
        self.check_input_size(&data)?;
        let cfg = self.effective_config();
        Ok(AvifDecoder {
            config: cfg,
            stop: self.stop,
            data,
            preferred: preferred.to_vec(),
            limits: self.limits,
            policy: self.policy,
            extract_gain_map: self.extract_gain_map,
            gain_map_render: self.gain_map_render,
            orientation: self.orientation,
        })
    }

    fn streaming_decoder_inner<'a>(
        mut self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifStreamingDecoder, At<Error>> {
        self.check_input_size(&data)?;
        let cfg = self.effective_config();
        let stop_token = self
            .stop
            .take()
            .unwrap_or_else(|| zencodec::StopToken::new(enough::Unstoppable));

        let mut decoder = crate::ManagedAvifDecoder::new(&data, &cfg)?;
        let native_info = decoder.probe_info()?;
        self.check_decode_limits(&native_info)?;

        // Native grayscale opt-in (zenavif#5) — mirrors the buffered
        // decode: alpha-free monochrome, not reconstructing, not a grid
        // (the grid branch below stitches RGB tiles), gray negotiated.
        // The monochrome strip path runs through full conversion +
        // `StripConverter::new_from_pixels`, which carries the gray
        // descriptor into the emitted strips.
        let mono_source = native_info.monochrome && !native_info.has_alpha;
        let reconstructing = matches!(
            self.gain_map_render,
            zencodec::GainMapRender::ReconstructHdr { .. }
        ) && native_info.gain_map.is_some();
        if mono_source
            && !reconstructing
            && !decoder.is_grid()
            && icc_allows_native_gray(&native_info)
            && wants_gray_output(preferred)
        {
            decoder.set_native_gray(true);
        }

        // ReconstructHdr path: gain-map application is whole-image (the
        // map is sampled at an image-to-map scale ratio), so decode the
        // full buffer, reconstruct, then emit fixed-height strips —
        // same shape as the orientation-bake path below. Files without
        // a gain map fall through to honest SDR streaming.
        if let zencodec::GainMapRender::ReconstructHdr { target_headroom } = self.gain_map_render
            && native_info.gain_map.is_some()
        {
            let (pixels, native_info) = decoder.decode_full(&stop_token)?;
            let pixels = set_cicp_on_pixels(pixels, &native_info);
            let pixels = attach_source_color_context(pixels, &native_info);
            let (hdr, (max_cll, max_fall)) = reconstruct_hdr_pixels(
                pixels,
                &native_info,
                target_headroom,
                self.config.inner(),
                &stop_token,
            )?;
            let (baked, _orientation, w, h) = bake_orientation(hdr, &native_info, self.orientation);
            let strip_descriptor = baked.descriptor();
            let mut info = apply_reported_orientation(
                convert_native_info(&native_info),
                &native_info,
                self.orientation,
            );
            // Measured envelope of the reconstructed pixels (zencodec
            // contract: MaxCLL/MaxFALL are measured; mastering display
            // passes through unchanged).
            info =
                info.with_content_light_level(zencodec::ContentLightLevel::new(max_cll, max_fall));
            let strip_height = 64u32.min(h.max(1));
            return Ok(AvifStreamingDecoder {
                info,
                y_offset: 0,
                output_width: w,
                output_height: h,
                decoder: None,
                stop: stop_token,
                grid_rows: 0,
                grid_cols: 0,
                current_grid_row: 0,
                strip_descriptor,
                strip_buffer: None,
                strip_converter: None,
                strip_height,
                strip_color_context: baked.color_context().cloned(),
                baked: Some(baked),
            });
        }

        // Bake path: orientation is not strip-local (transposes need the whole
        // image), so decode + bake the full buffer once and emit it in
        // fixed-height strips. Mirrors `Decode::decode`'s buffer pipeline. The
        // preserve path (default) falls through to the low-memory grid / strip-
        // converter streaming below, unchanged.
        if will_auto_orient(self.orientation) {
            let (pixels, native_info) = decoder.decode_full(&stop_token)?;
            let pixels = set_cicp_on_pixels(pixels, &native_info);
            let pixels = attach_source_color_context(pixels, &native_info);
            let pixels = negotiate_format(
                pixels,
                preferred,
                native_info.monochrome && !native_info.has_alpha,
            );
            let (baked, _orientation, w, h) =
                bake_orientation(pixels, &native_info, self.orientation);
            let strip_descriptor = baked.descriptor();
            let info = apply_reported_orientation(
                convert_native_info(&native_info),
                &native_info,
                self.orientation,
            );
            // Emit in cache-friendly fixed-height strips (or the whole image if
            // it's short). The baked buffer is contiguous, so a strip is just a
            // row-range view re-copied into a small buffer.
            let strip_height = 64u32.min(h.max(1));
            return Ok(AvifStreamingDecoder {
                info,
                y_offset: 0,
                output_width: w,
                output_height: h,
                decoder: None,
                stop: stop_token,
                grid_rows: 0,
                grid_cols: 0,
                current_grid_row: 0,
                strip_descriptor,
                strip_buffer: None,
                strip_converter: None,
                strip_height,
                strip_color_context: baked.color_context().cloned(),
                baked: Some(baked),
            });
        }

        let info = convert_native_info(&native_info);

        if decoder.is_grid() {
            let grid = decoder.grid_config().ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "grid_config missing after is_grid()",
                })
            })?;
            let output_width = grid.output_width;
            let output_height = grid.output_height;

            let base_desc = if native_info.bit_depth > 8 {
                if native_info.has_alpha {
                    PixelDescriptor::RGBA16_SRGB
                } else {
                    PixelDescriptor::RGB16_SRGB
                }
            } else if native_info.has_alpha {
                PixelDescriptor::RGBA8_SRGB
            } else {
                PixelDescriptor::RGB8_SRGB
            };

            // Apply CICP metadata to descriptor. No format negotiation for
            // the grid path — tiles produce native format and we stitch raw bytes.
            let mut strip_descriptor = base_desc;
            if let Some(tf) =
                zenpixels::TransferFunction::from_cicp(native_info.transfer_characteristics.0)
            {
                strip_descriptor = strip_descriptor.with_transfer(tf);
            }
            if let Some(p) = zenpixels::ColorPrimaries::from_cicp(native_info.color_primaries.0) {
                strip_descriptor = strip_descriptor.with_primaries(p);
            }

            return Ok(AvifStreamingDecoder {
                info,
                y_offset: 0,
                output_width,
                output_height,
                decoder: Some(decoder),
                stop: stop_token,
                grid_rows: grid.rows as u32,
                grid_cols: grid.columns as u32,
                current_grid_row: 0,
                strip_descriptor,
                strip_buffer: None,
                strip_converter: None,
                strip_height: 0,
                strip_color_context: color_context_for_layout(
                    &native_source_color(&native_info),
                    strip_descriptor.layout(),
                ),
                baked: None,
            });
        }

        // Non-grid: decode YUV, set up strip converter for on-demand conversion.
        // Use the frame-era info the converter returns (not the probe-era
        // one): the buffered path attaches its context from decode_full's
        // info, and strips must describe pixels identically.
        let (converter, frame_native) = decoder.decode_to_strip_converter(&stop_token)?;
        let desc = converter.descriptor();
        let w = converter.display_width() as u32;
        let h = converter.display_height() as u32;
        let strip_h = converter.optimal_strip_height() as u32;

        Ok(AvifStreamingDecoder {
            info,
            y_offset: 0,
            output_width: w,
            output_height: h,
            decoder: None,
            stop: stop_token,
            grid_rows: 0,
            grid_cols: 0,
            current_grid_row: 0,
            strip_descriptor: desc,
            strip_buffer: None,
            strip_converter: Some(converter),
            strip_height: strip_h,
            strip_color_context: color_context_for_layout(
                &native_source_color(&frame_native),
                desc.layout(),
            ),
            baked: None,
        })
    }

    fn animation_frame_decoder_inner<'a>(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifAnimationFrameDecoder, At<Error>> {
        // Reject animation when policy disallows it.
        if let Some(ref policy) = self.policy
            && !policy.resolve_animation(true)
        {
            return Err(at!(Error::UnsupportedOperation(
                zencodec::UnsupportedOperation::AnimationDecode,
            )));
        }
        self.check_input_size(&data)?;
        let cfg = self.effective_config();

        // Probe metadata before creating animation decoder (both parse the container,
        // but ManagedAvifDecoder gives us the native ImageInfo for conversion).
        let probe_dec = crate::ManagedAvifDecoder::new(&data, &cfg)?;
        let native_info = probe_dec.probe_info()?;
        self.check_decode_limits(&native_info)?;
        drop(probe_dec);

        let anim_dec = crate::AnimationDecoder::new(&data, &cfg)?;
        let anim_info = anim_dec.info().clone();

        // `convert_native_info` reports the Preserve view (stored dims +
        // intrinsic tag); the bake path rewrites the canvas to display dims +
        // Identity, matching the per-frame buffers `render_next_frame` bakes.
        let mut base_info = apply_reported_orientation(
            convert_native_info(&native_info),
            &native_info,
            self.orientation,
        )
        .with_sequence(ImageSequence::Animation {
            frame_count: Some(anim_info.frame_count as u32),
            loop_count: Some(anim_info.loop_count),
            random_access: true,
        });
        // Attach source encoding details to the shared animation ImageInfo.
        if let Ok(probe) = crate::detect::probe(&data) {
            base_info = base_info.with_source_encoding_details(probe);
        }
        if let Some(ref policy) = self.policy {
            apply_decode_policy(&mut base_info, policy);
        }

        // Resolve the orientation to bake into each frame: the intrinsic
        // transform on the bake path, `Identity` (no-op) on the preserve path.
        let bake_to = if will_auto_orient(self.orientation) {
            intrinsic_orientation(&native_info)
        } else {
            zencodec::Orientation::Identity
        };

        Ok(AvifAnimationFrameDecoder {
            anim_decoder: anim_dec,
            index: 0,
            frames_decoded: 0,
            start_frame_index: self.start_frame_index,
            info: Arc::new(base_info),
            total_frames: anim_info.frame_count as u32,
            loop_count: anim_info.loop_count,
            preferred: preferred.to_vec(),
            current_frame: None,
            limits: self.limits,
            accumulated_ms: 0,
            bake_to,
        })
    }

    fn push_decoder_inner(
        self,
        data: Cow<'_, [u8]>,
        sink: &mut dyn zencodec::decode::DecodeRowSink,
    ) -> Result<zencodec::decode::OutputInfo, At<Error>> {
        self.check_input_size(&data)?;
        let cfg = self.effective_config();
        let stop: &dyn Stop = match &self.stop {
            Some(s) => s,
            None => &enough::Unstoppable,
        };
        let mut decoder = crate::ManagedAvifDecoder::new(&data, &cfg)?;
        let probe_info = decoder.probe_info()?;
        self.check_decode_limits(&probe_info)?;
        let native_info = decoder.decode_to_sink(stop, sink)?;

        let desc = if native_info.bit_depth > 8 {
            if native_info.has_alpha {
                PixelDescriptor::RGBA16_SRGB
            } else {
                PixelDescriptor::RGB16_SRGB
            }
        } else if native_info.has_alpha {
            PixelDescriptor::RGBA8_SRGB
        } else {
            PixelDescriptor::RGB8_SRGB
        };
        Ok(zencodec::decode::OutputInfo::full_decode(
            native_info.width,
            native_info.height,
            desc,
        ))
    }
}

impl AvifDecoder<'_> {
    fn decode_inner(self) -> Result<DecodeOutput, At<Error>> {
        let stop: &dyn Stop = match &self.stop {
            Some(s) => s,
            None => &enough::Unstoppable,
        };
        let mut decoder = crate::ManagedAvifDecoder::new(&self.data, &self.config)?;
        let native_info = decoder.probe_info()?;

        // Check dimensions and memory limits before the expensive pixel decode.
        self.limits
            .check_dimensions(native_info.width, native_info.height)
            .map_err(|_| {
                at!(Error::ImageTooLarge {
                    width: native_info.width,
                    height: native_info.height,
                })
            })?;
        let bpp: u64 = if native_info.bit_depth > 8 {
            if native_info.has_alpha { 8 } else { 6 }
        } else if native_info.has_alpha {
            4
        } else {
            3
        };
        let estimated_mem = native_info.width as u64 * native_info.height as u64 * bpp;
        self.limits
            .check_memory(estimated_mem)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;

        // Native grayscale opt-in (zenavif#5): alpha-free monochrome
        // decodes straight to Gray8/Gray16 (1-2 bytes/pixel) when
        // negotiation selects it. Grid composition stitches RGB tiles and
        // HDR reconstruction needs an RGB base, so both stay expanded
        // (a gray preference is then satisfied post-hoc in
        // `negotiate_format` — exact, since mono RGB is R=G=B).
        let mono_source = native_info.monochrome && !native_info.has_alpha;
        let reconstructing = matches!(
            self.gain_map_render,
            zencodec::GainMapRender::ReconstructHdr { .. }
        ) && native_info.gain_map.is_some();
        if mono_source
            && !reconstructing
            && !decoder.is_grid()
            && icc_allows_native_gray(&native_info)
            && wants_gray_output(&self.preferred)
        {
            decoder.set_native_gray(true);
        }

        let (pixels, native_info) = decoder.decode_full(stop)?;

        // Set transfer function and primaries from CICP on the pixel descriptor.
        let pixels = set_cicp_on_pixels(pixels, &native_info);
        // Self-describing pixels: attach the authoritative source color
        // (class-gated). Conversions, orientation, and the load-bearing
        // reduction all propagate it; the HDR reconstruction below
        // replaces the buffer and re-tags it with a linear CICP (no SDR
        // ICC/transfer may carry onto linear f32).
        let pixels = attach_source_color_context(pixels, &native_info);
        // HDR reconstruction (GainMapRender::ReconstructHdr): apply the
        // gain map to the SDR base via ultrahdr-core, BEFORE orientation
        // bake (base and gain map share stored orientation) and before
        // SDR format negotiation (the output is linear f32 RGBA, 1.0 =
        // SDR white / 203 nits). MaxCLL/MaxFALL are MEASURED from the
        // reconstructed pixels per the zencodec contract.
        let mut reconstructed_cll: Option<(u16, u16)> = None;
        let reconstruct_target = match self.gain_map_render {
            zencodec::GainMapRender::ReconstructHdr { target_headroom }
                if native_info.gain_map.is_some() =>
            {
                Some(target_headroom)
            }
            _ => None,
        };
        let pixels = if let Some(target_headroom) = reconstruct_target {
            let (hdr, cll) =
                reconstruct_hdr_pixels(pixels, &native_info, target_headroom, &self.config, stop)?;
            reconstructed_cll = Some(cll);
            hdr
        } else {
            // BaseOnly / Components — or ReconstructHdr on a file with
            // no gain map, where the base IS the only rendition and an
            // honest SDR output is the correct rendering.
            negotiate_format(pixels, &self.preferred, mono_source)
        };
        // Orientation policy: `Correct` bakes the intrinsic `irot`/`imir`
        // orientation into the pixels and reports display dims + `Identity`;
        // `Preserve` (default) keeps stored orientation and reports the
        // intrinsic tag + stored dims. `convert_native_info` already reports the
        // preserve view, so only the bake path rewrites it.
        let (pixels, _orientation, _w, _h) =
            bake_orientation(pixels, &native_info, self.orientation);
        let mut info = apply_reported_orientation(
            convert_native_info(&native_info),
            &native_info,
            self.orientation,
        );
        if let Some(ref policy) = self.policy {
            apply_decode_policy(&mut info, policy);
        }
        if let Some((max_cll, max_fall)) = reconstructed_cll {
            // Measured envelope of the reconstructed pixels — the
            // signaled CLL described the alternate rendition, this
            // describes what we actually produced (zencodec contract:
            // MaxCLL/MaxFALL are measured; mastering display passes
            // through unchanged).
            info =
                info.with_content_light_level(zencodec::ContentLightLevel::new(max_cll, max_fall));
        }
        let mut output = DecodeOutput::new(pixels, info);
        if let Ok(probe) = crate::detect::probe(&self.data) {
            output = output.with_source_encoding_details(probe);
        }
        // Gain-map rendition intent. Components decodes the gain-map AV1
        // payload into a DecodedGainMap; ReconstructHdr ADDITIONALLY
        // applies it to the base via ultrahdr-core (above) — the output
        // pixels are linear f32 RGBA with 1.0 = SDR white, and the
        // components are still surfaced for transcode use. Unknown
        // future modes are refused, never mis-rendered.
        let surface_components = match self.gain_map_render {
            zencodec::GainMapRender::BaseOnly => false,
            zencodec::GainMapRender::Components
            | zencodec::GainMapRender::ReconstructHdr { .. } => true,
            _ => {
                return Err(at!(Error::InvalidParameters(
                    "unrecognized GainMapRender mode".into()
                )));
            }
        };

        // Attach gain map / depth map as typed extras only when opted in.
        // Metadata (`ImageInfo.supplements`, `GainMapPresence`) is always
        // populated regardless — only the heavy data blobs are gated.
        if (self.extract_gain_map || surface_components)
            && let Some(gm) = native_info.gain_map
            && let Some(metadata) = convert_gain_map_info(&gm)
        {
            // Components: decode the AV1-coded gain-map image into pixels.
            // Errors only when a present gain map is malformed.
            if surface_components {
                let (px, gw, gh, channels) =
                    crate::decode_av1::decode_av1_obu_with_config(&gm.gain_map_data, &self.config)?;
                let desc = if channels == 1 {
                    PixelDescriptor::GRAY8_SRGB
                } else {
                    PixelDescriptor::RGB8_SRGB
                };
                let pixels = zenpixels::PixelBuffer::from_vec(px, gw, gh, desc).map_err(|_| {
                    at!(Error::Decode {
                        code: -1,
                        msg: "gain-map pixel buffer creation failed",
                    })
                })?;
                output = output.with_extras(zencodec::decode::DecodedGainMap::new(
                    pixels,
                    metadata.clone(),
                ));
            }
            let source = zencodec::gainmap::GainMapSource::new(
                gm.gain_map_data,
                zencodec::ImageFormat::Avif,
                metadata,
            );
            output = output.with_extras(source);
        }
        if self.extract_gain_map
            && let Some(dm) = native_info.depth_map
        {
            output = output.with_extras(dm);
        }
        Ok(output)
    }
}

impl AvifStreamingDecoder {
    fn next_batch_inner(&mut self) -> Result<Option<(u32, PixelSlice<'_>)>, At<Error>> {
        if self.y_offset >= self.output_height {
            return Ok(None);
        }

        // Bake path: emit fixed-height strips copied from the pre-baked,
        // orientation-corrected full buffer.
        if let Some(ref baked) = self.baked {
            let remaining = self.output_height - self.y_offset;
            let h = self.strip_height.min(remaining);
            if h == 0 {
                return Ok(None);
            }
            let desc = self.strip_descriptor;
            let width = self.output_width;
            let strip_buf = self
                .strip_buffer
                .get_or_insert_with(|| PixelBuffer::new(width, h, desc));
            if strip_buf.height() != h {
                *strip_buf = PixelBuffer::new(width, h, desc);
            }
            {
                let baked_slice = baked.as_slice();
                let mut sm = strip_buf.as_slice_mut();
                for row in 0..h {
                    sm.row_mut(row)
                        .copy_from_slice(baked_slice.row(self.y_offset + row));
                }
            }
            let y = self.y_offset;
            self.y_offset += h;
            let slice = self.strip_buffer.as_ref().unwrap().as_slice().erase();
            let slice = match &self.strip_color_context {
                Some(ctx) => slice.with_color_context(Arc::clone(ctx)),
                None => slice,
            };
            return Ok(Some((y, slice)));
        }

        if self.decoder.is_some() {
            // Grid path: decode one tile-row per call.
            if self.current_grid_row >= self.grid_rows {
                return Ok(None);
            }

            let tiles = self.decoder.as_mut().unwrap().decode_tile_row(
                self.current_grid_row as usize,
                self.grid_cols as usize,
                &self.stop,
            )?;

            if tiles.is_empty() {
                return Ok(None);
            }

            let tile_h = tiles[0].height();
            let strip_h = tile_h.min(self.output_height.saturating_sub(self.y_offset));
            if strip_h == 0 {
                return Ok(None);
            }

            self.stitch_tiles(&tiles, strip_h);
            self.current_grid_row += 1;

            let y = self.y_offset;
            self.y_offset += strip_h;
            let slice = self.strip_buffer.as_ref().unwrap().as_slice().erase();
            let slice = match &self.strip_color_context {
                Some(ctx) => slice.with_color_context(Arc::clone(ctx)),
                None => slice,
            };
            return Ok(Some((y, slice)));
        }

        // Non-grid: convert strip from decoded YUV frames on demand.
        if let Some(ref converter) = self.strip_converter {
            let remaining = self.output_height - self.y_offset;
            let h = self.strip_height.min(remaining);
            if h == 0 {
                return Ok(None);
            }

            // Ensure strip buffer exists with the right dimensions
            let desc = self.strip_descriptor;
            let width = self.output_width;
            let strip_buf = self
                .strip_buffer
                .get_or_insert_with(|| PixelBuffer::new(width, self.strip_height, desc));

            // Resize if this is the last strip and it's shorter
            if strip_buf.height() != h {
                *strip_buf = PixelBuffer::new(width, h, desc);
            }

            converter
                .convert_strip(self.y_offset as usize, h as usize, strip_buf)
                .at()?;

            let y = self.y_offset;
            self.y_offset += h;
            let slice = self.strip_buffer.as_ref().unwrap().as_slice().erase();
            let slice = match &self.strip_color_context {
                Some(ctx) => slice.with_color_context(Arc::clone(ctx)),
                None => slice,
            };
            return Ok(Some((y, slice)));
        }

        Ok(None)
    }
}

impl AvifAnimationFrameDecoder {
    fn render_next_frame_inner(
        &mut self,
        stop: Option<&dyn zencodec::enough::Stop>,
    ) -> Result<Option<AnimationFrame<'_>>, At<Error>> {
        let stop: &dyn zencodec::enough::Stop = stop.unwrap_or(&enough::Unstoppable);
        loop {
            let frame = self.anim_decoder.next_frame(stop).at()?;
            let Some(frame) = frame else {
                return Ok(None);
            };
            let frame_index = self.frames_decoded;
            self.frames_decoded += 1;

            // Enforce max_frames limit (counts all decoded frames, including skipped).
            self.limits
                .check_frames(self.frames_decoded)
                .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;

            // Accumulate and enforce max_animation_ms.
            self.accumulated_ms += frame.duration_ms as u64;
            self.limits
                .check_animation_ms(self.accumulated_ms)
                .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;

            // Skip frames before the requested start index. We must still
            // decode them to maintain correct compositing state, but we
            // don't yield them to the caller.
            if frame_index < self.start_frame_index {
                continue;
            }

            // Animation frames stay on the RGB path (native-gray opt-in is
            // still-image only); no gray claims here.
            let pixels = attach_color_context_class_gated(frame.pixels, &self.info.source_color);
            let pixels = negotiate_format(pixels, &self.preferred, false);
            // Bake orientation into the frame on the bake path; `Identity` is a
            // no-op (preserve path keeps stored-orientation pixels unchanged).
            let pixels = if self.bake_to.is_identity() {
                pixels
            } else {
                zenpixels_convert::orient::apply_orientation(pixels.as_slice(), self.bake_to)
            };
            let idx = self.index as u32;
            self.index += 1;
            let duration_ms = frame.duration_ms;
            self.current_frame = Some(pixels);
            let slice = self.current_frame.as_ref().unwrap().as_slice().erase();
            return Ok(Some(AnimationFrame::new(slice, duration_ms, idx)));
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #[cfg(feature = "encode")]
    use super::*;
    #[cfg(feature = "encode")]
    use imgref::Img;

    #[cfg(feature = "encode")]
    #[test]
    fn fidelity_targets_roundtrip() {
        use zencodec::encode::{EncoderConfig as _, Fidelity};

        // Codec quality dial round-trips as itself, on the lossy path.
        let cq = AvifEncoderConfig::new().with_fidelity(Fidelity::codec_quality(70.0));
        assert_eq!(
            cq.resolved_target_fidelity(),
            Some(Fidelity::codec_quality(70.0))
        );
        assert_eq!(cq.is_lossless(), Some(false));

        // SSIM2 + butteraugli map onto the quality dial, reported as codec_quality.
        let s2 = AvifEncoderConfig::new().with_fidelity(Fidelity::ssim2(90.0));
        assert_eq!(
            s2.resolved_target_fidelity(),
            Some(Fidelity::codec_quality(90.0))
        );
        let bt = AvifEncoderConfig::new().with_fidelity(Fidelity::butteraugli(2.0));
        assert_eq!(
            bt.resolved_target_fidelity(),
            Some(Fidelity::codec_quality(72.0))
        );

        // Lossless is real (and reported) only under `encode-imazen`; otherwise
        // it degrades to top quality and is NOT reported as lossless.
        let ll = AvifEncoderConfig::new().with_fidelity(Fidelity::Lossless);
        if cfg!(feature = "encode-imazen") {
            assert_eq!(ll.resolved_target_fidelity(), Some(Fidelity::Lossless));
            assert_eq!(ll.is_lossless(), Some(true));
        } else {
            assert_eq!(
                ll.resolved_target_fidelity(),
                Some(Fidelity::codec_quality(100.0))
            );
            assert_eq!(ll.is_lossless(), Some(false));
        }
    }

    /// `AllocPreference` must not change decoded pixels: decoding the same
    /// AVIF under `Fallible` (the `try_reserve` path), `Infallible` (the
    /// `vec!` path), and the default (`CodecDefault`) must produce
    /// byte-identical output. Uses a real 8-bit 4:2:0 photo fixture, which
    /// exercises the big full-image RGB output buffer + the per-row scratch.
    mod alloc_pref_decode {
        use super::super::{AvifDecoderConfig, DecodeOutput};
        use alloc::borrow::Cow;
        use whereat::At;
        use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
        use zencodec::{AllocPreference, CodecError, ResourceLimits};

        extern crate alloc;

        /// A real, committed 8-bit 4:2:0 AVIF (kodim03). The default-features
        /// (decode-only) build can decode it without the `encode` feature.
        const KODIM03: &[u8] = include_bytes!("../tests/vectors/libavif/kodim03_yuv420_8bpc.avif");

        fn decode_bytes(
            encoded: &[u8],
            pref: Option<AllocPreference>,
        ) -> Result<Vec<u8>, At<CodecError>> {
            let job = AvifDecoderConfig::new().job();
            let job = match pref {
                Some(p) => {
                    job.with_limits(ResourceLimits::none().with_prefer_fallible_allocations(p))
                }
                None => job,
            };
            let out: DecodeOutput = job.decoder(Cow::Borrowed(encoded), &[])?.decode()?;
            Ok(out.into_buffer().copy_to_contiguous_bytes())
        }

        #[test]
        fn fallible_alloc_decode_matches_default() {
            let default = decode_bytes(KODIM03, None).expect("default decode");
            let fallible =
                decode_bytes(KODIM03, Some(AllocPreference::Fallible)).expect("fallible decode");
            let infallible = decode_bytes(KODIM03, Some(AllocPreference::Infallible))
                .expect("infallible decode");
            assert!(!default.is_empty(), "decode produced no pixels");
            assert_eq!(
                default, fallible,
                "Fallible decode must be byte-identical to the default decode"
            );
            assert_eq!(
                default, infallible,
                "Infallible decode must be byte-identical to the default decode"
            );
        }
    }

    /// Pin the negotiate-layer gray collapse in zenavif's exact feature
    /// configuration (zenpixels-convert with `default-features = false`,
    /// i.e. NO `icc-db`): the load-bearing reduction byte-verifies
    /// R==G==B instead of trusting metadata, and the ICC color-signaling
    /// rules decide whether the collapse may proceed.
    mod negotiate_gray {
        use super::super::{format_matches, negotiate_format, wants_gray_output};
        use alloc::sync::Arc;
        use zenpixels::{Cicp, ColorContext, PixelBuffer, PixelDescriptor, PixelFormat};

        extern crate alloc;

        fn gray_content_rgb8(w: u32, h: u32) -> PixelBuffer {
            let px: alloc::vec::Vec<rgb::Rgb<u8>> = (0..w * h)
                .map(|i| {
                    let g = (i * 7 % 256) as u8;
                    rgb::Rgb { r: g, g, b: g }
                })
                .collect();
            PixelBuffer::from_pixels(px, w, h).unwrap().into()
        }

        /// No color context (zenavif's decode reality today — ICC rides on
        /// `ImageInfo`, never the buffer): Carry plan, collapse proceeds,
        /// and the gray bytes equal the source channel exactly.
        #[test]
        fn collapses_without_context_and_matches_channel() {
            let buf = gray_content_rgb8(9, 4);
            let want: alloc::vec::Vec<u8> = (0..36).map(|i| (i * 7 % 256) as u8).collect();
            let out = negotiate_format(buf, &[PixelDescriptor::GRAY8_SRGB], true);
            assert_eq!(out.descriptor().pixel_format(), PixelFormat::Gray8);
            let s = out.as_slice();
            let got: alloc::vec::Vec<u8> = (0..4).flat_map(|y| s.row(y)[..9].to_vec()).collect();
            assert_eq!(got, want, "gray must be the exact channel value");
        }

        /// sRGB-described ICC: the collapse is allowed and the RGB-class
        /// ICC is dropped in favor of CICP-only signaling (an RGB profile
        /// cannot describe a Gray layout; sRGB needs no profile at all).
        #[test]
        fn srgb_icc_collapses_and_drops_profile() {
            let mut ctx = ColorContext::from_icc(alloc::vec![0u8; 16]);
            ctx.cicp = Some(Cicp::SRGB);
            let buf = gray_content_rgb8(8, 2).with_color_context(Arc::new(ctx));
            let out = negotiate_format(buf, &[PixelDescriptor::GRAY8_SRGB], true);
            assert_eq!(out.descriptor().pixel_format(), PixelFormat::Gray8);
            let new_ctx = out
                .as_slice()
                .color_context()
                .cloned()
                .expect("cicp-only context survives the collapse");
            assert!(
                new_ctx.icc.is_none(),
                "an RGB-class ICC must never ride on a Gray buffer"
            );
            assert_eq!(new_ctx.cicp, Some(Cicp::SRGB));
        }

        /// Underivable ICC (junk bytes, no cicp): the collapse is
        /// suppressed and negotiation falls through to the NEXT
        /// preference instead of mislabeling or faking gray. Without
        /// `icc-db` this is also the path every non-sRGB profile takes.
        #[test]
        fn unknown_icc_suppresses_and_falls_through() {
            let ctx = ColorContext::from_icc(alloc::vec![0xAAu8; 64]);
            let buf = gray_content_rgb8(8, 2).with_color_context(Arc::new(ctx.clone()));
            let out = negotiate_format(
                buf,
                &[PixelDescriptor::GRAY8_SRGB, PixelDescriptor::RGBA8_SRGB],
                true,
            );
            assert_eq!(
                out.descriptor().pixel_format(),
                PixelFormat::Rgba8,
                "suppressed collapse must fall through to the next preference"
            );
            assert!(
                out.as_slice()
                    .color_context()
                    .is_some_and(|c| c.icc.is_some()),
                "the original RGB-class context stays with the RGB-class pixels"
            );
        }

        /// Metadata claims mono but the pixels are NOT R==G==B: the
        /// byte-level verification refuses the collapse — this is the
        /// trust-nothing property the load-bearing reduction buys over
        /// `to_gray8()` (which would have averaged the lie into luma).
        #[test]
        fn lying_metadata_never_fakes_gray() {
            let px: alloc::vec::Vec<rgb::Rgb<u8>> = (0..16)
                .map(|i| rgb::Rgb {
                    r: 200,
                    g: (i * 3) as u8,
                    b: 10,
                })
                .collect();
            let buf: PixelBuffer = PixelBuffer::from_pixels(px, 8, 2).unwrap().into();
            let out = negotiate_format(buf, &[PixelDescriptor::GRAY8_SRGB], true);
            assert_ne!(
                out.descriptor().pixel_format(),
                PixelFormat::Gray8,
                "colorful pixels must never collapse, whatever the metadata says"
            );
        }

        /// Class gate: an RGB-class ICC never rides a Gray buffer — it
        /// is stripped and the raw CICP restored as the fallback signal.
        #[test]
        fn class_gate_strips_rgb_icc_from_gray() {
            use super::super::attach_color_context_class_gated;
            let mut icc = alloc::vec![0u8; 132];
            icc[16..20].copy_from_slice(b"RGB ");
            let mut sc = zencodec::decode::SourceColor::default();
            sc.icc_profile = Some(Arc::<[u8]>::from(icc.as_slice()));
            sc.cicp = Some(Cicp::SRGB);
            // Icc authority (the default): to_color_context drops the cicp.
            let gray: PixelBuffer =
                PixelBuffer::from_pixels(alloc::vec![rgb::Gray::<u8>::new(7); 8], 4, 2)
                    .unwrap()
                    .into();
            let out = attach_color_context_class_gated(gray, &sc);
            let ctx = out.as_slice().color_context().cloned().expect("ctx");
            assert!(ctx.icc.is_none(), "RGB-class ICC stripped from gray");
            assert_eq!(
                ctx.cicp,
                Some(Cicp::SRGB),
                "raw CICP restored as the fallback after the strip"
            );

            // Same source on an RGB-layout buffer: the ICC rides.
            let rgbbuf: PixelBuffer =
                PixelBuffer::from_pixels(alloc::vec![rgb::Rgb::<u8> { r: 7, g: 7, b: 7 }; 8], 4, 2)
                    .unwrap()
                    .into();
            let out = attach_color_context_class_gated(rgbbuf, &sc);
            let ctx = out.as_slice().color_context().cloned().expect("ctx");
            assert!(ctx.icc.is_some(), "RGB-class ICC valid on RGB pixels");
        }

        /// A GRAY-class ICC is allowed onto gray output (mono AVIFs with
        /// MIAF-correct profiles), and a truncated blob never passes.
        #[test]
        fn class_gate_accepts_gray_icc_and_rejects_short() {
            use super::super::icc_class_matches_layout;
            let mut gray_icc = alloc::vec![0u8; 132];
            gray_icc[16..20].copy_from_slice(b"GRAY");
            assert!(icc_class_matches_layout(
                &gray_icc,
                zenpixels::ChannelLayout::Gray
            ));
            assert!(!icc_class_matches_layout(
                &gray_icc,
                zenpixels::ChannelLayout::Rgb
            ));
            assert!(!icc_class_matches_layout(
                &gray_icc[..64],
                zenpixels::ChannelLayout::Gray
            ));
        }

        /// Sanity for the helpers this arm depends on.
        #[test]
        fn helper_contracts() {
            assert!(wants_gray_output(&[]));
            assert!(wants_gray_output(&[PixelDescriptor::GRAY8_SRGB]));
            assert!(!wants_gray_output(&[PixelDescriptor::RGB8_SRGB]));
            assert!(format_matches(
                PixelDescriptor::GRAY8_SRGB,
                PixelDescriptor::GRAY8_SRGB
            ));
        }
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoding_default_roundtrip() {
        let enc = AvifEncoderConfig::new().with_quality(80.0);
        let pixels = vec![
            Rgb {
                r: 128u8,
                g: 64,
                b: 32
            };
            64
        ];
        let img = Img::new(pixels, 8, 8);
        let output = enc.encode_rgb8(img.as_ref()).unwrap();
        assert!(!output.data().is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoding_rgba8() {
        let enc = AvifEncoderConfig::new().with_quality(80.0);
        let pixels = vec![
            Rgba {
                r: 100u8,
                g: 150,
                b: 200,
                a: 128
            };
            64
        ];
        let img = Img::new(pixels, 8, 8);
        let output = enc.encode_rgba8(img.as_ref()).unwrap();
        assert!(!output.data().is_empty());
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoding_gray8() {
        let enc = AvifEncoderConfig::new().with_quality(80.0);
        let pixels = vec![rgb::Gray::new(128u8); 64];
        let img = Img::new(pixels, 8, 8);
        let output = enc.encode_gray8(img.as_ref()).unwrap();
        assert!(!output.data().is_empty());
    }

    #[cfg(feature = "encode")]
    #[test]
    fn supported_descriptors_includes_rgbx_and_bgrx() {
        use zencodec::encode::EncoderConfig;
        let desc = AvifEncoderConfig::supported_descriptors();
        assert!(
            desc.contains(&PixelDescriptor::RGBX8_SRGB),
            "RGBX8_SRGB must be in supported_descriptors"
        );
        assert!(
            desc.contains(&PixelDescriptor::BGRX8_SRGB),
            "BGRX8_SRGB must be in supported_descriptors"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoding_rgbx8() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let w = 16u32;
        let h = 16u32;
        // RGBX layout: byte 3 is padding; set to non-opaque value to catch leaks.
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            buf.extend_from_slice(&[255, 128, 0, 0x13]);
        }
        let slice =
            PixelSlice::new(&buf, w, h, (w * 4) as usize, PixelDescriptor::RGBX8_SRGB).unwrap();

        let enc = AvifEncoderConfig::new().with_quality(80.0);
        let output = enc.job().encoder().unwrap().encode(slice.erase()).unwrap();
        assert!(!output.data().is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoding_bgrx8() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let w = 16u32;
        let h = 16u32;
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            // BGR order, pad byte non-opaque
            buf.extend_from_slice(&[0, 128, 255, 0x42]);
        }
        let slice =
            PixelSlice::new(&buf, w, h, (w * 4) as usize, PixelDescriptor::BGRX8_SRGB).unwrap();

        let enc = AvifEncoderConfig::new().with_quality(80.0);
        let output = enc.job().encoder().unwrap().encode(slice.erase()).unwrap();
        assert!(!output.data().is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encode_rgbx8_matches_rgb8() {
        // RGBX8 should produce the same bitstream as an equivalent RGB8 encode
        // (both route through crate::encode_rgb8 with identical RGB bytes).
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let w = 16u32;
        let h = 16u32;

        let mut rgbx = Vec::with_capacity((w * h * 4) as usize);
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for i in 0..(w * h) {
            let r = (i & 0xff) as u8;
            let g = ((i >> 1) & 0xff) as u8;
            let b = ((i >> 2) & 0xff) as u8;
            rgbx.extend_from_slice(&[r, g, b, 0x55]);
            rgb.extend_from_slice(&[r, g, b]);
        }

        let rgbx_slice =
            PixelSlice::new(&rgbx, w, h, (w * 4) as usize, PixelDescriptor::RGBX8_SRGB).unwrap();
        let rgb_slice =
            PixelSlice::new(&rgb, w, h, (w * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();

        let rgbx_out = AvifEncoderConfig::new()
            .with_quality(80.0)
            .job()
            .encoder()
            .unwrap()
            .encode(rgbx_slice.erase())
            .unwrap();
        let rgb_out = AvifEncoderConfig::new()
            .with_quality(80.0)
            .job()
            .encoder()
            .unwrap()
            .encode(rgb_slice.erase())
            .unwrap();

        assert_eq!(
            rgbx_out.data(),
            rgb_out.data(),
            "RGBX8 must encode identically to RGB8 (padding byte stripped)"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoding_with_metadata() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        let enc = AvifEncoderConfig::new().with_quality(80.0);
        let pixels = vec![
            Rgb {
                r: 255u8,
                g: 0,
                b: 0
            };
            16
        ];
        let img = Img::new(pixels, 4, 4);

        let exif = b"fake exif data";
        let output = enc
            .job()
            .with_exif(&exif[..])
            .encoder()
            .unwrap()
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.data().is_empty());
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_roundtrip() {
        let enc = AvifEncoderConfig::new()
            .with_quality(80.0)
            .with_effort_u32(10);
        let pixels = vec![
            Rgb {
                r: 200u8,
                g: 100,
                b: 50
            };
            64
        ];
        let img = Img::new(pixels, 8, 8);
        let encoded = enc.encode_rgb8(img.as_ref()).unwrap();

        let dec = AvifDecoderConfig::new();
        let output = dec.decode(encoded.data()).unwrap();
        assert_eq!(output.info().width, 8);
        assert_eq!(output.info().height, 8);
        assert_eq!(output.info().format, ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn f32_roundtrip_all_simd_tiers() {
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

        let report = for_each_token_permutation(CompileTimePolicy::Warn, |_perm| {
            let pixels: Vec<Rgb<f32>> = (0..16 * 16)
                .map(|i| {
                    let t = i as f32 / 255.0;
                    Rgb {
                        r: t,
                        g: (t * 0.7),
                        b: (t * 0.3),
                    }
                })
                .collect();
            let img = imgref::ImgVec::new(pixels, 16, 16);

            let enc = AvifEncoderConfig::new()
                .with_quality(100.0)
                .with_effort_u32(10);
            let output = enc.encode_rgb_f32(img.as_ref()).unwrap();
            assert!(!output.data().is_empty());

            let dec = AvifDecoderConfig::new();
            let dst = vec![
                Rgb {
                    r: 0.0f32,
                    g: 0.0,
                    b: 0.0,
                };
                16 * 16
            ];
            let mut dst_img = imgref::ImgVec::new(dst, 16, 16);
            let _info = dec
                .decode_into_rgb_f32(output.data(), dst_img.as_mut())
                .unwrap();

            for p in dst_img.buf().iter() {
                assert!(p.r >= 0.0 && p.r <= 1.0, "r out of range: {}", p.r);
                assert!(p.g >= 0.0 && p.g <= 1.0, "g out of range: {}", p.g);
                assert!(p.b >= 0.0 && p.b <= 1.0, "b out of range: {}", p.b);
            }
        });
        assert!(report.permutations_run >= 1);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn f32_rgba_roundtrip() {
        let pixels: Vec<Rgba<f32>> = (0..16 * 16)
            .map(|i| {
                let t = i as f32 / 255.0;
                Rgba {
                    r: t,
                    g: (t * 0.7),
                    b: (t * 0.3),
                    a: 1.0,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);

        let enc = AvifEncoderConfig::new()
            .with_quality(100.0)
            .with_effort_u32(10);
        let output = enc.encode_rgba_f32(img.as_ref()).unwrap();
        assert!(!output.data().is_empty());

        let dec = AvifDecoderConfig::new();
        let mut dst_img = imgref::ImgVec::new(
            vec![
                Rgba {
                    r: 0.0f32,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0
                };
                16 * 16
            ],
            16,
            16,
        );
        dec.decode_into_rgba_f32(output.data(), dst_img.as_mut())
            .unwrap();

        for p in dst_img.buf().iter() {
            assert!(p.r >= 0.0 && p.r <= 1.0, "r out of range: {}", p.r);
            assert!(p.g >= 0.0 && p.g <= 1.0, "g out of range: {}", p.g);
            assert!(p.b >= 0.0 && p.b <= 1.0, "b out of range: {}", p.b);
            assert!(p.a >= 0.0 && p.a <= 1.0, "a out of range: {}", p.a);
        }
    }

    #[cfg(feature = "encode")]
    #[test]
    fn f32_gray_roundtrip() {
        use rgb::Gray;

        let pixels: Vec<Gray<f32>> = (0..16 * 16).map(|i| Gray(i as f32 / 255.0)).collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);

        let enc = AvifEncoderConfig::new()
            .with_quality(100.0)
            .with_effort_u32(10);
        let output = enc.encode_gray_f32(img.as_ref()).unwrap();
        assert!(!output.data().is_empty());

        let dec = AvifDecoderConfig::new();
        let mut dst_img = imgref::ImgVec::new(vec![Gray(0.0f32); 16 * 16], 16, 16);
        dec.decode_into_gray_f32(output.data(), dst_img.as_mut())
            .unwrap();

        for p in dst_img.buf().iter() {
            assert!(
                p.value() >= 0.0 && p.value() <= 1.0,
                "gray out of range: {}",
                p.value()
            );
        }
    }

    #[cfg(feature = "encode")]
    #[test]
    fn effort_and_quality_getters() {
        use zencodec::encode::EncoderConfig;
        let config = AvifEncoderConfig::new()
            .with_generic_quality(75.0)
            .with_generic_effort(5);

        assert_eq!(config.generic_quality(), Some(75.0));
        assert_eq!(config.generic_effort(), Some(5));
        assert_eq!(config.is_lossless(), Some(false));
    }

    #[cfg(feature = "encode")]
    #[test]
    fn four_layer_encode_flow() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200
            };
            8 * 8
        ];
        let img = imgref::ImgVec::new(pixels, 8, 8);

        let config = AvifEncoderConfig::new().with_quality(80.0);
        let output = config
            .job()
            .encoder()
            .unwrap()
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn four_layer_decode_flow() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};

        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200
            };
            8 * 8
        ];
        let img = imgref::ImgVec::new(pixels, 8, 8);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        let config = AvifDecoderConfig::new();
        let decoded = config
            .job()
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(decoded.width(), 8);
        assert_eq!(decoded.height(), 8);
    }

    // ── Encoder trait roundtrip tests ──────────────────────────────────────

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgb8() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let pixels: Vec<Rgb<u8>> = (0..16 * 16)
            .map(|i| Rgb {
                r: (i % 256) as u8,
                g: ((i * 3) % 256) as u8,
                b: ((i * 7) % 256) as u8,
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgba8() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let pixels: Vec<Rgba<u8>> = (0..16 * 16)
            .map(|i| Rgba {
                r: (i % 256) as u8,
                g: ((i * 3) % 256) as u8,
                b: ((i * 7) % 256) as u8,
                a: ((i * 5) % 256) as u8,
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_gray8() {
        use rgb::Gray;
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let pixels: Vec<Gray<u8>> = (0..16 * 16).map(|i| Gray((i % 256) as u8)).collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgb_f32() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let pixels: Vec<Rgb<f32>> = (0..16 * 16)
            .map(|i| {
                let t = i as f32 / 255.0;
                Rgb {
                    r: t,
                    g: t * 0.5,
                    b: t * 0.25,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgba_f32() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let pixels: Vec<Rgba<f32>> = (0..16 * 16)
            .map(|i| {
                let t = i as f32 / 255.0;
                Rgba {
                    r: t,
                    g: t * 0.5,
                    b: t * 0.25,
                    a: 1.0,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_gray_f32() {
        use rgb::Gray;
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let pixels: Vec<Gray<f32>> = (0..16 * 16).map(|i| Gray(i as f32 / 255.0)).collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_dyn_encoder() {
        use zencodec::encode::{EncodeJob, EncoderConfig};

        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let config = AvifEncoderConfig::new().with_quality(50.0);
        let dyn_enc = config.job().dyn_encoder().unwrap();
        let output = dyn_enc
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    // ── HDR / 16-bit encoder tests ──────────────────────────────────────

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgb16_srgb() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let pixels: Vec<Rgb<u16>> = (0..16 * 16)
            .map(|i| {
                let v = (i * 256) as u16;
                Rgb {
                    r: v,
                    g: v / 2,
                    b: v / 3,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgba16_srgb() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let pixels: Vec<Rgba<u16>> = (0..16 * 16)
            .map(|i| {
                let v = (i * 256) as u16;
                Rgba {
                    r: v,
                    g: v / 2,
                    b: v / 3,
                    a: 65535,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgb16_pq_bt2020() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{ColorPrimaries, TransferFunction};

        let pixels: Vec<Rgb<u16>> = (0..16 * 16)
            .map(|i| {
                let v = (i * 256) as u16;
                Rgb {
                    r: v,
                    g: v / 2,
                    b: v / 3,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let desc = PixelDescriptor::RGB16_SRGB
            .with_transfer(TransferFunction::Pq)
            .with_primaries(ColorPrimaries::Bt2020);
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(slice.erase()).unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgba16_pq_bt2020() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{ColorPrimaries, TransferFunction};

        let pixels: Vec<Rgba<u16>> = (0..16 * 16)
            .map(|i| {
                let v = (i * 256) as u16;
                Rgba {
                    r: v,
                    g: v / 2,
                    b: v / 3,
                    a: 65535,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let desc = PixelDescriptor::RGBA16_SRGB
            .with_transfer(TransferFunction::Pq)
            .with_primaries(ColorPrimaries::Bt2020);
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(slice.erase()).unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgb16_hlg_bt2020() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{ColorPrimaries, TransferFunction};

        let pixels: Vec<Rgb<u16>> = (0..16 * 16)
            .map(|i| {
                let v = (i * 256) as u16;
                Rgb {
                    r: v,
                    g: v / 2,
                    b: v / 3,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let desc = PixelDescriptor::RGB16_SRGB
            .with_transfer(TransferFunction::Hlg)
            .with_primaries(ColorPrimaries::Bt2020);
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(slice.erase()).unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgba16_hlg_bt2020() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{ColorPrimaries, TransferFunction};

        let pixels: Vec<Rgba<u16>> = (0..16 * 16)
            .map(|i| {
                let v = (i * 256) as u16;
                Rgba {
                    r: v,
                    g: v / 2,
                    b: v / 3,
                    a: 65535,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let desc = PixelDescriptor::RGBA16_SRGB
            .with_transfer(TransferFunction::Hlg)
            .with_primaries(ColorPrimaries::Bt2020);
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(slice.erase()).unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgb16_display_p3() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::ColorPrimaries;

        let pixels: Vec<Rgb<u16>> = (0..16 * 16)
            .map(|i| {
                let v = (i * 256) as u16;
                Rgb {
                    r: v,
                    g: v / 2,
                    b: v / 3,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let desc = PixelDescriptor::RGB16_SRGB.with_primaries(ColorPrimaries::DisplayP3);
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(slice.erase()).unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgba16_display_p3() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::ColorPrimaries;

        let pixels: Vec<Rgba<u16>> = (0..16 * 16)
            .map(|i| {
                let v = (i * 256) as u16;
                Rgba {
                    r: v,
                    g: v / 2,
                    b: v / 3,
                    a: 65535,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let desc = PixelDescriptor::RGBA16_SRGB.with_primaries(ColorPrimaries::DisplayP3);
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(slice.erase()).unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_pq_bt2020_roundtrip() {
        use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{ColorPrimaries, TransferFunction};

        // Encode with PQ/BT.2020 descriptor
        let pixels: Vec<Rgb<u16>> = (0..16 * 16)
            .map(|i| {
                let v = ((i as u32 * 256) % 65536) as u16;
                Rgb {
                    r: v,
                    g: v / 2,
                    b: v / 3,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let desc = PixelDescriptor::RGB16_SRGB
            .with_transfer(TransferFunction::Pq)
            .with_primaries(ColorPrimaries::Bt2020);
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
        let config = AvifEncoderConfig::new().with_quality(80.0);
        let encoder = config.job().encoder().unwrap();
        let encoded = encoder.encode(slice.erase()).unwrap();
        assert!(!encoded.is_empty());

        // Decode and verify we get pixels back
        let dec_config = AvifDecoderConfig::new();
        let decoder = dec_config
            .job()
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .unwrap();
        let decoded = decoder.decode().unwrap();
        assert_eq!(decoded.info().width, 16);
        assert_eq!(decoded.info().height, 16);
    }

    /// Regression for the `apply_descriptor_color` CICP-override bug: a
    /// `Metadata`-set CICP must win over the pixel descriptor's color, and the
    /// emitted nclx matrix must stay consistent with it. We hand pixels whose
    /// descriptor reads sRGB / BT.709 (primaries=1) but set
    /// `Metadata.cicp = DISPLAY_P3` (primaries=12); the decoded nclx must report
    /// Display-P3, not the descriptor's BT.709.
    #[cfg(feature = "encode")]
    #[test]
    fn caller_cicp_wins_over_descriptor_color() {
        use zencodec::Cicp;
        use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        // sRGB / BT.709 descriptor pixels.
        let pixels = vec![
            Rgb {
                r: 200u8,
                g: 100,
                b: 50,
            };
            16 * 16
        ];
        let img = imgref::ImgVec::new(pixels, 16, 16);
        // PixelDescriptor::RGB8_SRGB ⇒ BT.709 primaries, sRGB transfer.
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(PixelDescriptor::RGB8_SRGB);

        // Caller pins Display-P3 via Metadata. Use the blessed metadata path.
        let meta = Metadata::none().with_cicp(Cicp::DISPLAY_P3);
        let encoder = AvifEncoderConfig::new()
            .with_quality(90.0)
            .job()
            .with_metadata_policy(meta, zencodec::MetadataPolicy::PreserveExact)
            .encoder()
            .unwrap();
        let encoded = encoder.encode(slice.erase()).unwrap();
        assert!(!encoded.is_empty());

        // Decode and read back the nclx CICP.
        let decoder = AvifDecoderConfig::new()
            .job()
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .unwrap();
        let decoded = decoder.decode().unwrap();
        let cicp = decoded
            .info()
            .source_color
            .cicp
            .expect("decoded AVIF must carry CICP");

        // Caller's Display-P3 (primaries 12) wins over the descriptor's BT.709 (1).
        assert_eq!(
            cicp.color_primaries,
            Cicp::DISPLAY_P3.color_primaries,
            "caller's Display-P3 primaries must win over descriptor BT.709"
        );
        assert_eq!(
            cicp.transfer_characteristics,
            Cicp::DISPLAY_P3.transfer_characteristics,
            "transfer must match the caller's CICP"
        );
        // The matrix code point must honestly describe the YCbCr math the encoder
        // actually used. zenravif's default RGB path encodes via BT.601 YCbCr and
        // writes matrix_coefficients = 6 — so the consistent value here is 6, NOT
        // the caller CICP's Identity(0) (which describes an RGB-domain image, not
        // how AVIF stored it). The bug was a *missing/stale* MC; the fix makes the
        // emitted nclx a coherent triple {primaries:12, transfer:13, matrix:6}.
        assert_eq!(
            cicp.matrix_coefficients, 6,
            "matrix must reflect the encoder's actual YCbCr matrix (BT.601)"
        );
    }

    /// The descriptor still drives CICP when the caller supplies none — the
    /// fallback the bug fix must preserve. sRGB/BT.709 descriptor with no
    /// Metadata CICP ⇒ nclx reports BT.709 primaries with a consistent matrix.
    #[cfg(feature = "encode")]
    #[test]
    fn descriptor_drives_cicp_without_caller_cicp() {
        use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let pixels = vec![
            Rgb {
                r: 30u8,
                g: 200,
                b: 120,
            };
            16 * 16
        ];
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(PixelDescriptor::RGB8_SRGB);

        // No Metadata CICP at all.
        let encoder = AvifEncoderConfig::new()
            .with_quality(90.0)
            .job()
            .encoder()
            .unwrap();
        let encoded = encoder.encode(slice.erase()).unwrap();

        let decoder = AvifDecoderConfig::new()
            .job()
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .unwrap();
        let decoded = decoder.decode().unwrap();
        let cicp = decoded
            .info()
            .source_color
            .cicp
            .expect("decoded AVIF must carry CICP");

        // Descriptor's BT.709 (primaries 1) flows through.
        assert_eq!(
            cicp.color_primaries, 1,
            "descriptor BT.709 primaries must drive nclx when no caller CICP"
        );
        // As above, the emitted matrix reflects the encoder's actual YCbCr math
        // (zenravif default RGB path = BT.601 = 6), kept consistent with the
        // descriptor-driven primaries/transfer.
        assert_eq!(
            cicp.matrix_coefficients, 6,
            "matrix must reflect the encoder's actual YCbCr matrix (BT.601)"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_pq_bt2020_narrow_range() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{ColorPrimaries, SignalRange, TransferFunction};

        // PQ BT.2020 with narrow/limited signal range
        let pixels: Vec<Rgb<u16>> = (0..16 * 16)
            .map(|i| {
                let v = (i * 256) as u16;
                Rgb {
                    r: v,
                    g: v / 2,
                    b: v / 3,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let desc = PixelDescriptor::RGB16_SRGB
            .with_transfer(TransferFunction::Pq)
            .with_primaries(ColorPrimaries::Bt2020)
            .with_signal_range(SignalRange::Narrow);
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(slice.erase()).unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgb_f32_pq_bt2020() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{ColorPrimaries, TransferFunction};

        // f32 PQ BT.2020 — should route through u16 path, not linear_to_srgb_u8
        let pixels: Vec<Rgb<f32>> = (0..16 * 16)
            .map(|i| {
                let v = i as f32 / 256.0;
                Rgb {
                    r: v,
                    g: v * 0.8,
                    b: v * 0.6,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let desc = PixelDescriptor::RGBF32_LINEAR
            .with_transfer(TransferFunction::Pq)
            .with_primaries(ColorPrimaries::Bt2020);
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(slice.erase()).unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_rgba_f32_hlg_bt2020() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{ColorPrimaries, TransferFunction};

        // f32 HLG BT.2020 — should route through u16 path
        let pixels: Vec<Rgba<f32>> = (0..16 * 16)
            .map(|i| {
                let v = i as f32 / 256.0;
                Rgba {
                    r: v,
                    g: v * 0.7,
                    b: v * 0.5,
                    a: 1.0,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let desc = PixelDescriptor::RGBAF32_LINEAR
            .with_transfer(TransferFunction::Hlg)
            .with_primaries(ColorPrimaries::Bt2020);
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
        let config = AvifEncoderConfig::new().with_quality(60.0);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(slice.erase()).unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_f32_pq_roundtrip_preserves_hdr() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{ColorPrimaries, TransferFunction};

        // Encode f32 PQ data, decode, verify the output has >8-bit depth
        // (proving it went through the u16 path, not the sRGB u8 path)
        let pixels: Vec<Rgb<f32>> = (0..16 * 16)
            .map(|i| {
                let v = i as f32 / 256.0;
                Rgb {
                    r: v,
                    g: v * 0.9,
                    b: v * 0.7,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let desc = PixelDescriptor::RGBF32_LINEAR
            .with_transfer(TransferFunction::Pq)
            .with_primaries(ColorPrimaries::Bt2020);
        let slice = PixelSlice::from(img.as_ref()).with_descriptor(desc);
        let config = AvifEncoderConfig::new().with_quality(90.0);
        let encoder = config.job().encoder().unwrap();
        let encoded = encoder.encode(slice.erase()).unwrap();

        // Decode and verify bit depth > 8 (proving 10-bit encode path was used)
        let dec = AvifDecoderConfig::new();
        let decoded = dec.decode(encoded.data()).unwrap();
        assert!(decoded.info().source_color.bit_depth.unwrap_or(8) >= 10);
    }

    // ── ResourceLimits enforcement tests ──────────────────────────────────

    #[cfg(feature = "encode")]
    #[test]
    fn encode_max_output_bytes_rejects() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let config = AvifEncoderConfig::new().with_quality(80.0);
        // 100 bytes is too small for any AVIF output
        let limits = ResourceLimits::none().with_max_output(100);
        let encoder = config.job().with_limits(limits).encoder().unwrap();
        let result = encoder.encode(PixelSlice::from(img.as_ref()).erase());
        assert!(
            result.is_err(),
            "encode should fail with max_output_bytes=100"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_max_input_bytes_rejects() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};

        // First encode a valid image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();
        assert!(encoded.data().len() > 100);

        // Decode with max_input_bytes=100 should fail
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none().with_max_input_bytes(100);
        let result = config
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .and_then(|dec| dec.decode());
        assert!(
            result.is_err(),
            "decode should fail with max_input_bytes=100"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_max_width_rejects() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};

        // Encode a 32x32 image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        // Decode with max_width=10 should fail for a 32-wide image
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none()
            .with_max_width(10)
            .with_max_height(10);
        let result = config
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .and_then(|dec| dec.decode());
        assert!(
            result.is_err(),
            "decode should fail with max_width=10 for 32px image"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_max_memory_rejects() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};

        // Encode a 32x32 image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        // 100 bytes of memory is not enough to decode a 32x32 image
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none().with_max_memory(100);
        let result = config
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .and_then(|dec| dec.decode());
        assert!(
            result.is_err(),
            "decode should fail with max_memory_bytes=100"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_push_decoder_checks_input_size() {
        use zencodec::decode::{DecodeJob, DecodeRowSink, DecoderConfig, SinkError};
        use zenpixels::PixelSliceMut;

        struct DiscardSink {
            buf: Vec<u8>,
        }
        impl DecodeRowSink for DiscardSink {
            fn provide_next_buffer(
                &mut self,
                _y: u32,
                height: u32,
                width: u32,
                descriptor: PixelDescriptor,
            ) -> Result<PixelSliceMut<'_>, SinkError> {
                let bpp = descriptor.bytes_per_pixel();
                let stride = width as usize * bpp;
                let needed = height as usize * stride;
                self.buf.resize(needed, 0);
                Ok(
                    PixelSliceMut::new(&mut self.buf, width, height, stride, descriptor)
                        .expect("buffer sized correctly"),
                )
            }
        }

        // Encode a valid image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        // push_decoder with max_input_bytes=100 should fail
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none().with_max_input_bytes(100);
        let mut sink = DiscardSink { buf: Vec::new() };
        let result = config.job().with_limits(limits).push_decoder(
            Cow::Borrowed(encoded.data()),
            &mut sink,
            &[],
        );
        assert!(
            result.is_err(),
            "push_decoder should fail with max_input_bytes=100"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_streaming_checks_input_size() {
        use zencodec::decode::{DecodeJob, DecoderConfig};

        // Encode a valid image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        // streaming_decoder with max_input_bytes=100 should fail
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none().with_max_input_bytes(100);
        let result = config
            .job()
            .with_limits(limits)
            .streaming_decoder(Cow::Borrowed(encoded.data()), &[]);
        assert!(
            result.is_err(),
            "streaming_decoder should fail with max_input_bytes=100"
        );
    }

    // ── ThreadingPolicy tests ──────────────────────────────────────────

    #[cfg(feature = "encode")]
    #[test]
    fn single_thread_encode_decode_roundtrip() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        // Encode with SingleThread threading policy
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            16 * 16
        ];
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let config = AvifEncoderConfig::new().with_quality(80.0);
        let limits = ResourceLimits::none().with_threading(zencodec::ThreadingPolicy::Sequential);
        let encoder = config.job().with_limits(limits).encoder().unwrap();
        let encoded = encoder
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!encoded.is_empty());

        // Decode with SingleThread threading policy
        let dec_config = AvifDecoderConfig::new();
        let dec_limits =
            ResourceLimits::none().with_threading(zencodec::ThreadingPolicy::Sequential);
        let decoded = dec_config
            .job()
            .with_limits(dec_limits)
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(decoded.info().width, 16);
        assert_eq!(decoded.info().height, 16);
    }

    // ── Issue fix verification tests ──────────────────────────────────

    #[cfg(feature = "encode")]
    #[test]
    fn decode_memory_limit_produces_resource_limit_error() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};

        // Encode a valid image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        // Set max_memory to 1 byte — decode should fail with ResourceLimit, not Encode
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none().with_max_memory(1);
        let decoder = config
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(encoded.data()), &[]);
        // The limit may be checked at decoder() or decode() stage
        let result = match decoder {
            Err(e) => Err(e),
            Ok(dec) => dec.decode().map(|_| ()),
        };
        assert!(result.is_err(), "expected error from memory limit");
        let err = result.err().unwrap();
        // Pattern B (envelope): the coarse category is read off `At<CodecError>`
        // directly (native `Error::ResourceLimit` maps to `Resource(Limits(Memory))`),
        // and the native detail stays reachable via downcast — faithful to the
        // original `matches!(.., Error::ResourceLimit(_))` intent.
        assert_eq!(
            err.error().category(),
            zencodec::ErrorCategory::Resource(zencodec::ResourceError::Limits(
                zencodec::LimitKind::Memory
            )),
            "expected a memory Resource(Limits) category, got: {err}"
        );
        assert!(
            matches!(
                err.error().detail().and_then(|d| d.downcast_ref::<Error>()),
                Some(Error::ResourceLimit(_))
            ),
            "envelope must carry the native Error::ResourceLimit detail, got: {err}"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_input_size_limit_produces_resource_limit_error() {
        use zencodec::decode::{DecodeJob, DecoderConfig};

        // Encode a valid image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        // Set max_input_bytes to 1 — decode should fail with ResourceLimit, not Encode
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none().with_max_input_bytes(1);
        let result = config
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(encoded.data()), &[]);
        assert!(result.is_err(), "expected error from memory limit");
        let err = result.err().unwrap();
        // Pattern B (envelope): the coarse category is read off `At<CodecError>`
        // directly (native `Error::ResourceLimit` maps to `Resource(Limits(Memory))`),
        // and the native detail stays reachable via downcast — faithful to the
        // original `matches!(.., Error::ResourceLimit(_))` intent.
        assert_eq!(
            err.error().category(),
            zencodec::ErrorCategory::Resource(zencodec::ResourceError::Limits(
                zencodec::LimitKind::Memory
            )),
            "expected a memory Resource(Limits) category, got: {err}"
        );
        assert!(
            matches!(
                err.error().detail().and_then(|d| d.downcast_ref::<Error>()),
                Some(Error::ResourceLimit(_))
            ),
            "envelope must carry the native Error::ResourceLimit detail, got: {err}"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encode_capabilities_no_native_gray_or_f32() {
        use zencodec::encode::EncoderConfig;
        let caps = AvifEncoderConfig::capabilities();
        assert!(
            !caps.native_gray(),
            "native_gray should be false: Gray8 expands to RGB"
        );
        assert!(
            !caps.native_f32(),
            "native_f32 should be false: f32 quantizes to u8/u16"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn avif_animation_frame_encoder_implements_trait() {
        fn _assert_trait<T: zencodec::encode::AnimationFrameEncoder + Send + 'static>() {}
        _assert_trait::<super::AvifAnimationFrameEncoder>();
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encode_capabilities_include_animation() {
        use zencodec::encode::EncoderConfig;
        let caps = AvifEncoderConfig::capabilities();
        assert!(caps.animation(), "animation should be true");
    }

    #[test]
    fn avif_animation_frame_decoder_implements_trait() {
        // AvifAnimationFrameDecoder implements AnimationFrameDecoder which includes loop_count()
        fn _assert_trait<T: zencodec::decode::AnimationFrameDecoder>() {}
        _assert_trait::<super::AvifAnimationFrameDecoder>();
    }

    #[test]
    fn animated_avif_animation_frame_decoder_roundtrip() {
        use super::AvifDecoderConfig;
        use std::borrow::Cow;
        use zencodec::decode::{AnimationFrameDecoder, DecodeJob, DecoderConfig};

        // This fixture lives in the codec-corpus repo, which CI does not
        // clone. Per the no-graceful-skips policy the skip decision belongs
        // to the CALLER: environments without the corpus declare it via
        // ZENAVIF_NO_CODEC_CORPUS=1 (set in ci.yml); everywhere else a
        // missing file is a loud failure, never a silent pass. (The old
        // `../codec-corpus` + exists()-return silently no-oped everywhere:
        // the corpus checkout lives one level further up.)
        if std::env::var_os("ZENAVIF_NO_CODEC_CORPUS").is_some() {
            return;
        }
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let rel = "codec-corpus/avif-conformance/valid/2.avif";
        let path = [manifest.join("..").join(rel), manifest.join("../..").join(rel)]
            .into_iter()
            .find(|p| p.exists())
            .unwrap_or_else(|| {
                panic!(
                    "codec-corpus fixture {rel} not found beside {} (clone                      imazen/codec-corpus at ~/work/, or set                      ZENAVIF_NO_CODEC_CORPUS=1 to declare it absent)",
                    manifest.display()
                )
            });
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

        let config = AvifDecoderConfig::new();
        let mut decoder = config
            .job()
            .animation_frame_decoder(Cow::Borrowed(&data), &[])
            .expect("animation_frame_decoder should succeed for animated AVIF");

        // Verify metadata
        let info = decoder.info();
        assert!(info.is_animation(), "should be detected as animation");
        assert!(
            info.width > 0 && info.height > 0,
            "dimensions must be nonzero"
        );

        // frame_count and loop_count should be populated
        let frame_count = decoder.frame_count();
        assert!(
            frame_count.is_some(),
            "frame_count should be Some for animated AVIF"
        );
        let total = frame_count.unwrap();
        assert!(
            total >= 2,
            "animated AVIF should have at least 2 frames, got {total}"
        );

        let loop_count = decoder.loop_count();
        assert!(
            loop_count.is_some(),
            "loop_count should be Some for animated AVIF"
        );

        // Decode all frames
        let mut frames_decoded = 0u32;
        loop {
            match decoder.render_next_frame(None) {
                Ok(Some(frame)) => {
                    assert_eq!(frame.frame_index(), frames_decoded);
                    let pixels = frame.pixels();
                    assert!(
                        pixels.width() > 0 && pixels.rows() > 0,
                        "frame {} pixels should have nonzero dimensions",
                        frames_decoded
                    );
                    frames_decoded += 1;
                }
                Ok(None) => break,
                Err(e) => panic!("render_next_frame failed at frame {frames_decoded}: {e}"),
            }
        }

        assert_eq!(
            frames_decoded, total,
            "should decode exactly {total} frames, got {frames_decoded}"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn zencodec_animation_encode_decode_roundtrip() {
        use zencodec::decode::{AnimationFrameDecoder, DecodeJob, DecoderConfig};
        use zencodec::encode::{AnimationFrameEncoder, EncodeJob, EncoderConfig};

        let config = AvifEncoderConfig::new()
            .with_generic_quality(80.0)
            .with_generic_effort(0);
        let mut enc = config
            .job()
            .with_canvas_size(64, 64)
            .with_loop_count(Some(0))
            .animation_frame_encoder()
            .expect("animation_frame_encoder should succeed");

        // Push 3 solid-color RGB8 frames
        let colors: [Rgb<u8>; 3] = [
            Rgb { r: 255, g: 0, b: 0 },
            Rgb { r: 0, g: 255, b: 0 },
            Rgb { r: 0, g: 0, b: 255 },
        ];
        for color in &colors {
            let pixels: Vec<Rgb<u8>> = vec![*color; 64 * 64];
            let img = imgref::ImgVec::new(pixels, 64, 64);
            let ps = PixelSlice::from(img.as_ref()).erase();
            enc.push_frame(ps, 100, None).unwrap();
        }

        let output = enc.finish(None).expect("animation finish should succeed");
        assert!(!output.is_empty(), "encoded animation should not be empty");

        // Decode via zencodec animation frame decoder
        let dec_config = AvifDecoderConfig::new();
        let mut decoder = dec_config
            .job()
            .animation_frame_decoder(Cow::Borrowed(output.data()), &[])
            .expect("should decode the animated AVIF");

        assert_eq!(decoder.frame_count(), Some(3));
        let mut count = 0u32;
        while let Ok(Some(_frame)) = decoder.render_next_frame(None) {
            count += 1;
        }
        assert_eq!(count, 3, "should decode exactly 3 frames");
    }

    // Gain map zencodec extras tests are in tests/gainmap_decode.rs
    // (integration test) to avoid pre-existing compile errors in this
    // module when `encode` feature is not enabled.

    // ── Memory-adaptive encode concurrency (max_memory_bytes on ENCODE) ──

    /// A tight explicit cap must reject the encode via the CALIBRATED
    /// thread-aware estimate — even though the raw input-buffer size
    /// (`w*h*bpp`, the pre-2026-08 check) fits comfortably. 512×512 RGB8:
    /// raw input = 768 KiB, calibrated single-thread conservative peak
    /// ≈ 24 MB; a 4 MB cap passes the former and must fail the latter,
    /// BEFORE any encoding work happens (the test is instant).
    #[cfg(feature = "encode")]
    #[test]
    fn encode_max_memory_calibrated_rejects() {
        use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};

        let (w, h) = (512usize, 512usize);
        let cap = 4u64 * 1024 * 1024;
        assert!(
            (w * h * 3) as u64 <= cap,
            "precondition: the raw input buffer must fit the cap, so only \
             the calibrated estimate can reject"
        );
        let est1 = crate::heuristics::estimate_encode_threaded(w as u32, h as u32, 3, 4, 1)
            .unwrap()
            .peak_memory_bytes_max;
        assert!(
            est1 > cap,
            "precondition: the single-thread calibrated peak ({est1}) must exceed the cap"
        );

        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 90,
                g: 120,
                b: 40,
            };
            w * h
        ];
        let img = imgref::ImgVec::new(pixels, w, h);
        let result = AvifEncoderConfig::new()
            .job()
            .with_limits(ResourceLimits::none().with_max_memory(cap))
            .encoder()
            .unwrap()
            .encode(PixelSlice::from(img.as_ref()).erase());
        let err = result.expect_err("tight max_memory must reject the encode pre-flight");
        let msg = format!("{err}").to_lowercase();
        assert!(
            msg.contains("mem"),
            "error should be the memory-limit error, got: {msg}"
        );
    }

    /// A moderate explicit cap — above the calibrated worst-case peak at
    /// every thread count (the tile bound caps the per-thread term) — must
    /// let the encode run, with no thread pin and no reduction note.
    #[cfg(feature = "encode")]
    #[test]
    fn encode_moderate_max_memory_succeeds() {
        use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};

        let (w, h) = (512usize, 512usize);
        // 512² has 4 tiles, so ≥ 4 threads all estimate alike; est at 4 is
        // the machine-independent worst case.
        let worst = crate::heuristics::estimate_encode_threaded(w as u32, h as u32, 3, 10, 4)
            .unwrap()
            .peak_memory_bytes_max;
        let cap = 64u64 * 1024 * 1024;
        assert!(
            worst < cap,
            "precondition: worst-case estimate fits the cap"
        );

        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 10,
                g: 200,
                b: 130,
            };
            w * h
        ];
        let img = imgref::ImgVec::new(pixels, w, h);
        let output = AvifEncoderConfig::new()
            .with_effort_u32(10) // speed 10 — fastest; memory model is speed-invariant
            .job()
            .with_limits(ResourceLimits::none().with_max_memory(cap))
            .encoder()
            .unwrap()
            .encode(PixelSlice::from(img.as_ref()).erase())
            .expect("encode under a moderate max_memory must succeed");
        assert!(!output.is_empty());
        assert!(
            output.extras::<String>().is_none(),
            "no thread reduction happened, so no note should be attached"
        );
    }

    /// A cap between the 1-thread and 2-thread conservative peaks forces the
    /// fit to walk the (explicitly requested) 8 threads down to 2 — the
    /// encode succeeds AND the reduction is recorded on the output
    /// (reductions are never silent). Deterministic on any machine: the
    /// start is the explicit request, and 512²'s tile bound (4) caps the
    /// per-thread term independent of core count.
    #[cfg(feature = "encode")]
    #[test]
    fn encode_thread_reduction_is_recorded() {
        use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};

        let (w, h) = (512usize, 512usize);
        let est = |threads: usize| {
            crate::heuristics::estimate_encode_threaded(w as u32, h as u32, 3, 10, threads)
                .unwrap()
                .peak_memory_bytes_max
        };
        // Admits 2 threads, not 3 (each extra thread costs the calibrated
        // per-thread term).
        let cap = est(2);
        assert!(
            est(3) > cap && est(1) < cap,
            "precondition: cap isolates 2 threads"
        );

        let mut config = AvifEncoderConfig::new().with_effort_u32(10);
        let inner = config.inner().clone().threads(Some(8));
        *config.inner_mut() = inner;

        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 200,
                g: 60,
                b: 60,
            };
            w * h
        ];
        let img = imgref::ImgVec::new(pixels, w, h);
        let output = config
            .job()
            .with_limits(ResourceLimits::none().with_max_memory(cap))
            .encoder()
            .unwrap()
            .encode(PixelSlice::from(img.as_ref()).erase())
            .expect("encode must succeed at the fitted thread count");
        let note = output
            .extras::<String>()
            .expect("thread reduction must be recorded on the output");
        assert!(
            note.contains("8") && note.contains("2"),
            "note should record 8 -> 2, got: {note}"
        );
    }

    /// `estimate_encode_resources` must be thread-aware: more cores → higher
    /// peak (per-thread working-set term) and lower wall time (Amdahl), with
    /// cpu_ms carrying the single-thread work.
    #[cfg(feature = "encode")]
    #[test]
    fn estimate_encode_resources_is_thread_aware() {
        use zencodec::encode::EncoderConfig as _;
        use zencodec::estimate::{ComputeEnvironment, ImageCharacteristics};
        use zenpixels::PixelDescriptor;

        let config = AvifEncoderConfig::new();
        let image = ImageCharacteristics::new(2048, 2048, PixelDescriptor::RGB8_SRGB);
        let e1 = config.estimate_encode_resources(&image, &ComputeEnvironment::new());
        let e8 = config.estimate_encode_resources(&image, &ComputeEnvironment::new().with_cores(8));
        assert!(
            e8.peak_memory_bytes_est().unwrap() > e1.peak_memory_bytes_est().unwrap(),
            "peaks must carry the per-thread term"
        );
        assert!(
            e8.peak_memory_bytes_max().unwrap() > e1.peak_memory_bytes_max().unwrap(),
            "max peak must carry the per-thread term"
        );
        assert!(
            e8.wall_ms().unwrap() < e1.wall_ms().unwrap(),
            "wall time must shrink with cores"
        );
        assert_eq!(
            e1.cpu_ms().unwrap(),
            e8.cpu_ms().unwrap(),
            "total CPU work is core-invariant"
        );
    }
}
