//! zencodec-types trait implementations for zenavif.
//!
//! Provides [`AvifEncoding`] and [`AvifDecoding`] types that implement the
//! [`Encoding`] / [`Decoding`] traits from zencodec-types, wrapping the native
//! zenavif API.

use imgref::ImgRef;
use rgb::{Rgb, Rgba};
use zencodec_types::{ImageFormat, ImageMetadata, Stop};

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
    limit_pixels: Option<u64>,
    limit_memory: Option<u64>,
    limit_output: Option<u64>,
}

#[cfg(feature = "encode")]
impl AvifEncoding {
    /// Create a default AVIF encoder config (quality 75, speed 4).
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: crate::EncoderConfig::new(),
            limit_pixels: None,
            limit_memory: None,
            limit_output: None,
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

    fn with_quality(mut self, quality: f32) -> Self {
        self.inner = self.inner.quality(quality);
        self
    }

    fn with_effort(mut self, effort: u32) -> Self {
        self.inner = self.inner.speed(effort.min(10) as u8);
        self
    }

    fn with_lossless(mut self, lossless: bool) -> Self {
        if lossless {
            self.inner = self.inner.quality(100.0);
        }
        self
    }

    fn with_alpha_quality(mut self, quality: f32) -> Self {
        self.inner = self.inner.alpha_quality(quality);
        self
    }

    fn with_limit_pixels(mut self, max: u64) -> Self {
        self.limit_pixels = Some(max);
        self
    }

    fn with_limit_memory(mut self, bytes: u64) -> Self {
        self.limit_memory = Some(bytes);
        self
    }

    fn with_limit_output(mut self, bytes: u64) -> Self {
        self.limit_output = Some(bytes);
        self
    }

    fn job(&self) -> AvifEncodeJob<'_> {
        AvifEncodeJob {
            config: self,
            stop: None,
            exif: None,
            limit_pixels: None,
            limit_memory: None,
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
    limit_pixels: Option<u64>,
    limit_memory: Option<u64>,
}

#[cfg(feature = "encode")]
impl<'a> AvifEncodeJob<'a> {
    fn do_encode_rgb8(self, img: ImgRef<'_, Rgb<u8>>) -> Result<zencodec_types::EncodeOutput, Error> {
        let mut cfg = self.config.inner.clone();
        if let Some(exif) = self.exif {
            cfg = cfg.exif(exif.to_vec());
        }
        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let result = crate::encode_rgb8(img, &cfg, stop).map_err(|e| e.into_inner())?;
        Ok(zencodec_types::EncodeOutput::new(result.avif_file, ImageFormat::Avif))
    }

    fn do_encode_rgba8(self, img: ImgRef<'_, Rgba<u8>>) -> Result<zencodec_types::EncodeOutput, Error> {
        let mut cfg = self.config.inner.clone();
        if let Some(exif) = self.exif {
            cfg = cfg.exif(exif.to_vec());
        }
        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let result = crate::encode_rgba8(img, &cfg, stop).map_err(|e| e.into_inner())?;
        Ok(zencodec_types::EncodeOutput::new(result.avif_file, ImageFormat::Avif))
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
        // AVIF/ravif only supports EXIF embedding
        if let Some(exif) = meta.exif {
            self.exif = Some(exif);
        }
        self
    }

    fn with_icc(self, _icc: &'a [u8]) -> Self {
        // ravif doesn't support ICC embedding; ignore
        self
    }

    fn with_exif(mut self, exif: &'a [u8]) -> Self {
        self.exif = Some(exif);
        self
    }

    fn with_xmp(self, _xmp: &'a [u8]) -> Self {
        // ravif doesn't support XMP embedding; ignore
        self
    }

    fn with_limit_pixels(mut self, max: u64) -> Self {
        self.limit_pixels = Some(max);
        self
    }

    fn with_limit_memory(mut self, bytes: u64) -> Self {
        self.limit_memory = Some(bytes);
        self
    }

