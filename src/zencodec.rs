//! zencodec-types trait implementations for zenavif.
//!
//! Provides [`AvifEncoding`] and [`AvifDecoding`] types that implement the
//! [`Encoding`] / [`Decoding`] traits from zencodec-types, wrapping the native
//! zenavif API.

#[cfg(feature = "encode")]
use imgref::ImgRef;
use imgref::ImgRefMut;
use rgb::alt::BGRA;
use rgb::{Gray, Rgb, Rgba};
#[cfg(feature = "encode")]
use zencodec_types::ImageMetadata;
use zencodec_types::{ImageFormat, ResourceLimits, Stop};

use crate::error::Error;

// ── Encoding ────────────────────────────────────────────────────────────────

/// AVIF encoder configuration implementing [`zencodec_types::Encoding`].
///
/// Wraps [`crate::EncoderConfig`] with limit fields for the trait interface.
///
/// # Examples
///
/// ```rust,ignore
/// use zencodec_types::Encoding;
/// use zenavif::AvifEncoding;
///
/// let enc = AvifEncoding::new()
///     .with_quality(80.0)
///     .with_effort(6);
/// ```
#[cfg(feature = "encode")]
#[derive(Clone, Debug)]
pub struct AvifEncoding {
    inner: crate::EncoderConfig,
    limits: ResourceLimits,
}

#[cfg(feature = "encode")]
impl AvifEncoding {
    /// Create a default AVIF encoder config (quality 75, speed 4).
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: crate::EncoderConfig::new(),
            limits: ResourceLimits::none(),
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
    pub fn with_effort(mut self, effort: u32) -> Self {
        self.inner = self.inner.speed(effort.min(10) as u8);
        self
    }

    /// Enable or disable lossless encoding.
    #[must_use]
    pub fn with_lossless(mut self, lossless: bool) -> Self {
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
impl Default for AvifEncoding {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "encode")]
impl zencodec_types::Encoding for AvifEncoding {
    type Error = Error;
    type Job<'a> = AvifEncodeJob<'a>;

    fn capabilities() -> &'static zencodec_types::CodecCapabilities {
        static CAPS: zencodec_types::CodecCapabilities = zencodec_types::CodecCapabilities::new()
            .with_encode_exif(true)
            .with_encode_cancel(true);
        &CAPS
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn job(&self) -> AvifEncodeJob<'_> {
        AvifEncodeJob {
            config: self,
            stop: None,
            exif: None,
            limits: ResourceLimits::none(),
        }
    }
}

// ── Encode job ──────────────────────────────────────────────────────────────

/// Per-operation AVIF encode job.
///
/// Created by [`AvifEncoding::job()`]. Borrows temporary data (stop token,
/// metadata) and is consumed by terminal encode methods.
#[cfg(feature = "encode")]
pub struct AvifEncodeJob<'a> {
    config: &'a AvifEncoding,
    stop: Option<&'a dyn Stop>,
    exif: Option<&'a [u8]>,
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

    fn do_encode_rgb8(
        self,
        img: ImgRef<'_, Rgb<u8>>,
    ) -> Result<zencodec_types::EncodeOutput, Error> {
        let mut cfg = self.config.inner.clone();
        if let Some(exif) = self.exif {
            cfg = cfg.exif(exif.to_vec());
        }
        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let result = crate::encode_rgb8(img, &cfg, stop).map_err(|e| e.into_inner())?;
        Ok(zencodec_types::EncodeOutput::new(
            result.avif_file,
            ImageFormat::Avif,
        ))
    }

    fn do_encode_rgba8(
        self,
        img: ImgRef<'_, Rgba<u8>>,
    ) -> Result<zencodec_types::EncodeOutput, Error> {
        let mut cfg = self.config.inner.clone();
        if let Some(exif) = self.exif {
            cfg = cfg.exif(exif.to_vec());
        }
        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let result = crate::encode_rgba8(img, &cfg, stop).map_err(|e| e.into_inner())?;
        Ok(zencodec_types::EncodeOutput::new(
            result.avif_file,
            ImageFormat::Avif,
        ))
    }
}

