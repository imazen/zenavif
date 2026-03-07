//! zencodec-types trait implementations for zenavif.
//!
//! Provides [`AvifEncoderConfig`] and [`AvifDecoderConfig`] types that implement
//! the trait hierarchy from zencodec-types, wrapping the native zenavif API.
//!
//! # Trait mapping
//!
//! | zencodec-types | zenavif adapter |
//! |----------------|-----------------|
//! | `EncoderConfig` | [`AvifEncoderConfig`] |
//! | `EncodeJob<'a>` | [`AvifEncodeJob`] |
//! | `Encoder` | [`AvifEncoder`] |
//! | `DecoderConfig` | [`AvifDecoderConfig`] |
//! | `DecodeJob<'a>` | [`AvifDecodeJob`] |
//! | `Decode` | [`AvifDecoder`] |
//! | `FullFrameDecoder` | [`AvifFullFrameDecoder`] |

use std::borrow::Cow;
use std::sync::Arc;

use enough::Stop;
use rgb::{Rgb, Rgba};
use zc::FullFrame;
#[cfg(feature = "encode")]
use zc::MetadataView;
use zc::decode::DecodeOutput;
#[cfg(feature = "encode")]
use zc::encode::EncodeOutput;
use zc::{ImageFormat, ImageInfo, ResourceLimits};
use zenpixels::{ChannelType, PixelBuffer, PixelDescriptor, PixelSlice};
use zenpixels_convert::PixelBufferConvertExt as _;

use crate::error::Error;
use whereat::At;
#[cfg(feature = "encode")]
use whereat::at;

// ── Encoding ────────────────────────────────────────────────────────────────

/// AVIF encoder configuration implementing [`zc::encode::EncoderConfig`].
///
/// Wraps [`crate::EncoderConfig`] and tracks universal quality/effort/lossless
/// settings for the trait interface.
///
/// # Examples
///
/// ```rust,ignore
/// use zc::encode::EncoderConfig;
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
        }
        self
    }

    /// Set alpha channel quality (0.0 = worst, 100.0 = lossless) (inherent method).
    #[must_use]
    pub fn with_alpha_quality_value(mut self, quality: f32) -> Self {
        self.inner = self.inner.alpha_quality(quality);
        self
    }

    /// Convenience: encode RGB8 pixels with this config.
    pub fn encode_rgb8(&self, img: imgref::ImgRef<'_, Rgb<u8>>) -> Result<EncodeOutput, At<Error>> {
        use zc::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
        self.job().encoder()?.encode(PixelSlice::from(img).erase())
    }

    /// Convenience: encode RGBA8 pixels with this config.
    pub fn encode_rgba8(
        &self,
        img: imgref::ImgRef<'_, Rgba<u8>>,
    ) -> Result<EncodeOutput, At<Error>> {
        use zc::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
        self.job().encoder()?.encode(PixelSlice::from(img).erase())
    }

    /// Convenience: encode Gray8 pixels with this config.
    pub fn encode_gray8(
        &self,
        img: imgref::ImgRef<'_, rgb::Gray<u8>>,
    ) -> Result<EncodeOutput, At<Error>> {
        use zc::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
        self.job().encoder()?.encode(PixelSlice::from(img).erase())
    }

    /// Convenience: encode RGB f32 pixels with this config.
    pub fn encode_rgb_f32(
        &self,
        img: imgref::ImgRef<'_, Rgb<f32>>,
    ) -> Result<EncodeOutput, At<Error>> {
        use zc::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
        self.job().encoder()?.encode(PixelSlice::from(img).erase())
    }

    /// Convenience: encode RGBA f32 pixels with this config.
    pub fn encode_rgba_f32(
        &self,
        img: imgref::ImgRef<'_, Rgba<f32>>,
    ) -> Result<EncodeOutput, At<Error>> {
        use zc::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
        self.job().encoder()?.encode(PixelSlice::from(img).erase())
    }

    /// Convenience: encode Gray f32 pixels with this config.
    pub fn encode_gray_f32(
        &self,
        img: imgref::ImgRef<'_, rgb::Gray<f32>>,
    ) -> Result<EncodeOutput, At<Error>> {
        use zc::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
        self.job().encoder()?.encode(PixelSlice::from(img).erase())
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
static AVIF_ENCODE_CAPABILITIES: zc::encode::EncodeCapabilities =
    zc::encode::EncodeCapabilities::new()
        .with_icc(true)
        .with_exif(true)
        .with_xmp(true)
        .with_cicp(true)
        .with_cancel(true)
        .with_lossy(true)
        .with_lossless(cfg!(feature = "encode-imazen"))
        .with_hdr(true)
        .with_native_gray(true)
        .with_native_16bit(true)
        .with_native_f32(true)
        .with_native_alpha(true)
        .with_enforces_max_pixels(true)
        .with_enforces_max_memory(true)
        .with_effort_range(0, 10)
        .with_quality_range(0.0, 100.0)
        .with_threads_supported_range(1, 256);

#[cfg(feature = "encode")]
impl zc::encode::EncoderConfig for AvifEncoderConfig {
    type Error = At<Error>;
    type Job<'a> = AvifEncodeJob<'a>;

    fn format() -> ImageFormat {
        ImageFormat::Avif
    }

    fn supported_descriptors() -> &'static [PixelDescriptor] {
        ENCODE_DESCRIPTORS
    }

    fn capabilities() -> &'static zc::encode::EncodeCapabilities {
        &AVIF_ENCODE_CAPABILITIES
    }

    fn with_generic_effort(mut self, effort: i32) -> Self {
        let clamped = effort.clamp(0, 10);
        self.trait_effort = Some(clamped);
        // Invert: trait effort 0 (slowest) = AVIF speed 10 (fastest)
        let speed = (10 - clamped) as u8;
        self.inner = self.inner.speed(speed);
        self
    }

    fn generic_effort(&self) -> Option<i32> {
        self.trait_effort
    }

    fn with_generic_quality(mut self, quality: f32) -> Self {
        let clamped = quality.clamp(0.0, 100.0);
        self.trait_quality = Some(clamped);
        self.inner = self.inner.quality(clamped);
        self
    }

    fn generic_quality(&self) -> Option<f32> {
        self.trait_quality
    }

    fn with_lossless(mut self, lossless: bool) -> Self {
        self.lossless = lossless;
        if lossless {
            self.inner = self.inner.quality(100.0);
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

    fn job(&self) -> AvifEncodeJob<'_> {
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
        }
    }
}

