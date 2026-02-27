//! zencodec-types trait implementations for zenavif.
//!
//! Provides [`AvifEncoderConfig`] and [`AvifDecoderConfig`] types that implement
//! the 4-layer trait hierarchy from zencodec-types, wrapping the native zenavif API.
//!
//! # Trait mapping
//!
//! | zencodec-types | zenavif adapter |
//! |----------------|-----------------|
//! | `EncoderConfig` | [`AvifEncoderConfig`] |
//! | `EncodeJob<'a>` | [`AvifEncodeJob`] |
//! | `EncodeRgb8` etc. | [`AvifEncoder`] |
//! | `FrameEncodeRgb8` etc. | [`AvifFrameEncoder`] |
//! | `DecoderConfig` | [`AvifDecoderConfig`] |
//! | `DecodeJob<'a>` | [`AvifDecodeJob`] |
//! | `Decode` | [`AvifDecoder`] |
//! | `FrameDecode` | [`AvifFrameDecoder`] |

use std::sync::Arc;

use rgb::{Rgb, Rgba};
#[cfg(feature = "encode")]
use zencodec_types::EncodeOutput;
#[cfg(feature = "encode")]
use zencodec_types::MetadataView;
#[cfg(feature = "encode")]
use zencodec_types::PixelSlice;
use zencodec_types::{
    ChannelType, DecodeFrame, DecodeOutput, ImageFormat, ImageInfo, PixelData, PixelDescriptor,
    ResourceLimits, Stop,
};

use crate::error::Error;

// ── Encoding ────────────────────────────────────────────────────────────────

/// AVIF encoder configuration implementing [`zencodec_types::EncoderConfig`].
///
/// Wraps [`crate::EncoderConfig`] and tracks universal quality/effort/lossless
/// settings for the trait interface.
///
/// # Examples
///
/// ```rust,ignore
/// use zencodec_types::EncoderConfig;
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
    pub fn encode_rgb8(&self, img: imgref::ImgRef<'_, Rgb<u8>>) -> Result<EncodeOutput, Error> {
        use zencodec_types::{EncodeJob as _, EncodeRgb8 as _, EncoderConfig as _};
        self.job().encoder()?.encode_rgb8(PixelSlice::from(img))
    }

    /// Convenience: encode RGBA8 pixels with this config.
    pub fn encode_rgba8(&self, img: imgref::ImgRef<'_, Rgba<u8>>) -> Result<EncodeOutput, Error> {
        use zencodec_types::{EncodeJob as _, EncodeRgba8 as _, EncoderConfig as _};
        self.job().encoder()?.encode_rgba8(PixelSlice::from(img))
    }

    /// Convenience: encode Gray8 pixels with this config.
    pub fn encode_gray8(
        &self,
        img: imgref::ImgRef<'_, rgb::Gray<u8>>,
    ) -> Result<EncodeOutput, Error> {
        use zencodec_types::{EncodeGray8 as _, EncodeJob as _, EncoderConfig as _};
        self.job().encoder()?.encode_gray8(PixelSlice::from(img))
    }

    /// Convenience: encode RGB f32 pixels with this config.
    pub fn encode_rgb_f32(&self, img: imgref::ImgRef<'_, Rgb<f32>>) -> Result<EncodeOutput, Error> {
        use zencodec_types::{EncodeJob as _, EncodeRgbF32 as _, EncoderConfig as _};
        self.job().encoder()?.encode_rgb_f32(PixelSlice::from(img))
    }

    /// Convenience: encode RGBA f32 pixels with this config.
    pub fn encode_rgba_f32(
        &self,
        img: imgref::ImgRef<'_, Rgba<f32>>,
    ) -> Result<EncodeOutput, Error> {
        use zencodec_types::{EncodeJob as _, EncodeRgbaF32 as _, EncoderConfig as _};
        self.job().encoder()?.encode_rgba_f32(PixelSlice::from(img))
    }

    /// Convenience: encode Gray f32 pixels with this config.
    pub fn encode_gray_f32(
        &self,
        img: imgref::ImgRef<'_, rgb::Gray<f32>>,
    ) -> Result<EncodeOutput, Error> {
        use zencodec_types::{EncodeGrayF32 as _, EncodeJob as _, EncoderConfig as _};
        self.job().encoder()?.encode_gray_f32(PixelSlice::from(img))
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
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::BGRA8_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::GRAYF32_LINEAR,
];

