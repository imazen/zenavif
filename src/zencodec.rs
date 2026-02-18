//! zencodec-types trait implementations for zenavif.
//!
//! Provides [`AvifEncoderConfig`] and [`AvifDecoderConfig`] types that implement
//! the 4-layer trait hierarchy from zencodec-types, wrapping the native zenavif API.

use std::sync::Arc;

use rgb::{Rgb, Rgba};
#[cfg(feature = "encode")]
use zencodec_types::ImageMetadata;
use zencodec_types::{
    DecodeFrame, DecodeOutput, EncodeOutput, ImageFormat, ImageInfo, PixelData, PixelDescriptor,
    PixelSlice, PixelSliceMut, ResourceLimits, Stop,
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

    /// Set alpha channel quality (0.0 = worst, 100.0 = lossless).
    #[must_use]
    pub fn with_alpha_quality(mut self, quality: f32) -> Self {
        self.inner = self.inner.alpha_quality(quality);
        self
    }
}

#[cfg(feature = "encode")]
impl Default for AvifEncoderConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "encode")]
static ENCODE_CAPS: zencodec_types::CodecCapabilities = zencodec_types::CodecCapabilities::new()
    .with_encode_exif(true)
    .with_encode_cancel(true)
    .with_effort_range(0, 10)
    .with_quality_range(0.0, 100.0);

#[cfg(feature = "encode")]
static ENCODE_DESCRIPTORS: &[PixelDescriptor] =
    &[PixelDescriptor::RGB8_SRGB, PixelDescriptor::RGBA8_SRGB];

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

    fn capabilities() -> &'static zencodec_types::CodecCapabilities {
        &ENCODE_CAPS
    }

    fn with_effort(mut self, effort: i32) -> Self {
        let clamped = effort.clamp(0, 10);
        self.trait_effort = Some(clamped);
        // Invert: trait effort 0 (slowest) = AVIF speed 10 (fastest)
        let speed = (10 - clamped) as u8;
        self.inner = self.inner.speed(speed);
        self
    }

    fn effort(&self) -> Option<i32> {
        self.trait_effort
    }

    fn with_calibrated_quality(mut self, quality: f32) -> Self {
        let clamped = quality.clamp(0.0, 100.0);
        self.trait_quality = Some(clamped);
        self.inner = self.inner.quality(clamped);
        self
    }

    fn calibrated_quality(&self) -> Option<f32> {
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

    fn job(&self) -> AvifEncodeJob<'_> {
        AvifEncodeJob {
            config: self,
            stop: None,
            exif: None,
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
    type Encoder = AvifEncoder<'a>;
    type FrameEncoder = AvifFrameEncoder;

    fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
        self.stop = Some(stop);
        self
    }

    fn with_metadata(mut self, meta: &'a ImageMetadata<'a>) -> Self {
        if let Some(exif) = meta.exif {
            self.exif = Some(exif);
        }
        self
    }

    fn with_limits(self, _limits: ResourceLimits) -> Self {
        // AVIF encoder doesn't have resource limits
        self
    }

    fn encoder(self) -> AvifEncoder<'a> {
        AvifEncoder {
            config: self.config.inner.clone(),
            stop: self.stop,
            exif: self.exif,
        }
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
}

#[cfg(feature = "encode")]
impl AvifEncoder<'_> {
    fn build_config(&self) -> crate::EncoderConfig {
        let mut cfg = self.config.clone();
        if let Some(exif) = self.exif {
            cfg = cfg.exif(exif.to_vec());
        }
        cfg
    }
}

/// Collect pixel data from a `PixelSlice` into contiguous bytes.
fn collect_contiguous_bytes(pixels: &PixelSlice<'_>) -> Vec<u8> {
    let h = pixels.rows();
    let w = pixels.width();
    let bpp = pixels.descriptor().bytes_per_pixel();
    let row_bytes = w as usize * bpp;
    let mut out = Vec::with_capacity(row_bytes * h as usize);
    for y in 0..h {
        out.extend_from_slice(&pixels.row(y)[..row_bytes]);
    }
    out
}