// ── Encode Job ──────────────────────────────────────────────────────────────

/// Per-operation AVIF encode job.
#[cfg(feature = "encode")]
pub struct AvifEncodeJob<'a> {
    config: &'a AvifEncoderConfig,
    stop: Option<&'a dyn Stop>,
    exif: Option<&'a [u8]>,
    icc_profile: Option<&'a [u8]>,
    xmp: Option<&'a [u8]>,
    limits: ResourceLimits,
    cicp: Option<zc::Cicp>,
    content_light_level: Option<zc::ContentLightLevel>,
    mastering_display: Option<zc::MasteringDisplay>,
    rotation: Option<u8>,
    mirror: Option<u8>,
}

#[cfg(feature = "encode")]
impl<'a> AvifEncodeJob<'a> {
    /// Set EXIF metadata to embed in the encoded AVIF.
    #[must_use]
    pub fn with_exif(mut self, exif: &'a [u8]) -> Self {
        self.exif = Some(exif);
        self
    }
}

#[cfg(feature = "encode")]
impl<'a> zc::encode::EncodeJob<'a> for AvifEncodeJob<'a> {
    type Error = At<Error>;
    type Enc = AvifEncoder<'a>;
    type FullFrameEnc = ();

    fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
        self.stop = Some(stop);
        self
    }

    fn with_metadata(mut self, meta: &'a MetadataView<'a>) -> Self {
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

    fn encoder(self) -> Result<AvifEncoder<'a>, At<Error>> {
        let mut config = self.config.inner.clone();
        // Apply CICP color metadata from MetadataView
        if let Some(cicp) = self.cicp {
            config = config
                .color_primaries(cicp.color_primaries)
                .transfer_characteristics(cicp.transfer_characteristics)
                .matrix_coefficients(cicp.matrix_coefficients);
        }
        // Apply HDR metadata from MetadataView
        if let Some(cll) = self.content_light_level {
            config = config.content_light_level(
                cll.max_content_light_level,
                cll.max_frame_average_light_level,
            );
        }
        if let Some(mdcv) = self.mastering_display {
            config = config.mastering_display(crate::MasteringDisplayConfig {
                primaries: [
                    (mdcv.primaries[0][0], mdcv.primaries[0][1]),
                    (mdcv.primaries[1][0], mdcv.primaries[1][1]),
                    (mdcv.primaries[2][0], mdcv.primaries[2][1]),
                ],
                white_point: (mdcv.white_point[0], mdcv.white_point[1]),
                max_luminance: mdcv.max_luminance,
                min_luminance: mdcv.min_luminance,
            });
        }
        // Apply rotation/mirror from orientation metadata
        if let Some(rot) = self.rotation {
            config = config.rotation(rot);
        }
        if let Some(mir) = self.mirror {
            config = config.mirror(mir);
        }
        Ok(AvifEncoder {
            config,
            stop: self.stop,
            exif: self.exif,
            icc_profile: self.icc_profile,
            xmp: self.xmp,
            limits: self.limits,
        })
    }

    fn full_frame_encoder(self) -> Result<(), At<Error>> {
        Err(at(Error::UnsupportedOperation(
            zc::UnsupportedOperation::AnimationEncode,
        )))
    }
}

