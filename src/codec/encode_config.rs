//! [`AvifEncoderConfig`] — the [`zencodec::encode::EncoderConfig`] adapter:
//! universal quality/effort/lossless settings over the native
//! [`crate::EncoderConfig`], the supported-descriptor and capability statics,
//! and the calibrated generic-quality to AVIF-quality table.

use rgb::{Rgb, Rgba};
use whereat::At;
use zencodec::encode::EncodeOutput;
use zencodec::{CodecError, ImageFormat, ResourceLimits};
use zenpixels::{PixelDescriptor, PixelSlice};

use super::encode_job::AvifEncodeJob;

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
    pub(super) inner: crate::EncoderConfig,
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
pub(super) static AVIF_ENCODE_CAPABILITIES: zencodec::encode::EncodeCapabilities =
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn encode_capabilities_include_animation() {
        use zencodec::encode::EncoderConfig;
        let caps = AvifEncoderConfig::capabilities();
        assert!(caps.animation(), "animation should be true");
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