    fn encode_rgb8(self, img: ImgRef<'_, Rgb<u8>>) -> Result<zencodec_types::EncodeOutput, Self::Error> {
        self.do_encode_rgb8(img)
    }

    fn encode_rgba8(self, img: ImgRef<'_, Rgba<u8>>) -> Result<zencodec_types::EncodeOutput, Self::Error> {
        self.do_encode_rgba8(img)
    }

    fn encode_gray8(self, img: ImgRef<'_, rgb::Gray<u8>>) -> Result<zencodec_types::EncodeOutput, Self::Error> {
        // Expand grayscale to RGB
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
///     .with_limit_pixels(100_000_000);
/// let output = dec.decode(&avif_bytes)?;
/// ```
#[cfg(any(feature = "managed", feature = "asm"))]
#[derive(Clone, Debug)]
pub struct AvifDecoding {
    inner: crate::DecoderConfig,
    limit_file_size: Option<u64>,
}

#[cfg(any(feature = "managed", feature = "asm"))]
impl AvifDecoding {
    /// Create a default AVIF decoder config.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: crate::DecoderConfig::new(),
            limit_file_size: None,
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

#[cfg(any(feature = "managed", feature = "asm"))]
impl Default for AvifDecoding {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a zenavif `DecodedImage` to zencodec `PixelData`.
#[cfg(any(feature = "managed", feature = "asm"))]
fn to_pixel_data(img: crate::DecodedImage) -> zencodec_types::PixelData {
    match img {
        crate::DecodedImage::Rgb8(v) => zencodec_types::PixelData::Rgb8(v),
        crate::DecodedImage::Rgba8(v) => zencodec_types::PixelData::Rgba8(v),
        crate::DecodedImage::Rgb16(v) => zencodec_types::PixelData::Rgb16(v),
        crate::DecodedImage::Rgba16(v) => zencodec_types::PixelData::Rgba16(v),
        crate::DecodedImage::Gray8(v) => {
            // zencodec PixelData::Gray8 uses Gray<u8>, zenavif uses u8
            let w = v.width();
            let h = v.height();
            let (buf, _, _) = v.as_ref().to_contiguous_buf();
            let gray: Vec<rgb::Gray<u8>> = buf.iter().map(|&b| rgb::Gray::new(b)).collect();
            zencodec_types::PixelData::Gray8(imgref::ImgVec::new(gray, w, h))
        }
        crate::DecodedImage::Gray16(v) => {
            // zencodec doesn't have Gray16; expand to Rgb16
            let w = v.width();
            let h = v.height();
            let (buf, _, _) = v.as_ref().to_contiguous_buf();
            let rgb: Vec<Rgb<u16>> = buf
                .iter()
                .map(|&g| Rgb { r: g, g, b: g })
                .collect();
            zencodec_types::PixelData::Rgb16(imgref::ImgVec::new(rgb, w, h))
        }
    }
}

#[cfg(any(feature = "managed", feature = "asm"))]
impl zencodec_types::Decoding for AvifDecoding {
    type Error = Error;
    type Job<'a> = AvifDecodeJob<'a>;

    fn with_limit_pixels(mut self, max: u64) -> Self {
        self.inner = self.inner.frame_size_limit(max.min(u32::MAX as u64) as u32);
        self
    }

    fn with_limit_memory(self, _bytes: u64) -> Self {
        // zenavif doesn't have a memory limit; ignore
        self
    }

    fn with_limit_dimensions(mut self, width: u32, height: u32) -> Self {
        let max = width as u64 * height as u64;
        self.inner = self.inner.frame_size_limit(max.min(u32::MAX as u64) as u32);
        self
    }

    fn with_limit_file_size(mut self, bytes: u64) -> Self {
        self.limit_file_size = Some(bytes);
        self
    }

    fn job(&self) -> AvifDecodeJob<'_> {
        AvifDecodeJob {
            config: self,
            stop: None,
            limit_pixels: None,
            limit_memory: None,
        }
    }

    fn probe(&self, data: &[u8]) -> Result<zencodec_types::ImageInfo, Self::Error> {
        // Full decode to extract dimensions and metadata.
        // A lighter-weight probe would require zenavif-parse's `eager` feature.
        let decoded = crate::decode_with(data, &self.inner, &enough::Unstoppable)
            .map_err(|e| e.into_inner())?;

        let info = zencodec_types::ImageInfo::new(
            decoded.width() as u32,
            decoded.height() as u32,
            ImageFormat::Avif,
        )
        .with_alpha(decoded.has_alpha());

        Ok(info)
    }
}

// ── Decode job ──────────────────────────────────────────────────────────────

/// Per-operation AVIF decode job.
///
/// Created by [`AvifDecoding::job()`]. Borrows a stop token and is consumed
/// by terminal decode methods.
#[cfg(any(feature = "managed", feature = "asm"))]
pub struct AvifDecodeJob<'a> {
    config: &'a AvifDecoding,
    stop: Option<&'a dyn Stop>,
    limit_pixels: Option<u64>,
    limit_memory: Option<u64>,
}

#[cfg(any(feature = "managed", feature = "asm"))]
impl<'a> zencodec_types::DecodingJob<'a> for AvifDecodeJob<'a> {
    type Error = Error;

    fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
        self.stop = Some(stop);
        self
    }

