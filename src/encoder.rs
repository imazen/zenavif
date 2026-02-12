//! AVIF encoding via ravif
//!
//! Provides [`EncoderConfig`] for configuring encoding and
//! [`encode_rgb8`] / [`encode_rgba8`] / [`encode_rgb16`] / [`encode_rgba16`]
//! for encoding images.

use crate::Result;
use crate::error::Error;
use enough::Stop;
use imgref::ImgRef;
use rgb::{Rgb, Rgba};
use whereat::at;

/// Encoded AVIF image output
#[derive(Debug, Clone)]
pub struct EncodedImage {
    /// The complete AVIF file bytes
    pub avif_file: Vec<u8>,
    /// Bytes used for the color AV1 payload
    pub color_byte_size: usize,
    /// Bytes used for the alpha AV1 payload
    pub alpha_byte_size: usize,
}

/// Bit depth for encoding
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodeBitDepth {
    /// 8 bits per channel
    Eight,
    /// 10 bits per channel
    Ten,
    /// Automatic selection based on input
    #[default]
    Auto,
}

/// Internal color model for encoding
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodeColorModel {
    /// YCbCr color model (smaller files, standard)
    #[default]
    YCbCr,
    /// RGB color model (lossless-friendly)
    Rgb,
}

/// Alpha channel handling mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodeAlphaMode {
    /// Unassociated alpha, clean color values under transparent pixels
    #[default]
    UnassociatedClean,
    /// Unassociated alpha, preserve original color values (may compress worse)
    UnassociatedDirty,
    /// Premultiplied alpha
    Premultiplied,
}

/// Configuration for AVIF encoding
///
/// Uses a builder pattern matching [`crate::DecoderConfig`].
///
/// # Example
///
/// ```
/// use zenavif::EncoderConfig;
///
/// let config = EncoderConfig::new()
///     .quality(80.0)
///     .speed(6);
/// ```
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    pub(crate) quality: f32,
    pub(crate) speed: u8,
    pub(crate) alpha_quality: Option<f32>,
    pub(crate) bit_depth: EncodeBitDepth,
    pub(crate) color_model: EncodeColorModel,
    pub(crate) alpha_color_mode: EncodeAlphaMode,
    pub(crate) threads: Option<usize>,
    pub(crate) exif: Option<Vec<u8>>,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            quality: 75.0,
            speed: 4,
            alpha_quality: None,
            bit_depth: EncodeBitDepth::default(),
            color_model: EncodeColorModel::default(),
            alpha_color_mode: EncodeAlphaMode::default(),
            threads: None,
            exif: None,
        }
    }
}

impl EncoderConfig {
    /// Create a new encoder configuration with default settings
    ///
    /// Defaults: quality 75, speed 4, auto bit depth, YCbCr color model
    pub fn new() -> Self {
        Self::default()
    }

    /// Set encoding quality (1.0 = worst, 100.0 = best/lossless)
    pub fn quality(mut self, quality: f32) -> Self {
        self.quality = quality;
        self
    }

    /// Set encoding speed (1 = slowest/best, 10 = fastest/worst)
    pub fn speed(mut self, speed: u8) -> Self {
        self.speed = speed;
        self
    }

    /// Set separate quality for the alpha channel
    ///
    /// If not set, uses the same quality as color.
    pub fn alpha_quality(mut self, quality: f32) -> Self {
        self.alpha_quality = Some(quality);
        self
    }

    /// Set the output bit depth
    pub fn bit_depth(mut self, depth: EncodeBitDepth) -> Self {
        self.bit_depth = depth;
        self
    }

    /// Set the internal color model
    ///
    /// YCbCr (default) produces smaller files. RGB may be better for lossless.
    pub fn color_model(mut self, model: EncodeColorModel) -> Self {
        self.color_model = model;
        self
    }

    /// Set the alpha channel handling mode
    pub fn alpha_color_mode(mut self, mode: EncodeAlphaMode) -> Self {
        self.alpha_color_mode = mode;
        self
    }

    /// Set the number of threads
    ///
    /// `None` uses the rayon default. `Some(1)` for single-threaded.
    pub fn threads(mut self, threads: Option<usize>) -> Self {
        self.threads = threads;
        self
    }

    /// Embed EXIF metadata in the output
    pub fn exif(mut self, exif_data: Vec<u8>) -> Self {
        self.exif = Some(exif_data);
        self
    }
}