#[cfg(feature = "encode")]
impl zencodec_types::EncoderConfig for AvifEncoderConfig {
    type Error = Error;
    type Job<'a> = AvifEncodeJob<'a>;

    fn format() -> ImageFormat {
        ImageFormat::Avif
    }

    fn supported_descriptors() -> &'static [PixelDescriptor] {
        ENCODE_DESCRIPTORS
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
        // The native config doesn't expose the alpha quality getter directly,
        // so we return None (default no-op behavior).
        None
    }

    fn job(&self) -> AvifEncodeJob<'_> {
        AvifEncodeJob {
            config: self,
            stop: None,
            exif: None,
            icc_profile: None,
            xmp: None,
            limits: ResourceLimits::none(),
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
impl<'a> zencodec_types::EncodeJob<'a> for AvifEncodeJob<'a> {
    type Error = Error;
    type Enc = AvifEncoder<'a>;
    type FrameEnc = AvifFrameEncoder;

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
        self
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn encoder(self) -> Result<AvifEncoder<'a>, Error> {
        Ok(AvifEncoder {
            config: self.config.inner.clone(),
            stop: self.stop,
            exif: self.exif,
            icc_profile: self.icc_profile,
            xmp: self.xmp,
            limits: self.limits,
        })
    }

    fn frame_encoder(self) -> Result<AvifFrameEncoder, Error> {
        Err(Error::Unsupported(
            "AVIF animation encoding not supported via trait interface",
        ))
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

    fn check_limits(&self, w: usize, h: usize, bpp: u64) -> Result<(), Error> {
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
}

#[cfg(feature = "encode")]
impl zencodec_types::EncodeRgb8 for AvifEncoder<'_> {
    type Error = Error;
    fn encode_rgb8(self, pixels: PixelSlice<'_, Rgb<u8>>) -> Result<EncodeOutput, Error> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 3)?;
        let cfg = self.build_config();
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgb: Vec<Rgb<u8>> = raw
            .chunks_exact(3)
            .map(|c| Rgb {
                r: c[0],
                g: c[1],
                b: c[2],
            })
            .collect();
        let img = imgref::ImgVec::new(rgb, w, h);
        let result = crate::encode_rgb8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
    }
}

#[cfg(feature = "encode")]
impl zencodec_types::EncodeRgba8 for AvifEncoder<'_> {
    type Error = Error;
    fn encode_rgba8(self, pixels: PixelSlice<'_, Rgba<u8>>) -> Result<EncodeOutput, Error> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 4)?;
        let cfg = self.build_config();
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgba: Vec<Rgba<u8>> = raw
            .chunks_exact(4)
            .map(|c| Rgba {
                r: c[0],
                g: c[1],
                b: c[2],
                a: c[3],
            })
            .collect();
        let img = imgref::ImgVec::new(rgba, w, h);
        let result = crate::encode_rgba8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
    }
}

#[cfg(feature = "encode")]
impl zencodec_types::EncodeGray8 for AvifEncoder<'_> {
    type Error = Error;
    fn encode_gray8(self, pixels: PixelSlice<'_, rgb::Gray<u8>>) -> Result<EncodeOutput, Error> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        self.check_limits(w, h, 1)?;
        let cfg = self.build_config();
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        // Gray → RGB for encoding (AVIF encoder expects color planes)
        let rgb: Vec<Rgb<u8>> = raw.iter().map(|&g| Rgb { r: g, g, b: g }).collect();
        let img = imgref::ImgVec::new(rgb, w, h);
        let result = crate::encode_rgb8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
    }
}

#[cfg(feature = "encode")]
impl zencodec_types::EncodeRgbF32 for AvifEncoder<'_> {
    type Error = Error;
    fn encode_rgb_f32(self, pixels: PixelSlice<'_, Rgb<f32>>) -> Result<EncodeOutput, Error> {
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
        let result = crate::encode_rgb8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
    }
}

#[cfg(feature = "encode")]
impl zencodec_types::EncodeRgbaF32 for AvifEncoder<'_> {
    type Error = Error;
    fn encode_rgba_f32(self, pixels: PixelSlice<'_, Rgba<f32>>) -> Result<EncodeOutput, Error> {
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
        let result = crate::encode_rgba8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
    }
}

#[cfg(feature = "encode")]
impl zencodec_types::EncodeGrayF32 for AvifEncoder<'_> {
    type Error = Error;
    fn encode_gray_f32(
        self,
        pixels: PixelSlice<'_, rgb::Gray<f32>>,
    ) -> Result<EncodeOutput, Error> {
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
        let result = crate::encode_rgb8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
        Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
    }
}