    fn with_limit_pixels(mut self, max: u64) -> Self {
        self.limit_pixels = Some(max);
        self
    }

    fn with_limit_memory(mut self, bytes: u64) -> Self {
        self.limit_memory = Some(bytes);
        self
    }

    fn decode(self, data: &[u8]) -> Result<zencodec_types::DecodeOutput, Self::Error> {
        let mut cfg = self.config.inner.clone();
        if let Some(max) = self.limit_pixels {
            cfg = cfg.frame_size_limit(max.min(u32::MAX as u64) as u32);
        }

        let stop: &dyn Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let decoded = crate::decode_with(data, &cfg, stop).map_err(|e| e.into_inner())?;

        let w = decoded.width() as u32;
        let h = decoded.height() as u32;
        let has_alpha = decoded.has_alpha();

        let info = zencodec_types::ImageInfo::new(w, h, ImageFormat::Avif)
            .with_alpha(has_alpha);

        let pixels = to_pixel_data(decoded);
        Ok(zencodec_types::DecodeOutput::new(pixels, info))
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use imgref::Img;

    #[cfg(feature = "encode")]
    #[test]
    fn encoding_default_roundtrip() {
        use zencodec_types::Encoding;
        let enc = AvifEncoding::new().with_quality(80.0);
        let pixels = vec![Rgb { r: 128u8, g: 64, b: 32 }; 64];
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
                a: 128,
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
        use zencodec_types::Encoding;
        let enc = AvifEncoding::new().with_quality(80.0);
        let pixels = vec![Rgb { r: 255u8, g: 0, b: 0 }; 16];
        let img = Img::new(pixels, 4, 4);

        let exif = b"fake exif data";
        let output = enc
            .job()
            .with_exif(exif)
            .encode_rgb8(img.as_ref())
            .unwrap();
        assert!(!output.bytes().is_empty());
    }

    #[cfg(all(feature = "encode", any(feature = "managed", feature = "asm")))]
    #[test]
    fn decode_roundtrip() {
        use zencodec_types::{Decoding, Encoding};
        let enc = AvifEncoding::new().with_quality(80.0).with_effort(10);
        let pixels = vec![Rgb { r: 200u8, g: 100, b: 50 }; 64];
        let img = Img::new(pixels, 8, 8);
        let encoded = enc.encode_rgb8(img.as_ref()).unwrap();

        let dec = AvifDecoding::new();
        let output = dec.decode(encoded.bytes()).unwrap();
        assert_eq!(output.info().width, 8);
        assert_eq!(output.info().height, 8);
        assert_eq!(output.info().format, ImageFormat::Avif);
    }
}
