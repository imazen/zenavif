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
    GainMapPresence, ImageFormat, ImageInfo, ImageSequence, ResourceLimits, Supplements,
};
use zenpixels::{ChannelType, ColorAuthority, PixelBuffer, PixelDescriptor, PixelSlice};
use zenpixels_convert::PixelBufferConvertTypedExt as _;

use crate::error::Error;
use whereat::{At, at};

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
    pub fn encode_rgb8(&self, img: imgref::ImgRef<'_, Rgb<u8>>) -> Result<EncodeOutput, At<Error>> {
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
    ) -> Result<EncodeOutput, At<Error>> {
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
    ) -> Result<EncodeOutput, At<Error>> {
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
    ) -> Result<EncodeOutput, At<Error>> {
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
    ) -> Result<EncodeOutput, At<Error>> {
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
    ) -> Result<EncodeOutput, At<Error>> {
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
    type Error = At<Error>;
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

    fn with_alpha_quality(mut self, quality: f32) -> Self {
        self.inner = self.inner.alpha_quality(quality);
        self
    }

    fn alpha_quality(&self) -> Option<f32> {
        self.inner.alpha_quality
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
    type Error = At<Error>;
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

    fn encoder(self) -> Result<AvifEncoder, At<Error>> {
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
            // Convert from f32 CIE xy (0.0–1.0) to 0.16 fixed-point (u16)
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
                // 24.8 fixed-point: multiply by 256
                max_luminance: (mdcv.max_luminance * 256.0 + 0.5) as u32,
                // 18.14 fixed-point: multiply by 16384
                min_luminance: (mdcv.min_luminance * 16384.0 + 0.5) as u32,
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
        // own default rather than pinning a thread count.
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

    fn animation_frame_encoder(self) -> Result<AvifAnimationFrameEncoder, At<Error>> {
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
        // Apply threading policy
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

    fn check_limits(&self, w: usize, h: usize, bpp: u64) -> Result<(), At<Error>> {
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
            .map_err(|e| at!(Error::Encode(format!("{e}"))))?;
        Ok(())
    }

    fn make_output(&self, data: Vec<u8>) -> Result<EncodeOutput, At<Error>> {
        self.limits
            .check_output_size(data.len() as u64)
            .map_err(|e| at!(Error::Encode(format!("{e}"))))?;
        Ok(EncodeOutput::new(data, ImageFormat::Avif))
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
    fn encode_f32_as_u16_rgb(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 6)?; // 6 bytes per u16 RGB pixel
        let cfg = self.build_config();
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
    fn encode_f32_as_u16_rgba(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 8)?; // 8 bytes per u16 RGBA pixel
        let cfg = self.build_config();
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

    fn do_encode_rgb8(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 3)?;
        let cfg = self.build_config();
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgb: &[Rgb<u8>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgb, w, h);
        let result = crate::encode_rgb8(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgba8(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 4)?;
        let cfg = self.build_config();
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgba: &[Rgba<u8>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgba, w, h);
        let result = crate::encode_rgba8(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_gray8(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 1)?;
        let cfg = self.build_config();
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        // Gray → RGB for encoding (AVIF encoder expects color planes)
        let rgb: Vec<Rgb<u8>> = raw.iter().map(|&g| Rgb { r: g, g, b: g }).collect();
        let img = imgref::ImgVec::new(rgb, w, h);
        let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgb_f32(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        use linear_srgb::default::linear_to_srgb_u8;
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 12)?;
        let cfg = self.build_config();
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

    fn do_encode_rgba_f32(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        use linear_srgb::default::linear_to_srgb_u8;
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 16)?;
        let cfg = self.build_config();
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

    fn do_encode_gray_f32(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        use linear_srgb::default::linear_to_srgb_u8;
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 4)?;
        let cfg = self.build_config();
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

    fn do_encode_rgb16(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 6)?;
        let cfg = self.build_config();
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgb: &[Rgb<u16>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgb, w, h);
        let result = crate::encode_rgb16(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgba16(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 8)?;
        let cfg = self.build_config();
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
    type Error = At<Error>;

    fn reject(op: zencodec::UnsupportedOperation) -> At<Error> {
        at!(Error::UnsupportedOperation(op))
    }

    fn encode_srgba8(
        self,
        data: &mut [u8],
        make_opaque: bool,
        width: u32,
        height: u32,
        stride_pixels: u32,
    ) -> Result<EncodeOutput, At<Error>> {
        let w = width as usize;
        let h = height as usize;
        let stride = stride_pixels as usize;
        self.check_limits(w, h, 4)?;
        let cfg = self.build_config();
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

    fn encode(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
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
                self.check_limits(w, h, 4)?;
                let cfg = self.build_config();
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
                self.check_limits(w, h, 3)?;
                let cfg = self.build_config();
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
                self.check_limits(w, h, 3)?;
                let cfg = self.build_config();
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
    type Error = At<Error>;

    fn reject(op: zencodec::UnsupportedOperation) -> At<Error> {
        at!(Error::UnsupportedOperation(op))
    }

    fn push_frame(
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
                return Err(at!(Error::Encode(format!(
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
            .map_err(|e| at!(Error::Encode(format!("{e}"))))?;

        // Enforce max_frames limit.
        self.frame_count += 1;
        self.limits
            .check_frames(self.frame_count)
            .map_err(|e| at!(Error::Encode(format!("{e}"))))?;

        let fmt = desc.pixel_format();

        // Validate consistent pixel format across frames
        if let Some(expected) = self.pixel_format {
            if fmt != expected {
                return Err(at!(Error::Encode(format!(
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

    fn finish(self, stop: Option<&dyn Stop>) -> Result<EncodeOutput, At<Error>> {
        if let Some(s) = stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }
        if let Some(ref s) = self.stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }

        if self.frames.is_empty() {
            return Err(at!(Error::Encode("no frames to encode".into())));
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
            .map_err(|e| at!(Error::Encode(format!("{e}"))))?;

        Ok(EncodeOutput::new(avif_file, ImageFormat::Avif))
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
    pub fn decode(&self, data: &[u8]) -> Result<DecodeOutput, At<Error>> {
        use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
        self.clone()
            .job()
            .decoder(Cow::Borrowed(data), &[])?
            .decode()
    }

    /// Convenience: probe image header with this config.
    pub fn probe_header(&self, data: &[u8]) -> Result<ImageInfo, At<Error>> {
        use zencodec::decode::{DecodeJob as _, DecoderConfig as _};
        self.clone().job().probe(data)
    }

    /// Convenience: probe full image metadata (may be expensive).
    pub fn probe_full(&self, data: &[u8]) -> Result<ImageInfo, At<Error>> {
        use zencodec::decode::{DecodeJob as _, DecoderConfig as _};
        self.clone().job().probe_full(data)
    }

    /// Convenience: decode into a pre-allocated RGB8 buffer.
    pub fn decode_into_rgb8(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgb<u8>>,
    ) -> Result<ImageInfo, At<Error>> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgb8();
        let src_ref = src.as_imgref();
        let w = dst.width().min(src_ref.width());
        let h = dst.height().min(src_ref.height());
        for y in 0..h {
            let src_row = src_ref.rows().nth(y).ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Source row index out of bounds",
                })
            })?;
            let dst_row = &mut dst.rows_mut().nth(y).ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Destination row index out of bounds",
                })
            })?[..w];
            dst_row.copy_from_slice(&src_row[..w]);
        }
        Ok(info)
    }

    /// Convenience: decode into a pre-allocated RGBA8 buffer.
    pub fn decode_into_rgba8(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgba<u8>>,
    ) -> Result<ImageInfo, At<Error>> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgba8();
        let src_ref = src.as_imgref();
        let w = dst.width().min(src_ref.width());
        let h = dst.height().min(src_ref.height());
        for y in 0..h {
            let src_row = src_ref.rows().nth(y).ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Source row index out of bounds",
                })
            })?;
            let dst_row = &mut dst.rows_mut().nth(y).ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Destination row index out of bounds",
                })
            })?[..w];
            dst_row.copy_from_slice(&src_row[..w]);
        }
        Ok(info)
    }

    /// Convenience: decode into a pre-allocated RGB f32 buffer.
    pub fn decode_into_rgb_f32(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgb<f32>>,
    ) -> Result<ImageInfo, At<Error>> {
        use linear_srgb::default::srgb_u8_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgb8();
        let src_ref = src.as_imgref();
        let w = dst.width().min(src_ref.width());
        let h = dst.height().min(src_ref.height());
        for y in 0..h {
            let src_row = src_ref.rows().nth(y).ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Source row index out of bounds",
                })
            })?;
            let dst_row = &mut dst.rows_mut().nth(y).ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Destination row index out of bounds",
                })
            })?[..w];
            for (i, px) in src_row[..w].iter().enumerate() {
                dst_row[i] = Rgb {
                    r: srgb_u8_to_linear(px.r),
                    g: srgb_u8_to_linear(px.g),
                    b: srgb_u8_to_linear(px.b),
                };
            }
        }
        Ok(info)
    }

    /// Convenience: decode into a pre-allocated RGBA f32 buffer.
    pub fn decode_into_rgba_f32(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgba<f32>>,
    ) -> Result<ImageInfo, At<Error>> {
        use linear_srgb::default::srgb_u8_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgba8();
        let src_ref = src.as_imgref();
        let w = dst.width().min(src_ref.width());
        let h = dst.height().min(src_ref.height());
        for y in 0..h {
            let src_row = src_ref.rows().nth(y).ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Source row index out of bounds",
                })
            })?;
            let dst_row = &mut dst.rows_mut().nth(y).ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Destination row index out of bounds",
                })
            })?[..w];
            for (i, px) in src_row[..w].iter().enumerate() {
                dst_row[i] = Rgba {
                    r: srgb_u8_to_linear(px.r),
                    g: srgb_u8_to_linear(px.g),
                    b: srgb_u8_to_linear(px.b),
                    a: px.a as f32 / 255.0,
                };
            }
        }
        Ok(info)
    }

    /// Convenience: decode into a pre-allocated Gray f32 buffer.
    pub fn decode_into_gray_f32(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, rgb::Gray<f32>>,
    ) -> Result<ImageInfo, At<Error>> {
        use linear_srgb::default::srgb_u8_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgb8();
        let src_ref = src.as_imgref();
        let w = dst.width().min(src_ref.width());
        let h = dst.height().min(src_ref.height());
        // BT.709 luma coefficients in linear light
        let (kr, kb) =
            crate::yuv_convert::matrix_coefficients(crate::yuv_convert::YuvMatrix::Bt709);
        let kg = 1.0 - kr - kb;
        for y in 0..h {
            let src_row = src_ref.rows().nth(y).ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Source row index out of bounds",
                })
            })?;
            let dst_row = &mut dst.rows_mut().nth(y).ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Destination row index out of bounds",
                })
            })?[..w];
            for (i, px) in src_row[..w].iter().enumerate() {
                let r = srgb_u8_to_linear(px.r);
                let g = srgb_u8_to_linear(px.g);
                let b = srgb_u8_to_linear(px.b);
                let luma = kr * r + kg * g + kb * b;
                dst_row[i] = rgb::Gray(luma);
            }
        }
        Ok(info)
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
        .with_native_gray(false)
        .with_native_16bit(true)
        .with_native_alpha(true)
        .with_enforces_max_pixels(true)
        .with_enforces_max_memory(true)
        .with_enforces_max_input_bytes(true)
        .with_threads_supported_range(1, 256);