#[cfg(feature = "encode")]
impl zencodec_types::Encoder for AvifEncoder<'_> {
    type Error = Error;

    fn encode(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, Error> {
        let desc = pixels.descriptor();
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.build_config();
        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);

        if desc == PixelDescriptor::RGB8_SRGB {
            let raw = collect_contiguous_bytes(&pixels);
            let rgb: Vec<Rgb<u8>> = raw
                .chunks_exact(3)
                .map(|c| Rgb {
                    r: c[0],
                    g: c[1],
                    b: c[2],
                })
                .collect();
            let img = imgref::ImgVec::new(rgb, w, h);
            let result =
                crate::encode_rgb8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
            Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
        } else if desc == PixelDescriptor::RGBA8_SRGB {
            let raw = collect_contiguous_bytes(&pixels);
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
            let result =
                crate::encode_rgba8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
            Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
        } else if desc == PixelDescriptor::BGRA8_SRGB {
            let raw = collect_contiguous_bytes(&pixels);
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
            let result =
                crate::encode_rgba8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
            Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
        } else if desc == PixelDescriptor::GRAY8_SRGB {
            let raw = collect_contiguous_bytes(&pixels);
            let rgb: Vec<Rgb<u8>> = raw.iter().map(|&g| Rgb { r: g, g, b: g }).collect();
            let img = imgref::ImgVec::new(rgb, w, h);
            let result =
                crate::encode_rgb8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
            Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
        } else if desc == PixelDescriptor::RGBF32_LINEAR {
            use linear_srgb::default::linear_to_srgb_u8;
            let raw = collect_contiguous_bytes(&pixels);
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
            let result =
                crate::encode_rgb8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
            Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
        } else if desc == PixelDescriptor::RGBAF32_LINEAR {
            use linear_srgb::default::linear_to_srgb_u8;
            let raw = collect_contiguous_bytes(&pixels);
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
            let result =
                crate::encode_rgba8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
            Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
        } else if desc == PixelDescriptor::GRAYF32_LINEAR {
            use linear_srgb::default::linear_to_srgb_u8;
            let raw = collect_contiguous_bytes(&pixels);
            let rgb: Vec<Rgb<u8>> = raw
                .chunks_exact(4)
                .map(|c| {
                    let v = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                    let s = linear_to_srgb_u8(v.clamp(0.0, 1.0));
                    Rgb { r: s, g: s, b: s }
                })
                .collect();
            let img = imgref::ImgVec::new(rgb, w, h);
            let result =
                crate::encode_rgb8(img.as_ref(), &cfg, stop).map_err(|e| e.into_inner())?;
            Ok(EncodeOutput::new(result.avif_file, ImageFormat::Avif))
        } else {
            Err(Error::Unsupported(
                "unsupported pixel format for AVIF encode",
            ))
        }
    }

    fn push_rows(&mut self, _rows: PixelSlice<'_>) -> Result<(), Error> {
        Err(Error::Unsupported(
            "AVIF does not support row-level push encoding",
        ))
    }

    fn finish(self) -> Result<EncodeOutput, Error> {
        Err(Error::Unsupported(
            "AVIF does not support row-level push encoding",
        ))
    }

    fn encode_from(
        self,
        _source: &mut dyn FnMut(u32, PixelSliceMut<'_>) -> usize,
    ) -> Result<EncodeOutput, Error> {
        Err(Error::Unsupported(
            "AVIF does not support pull-from-source encoding",
        ))
    }
}

// ── Frame Encoder (stub) ────────────────────────────────────────────────────

/// Stub frame encoder for AVIF (animation not supported via trait interface).
#[cfg(feature = "encode")]
pub struct AvifFrameEncoder;

#[cfg(feature = "encode")]
impl zencodec_types::FrameEncoder for AvifFrameEncoder {
    type Error = Error;

    fn push_frame(&mut self, _pixels: PixelSlice<'_>, _duration_ms: u32) -> Result<(), Error> {
        Err(Error::Unsupported(
            "AVIF animation encoding not supported via trait interface",
        ))
    }

    fn begin_frame(&mut self, _duration_ms: u32) -> Result<(), Error> {
        Err(Error::Unsupported(
            "AVIF animation encoding not supported via trait interface",
        ))
    }

    fn push_rows(&mut self, _rows: PixelSlice<'_>) -> Result<(), Error> {
        Err(Error::Unsupported(
            "AVIF animation encoding not supported via trait interface",
        ))
    }