#[cfg(feature = "encode")]
impl<'a> zencodec_types::EncodingJob<'a> for AvifEncodeJob<'a> {
    type Error = Error;

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

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn encode_rgb8(
        self,
        img: ImgRef<'_, Rgb<u8>>,
    ) -> Result<zencodec_types::EncodeOutput, Self::Error> {
        self.do_encode_rgb8(img)
    }

    fn encode_rgba8(
        self,
        img: ImgRef<'_, Rgba<u8>>,
    ) -> Result<zencodec_types::EncodeOutput, Self::Error> {
        self.do_encode_rgba8(img)
    }

    fn encode_gray8(
        self,
        img: ImgRef<'_, Gray<u8>>,
    ) -> Result<zencodec_types::EncodeOutput, Self::Error> {
        let (buf, w, h) = img.to_contiguous_buf();
        let rgb: Vec<Rgb<u8>> = buf
            .iter()
            .map(|p| {
                let v = p.value();
                Rgb { r: v, g: v, b: v }
            })
            .collect();
        let rgb_img = imgref::ImgVec::new(rgb, w, h);
        self.do_encode_rgb8(rgb_img.as_ref())
    }

    fn encode_bgra8(
        self,
        img: ImgRef<'_, BGRA<u8>>,
    ) -> Result<zencodec_types::EncodeOutput, Self::Error> {
        // Swizzle BGRA → RGBA, encode with alpha
        let (buf, w, h) = img.to_contiguous_buf();
        let rgba: Vec<Rgba<u8>> = buf
            .iter()
            .map(|p| Rgba {
                r: p.r,
                g: p.g,
                b: p.b,
                a: p.a,
            })
            .collect();
        let rgba_img = imgref::ImgVec::new(rgba, w, h);
        self.do_encode_rgba8(rgba_img.as_ref())
    }

    fn encode_bgrx8(
        self,
        img: ImgRef<'_, BGRA<u8>>,
    ) -> Result<zencodec_types::EncodeOutput, Self::Error> {
        // Swizzle BGRA → RGB (drop padding byte)
        let (buf, w, h) = img.to_contiguous_buf();
        let rgb: Vec<Rgb<u8>> = buf
            .iter()
            .map(|p| Rgb {
                r: p.r,
                g: p.g,
                b: p.b,
            })
            .collect();
        let rgb_img = imgref::ImgVec::new(rgb, w, h);
        self.do_encode_rgb8(rgb_img.as_ref())
    }

    fn encode_rgb_f32(
        self,
        img: ImgRef<'_, Rgb<f32>>,
    ) -> Result<zencodec_types::EncodeOutput, Self::Error> {
        use linear_srgb::default::linear_to_srgb_u8;
        let (buf, w, h) = img.to_contiguous_buf();
        let rgb: Vec<Rgb<u8>> = buf
            .iter()
            .map(|p| Rgb {
                r: linear_to_srgb_u8(p.r.clamp(0.0, 1.0)),
                g: linear_to_srgb_u8(p.g.clamp(0.0, 1.0)),
                b: linear_to_srgb_u8(p.b.clamp(0.0, 1.0)),
            })
            .collect();
        let rgb_img = imgref::ImgVec::new(rgb, w, h);
        self.do_encode_rgb8(rgb_img.as_ref())
    }

    fn encode_rgba_f32(
        self,
        img: ImgRef<'_, Rgba<f32>>,
    ) -> Result<zencodec_types::EncodeOutput, Self::Error> {
        use linear_srgb::default::linear_to_srgb_u8;
        let (buf, w, h) = img.to_contiguous_buf();
        let rgba: Vec<Rgba<u8>> = buf
            .iter()
            .map(|p| Rgba {
                r: linear_to_srgb_u8(p.r.clamp(0.0, 1.0)),
                g: linear_to_srgb_u8(p.g.clamp(0.0, 1.0)),
                b: linear_to_srgb_u8(p.b.clamp(0.0, 1.0)),
                a: (p.a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
            })
            .collect();
        let rgba_img = imgref::ImgVec::new(rgba, w, h);
        self.do_encode_rgba8(rgba_img.as_ref())
    }

    fn encode_gray_f32(
        self,
        img: ImgRef<'_, Gray<f32>>,
    ) -> Result<zencodec_types::EncodeOutput, Self::Error> {
        use linear_srgb::default::linear_to_srgb_u8;
        let (buf, w, h) = img.to_contiguous_buf();
        let rgb: Vec<Rgb<u8>> = buf
            .iter()
            .map(|g| {
                let v = linear_to_srgb_u8(g.value().clamp(0.0, 1.0));
                Rgb { r: v, g: v, b: v }
            })
            .collect();
        let rgb_img = imgref::ImgVec::new(rgb, w, h);
        self.do_encode_rgb8(rgb_img.as_ref())
    }
}