impl zencodec::decode::DecoderConfig for AvifDecoderConfig {
    type Error = At<Error>;
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
    /// Gain-map rendition intent (zencodec 0.1.21). `Components` (and
    /// `ReconstructHdr`, which zenavif downgrades to surfacing — see
    /// `with_gain_map_render`) additionally decodes the gain-map AV1 payload
    /// into a [`zencodec::decode::DecodedGainMap`]. Default `BaseOnly`.
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
    type Error = At<Error>;
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

    /// zenavif surfaces gain maps but does not apply them
    /// (`DecodeCapabilities::reconstructs_hdr()` is `false`): per the
    /// [`GainMapRender::ReconstructHdr`] contract, a decoder without
    /// reconstruction surfaces [`GainMapRender::Components`] instead — the
    /// SDR base stays SDR-labeled and the caller applies the gain map one
    /// layer up (ultrahdr-core).
    fn with_gain_map_render(mut self, render: zencodec::GainMapRender) -> Self {
        self.gain_map_render = render;
        self
    }

    fn with_orientation(mut self, hint: zencodec::OrientationHint) -> Self {
        self.orientation = hint;
        self.config.orientation = hint;
        self
    }

    fn probe(&self, data: &[u8]) -> Result<ImageInfo, At<Error>> {
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

    fn output_info(&self, data: &[u8]) -> Result<zencodec::decode::OutputInfo, At<Error>> {
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

    fn push_decoder(
        self,
        data: Cow<'a, [u8]>,
        sink: &mut dyn zencodec::decode::DecodeRowSink,
        preferred: &[PixelDescriptor],
    ) -> Result<zencodec::decode::OutputInfo, At<Error>> {
        // Bake path: orientation isn't row-local, so a true row-streamed sink
        // would emit pixels in stored orientation. Route through a full decode
        // (which bakes the buffer upright and reports display dims) and copy the
        // baked rows to the sink. `Preserve` (default) keeps the native, low-
        // memory streaming sink unchanged.
        if will_auto_orient(self.orientation) {
            return zencodec::helpers::copy_decode_to_sink(self, data, sink, preferred, |e| {
                at!(Error::Encode(e.to_string()))
            });
        }

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

    fn decoder(
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

    fn streaming_decoder(
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

        // Bake path: orientation is not strip-local (transposes need the whole
        // image), so decode + bake the full buffer once and emit it in
        // fixed-height strips. Mirrors `Decode::decode`'s buffer pipeline. The
        // preserve path (default) falls through to the low-memory grid / strip-
        // converter streaming below, unchanged.
        if will_auto_orient(self.orientation) {
            let (pixels, native_info) = decoder.decode_full(&stop_token)?;
            let pixels = set_cicp_on_pixels(pixels, &native_info);
            let pixels = negotiate_format(pixels, preferred);
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
                baked: Some(baked),
            });
        }

        let info = convert_native_info(&native_info);

        if decoder.is_grid() {
            let grid = decoder
                .grid_config()
                .ok_or_else(|| at!(Error::Unsupported("grid_config missing after is_grid()")))?;
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
                baked: None,
            });
        }

        // Non-grid: decode YUV, set up strip converter for on-demand conversion.
        let (converter, _native) = decoder.decode_to_strip_converter(&stop_token)?;
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
            baked: None,
        })
    }

    fn animation_frame_decoder(
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
fn negotiate_format(pixels: PixelBuffer, preferred: &[PixelDescriptor]) -> PixelBuffer {
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
    type Error = At<Error>;

    fn decode(self) -> Result<DecodeOutput, At<Error>> {
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

        let (pixels, native_info) = decoder.decode_full(stop)?;

        // Set transfer function and primaries from CICP on the pixel descriptor.
        let pixels = set_cicp_on_pixels(pixels, &native_info);
        let pixels = negotiate_format(pixels, &self.preferred);
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
        let mut output = DecodeOutput::new(pixels, info);
        if let Ok(probe) = crate::detect::probe(&self.data) {
            output = output.with_source_encoding_details(probe);
        }
        // Gain-map rendition intent. zenavif surfaces (it does not apply):
        // Components decodes the gain-map AV1 payload into a DecodedGainMap;
        // ReconstructHdr downgrades to Components per the zencodec contract
        // (reconstructs_hdr() is false — the base stays honestly SDR-labeled
        // and the caller applies via ultrahdr-core). Unknown future modes are
        // refused, never mis-rendered.
        let surface_components = match self.gain_map_render {
            zencodec::GainMapRender::BaseOnly => false,
            zencodec::GainMapRender::Components
            | zencodec::GainMapRender::ReconstructHdr { .. } => true,
            _ => {
                return Err(at!(Error::Unsupported("unrecognized GainMapRender mode")));
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
                let (px, gw, gh, channels) = crate::decode_av1_obu(&gm.gain_map_data)?;
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
                    if actual_w == 0 {
                        continue;
                    }
                    let tile_slice = tile.as_slice();
                    let src = tile_slice.row(py);
                    let copy_bytes = actual_w * bpp;
                    let dst_start = x_offset * bpp;
                    dst_row[dst_start..dst_start + copy_bytes].copy_from_slice(&src[..copy_bytes]);
                    x_offset += tile_w;
                }
            }
        }
        self.strip_buffer = Some(strip);
    }
}