    fn end_frame(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported(
            "AVIF animation encoding not supported via trait interface",
        ))
    }

    fn pull_frame(
        &mut self,
        _duration_ms: u32,
        _source: &mut dyn FnMut(u32, PixelSliceMut<'_>) -> usize,
    ) -> Result<(), Error> {
        Err(Error::Unsupported(
            "AVIF animation encoding not supported via trait interface",
        ))
    }

    fn finish(self) -> Result<EncodeOutput, Error> {
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
    #[must_use]
    pub fn inner(&self) -> &crate::DecoderConfig {
        &self.inner
    }

    /// Mutable access to the underlying [`crate::DecoderConfig`].
    pub fn inner_mut(&mut self) -> &mut crate::DecoderConfig {
        &mut self.inner
    }
}

impl Default for AvifDecoderConfig {
    fn default() -> Self {
        Self::new()
    }
}

static DECODE_CAPS: zencodec_types::CodecCapabilities = zencodec_types::CodecCapabilities::new()
    .with_decode_cancel(true)
    .with_decode_animation(true);

static DECODE_DESCRIPTORS: &[PixelDescriptor] =
    &[PixelDescriptor::RGB8_SRGB, PixelDescriptor::RGBA8_SRGB];

impl zencodec_types::DecoderConfig for AvifDecoderConfig {
    type Error = Error;
    type Job<'a> = AvifDecodeJob<'a>;

    fn format() -> ImageFormat {
        ImageFormat::Avif
    }

    fn supported_descriptors() -> &'static [PixelDescriptor] {
        DECODE_DESCRIPTORS
    }

    fn capabilities() -> &'static zencodec_types::CodecCapabilities {
        &DECODE_CAPS
    }

    fn job(&self) -> AvifDecodeJob<'_> {
        AvifDecodeJob {
            config: self,
            stop: None,
            limits: ResourceLimits::none(),
        }
    }

    fn probe_header(&self, data: &[u8]) -> Result<ImageInfo, Error> {
        let decoded = crate::decode_with(data, &self.inner, &enough::Unstoppable)
            .map_err(|e| e.into_inner())?;

        let info = ImageInfo::new(decoded.width(), decoded.height(), ImageFormat::Avif)
            .with_alpha(decoded.has_alpha());

        Ok(info)
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
    type Decoder = AvifDecoder<'a>;
    type FrameDecoder = AvifFrameDecoder;

    fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
        self.stop = Some(stop);
        self
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn output_info(&self, data: &[u8]) -> Result<zencodec_types::OutputInfo, Error> {
        // AVIF requires a full decode to know dimensions, use probe
        let decoded = crate::decode_with(data, &self.config.inner, &enough::Unstoppable)
            .map_err(|e| e.into_inner())?;
        let desc = if decoded.has_alpha() {
            PixelDescriptor::RGBA8_SRGB
        } else {
            PixelDescriptor::RGB8_SRGB
        };
        Ok(zencodec_types::OutputInfo::full_decode(
            decoded.width(),
            decoded.height(),
            desc,
        ))
    }

    fn decoder(self) -> AvifDecoder<'a> {
        let cfg = self.effective_config();
        AvifDecoder {
            config: cfg,
            stop: self.stop,
        }
    }

    fn frame_decoder(self, data: &[u8]) -> Result<AvifFrameDecoder, Error> {
        let cfg = self.effective_config();
        let mut anim_dec = crate::AnimationDecoder::new(data, &cfg).map_err(|e| e.into_inner())?;

        let anim_info = anim_dec.info().clone();
        let base_info = ImageInfo::new(0, 0, ImageFormat::Avif)
            .with_alpha(anim_info.has_alpha)
            .with_animation(true)
            .with_frame_count(anim_info.frame_count as u32);

        // Eagerly decode all frames using the stop token
        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let mut frames = Vec::new();
        while let Some(frame) = anim_dec.next_frame(stop).map_err(|e| e.into_inner())? {
            frames.push((frame.pixels, frame.duration_ms));
        }

        // Update base_info with actual dimensions from first frame
        let base_info = if let Some((px, _)) = frames.first() {
            ImageInfo::new(px.width(), px.height(), ImageFormat::Avif)
                .with_alpha(anim_info.has_alpha)
                .with_animation(true)
                .with_frame_count(anim_info.frame_count as u32)
        } else {
            base_info
        };

        Ok(AvifFrameDecoder {
            frames,
            index: 0,
            info: Arc::new(base_info),
            total_frames: anim_info.frame_count as u32,
        })
    }
}

// ── Pixel conversion helpers ────────────────────────────────────────────────

/// Convert AVIF-native pixel data to RGB8.
///
/// AVIF only produces `Rgb8` or `Rgba8`; this handles the Rgba8 → Rgb8 case.
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
        other => unreachable!("AVIF decoder produced unexpected format: {other:?}"),
    }
}

/// Convert AVIF-native pixel data to RGBA8.
///
/// AVIF only produces `Rgb8` or `Rgba8`; this handles the Rgb8 → Rgba8 case.
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
        other => unreachable!("AVIF decoder produced unexpected format: {other:?}"),
    }
}