// ── Decoding ────────────────────────────────────────────────────────────────

/// AVIF decoder configuration implementing [`zencodec_types::Decoding`].
///
/// Wraps [`crate::DecoderConfig`] with the trait interface.
///
/// # Examples
///
/// ```rust,ignore
/// use zencodec_types::Decoding;
/// use zenavif::AvifDecoding;
///
/// let dec = AvifDecoding::new()
///     .with_limits(ResourceLimits::none().with_max_pixels(100_000_000));
/// let output = dec.decode(&avif_bytes)?;
/// ```
#[derive(Clone, Debug)]
pub struct AvifDecoding {
    inner: crate::DecoderConfig,
    limits: ResourceLimits,
}

impl AvifDecoding {
    /// Create a default AVIF decoder config.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: crate::DecoderConfig::new(),
            limits: ResourceLimits::none(),
        }
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

impl Default for AvifDecoding {
    fn default() -> Self {
        Self::new()
    }
}

impl zencodec_types::Decoding for AvifDecoding {
    type Error = Error;
    type Job<'a> = AvifDecodeJob<'a>;

    fn capabilities() -> &'static zencodec_types::CodecCapabilities {
        static CAPS: zencodec_types::CodecCapabilities =
            zencodec_types::CodecCapabilities::new().with_decode_cancel(true);
        &CAPS
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        // Apply pixel limit to the underlying decoder config if set.
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

    fn job(&self) -> AvifDecodeJob<'_> {
        AvifDecodeJob {
            config: self,
            stop: None,
            limits: ResourceLimits::none(),
        }
    }

    fn probe_header(&self, data: &[u8]) -> Result<zencodec_types::ImageInfo, Self::Error> {
        let decoded = crate::decode_with(data, &self.inner, &enough::Unstoppable)
            .map_err(|e| e.into_inner())?;

        let info =
            zencodec_types::ImageInfo::new(decoded.width(), decoded.height(), ImageFormat::Avif)
                .with_alpha(decoded.has_alpha());

        Ok(info)
    }
}

// ── Decode job ──────────────────────────────────────────────────────────────

/// Per-operation AVIF decode job.
///
/// Created by [`AvifDecoding::job()`]. Borrows a stop token and is consumed
/// by terminal decode methods.
pub struct AvifDecodeJob<'a> {
    config: &'a AvifDecoding,
    stop: Option<&'a dyn Stop>,
    limits: ResourceLimits,
}