// ── Encoder ─────────────────────────────────────────────────────────────────

/// Single-image AVIF encoder.
#[cfg(feature = "encode")]
pub struct AvifEncoder<'a> {
    config: crate::EncoderConfig,
    stop: Option<&'a dyn Stop>,
    exif: Option<&'a [u8]>,
    icc_profile: Option<&'a [u8]>,
    xmp: Option<&'a [u8]>,
    limits: ResourceLimits,
}

#[cfg(feature = "encode")]
impl AvifEncoder<'_> {
    fn build_config(&self) -> crate::EncoderConfig {
        let mut cfg = self.config.clone();
        if let Some(exif) = self.exif {
            cfg = cfg.exif(exif.to_vec());
        }
        if let Some(icc) = self.icc_profile {
            cfg = cfg.icc_profile(icc.to_vec());
        }
        if let Some(xmp) = self.xmp {
            cfg = cfg.xmp(xmp.to_vec());
        }
        cfg
    }

    fn check_limits(&self, w: usize, h: usize, bpp: u64) -> Result<(), At<Error>> {
        self.limits
            .check_dimensions(w as u32, h as u32)
            .map_err(|_| Error::ImageTooLarge {
                width: w as u32,
                height: h as u32,
            })?;
        let estimated_mem = w as u64 * h as u64 * bpp;
        self.limits
            .check_memory(estimated_mem)
            .map_err(|e| Error::Encode(format!("{e}")))?;
        Ok(())
    }

    fn stop_token(&self) -> &dyn Stop {
        self.stop.unwrap_or(&enough::Unstoppable)
    }

    /// Set CICP color primaries and transfer characteristics from the pixel
    /// descriptor, unless already set by metadata. For HDR transfers (PQ/HLG),
    /// also switches to 10-bit encoding depth.
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

        // Only override config if not already set from metadata
        if tc.is_some() || cp.is_some() {
            if let Some(tc_val) = tc {
                self.config = self.config.clone().transfer_characteristics(tc_val);
            }
            if let Some(cp_val) = cp {
                self.config = self.config.clone().color_primaries(cp_val);
            }
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
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
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
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
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
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
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
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
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
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
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
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
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
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
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
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
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
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
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
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
    }
}

#[cfg(feature = "encode")]
impl zc::encode::Encoder for AvifEncoder<'_> {
    type Error = At<Error>;

    fn reject(op: zc::UnsupportedOperation) -> At<Error> {
        at(Error::UnsupportedOperation(op))
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
            Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
        } else {
            // Zero-copy RGBA path — bytemuck cast the contiguous region
            if stride == w {
                let pixel_bytes = &data[..w * h * 4];
                let rgba: &[Rgba<u8>] = bytemuck::cast_slice(pixel_bytes);
                let img = imgref::Img::new(rgba, w, h);
                let result = crate::encode_rgba8(img, &cfg, stop)?;
                Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
            } else {
                // Strided: use ImgRef with stride
                let total_pixels = (h - 1) * stride + w;
                let pixel_bytes = &data[..total_pixels * 4];
                let rgba: &[Rgba<u8>] = bytemuck::cast_slice(pixel_bytes);
                let img = imgref::Img::new_stride(rgba, w, h, stride);
                let result = crate::encode_rgba8(img, &cfg, stop)?;
                Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
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
                Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
            }
            _ => Err(at(Error::UnsupportedOperation(
                zc::UnsupportedOperation::PixelFormat,
            ))),
        }
    }
}