// ── Frame Encoder (stub) ────────────────────────────────────────────────────

/// Stub frame encoder for AVIF (animation not supported via trait interface).
#[cfg(feature = "encode")]
pub struct AvifFrameEncoder;

#[cfg(feature = "encode")]
impl zencodec_types::FrameEncodeRgb8 for AvifFrameEncoder {
    type Error = Error;

    fn push_frame_rgb8(
        &mut self,
        _pixels: PixelSlice<'_, Rgb<u8>>,
        _duration_ms: u32,
    ) -> Result<(), Error> {
        Err(Error::Unsupported(
            "AVIF animation encoding not supported via trait interface",
        ))
    }

    fn finish_rgb8(self) -> Result<EncodeOutput, Error> {
        Err(Error::Unsupported(
            "AVIF animation encoding not supported via trait interface",
        ))
    }
}

#[cfg(feature = "encode")]
impl zencodec_types::FrameEncodeRgba8 for AvifFrameEncoder {
    type Error = Error;

    fn push_frame_rgba8(
        &mut self,
        _pixels: PixelSlice<'_, Rgba<u8>>,
        _duration_ms: u32,
    ) -> Result<(), Error> {
        Err(Error::Unsupported(
            "AVIF animation encoding not supported via trait interface",
        ))
    }

    fn finish_rgba8(self) -> Result<EncodeOutput, Error> {
        Err(Error::Unsupported(
            "AVIF animation encoding not supported via trait interface",
        ))
    }
}

// ── Decoding ────────────────────────────────────────────────────────────────