/// Build a ravif Encoder from our config
fn build_ravif_encoder(config: &EncoderConfig) -> ravif::Encoder<'_> {
    let mut enc = ravif::Encoder::new()
        .with_quality(config.quality)
        .with_speed(config.speed)
        .with_bit_depth(match config.bit_depth {
            EncodeBitDepth::Eight => ravif::BitDepth::Eight,
            EncodeBitDepth::Ten => ravif::BitDepth::Ten,
            EncodeBitDepth::Auto => ravif::BitDepth::Auto,
        })
        .with_internal_color_model(match config.color_model {
            EncodeColorModel::YCbCr => ravif::ColorModel::YCbCr,
            EncodeColorModel::Rgb => ravif::ColorModel::RGB,
        })
        .with_alpha_color_mode(match config.alpha_color_mode {
            EncodeAlphaMode::UnassociatedClean => ravif::AlphaColorMode::UnassociatedClean,
            EncodeAlphaMode::UnassociatedDirty => ravif::AlphaColorMode::UnassociatedDirty,
            EncodeAlphaMode::Premultiplied => ravif::AlphaColorMode::Premultiplied,
        })
        .with_num_threads(config.threads);

    if let Some(aq) = config.alpha_quality {
        enc = enc.with_alpha_quality(aq);
    }
    if let Some(ref exif_data) = config.exif {
        enc = enc.with_exif(exif_data.as_slice());
    }
    enc
}

/// Encode an 8-bit RGB image to AVIF
///
/// # Arguments
///
/// * `img` - RGB8 image buffer
/// * `config` - Encoder configuration
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_rgb8(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at(Error::from(e)))?;
    let enc = build_ravif_encoder(config);
    let result = enc
        .encode_rgb(img)
        .map_err(|e| at(Error::Encode(e.to_string())))?;
    Ok(EncodedImage {
        avif_file: result.avif_file,
        color_byte_size: result.color_byte_size,
        alpha_byte_size: result.alpha_byte_size,
    })
}

/// Encode an 8-bit RGBA image to AVIF
///
/// # Arguments
///
/// * `img` - RGBA8 image buffer
/// * `config` - Encoder configuration
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_rgba8(
    img: ImgRef<'_, Rgba<u8>>,
    config: &EncoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at(Error::from(e)))?;
    let enc = build_ravif_encoder(config);
    let result = enc
        .encode_rgba(img)
        .map_err(|e| at(Error::Encode(e.to_string())))?;
    Ok(EncodedImage {
        avif_file: result.avif_file,
        color_byte_size: result.color_byte_size,
        alpha_byte_size: result.alpha_byte_size,
    })
}

/// Encode a 16-bit RGB image to AVIF (10-bit AV1)
///
/// Input values should be in 10-bit range (0–1023). Values outside this
/// range will be clamped by the encoder.
///
/// # Arguments
///
/// * `img` - RGB16 image buffer
/// * `config` - Encoder configuration
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_rgb16(
    img: ImgRef<'_, Rgb<u16>>,
    config: &EncoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at(Error::from(e)))?;
    let enc = build_ravif_encoder(config);
    let width = img.width();
    let height = img.height();
    let pixels: Vec<[u16; 3]> = img.pixels().map(|p| [p.r, p.g, p.b]).collect();
    let result = enc
        .encode_raw_planes_10_bit(
            width,
            height,
            pixels,
            None::<std::iter::Empty<u16>>,
            ravif::PixelRange::Full,
            ravif::MatrixCoefficients::Identity,
        )
        .map_err(|e| at(Error::Encode(e.to_string())))?;
    Ok(EncodedImage {
        avif_file: result.avif_file,
        color_byte_size: result.color_byte_size,
        alpha_byte_size: result.alpha_byte_size,
    })
}

/// Encode a 16-bit RGBA image to AVIF (10-bit AV1)
///
/// Input color values should be in 10-bit range (0–1023). Alpha values
/// should also be in 10-bit range. Values outside this range will be
/// clamped by the encoder.
///
/// # Arguments
///
/// * `img` - RGBA16 image buffer
/// * `config` - Encoder configuration
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_rgba16(
    img: ImgRef<'_, Rgba<u16>>,
    config: &EncoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at(Error::from(e)))?;
    let enc = build_ravif_encoder(config);
    let width = img.width();
    let height = img.height();
    let pixels: Vec<[u16; 3]> = img.pixels().map(|p| [p.r, p.g, p.b]).collect();
    let alpha: Vec<u16> = img.pixels().map(|p| p.a).collect();
    let result = enc
        .encode_raw_planes_10_bit(
            width,
            height,
            pixels,
            Some(alpha),
            ravif::PixelRange::Full,
            ravif::MatrixCoefficients::Identity,
        )
        .map_err(|e| at(Error::Encode(e.to_string())))?;
    Ok(EncodedImage {
        avif_file: result.avif_file,
        color_byte_size: result.color_byte_size,
        alpha_byte_size: result.alpha_byte_size,
    })
}