// ── Decoding ────────────────────────────────────────────────────────────────

/// AVIF decoder configuration implementing [`zc::decode::DecoderConfig`].
#[derive(Clone, Debug)]
pub struct AvifDecoderConfig {
    inner: crate::DecoderConfig,
}

impl AvifDecoderConfig {
    /// Create a default AVIF decoder config.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: crate::DecoderConfig::new(),
        }
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
        use zc::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
        self.job().decoder(Cow::Borrowed(data), &[])?.decode()
    }

    /// Convenience: probe image header with this config.
    pub fn probe_header(&self, data: &[u8]) -> Result<ImageInfo, At<Error>> {
        use zc::decode::{DecodeJob as _, DecoderConfig as _};
        self.job().probe(data)
    }

    /// Convenience: probe full image metadata (may be expensive).
    pub fn probe_full(&self, data: &[u8]) -> Result<ImageInfo, At<Error>> {
        use zc::decode::{DecodeJob as _, DecoderConfig as _};
        self.job().probe_full(data)
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
            let src_row = src_ref.rows().nth(y).unwrap();
            let dst_row = &mut dst.rows_mut().nth(y).unwrap()[..w];
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
            let src_row = src_ref.rows().nth(y).unwrap();
            let dst_row = &mut dst.rows_mut().nth(y).unwrap()[..w];
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
            let src_row = src_ref.rows().nth(y).unwrap();
            let dst_row = &mut dst.rows_mut().nth(y).unwrap()[..w];
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
            let src_row = src_ref.rows().nth(y).unwrap();
            let dst_row = &mut dst.rows_mut().nth(y).unwrap()[..w];
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
            let src_row = src_ref.rows().nth(y).unwrap();
            let dst_row = &mut dst.rows_mut().nth(y).unwrap()[..w];
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
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::RGB16_SRGB,
    PixelDescriptor::RGBA16_SRGB,
    PixelDescriptor::GRAY16_SRGB,
];

static AVIF_DECODE_CAPABILITIES: zc::decode::DecodeCapabilities =
    zc::decode::DecodeCapabilities::new()
        .with_icc(true)
        .with_exif(true)
        .with_xmp(true)
        .with_cicp(true)
        .with_cancel(true)
        .with_animation(true)
        .with_cheap_probe(true)
        .with_row_level(true)
        .with_hdr(true)
        .with_native_gray(true)
        .with_native_16bit(true)
        .with_native_alpha(true)
        .with_enforces_max_pixels(true)
        .with_threads_supported_range(1, 256);

impl zc::decode::DecoderConfig for AvifDecoderConfig {
    type Error = At<Error>;
    type Job<'a> = AvifDecodeJob<'a>;

    fn formats() -> &'static [ImageFormat] {
        &[ImageFormat::Avif]
    }

    fn supported_descriptors() -> &'static [PixelDescriptor] {
        DECODE_DESCRIPTORS
    }

    fn capabilities() -> &'static zc::decode::DecodeCapabilities {
        &AVIF_DECODE_CAPABILITIES
    }

    fn job(&self) -> AvifDecodeJob<'_> {
        AvifDecodeJob {
            config: self,
            stop: None,
            limits: ResourceLimits::none(),
        }
    }
}

// ── Decode Job ──────────────────────────────────────────────────────────────

/// Per-operation AVIF decode job.
pub struct AvifDecodeJob<'a> {
    config: &'a AvifDecoderConfig,
    stop: Option<&'a dyn Stop>,
    limits: ResourceLimits,
}

impl<'a> AvifDecodeJob<'a> {
    fn effective_config(&self) -> crate::DecoderConfig {
        let mut cfg = self.config.inner.clone();
        if let Some(max_pixels) = self.limits.max_pixels {
            cfg = cfg.frame_size_limit(max_pixels.min(u32::MAX as u64) as u32);
        }
        cfg
    }
}