impl zencodec::decode::StreamingDecode for AvifStreamingDecoder {
    type Error = At<Error>;

    fn next_batch(&mut self) -> Result<Option<(u32, PixelSlice<'_>)>, At<Error>> {
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
                .map_err(|e| e.decompose().0)?;

            let y = self.y_offset;
            self.y_offset += h;
            let slice = self.strip_buffer.as_ref().unwrap().as_slice().erase();
            return Ok(Some((y, slice)));
        }

        Ok(None)
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
    type Error = At<Error>;

    fn wrap_sink_error(err: zencodec::decode::SinkError) -> Self::Error {
        at!(Error::ResourceLimit(err.to_string()))
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
    ) -> Result<Option<AnimationFrame<'_>>, At<Error>> {
        let stop: &dyn zencodec::enough::Stop = stop.unwrap_or(&enough::Unstoppable);
        loop {
            let frame = self
                .anim_decoder
                .next_frame(stop)
                .map_err(|e| e.decompose().0)?;
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

            let pixels = negotiate_format(frame.pixels, &self.preferred);
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

    fn render_next_frame_to_sink(
        &mut self,
        stop: Option<&dyn zencodec::enough::Stop>,
        sink: &mut dyn zencodec::decode::DecodeRowSink,
    ) -> Result<Option<zencodec::decode::OutputInfo>, Self::Error> {
        zencodec::helpers::copy_frame_to_sink(self, stop, sink)
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
        assert!(
            matches!(err.error(), Error::ResourceLimit(_)),
            "expected Error::ResourceLimit, got: {}",
            err
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
        assert!(
            matches!(err.error(), Error::ResourceLimit(_)),
            "expected Error::ResourceLimit, got: {}",
            err
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

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../codec-corpus/avif-conformance/valid/2.avif");
        if !path.exists() {
            eprintln!("skipping: corpus file not found at {}", path.display());
            return;
        }
        let data = std::fs::read(&path).unwrap();

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
}