// ── Decoder ─────────────────────────────────────────────────────────────────

/// Single-image AVIF decoder.
pub struct AvifDecoder<'a> {
    config: crate::DecoderConfig,
    stop: Option<&'a dyn Stop>,
}

impl zencodec_types::Decoder for AvifDecoder<'_> {
    type Error = Error;

    fn decode(self, data: &[u8]) -> Result<DecodeOutput, Error> {
        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let pixels = crate::decode_with(data, &self.config, stop).map_err(|e| e.into_inner())?;

        let w = pixels.width();
        let h = pixels.height();
        let has_alpha = pixels.has_alpha();

        let info = ImageInfo::new(w, h, ImageFormat::Avif).with_alpha(has_alpha);
        Ok(DecodeOutput::new(pixels, info))
    }

    fn decode_into(self, data: &[u8], mut dst: PixelSliceMut<'_>) -> Result<ImageInfo, Error> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        let desc = dst.descriptor();
        let w = dst.width();
        let h = dst.rows();
        let pixels = output.into_pixels();

        if desc == PixelDescriptor::RGB8_SRGB {
            let src = to_rgb8(pixels);
            let row_bytes = w as usize * 3;
            for y in 0..h {
                let src_row = src.as_ref().rows().nth(y as usize).unwrap();
                let dst_row = &mut dst.row_mut(y)[..row_bytes];
                use rgb::ComponentBytes;
                dst_row.copy_from_slice(src_row.as_bytes());
            }
        } else if desc == PixelDescriptor::RGBA8_SRGB {
            let src = to_rgba8(pixels);
            let row_bytes = w as usize * 4;
            for y in 0..h {
                let src_row = src.as_ref().rows().nth(y as usize).unwrap();
                let dst_row = &mut dst.row_mut(y)[..row_bytes];
                use rgb::ComponentBytes;
                dst_row.copy_from_slice(src_row.as_bytes());
            }
        } else if desc == PixelDescriptor::BGRA8_SRGB {
            let src = to_rgba8(pixels);
            let row_bytes = w as usize * 4;
            for y in 0..h {
                let src_row = src.as_ref().rows().nth(y as usize).unwrap();
                let dst_row = &mut dst.row_mut(y)[..row_bytes];
                for (i, px) in src_row.iter().enumerate() {
                    let off = i * 4;
                    dst_row[off] = px.b;
                    dst_row[off + 1] = px.g;
                    dst_row[off + 2] = px.r;
                    dst_row[off + 3] = px.a;
                }
            }
        } else if desc == PixelDescriptor::GRAY8_SRGB {
            let src = to_rgb8(pixels);
            for y in 0..h {
                let src_row = src.as_ref().rows().nth(y as usize).unwrap();
                let dst_row = &mut dst.row_mut(y)[..w as usize];
                for (i, px) in src_row.iter().enumerate() {
                    let luma =
                        ((px.r as u16 * 77 + px.g as u16 * 150 + px.b as u16 * 29) >> 8) as u8;
                    dst_row[i] = luma;
                }
            }
        } else if desc == PixelDescriptor::RGBF32_LINEAR {
            use linear_srgb::default::srgb_u8_to_linear;
            let src = to_rgb8(pixels);
            let row_bytes = w as usize * 12;
            for y in 0..h {
                let src_row = src.as_ref().rows().nth(y as usize).unwrap();
                let dst_row = &mut dst.row_mut(y)[..row_bytes];
                for (i, px) in src_row.iter().enumerate() {
                    let off = i * 12;
                    dst_row[off..off + 4]
                        .copy_from_slice(&srgb_u8_to_linear(px.r).to_le_bytes());
                    dst_row[off + 4..off + 8]
                        .copy_from_slice(&srgb_u8_to_linear(px.g).to_le_bytes());
                    dst_row[off + 8..off + 12]
                        .copy_from_slice(&srgb_u8_to_linear(px.b).to_le_bytes());
                }
            }
        } else if desc == PixelDescriptor::RGBAF32_LINEAR {
            use linear_srgb::default::srgb_u8_to_linear;
            let src = to_rgba8(pixels);
            let row_bytes = w as usize * 16;
            for y in 0..h {
                let src_row = src.as_ref().rows().nth(y as usize).unwrap();
                let dst_row = &mut dst.row_mut(y)[..row_bytes];
                for (i, px) in src_row.iter().enumerate() {
                    let off = i * 16;
                    dst_row[off..off + 4]
                        .copy_from_slice(&srgb_u8_to_linear(px.r).to_le_bytes());
                    dst_row[off + 4..off + 8]
                        .copy_from_slice(&srgb_u8_to_linear(px.g).to_le_bytes());
                    dst_row[off + 8..off + 12]
                        .copy_from_slice(&srgb_u8_to_linear(px.b).to_le_bytes());
                    dst_row[off + 12..off + 16]
                        .copy_from_slice(&(px.a as f32 / 255.0).to_le_bytes());
                }
            }
        } else if desc == PixelDescriptor::GRAYF32_LINEAR {
            use linear_srgb::default::srgb_u8_to_linear;
            let src = to_rgb8(pixels);
            let row_bytes = w as usize * 4;
            for y in 0..h {
                let src_row = src.as_ref().rows().nth(y as usize).unwrap();
                let dst_row = &mut dst.row_mut(y)[..row_bytes];
                for (i, px) in src_row.iter().enumerate() {
                    let r = srgb_u8_to_linear(px.r);
                    let g = srgb_u8_to_linear(px.g);
                    let b = srgb_u8_to_linear(px.b);
                    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                    dst_row[i * 4..(i + 1) * 4].copy_from_slice(&luma.to_le_bytes());
                }
            }
        } else {
            return Err(Error::Unsupported(
                "unsupported pixel format for AVIF decode_into",
            ));
        }

        Ok(info)
    }

    fn decode_rows(
        self,
        data: &[u8],
        _sink: &mut dyn FnMut(u32, PixelSlice<'_>),
    ) -> Result<ImageInfo, Error> {
        let output = self.decode(data)?;
        Ok(output.info().clone())
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

impl zencodec_types::FrameDecoder for AvifFrameDecoder {
    type Error = Error;

    fn frame_count(&self) -> Option<u32> {
        Some(self.total_frames)
    }

    fn next_frame(&mut self) -> Result<Option<DecodeFrame>, Error> {
        if self.index >= self.frames.len() {
            return Ok(None);
        }
        let (pixels, duration_ms) = self.frames.remove(0);
        let idx = self.index as u32;
        self.index += 1;
        Ok(Some(DecodeFrame::new(
            pixels,
            Arc::clone(&self.info),
            duration_ms,
            idx,
        )))
    }

    fn next_frame_into(
        &mut self,
        _dst: PixelSliceMut<'_>,
        _prior_frame: Option<u32>,
    ) -> Result<Option<ImageInfo>, Error> {
        Err(Error::Unsupported(
            "AVIF animation decode_into not yet supported",
        ))
    }

    fn next_frame_rows(
        &mut self,
        _sink: &mut dyn FnMut(u32, PixelSlice<'_>),
    ) -> Result<Option<ImageInfo>, Error> {
        Err(Error::Unsupported(
            "AVIF animation row-level decode not supported",
        ))
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
        use zencodec_types::EncoderConfig;
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
        use zencodec_types::EncoderConfig;
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
        use zencodec_types::EncoderConfig;
        let enc = AvifEncoderConfig::new().with_quality(80.0);
        let pixels = vec![rgb::Gray::new(128u8); 64];
        let img = Img::new(pixels, 8, 8);
        let output = enc.encode_gray8(img.as_ref()).unwrap();
        assert!(!output.bytes().is_empty());
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoding_with_metadata() {
        use zencodec_types::{EncodeJob, Encoder, EncoderConfig};
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
            .encode(PixelSlice::from(img.as_ref()))
            .unwrap();
        assert!(!output.bytes().is_empty());
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_roundtrip() {
        use zencodec_types::{DecoderConfig, EncoderConfig};
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
        use zencodec_types::{DecoderConfig, EncoderConfig};

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
        use zencodec_types::{DecoderConfig, EncoderConfig};

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
        use zencodec_types::{DecoderConfig, EncoderConfig, Gray};

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
            .with_calibrated_quality(75.0)
            .with_effort(5);

        assert_eq!(config.calibrated_quality(), Some(75.0));
        assert_eq!(config.effort(), Some(5));
        assert_eq!(config.is_lossless(), Some(false));
    }

    #[cfg(feature = "encode")]
    #[test]
    fn four_layer_encode_flow() {
        use zencodec_types::{EncodeJob, Encoder, EncoderConfig};

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
            .encode(PixelSlice::from(img.as_ref()))
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn four_layer_decode_flow() {
        use zencodec_types::{DecodeJob, Decoder, DecoderConfig, EncoderConfig};

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
        let decoded = config.job().decoder().decode(encoded.bytes()).unwrap();
        assert_eq!(decoded.width(), 8);
        assert_eq!(decoded.height(), 8);
    }
}