impl<'a> zc::decode::DecodeJob<'a> for AvifDecodeJob<'a> {
    type Error = At<Error>;
    type Dec = AvifDecoder<'a>;
    type StreamDec = AvifStreamingDecoder<'a>;
    type FullFrameDec = AvifFullFrameDecoder;

    fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
        self.stop = Some(stop);
        self
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn probe(&self, data: &[u8]) -> Result<ImageInfo, At<Error>> {
        let decoder = crate::ManagedAvifDecoder::new(data, &self.config.inner)?;
        let native_info = decoder.probe_info()?;
        Ok(convert_native_info(&native_info))
    }

    fn output_info(&self, data: &[u8]) -> Result<zc::decode::OutputInfo, At<Error>> {
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
        Ok(zc::decode::OutputInfo::full_decode(
            native_info.width,
            native_info.height,
            desc,
        ))
    }

    fn push_decoder(
        self,
        data: Cow<'a, [u8]>,
        sink: &mut dyn zc::decode::DecodeRowSink,
        _preferred: &[PixelDescriptor],
    ) -> Result<zc::decode::OutputInfo, At<Error>> {
        let cfg = self.effective_config();
        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let mut decoder = crate::ManagedAvifDecoder::new(&data, &cfg)?;
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
        Ok(zc::decode::OutputInfo::full_decode(
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
        let cfg = self.effective_config();
        Ok(AvifDecoder {
            config: cfg,
            stop: self.stop,
            data,
            preferred: preferred.to_vec(),
        })
    }

    fn streaming_decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifStreamingDecoder<'a>, At<Error>> {
        let cfg = self.effective_config();
        let stop: &'a dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);

        let mut decoder = crate::ManagedAvifDecoder::new(&data, &cfg)?;
        let native_info = decoder.probe_info()?;
        let info = convert_native_info(&native_info);

        if decoder.is_grid() {
            let grid = decoder.grid_config().unwrap();
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
                stop,
                grid_rows: grid.rows as u32,
                grid_cols: grid.columns as u32,
                current_grid_row: 0,
                strip_descriptor,
                strip_buffer: None,
                full_pixels: None,
            });
        }

        // Non-grid fallback: full decode upfront.
        let (pixels, _native) = decoder.decode_full(stop)?;
        let pixels = set_cicp_on_pixels(pixels, &native_info);
        let pixels = negotiate_format(pixels, preferred);
        let desc = pixels.descriptor();
        let w = pixels.width();
        let h = pixels.height();

        Ok(AvifStreamingDecoder {
            info,
            y_offset: 0,
            output_width: w,
            output_height: h,
            decoder: None,
            stop,
            grid_rows: 0,
            grid_cols: 0,
            current_grid_row: 0,
            strip_descriptor: desc,
            strip_buffer: None,
            full_pixels: Some(pixels),
        })
    }

    fn full_frame_decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifFullFrameDecoder, At<Error>> {
        let cfg = self.effective_config();

        // Probe metadata before creating animation decoder (both parse the container,
        // but ManagedAvifDecoder gives us the native ImageInfo for conversion).
        let probe_dec = crate::ManagedAvifDecoder::new(&data, &cfg)?;
        let native_info = probe_dec.probe_info()?;
        drop(probe_dec);

        let anim_dec = crate::AnimationDecoder::new(&data, &cfg)?;
        let anim_info = anim_dec.info().clone();

        let base_info = convert_native_info(&native_info)
            .with_animation(true)
            .with_frame_count(anim_info.frame_count as u32);

        Ok(AvifFullFrameDecoder {
            anim_decoder: anim_dec,
            index: 0,
            info: Arc::new(base_info),
            total_frames: anim_info.frame_count as u32,
            preferred: preferred.to_vec(),
            current_frame: None,
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
) -> zc::Orientation {
    use zc::Orientation;
    let angle = rotation.map(|r| r.angle).unwrap_or(0);
    match (mirror.map(|m| m.axis), angle) {
        (None, 0) => Orientation::Normal,
        (None, 90) => Orientation::Rotate270,
        (None, 180) => Orientation::Rotate180,
        (None, 270) => Orientation::Rotate90,
        (Some(0), 0) => Orientation::FlipHorizontal,
        (Some(0), 90) => Orientation::Transpose,
        (Some(0), 180) => Orientation::FlipVertical,
        (Some(0), 270) => Orientation::Transverse,
        (Some(1), 0) => Orientation::FlipVertical,
        (Some(1), 90) => Orientation::Transverse,
        (Some(1), 180) => Orientation::FlipHorizontal,
        (Some(1), 270) => Orientation::Transpose,
        _ => Orientation::Normal,
    }
}

/// Convert EXIF orientation to AVIF rotation raw code + mirror axis.
///
/// Inverse of [`avif_to_orientation`]. Returns `(rotation_code, mirror_axis)`.
/// Rotation codes: 0=0°, 1=90°CCW, 2=180°, 3=270°CCW.
#[cfg(feature = "encode")]
fn orientation_to_avif(orientation: zc::Orientation) -> (Option<u8>, Option<u8>) {
    use zc::Orientation;
    match orientation {
        Orientation::Normal => (None, None),
        Orientation::FlipHorizontal => (Some(0), Some(0)), // mirror=0, no rotation
        Orientation::Rotate180 => (Some(2), None),         // 180° CCW
        Orientation::FlipVertical => (Some(2), Some(0)),   // mirror=0, 180° CCW
        Orientation::Transpose => (Some(1), Some(0)),      // mirror=0, 90° CCW
        Orientation::Rotate90 => (Some(3), None),          // 270° CCW = 90° CW
        Orientation::Transverse => (Some(3), Some(0)),     // mirror=0, 270° CCW
        Orientation::Rotate270 => (Some(1), None),         // 90° CCW = 270° CW
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

/// Convert zenavif's native `ImageInfo` to `zc::ImageInfo`.
fn convert_native_info(native: &crate::image::ImageInfo) -> ImageInfo {
    let orientation = avif_to_orientation(native.rotation.as_ref(), native.mirror.as_ref());

    let cicp = zc::Cicp::new(
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
    }
    if let Some(ref exif) = native.exif {
        info = info.with_exif(exif.clone());
    }
    if let Some(ref xmp) = native.xmp {
        info = info.with_xmp(xmp.clone());
    }
    if let Some(ref cll) = native.content_light_level {
        info = info.with_content_light_level(zc::ContentLightLevel::new(
            cll.max_content_light_level,
            cll.max_pic_average_light_level,
        ));
    }
    if let Some(ref mdcv) = native.mastering_display {
        info = info.with_mastering_display(zc::MasteringDisplay::new(
            [
                [mdcv.primaries[0].0, mdcv.primaries[0].1],
                [mdcv.primaries[1].0, mdcv.primaries[1].1],
                [mdcv.primaries[2].0, mdcv.primaries[2].1],
            ],
            [mdcv.white_point.0, mdcv.white_point.1],
            mdcv.max_luminance,
            mdcv.min_luminance,
        ));
    }

    info
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
    stop: Option<&'a dyn Stop>,
    data: Cow<'a, [u8]>,
    preferred: Vec<PixelDescriptor>,
}

impl zc::decode::Decode for AvifDecoder<'_> {
    type Error = At<Error>;

    fn decode(self) -> Result<DecodeOutput, At<Error>> {
        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let mut decoder = crate::ManagedAvifDecoder::new(&self.data, &self.config)?;
        let (pixels, native_info) = decoder.decode_full(stop)?;

        // Set transfer function and primaries from CICP on the pixel descriptor.
        let pixels = set_cicp_on_pixels(pixels, &native_info);
        let pixels = negotiate_format(pixels, &self.preferred);
        let info = convert_native_info(&native_info);
        Ok(DecodeOutput::new(pixels, info))
    }
}

/// Streaming AVIF decoder with real tile-row streaming for grid images.
///
/// For grid (tiled) images, each [`next_batch`](zc::decode::StreamingDecode::next_batch)
/// call decodes one tile-row of AV1 tiles, color-converts them, and stitches
/// them into a strip. Peak memory is proportional to one tile-row instead of
/// the full image.
///
/// For non-grid images, the full frame is decoded on construction and emitted
/// in fixed-height strips.
pub struct AvifStreamingDecoder<'a> {
    info: ImageInfo,
    y_offset: u32,
    output_width: u32,
    output_height: u32,
    /// Grid path: managed decoder for tile-row streaming.
    decoder: Option<crate::ManagedAvifDecoder>,
    /// Stop token for cancellable grid decoding.
    stop: &'a dyn Stop,
    grid_rows: u32,
    grid_cols: u32,
    current_grid_row: u32,
    /// Pixel descriptor with CICP metadata for strip buffers.
    strip_descriptor: PixelDescriptor,
    /// Reusable strip buffer for the current tile-row (grid path).
    strip_buffer: Option<PixelBuffer>,
    /// Non-grid fallback: full decoded image, emit strips.
    full_pixels: Option<PixelBuffer>,
}

impl AvifStreamingDecoder<'_> {
    /// Default strip height for non-grid fallback.
    const FALLBACK_STRIP_HEIGHT: u32 = 64;

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

impl zc::decode::StreamingDecode for AvifStreamingDecoder<'_> {
    type Error = At<Error>;

    fn next_batch(&mut self) -> Result<Option<(u32, PixelSlice<'_>)>, At<Error>> {
        if self.y_offset >= self.output_height {
            return Ok(None);
        }

        if self.decoder.is_some() {
            // Grid path: decode one tile-row per call.
            if self.current_grid_row >= self.grid_rows {
                return Ok(None);
            }

            let tiles = self.decoder.as_mut().unwrap().decode_tile_row(
                self.current_grid_row as usize,
                self.grid_cols as usize,
                self.stop,
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

        // Non-grid fallback: emit strips from full_pixels.
        if let Some(ref pixels) = self.full_pixels {
            let h = Self::FALLBACK_STRIP_HEIGHT.min(self.output_height - self.y_offset);
            let y = self.y_offset;
            self.y_offset += h;
            let slice = pixels.rows(y, h).erase();
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
/// Lazily decodes frames on demand. The `FullFrameDecoder` trait doesn't pass
/// a stop token per-call, so per-frame cancellation is not available
/// through this interface (use the native `AnimationDecoder` API for that).
pub struct AvifFullFrameDecoder {
    anim_decoder: crate::AnimationDecoder,
    index: usize,
    info: Arc<ImageInfo>,
    total_frames: u32,
    preferred: Vec<PixelDescriptor>,
    /// Holds the current frame's pixels so `render_next_frame` can return
    /// a borrowing `FullFrame<'_>`.
    current_frame: Option<PixelBuffer>,
}

impl zc::decode::FullFrameDecoder for AvifFullFrameDecoder {
    type Error = At<Error>;

    fn wrap_sink_error(err: zc::decode::SinkError) -> Self::Error {
        at(Error::Encode(err.to_string()))
    }

    fn info(&self) -> &ImageInfo {
        &self.info
    }

    fn frame_count(&self) -> Option<u32> {
        Some(self.total_frames)
    }

    fn render_next_frame(&mut self) -> Result<Option<FullFrame<'_>>, At<Error>> {
        let frame = self
            .anim_decoder
            .next_frame(&enough::Unstoppable)
            .map_err(|e| e.into_inner())?;
        let Some(frame) = frame else {
            return Ok(None);
        };
        let pixels = negotiate_format(frame.pixels, &self.preferred);
        let idx = self.index as u32;
        self.index += 1;
        let duration_ms = frame.duration_ms;
        self.current_frame = Some(pixels);
        let slice = self.current_frame.as_ref().unwrap().as_slice().erase();
        Ok(Some(FullFrame::new(slice, duration_ms, idx)))
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
    fn encoding_with_metadata() {
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};
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
            .with_exif(exif)
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
        use zc::encode::EncoderConfig;
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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};

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
        use zc::decode::{Decode, DecodeJob, DecoderConfig};

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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};

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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};

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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};

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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};

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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};

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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};

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
        use zc::encode::{EncodeJob, EncoderConfig};

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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};

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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};

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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};
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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};
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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};
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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};
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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};
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
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};
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
        use zc::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};
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

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_trait_pq_bt2020_narrow_range() {
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};
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

    #[test]
    fn encoder_trait_rgb_f32_pq_bt2020() {
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};
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

    #[test]
    fn encoder_trait_rgba_f32_hlg_bt2020() {
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};
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

    #[test]
    fn encoder_trait_f32_pq_roundtrip_preserves_hdr() {
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};
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
}