impl<'a> zencodec_types::DecodingJob<'a> for AvifDecodeJob<'a> {
    type Error = Error;

    fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
        self.stop = Some(stop);
        self
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn decode(self, data: &[u8]) -> Result<zencodec_types::DecodeOutput, Self::Error> {
        let mut cfg = self.config.inner.clone();
        if let Some(max_pixels) = self.limits.max_pixels {
            cfg = cfg.frame_size_limit(max_pixels.min(u32::MAX as u64) as u32);
        }

        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let pixels = crate::decode_with(data, &cfg, stop).map_err(|e| e.into_inner())?;

        let w = pixels.width();
        let h = pixels.height();
        let has_alpha = pixels.has_alpha();

        let info = zencodec_types::ImageInfo::new(w, h, ImageFormat::Avif).with_alpha(has_alpha);

        Ok(zencodec_types::DecodeOutput::new(pixels, info))
    }

    fn decode_into_rgb8(
        self,
        data: &[u8],
        mut dst: ImgRefMut<'_, Rgb<u8>>,
    ) -> Result<zencodec_types::ImageInfo, Self::Error> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = output.into_rgb8();
        for (src_row, dst_row) in src.as_ref().rows().zip(dst.rows_mut()) {
            let n = src_row.len().min(dst_row.len());
            dst_row[..n].copy_from_slice(&src_row[..n]);
        }
        Ok(info)
    }

    fn decode_into_rgba8(
        self,
        data: &[u8],
        mut dst: ImgRefMut<'_, Rgba<u8>>,
    ) -> Result<zencodec_types::ImageInfo, Self::Error> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = output.into_rgba8();
        for (src_row, dst_row) in src.as_ref().rows().zip(dst.rows_mut()) {
            let n = src_row.len().min(dst_row.len());
            dst_row[..n].copy_from_slice(&src_row[..n]);
        }
        Ok(info)
    }

    fn decode_into_gray8(
        self,
        data: &[u8],
        mut dst: ImgRefMut<'_, Gray<u8>>,
    ) -> Result<zencodec_types::ImageInfo, Self::Error> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = output.into_rgb8();
        for (src_row, dst_row) in src.as_ref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                let luma = ((s.r as u16 * 77 + s.g as u16 * 150 + s.b as u16 * 29) >> 8) as u8;
                *d = Gray::new(luma);
            }
        }
        Ok(info)
    }

    fn decode_into_bgra8(
        self,
        data: &[u8],
        mut dst: ImgRefMut<'_, BGRA<u8>>,
    ) -> Result<zencodec_types::ImageInfo, Self::Error> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = output.into_rgba8();
        for (src_row, dst_row) in src.as_ref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                *d = BGRA {
                    b: s.b,
                    g: s.g,
                    r: s.r,
                    a: s.a,
                };
            }
        }
        Ok(info)
    }

    fn decode_into_bgrx8(
        self,
        data: &[u8],
        mut dst: ImgRefMut<'_, BGRA<u8>>,
    ) -> Result<zencodec_types::ImageInfo, Self::Error> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = output.into_rgb8();
        for (src_row, dst_row) in src.as_ref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                *d = BGRA {
                    b: s.b,
                    g: s.g,
                    r: s.r,
                    a: 255,
                };
            }
        }
        Ok(info)
    }

    fn decode_into_rgb_f32(
        self,
        data: &[u8],
        mut dst: ImgRefMut<'_, Rgb<f32>>,
    ) -> Result<zencodec_types::ImageInfo, Self::Error> {
        use linear_srgb::default::srgb_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        // Use into_rgb_f32() to preserve full precision from 10/12-bit AVIF
        let src = output.into_rgb_f32();
        for (src_row, dst_row) in src.as_ref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                *d = Rgb {
                    r: srgb_to_linear(s.r),
                    g: srgb_to_linear(s.g),
                    b: srgb_to_linear(s.b),
                };
            }
        }
        Ok(info)
    }

    fn decode_into_rgba_f32(
        self,
        data: &[u8],
        mut dst: ImgRefMut<'_, Rgba<f32>>,
    ) -> Result<zencodec_types::ImageInfo, Self::Error> {
        use linear_srgb::default::srgb_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = output.into_rgba_f32();
        for (src_row, dst_row) in src.as_ref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                *d = Rgba {
                    r: srgb_to_linear(s.r),
                    g: srgb_to_linear(s.g),
                    b: srgb_to_linear(s.b),
                    a: s.a, // alpha is linear already
                };
            }
        }
        Ok(info)
    }

    fn decode_into_gray_f32(
        self,
        data: &[u8],
        mut dst: ImgRefMut<'_, Gray<f32>>,
    ) -> Result<zencodec_types::ImageInfo, Self::Error> {
        use linear_srgb::default::srgb_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        let src = output.into_rgb_f32();
        for (src_row, dst_row) in src.as_ref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                let r = srgb_to_linear(s.r);
                let g = srgb_to_linear(s.g);
                let b = srgb_to_linear(s.b);
                *d = Gray::new(0.2126 * r + 0.7152 * g + 0.0722 * b);
            }
        }
        Ok(info)
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
        use zencodec_types::Encoding;
        let enc = AvifEncoding::new().with_quality(80.0);
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
        use zencodec_types::Encoding;
        let enc = AvifEncoding::new().with_quality(80.0);
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
        use zencodec_types::Encoding;
        let enc = AvifEncoding::new().with_quality(80.0);
        let pixels = vec![rgb::Gray::new(128u8); 64];
        let img = Img::new(pixels, 8, 8);
        let output = enc.encode_gray8(img.as_ref()).unwrap();
        assert!(!output.bytes().is_empty());
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoding_with_metadata() {
        use zencodec_types::{Encoding, EncodingJob};
        let enc = AvifEncoding::new().with_quality(80.0);
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
        let output = enc.job().with_exif(exif).encode_rgb8(img.as_ref()).unwrap();
        assert!(!output.bytes().is_empty());
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_roundtrip() {
        use zencodec_types::{Decoding, Encoding};
        let enc = AvifEncoding::new().with_quality(80.0).with_effort(10);
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

        let dec = AvifDecoding::new();
        let output = dec.decode(encoded.bytes()).unwrap();
        assert_eq!(output.info().width, 8);
        assert_eq!(output.info().height, 8);
        assert_eq!(output.info().format, ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn f32_roundtrip_all_simd_tiers() {
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
        use zencodec_types::{Decoding, Encoding};

        let report = for_each_token_permutation(CompileTimePolicy::Warn, |_perm| {
            // Encode linear f32 → AVIF → decode back to f32
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

            let enc = AvifEncoding::new().with_quality(100.0).with_effort(10);
            let output = enc.encode_rgb_f32(img.as_ref()).unwrap();
            assert!(!output.bytes().is_empty());

            let dec = AvifDecoding::new();
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

            // Verify values are in valid range
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
        use zencodec_types::{Decoding, Encoding};

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

        let enc = AvifEncoding::new().with_quality(100.0).with_effort(10);
        let output = enc.encode_rgba_f32(img.as_ref()).unwrap();
        assert!(!output.bytes().is_empty());

        let dec = AvifDecoding::new();
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
        use zencodec_types::{Decoding, Encoding, Gray};

        let pixels: Vec<Gray<f32>> = (0..16 * 16).map(|i| Gray(i as f32 / 255.0)).collect();
        let img = imgref::ImgVec::new(pixels, 16, 16);

        let enc = AvifEncoding::new().with_quality(100.0).with_effort(10);
        let output = enc.encode_gray_f32(img.as_ref()).unwrap();
        assert!(!output.bytes().is_empty());

        let dec = AvifDecoding::new();
        let mut dst_img = imgref::ImgVec::new(vec![Gray(0.0f32); 16 * 16], 16, 16);
        dec.decode_into_gray_f32(output.bytes(), dst_img.as_mut())
            .unwrap();

        for p in dst_img.buf().iter() {
            assert!(p.0 >= 0.0 && p.0 <= 1.0, "gray out of range: {}", p.0);
        }
    }
}