/// AVIF decoder configuration implementing [`zencodec_types::DecoderConfig`].
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
    pub fn decode(&self, data: &[u8]) -> Result<DecodeOutput, Error> {
        use zencodec_types::{Decode as _, DecodeJob as _, DecoderConfig as _};
        self.job().decoder()?.decode(data, &[])
    }

    /// Convenience: probe image header with this config.
    pub fn probe_header(&self, data: &[u8]) -> Result<ImageInfo, Error> {
        use zencodec_types::{DecodeJob as _, DecoderConfig as _};
        self.job().probe(data)
    }

    /// Convenience: probe full image metadata (may be expensive).
    pub fn probe_full(&self, data: &[u8]) -> Result<ImageInfo, Error> {
        use zencodec_types::{DecodeJob as _, DecoderConfig as _};
        self.job().probe_full(data)
    }

    /// Convenience: decode into a pre-allocated RGB8 buffer.
    pub fn decode_into_rgb8(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgb<u8>>,
    ) -> Result<ImageInfo, Error> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = to_rgb8(output.into_pixels());
        let w = dst.width().min(src.width());
        let h = dst.height().min(src.height());
        for y in 0..h {
            let src_row = src.as_ref().rows().nth(y).unwrap();
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
    ) -> Result<ImageInfo, Error> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = to_rgba8(output.into_pixels());
        let w = dst.width().min(src.width());
        let h = dst.height().min(src.height());
        for y in 0..h {
            let src_row = src.as_ref().rows().nth(y).unwrap();
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
    ) -> Result<ImageInfo, Error> {
        use linear_srgb::default::srgb_u8_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = to_rgb8(output.into_pixels());
        let w = dst.width().min(src.width());
        let h = dst.height().min(src.height());
        for y in 0..h {
            let src_row = src.as_ref().rows().nth(y).unwrap();
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
    ) -> Result<ImageInfo, Error> {
        use linear_srgb::default::srgb_u8_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = to_rgba8(output.into_pixels());
        let w = dst.width().min(src.width());
        let h = dst.height().min(src.height());
        for y in 0..h {
            let src_row = src.as_ref().rows().nth(y).unwrap();
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
    ) -> Result<ImageInfo, Error> {
        use linear_srgb::default::srgb_u8_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = to_rgb8(output.into_pixels());
        let w = dst.width().min(src.width());
        let h = dst.height().min(src.height());
        for y in 0..h {
            let src_row = src.as_ref().rows().nth(y).unwrap();
            let dst_row = &mut dst.rows_mut().nth(y).unwrap()[..w];
            for (i, px) in src_row[..w].iter().enumerate() {
                let r = srgb_u8_to_linear(px.r);
                let g = srgb_u8_to_linear(px.g);
                let b = srgb_u8_to_linear(px.b);
                let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
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

impl zencodec_types::DecoderConfig for AvifDecoderConfig {
    type Error = Error;
    type Job<'a> = AvifDecodeJob<'a>;

    fn format() -> ImageFormat {
        ImageFormat::Avif
    }

    fn supported_descriptors() -> &'static [PixelDescriptor] {
        DECODE_DESCRIPTORS
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

impl<'a> zencodec_types::DecodeJob<'a> for AvifDecodeJob<'a> {
    type Error = Error;
    type Dec = AvifDecoder<'a>;
    type FrameDec = AvifFrameDecoder;

    fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
        self.stop = Some(stop);
        self
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn probe(&self, data: &[u8]) -> Result<ImageInfo, Error> {
        let decoder =
            crate::ManagedAvifDecoder::new(data, &self.config.inner).map_err(|e| e.into_inner())?;
        let native_info = decoder.probe_info().map_err(|e| e.into_inner())?;
        Ok(convert_native_info(&native_info))
    }

    fn output_info(&self, data: &[u8]) -> Result<zencodec_types::OutputInfo, Error> {
        let decoder =
            crate::ManagedAvifDecoder::new(data, &self.config.inner).map_err(|e| e.into_inner())?;
        let native_info = decoder.probe_info().map_err(|e| e.into_inner())?;
        let desc = if native_info.has_alpha {
            PixelDescriptor::RGBA8_SRGB
        } else {
            PixelDescriptor::RGB8_SRGB
        };
        Ok(zencodec_types::OutputInfo::full_decode(
            native_info.width,
            native_info.height,
            desc,
        ))
    }

    fn decoder(self) -> Result<AvifDecoder<'a>, Error> {
        let cfg = self.effective_config();
        Ok(AvifDecoder {
            config: cfg,
            stop: self.stop,
        })
    }

    fn frame_decoder(self, data: &[u8]) -> Result<AvifFrameDecoder, Error> {
        let cfg = self.effective_config();

        // Probe metadata before creating animation decoder (both parse the container,
        // but ManagedAvifDecoder gives us the native ImageInfo for conversion).
        let probe_dec = crate::ManagedAvifDecoder::new(data, &cfg).map_err(|e| e.into_inner())?;
        let native_info = probe_dec.probe_info().map_err(|e| e.into_inner())?;
        drop(probe_dec);

        let mut anim_dec = crate::AnimationDecoder::new(data, &cfg).map_err(|e| e.into_inner())?;
        let anim_info = anim_dec.info().clone();

        // Eagerly decode all frames using the stop token
        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let mut frames = Vec::new();
        while let Some(frame) = anim_dec.next_frame(stop).map_err(|e| e.into_inner())? {
            frames.push((frame.pixels, frame.duration_ms));
        }

        // Build base info from probed metadata, override dimensions from decoded frame
        let mut base_info = convert_native_info(&native_info)
            .with_animation(true)
            .with_frame_count(anim_info.frame_count as u32);
        if let Some((px, _)) = frames.first() {
            base_info.width = px.width();
            base_info.height = px.height();
        }

        Ok(AvifFrameDecoder {
            frames,
            index: 0,
            info: Arc::new(base_info),
            total_frames: anim_info.frame_count as u32,
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
) -> zencodec_types::Orientation {
    use zencodec_types::Orientation;
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

/// Convert zenavif's native `ImageInfo` to `zencodec_types::ImageInfo`.
fn convert_native_info(native: &crate::image::ImageInfo) -> ImageInfo {
    let orientation = avif_to_orientation(native.rotation.as_ref(), native.mirror.as_ref());

    let cicp = zencodec_types::Cicp::new(
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
        info = info.with_content_light_level(zencodec_types::ContentLightLevel::new(
            cll.max_content_light_level,
            cll.max_pic_average_light_level,
        ));
    }
    if let Some(ref mdcv) = native.mastering_display {
        info = info.with_mastering_display(zencodec_types::MasteringDisplay::new(
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

/// Convert a 16-bit channel value to 8-bit.
fn u16_to_u8(v: u16) -> u8 {
    ((v as u32 * 255 + 32768) / 65535) as u8
}

/// Convert AVIF-native pixel data to RGB8.
///
/// Handles Rgb8, Rgba8 (drop alpha), Rgb16 (downscale), Rgba16 (downscale + drop alpha).
fn to_rgb8(pixels: PixelData) -> imgref::ImgVec<Rgb<u8>> {
    match pixels {
        PixelData::Rgb8(img) => img,
        PixelData::Rgba8(img) => {
            let w = img.width();
            let h = img.height();
            let buf: Vec<Rgb<u8>> = img
                .into_buf()
                .into_iter()
                .map(|p| Rgb {
                    r: p.r,
                    g: p.g,
                    b: p.b,
                })
                .collect();
            imgref::ImgVec::new(buf, w, h)
        }
        PixelData::Rgb16(img) => {
            let w = img.width();
            let h = img.height();
            let buf: Vec<Rgb<u8>> = img
                .into_buf()
                .into_iter()
                .map(|p| Rgb {
                    r: u16_to_u8(p.r),
                    g: u16_to_u8(p.g),
                    b: u16_to_u8(p.b),
                })
                .collect();
            imgref::ImgVec::new(buf, w, h)
        }
        PixelData::Rgba16(img) => {
            let w = img.width();
            let h = img.height();
            let buf: Vec<Rgb<u8>> = img
                .into_buf()
                .into_iter()
                .map(|p| Rgb {
                    r: u16_to_u8(p.r),
                    g: u16_to_u8(p.g),
                    b: u16_to_u8(p.b),
                })
                .collect();
            imgref::ImgVec::new(buf, w, h)
        }
        other => unreachable!("AVIF decoder produced unexpected format: {other:?}"),
    }
}

/// Convert AVIF-native pixel data to RGBA8.
///
/// Handles Rgba8, Rgb8 (add opaque alpha), Rgb16, Rgba16 (downscale).
fn to_rgba8(pixels: PixelData) -> imgref::ImgVec<Rgba<u8>> {
    match pixels {
        PixelData::Rgba8(img) => img,
        PixelData::Rgb8(img) => {
            let w = img.width();
            let h = img.height();
            let buf: Vec<Rgba<u8>> = img
                .into_buf()
                .into_iter()
                .map(|p| Rgba {
                    r: p.r,
                    g: p.g,
                    b: p.b,
                    a: 255,
                })
                .collect();
            imgref::ImgVec::new(buf, w, h)
        }
        PixelData::Rgba16(img) => {
            let w = img.width();
            let h = img.height();
            let buf: Vec<Rgba<u8>> = img
                .into_buf()
                .into_iter()
                .map(|p| Rgba {
                    r: u16_to_u8(p.r),
                    g: u16_to_u8(p.g),
                    b: u16_to_u8(p.b),
                    a: u16_to_u8(p.a),
                })
                .collect();
            imgref::ImgVec::new(buf, w, h)
        }
        PixelData::Rgb16(img) => {
            let w = img.width();
            let h = img.height();
            let buf: Vec<Rgba<u8>> = img
                .into_buf()
                .into_iter()
                .map(|p| Rgba {
                    r: u16_to_u8(p.r),
                    g: u16_to_u8(p.g),
                    b: u16_to_u8(p.b),
                    a: 255,
                })
                .collect();
            imgref::ImgVec::new(buf, w, h)
        }
        other => unreachable!("AVIF decoder produced unexpected format: {other:?}"),
    }
}

/// Apply preferred format negotiation to native decoder output.
///
/// If `preferred` is empty, returns `pixels` unchanged (native format).
/// If `preferred` is non-empty, finds the first descriptor we can satisfy:
/// - Same or lower bit depth: downconvert (caller explicitly asked for it)
/// - Higher bit depth than native: skip (can't upscale losslessly)
fn negotiate_format(pixels: PixelData, preferred: &[PixelDescriptor]) -> PixelData {
    if preferred.is_empty() {
        return pixels;
    }

    let native = pixels.descriptor();

    // If the native format is already in the preferred list, return as-is.
    if preferred.contains(&native) {
        return pixels;
    }

    // Find first preferred descriptor we can produce.
    for pref in preferred {
        // Can't upscale bit depth losslessly.
        if pref.channel_type.byte_size() > native.channel_type.byte_size() {
            continue;
        }

        // If caller wants 8-bit and we have 16-bit, downconvert.
        if pref.channel_type == ChannelType::U8 && native.channel_type == ChannelType::U16 {
            if pref.layout.has_alpha() {
                return PixelData::Rgba8(pixels.to_rgba8());
            }
            return PixelData::Rgb8(pixels.to_rgb8());
        }

        // Same bit depth but different layout (e.g., RGB vs RGBA).
        if pref.channel_type == native.channel_type {
            if pref.layout.has_alpha() && !native.layout.has_alpha() {
                // Adding alpha = not lossless from source perspective, but acceptable
                // (alpha=255 is the convention)
                if native.channel_type == ChannelType::U8 {
                    return PixelData::Rgba8(pixels.to_rgba8());
                }
                // For 16-bit, to_rgba8 loses precision — skip if we can't match
                continue;
            }
            if !pref.layout.has_alpha() && native.layout.has_alpha() {
                // Dropping alpha is lossy, but caller asked for it
                if native.channel_type == ChannelType::U8 {
                    return PixelData::Rgb8(pixels.to_rgb8());
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
}

impl zencodec_types::Decode for AvifDecoder<'_> {
    type Error = Error;

    fn decode(self, data: &[u8], preferred: &[PixelDescriptor]) -> Result<DecodeOutput, Error> {
        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let mut decoder =
            crate::ManagedAvifDecoder::new(data, &self.config).map_err(|e| e.into_inner())?;
        let (pixels, native_info) = decoder.decode_full(stop).map_err(|e| e.into_inner())?;
        let pixels = negotiate_format(pixels, preferred);
        let info = convert_native_info(&native_info);
        Ok(DecodeOutput::new(pixels, info))
    }
}

// ── Frame Decoder ───────────────────────────────────────────────────────────

/// Animation AVIF frame decoder.
///
/// Pre-decodes all frames eagerly since `AnimationDecoder` requires
/// a stop token per-frame that can't be stored across calls.
pub struct AvifFrameDecoder {
    frames: Vec<(PixelData, u32)>,
    index: usize,
    info: Arc<ImageInfo>,
    total_frames: u32,
}

impl zencodec_types::FrameDecode for AvifFrameDecoder {
    type Error = Error;

    fn frame_count(&self) -> Option<u32> {
        Some(self.total_frames)
    }

    fn next_frame(&mut self, preferred: &[PixelDescriptor]) -> Result<Option<DecodeFrame>, Error> {
        if self.index >= self.frames.len() {
            return Ok(None);
        }
        let (pixels, duration_ms) = self.frames.remove(0);
        let pixels = negotiate_format(pixels, preferred);
        let idx = self.index as u32;
        self.index += 1;
        Ok(Some(DecodeFrame::new(
            pixels,
            Arc::clone(&self.info),
            duration_ms,
            idx,
        )))
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
        assert!(!output.bytes().is_empty());
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
        assert!(!output.bytes().is_empty());
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoding_gray8() {
        let enc = AvifEncoderConfig::new().with_quality(80.0);
        let pixels = vec![rgb::Gray::new(128u8); 64];
        let img = Img::new(pixels, 8, 8);
        let output = enc.encode_gray8(img.as_ref()).unwrap();
        assert!(!output.bytes().is_empty());
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoding_with_metadata() {
        use zencodec_types::{EncodeJob, EncodeRgb8, EncoderConfig};
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
            .encode_rgb8(PixelSlice::from(img.as_ref()))
            .unwrap();
        assert!(!output.bytes().is_empty());
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
        let output = dec.decode(encoded.bytes()).unwrap();
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
            assert!(!output.bytes().is_empty());

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
                .decode_into_rgb_f32(output.bytes(), dst_img.as_mut())
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
        assert!(!output.bytes().is_empty());

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
        dec.decode_into_rgba_f32(output.bytes(), dst_img.as_mut())
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
        use zencodec_types::Gray;

        let pixels: Vec<Gray<f32>> = (0..16 * 16).map(|i| Gray(i as f32 / 255.0)).collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);

        let enc = AvifEncoderConfig::new()
            .with_quality(100.0)
            .with_effort_u32(10);
        let output = enc.encode_gray_f32(img.as_ref()).unwrap();
        assert!(!output.bytes().is_empty());

        let dec = AvifDecoderConfig::new();
        let mut dst_img = imgref::ImgVec::new(vec![Gray(0.0f32); 16 * 16], 16, 16);
        dec.decode_into_gray_f32(output.bytes(), dst_img.as_mut())
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
        use zencodec_types::EncoderConfig;
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
        use zencodec_types::{EncodeJob, EncodeRgb8, EncoderConfig};

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
            .encode_rgb8(PixelSlice::from(img.as_ref()))
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn four_layer_decode_flow() {
        use zencodec_types::{Decode, DecodeJob, DecoderConfig};

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
            .decoder()
            .unwrap()
            .decode(encoded.bytes(), &[])
            .unwrap();
        assert_eq!(decoded.width(), 8);
        assert_eq!(decoded.height(), 8);
    }
}
