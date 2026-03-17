//! AVIF encoding via ravif
//!
//! Provides [`EncoderConfig`] for configuring encoding and
//! [`encode_rgb8`] / [`encode_rgba8`] / [`encode_rgb16`] / [`encode_rgba16`]
//! for encoding images.

use crate::Result;
use crate::error::Error;
use enough::Stop;
use imgref::{ImgRef, ImgVec};
use rgb::{RGB8, RGBA8, Rgb, Rgba};
use rgb::{RGB16, RGBA16};
use whereat::at;

/// Pre-encoded gain map data for embedding in an AVIF file.
///
/// Contains a pre-encoded AV1 bitstream of the gain map image plus the
/// ISO 21496-1 binary metadata. Used for UltraHDR / SDR+HDR tone mapping.
///
/// The gain map is typically a lower-resolution, monochrome or RGB image
/// encoding the per-pixel gain needed to reconstruct the HDR rendition from
/// the SDR base image.
#[derive(Debug, Clone)]
pub struct GainMapConfig {
    /// Pre-encoded AV1 bitstream of the gain map image.
    pub av1_data: Vec<u8>,
    /// Width of the gain map image in pixels.
    pub width: u32,
    /// Height of the gain map image in pixels.
    pub height: u32,
    /// Bit depth of the gain map AV1 data (typically 8 or 10).
    pub bit_depth: u8,
    /// ISO 21496-1 binary metadata blob.
    pub metadata: Vec<u8>,
}

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

/// Pixel value range for AV1 encoding.
///
/// Full range uses the entire value range (0–255 for 8-bit, 0–1023 for 10-bit).
/// Limited/narrow range uses the broadcast range (16–235 luma, 16–240 chroma
/// for 8-bit; 64–940 for 10-bit). Use limited range for broadcast/studio
/// content that is already in narrow range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodePixelRange {
    /// Full range (0–255 / 0–1023). Default.
    #[default]
    Full,
    /// Limited/narrow range (16–235 / 64–940). For broadcast/studio content.
    Limited,
}

/// Mastering display metadata for HDR encoding (SMPTE ST 2086)
///
/// All chromaticity values are in CIE 1931 0.16 fixed-point (0–65535 maps to 0.0–1.0).
/// Luminance values use 24.8 (max) and 18.14 (min) fixed-point encoding.
#[derive(Debug, Clone, Copy)]
pub struct MasteringDisplayConfig {
    /// Chromaticity coordinates for red, green, blue primaries: [(x, y); 3]
    pub primaries: [(u16, u16); 3],
    /// White point chromaticity (x, y)
    pub white_point: (u16, u16),
    /// Maximum display luminance (24.8 fixed-point cd/m²)
    pub max_luminance: u32,
    /// Minimum display luminance (18.14 fixed-point cd/m²)
    pub min_luminance: u32,
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
    /// XMP metadata to embed
    pub(crate) xmp: Option<Vec<u8>>,
    /// ICC color profile to embed
    pub(crate) icc_profile: Option<Vec<u8>>,
    /// Image rotation (counter-clockwise degrees: 0, 90, 180, 270)
    pub(crate) rotation: Option<u8>,
    /// Image mirror axis (0 = vertical, 1 = horizontal)
    pub(crate) mirror: Option<u8>,
    /// Content light level (max_cll, max_fall)
    pub(crate) content_light_level: Option<(u16, u16)>,
    /// Mastering display metadata
    pub(crate) mastering_display: Option<MasteringDisplayConfig>,
    /// CICP color primaries code point (ITU-T H.273)
    pub(crate) color_primaries: Option<u8>,
    /// CICP transfer characteristics code point (ITU-T H.273)
    pub(crate) transfer_characteristics: Option<u8>,
    /// CICP matrix coefficients code point (ITU-T H.273)
    pub(crate) matrix_coefficients: Option<u8>,
    /// Pixel range: full (0–255/0–1023) or limited/narrow (16–235/64–940)
    pub(crate) pixel_range: Option<EncodePixelRange>,
    /// Pre-encoded gain map for UltraHDR / ISO 21496-1
    pub(crate) gain_map: Option<GainMapConfig>,
    /// Enable AV1 quantization matrices (imazen/rav1e fork)
    #[cfg(feature = "encode-imazen")]
    pub(crate) enable_qm: bool,
    /// Enable variance adaptive quantization (imazen/rav1e fork)
    #[cfg(feature = "encode-imazen")]
    pub(crate) enable_vaq: bool,
    /// VAQ strength 0.0–4.0 (imazen/rav1e fork)
    #[cfg(feature = "encode-imazen")]
    pub(crate) vaq_strength: f64,
    /// Use Tune::StillImage instead of Tune::Psychovisual (imazen/rav1e fork)
    #[cfg(feature = "encode-imazen")]
    pub(crate) tune_still_image: bool,
    /// Mathematically lossless encoding (quantizer=0) (imazen/rav1e fork)
    #[cfg(feature = "encode-imazen")]
    pub(crate) lossless: bool,
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
            xmp: None,
            icc_profile: None,
            rotation: None,
            mirror: None,
            content_light_level: None,
            mastering_display: None,
            color_primaries: None,
            transfer_characteristics: None,
            matrix_coefficients: None,
            pixel_range: None,
            gain_map: None,
            #[cfg(feature = "encode-imazen")]
            enable_qm: true,
            #[cfg(feature = "encode-imazen")]
            enable_vaq: false,
            #[cfg(feature = "encode-imazen")]
            vaq_strength: 1.0,
            #[cfg(feature = "encode-imazen")]
            tune_still_image: false,
            #[cfg(feature = "encode-imazen")]
            lossless: false,
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

    /// Embed XMP metadata in the output
    pub fn xmp(mut self, xmp_data: Vec<u8>) -> Self {
        self.xmp = Some(xmp_data);
        self
    }

    /// Embed an ICC color profile in the output
    pub fn icc_profile(mut self, profile: Vec<u8>) -> Self {
        self.icc_profile = Some(profile);
        self
    }

    /// Set image rotation (counter-clockwise degrees: 0, 90, 180, 270)
    pub fn rotation(mut self, angle: u8) -> Self {
        self.rotation = Some(angle);
        self
    }

    /// Set image mirror axis (0 = vertical/left-right, 1 = horizontal/top-bottom)
    pub fn mirror(mut self, axis: u8) -> Self {
        self.mirror = Some(axis);
        self
    }

    /// Set content light level metadata (HDR)
    ///
    /// * `max_cll` - Maximum content light level (cd/m²)
    /// * `max_fall` - Maximum frame-average light level (cd/m²)
    pub fn content_light_level(mut self, max_cll: u16, max_fall: u16) -> Self {
        self.content_light_level = Some((max_cll, max_fall));
        self
    }

    /// Set mastering display metadata (HDR, SMPTE ST 2086)
    pub fn mastering_display(mut self, md: MasteringDisplayConfig) -> Self {
        self.mastering_display = Some(md);
        self
    }

    /// Set CICP color primaries code point (ITU-T H.273).
    ///
    /// Common values: 1 = BT.709/sRGB, 9 = BT.2020, 12 = Display P3.
    pub fn color_primaries(mut self, cp: u8) -> Self {
        self.color_primaries = Some(cp);
        self
    }

    /// Set CICP transfer characteristics code point (ITU-T H.273).
    ///
    /// Common values: 1 = BT.709, 13 = sRGB, 16 = PQ (HDR10), 18 = HLG.
    pub fn transfer_characteristics(mut self, tc: u8) -> Self {
        self.transfer_characteristics = Some(tc);
        self
    }

    /// Set CICP matrix coefficients code point (ITU-T H.273).
    ///
    /// Common values: 0 = Identity/RGB, 1 = BT.709, 6 = BT.601, 9 = BT.2020.
    pub fn matrix_coefficients(mut self, mc: u8) -> Self {
        self.matrix_coefficients = Some(mc);
        self
    }

    /// Set pixel value range for AV1 encoding.
    ///
    /// Default is full range. Use limited/narrow range for broadcast content
    /// that already uses studio levels (16–235 for 8-bit, 64–940 for 10-bit).
    pub fn pixel_range(mut self, range: EncodePixelRange) -> Self {
        self.pixel_range = Some(range);
        self
    }

    /// Embed a pre-encoded gain map for UltraHDR / ISO 21496-1.
    ///
    /// The gain map enables SDR/HDR tone mapping: the primary image is the SDR
    /// base, and the gain map allows reconstruction of the HDR rendition.
    ///
    /// * `av1_data` - Pre-encoded AV1 bitstream of the gain map image.
    /// * `width` - Width of the gain map image in pixels.
    /// * `height` - Height of the gain map image in pixels.
    /// * `bit_depth` - Bit depth of the gain map AV1 data (typically 8 or 10).
    /// * `metadata` - ISO 21496-1 binary metadata blob.
    pub fn with_gain_map(
        mut self,
        av1_data: Vec<u8>,
        width: u32,
        height: u32,
        bit_depth: u8,
        metadata: Vec<u8>,
    ) -> Self {
        self.gain_map = Some(GainMapConfig {
            av1_data,
            width,
            height,
            bit_depth,
            metadata,
        });
        self
    }

    /// Enable/disable AV1 quantization matrices (imazen/rav1e fork).
    ///
    /// QM applies frequency-dependent quantization weights for ~10% BD-rate improvement.
    /// Default: enabled.
    #[cfg(feature = "encode-imazen")]
    pub fn with_qm(mut self, enable: bool) -> Self {
        self.enable_qm = enable;
        self
    }

    /// Enable/disable variance adaptive quantization (imazen/rav1e fork).
    ///
    /// Allocates more bits to smooth regions, fewer to textured regions.
    /// Default: enabled, strength 0.5.
    #[cfg(feature = "encode-imazen")]
    pub fn with_vaq(mut self, enable: bool, strength: f64) -> Self {
        self.enable_vaq = enable;
        self.vaq_strength = strength;
        self
    }

    /// Enable/disable still-image tuning (imazen/rav1e fork).
    ///
    /// Uses perceptual distortion metric with reduced CDEF/deblock for detail preservation.
    /// Default: enabled.
    #[cfg(feature = "encode-imazen")]
    pub fn with_still_image_tuning(mut self, enable: bool) -> Self {
        self.tune_still_image = enable;
        self
    }

    /// Enable/disable mathematically lossless encoding (imazen/rav1e fork).
    ///
    /// Sets quantizer to 0. Default: disabled.
    #[cfg(feature = "encode-imazen")]
    pub fn with_lossless(mut self, lossless: bool) -> Self {
        self.lossless = lossless;
        self
    }

    /// Convenience preset: optimal still image settings (imazen/rav1e fork).
    ///
    /// Enables QM, VAQ (strength 0.5), and still-image tuning.
    #[cfg(feature = "encode-imazen")]
    pub fn still_image_preset(self) -> Self {
        self.with_qm(true)
            .with_vaq(true, 0.5)
            .with_still_image_tuning(true)
    }
}

/// Convert a CICP color primaries code point to the ravif enum.
fn cicp_to_color_primaries(cp: u8) -> ravif::ColorPrimaries {
    match cp {
        1 => ravif::ColorPrimaries::BT709,
        4 => ravif::ColorPrimaries::BT470M,
        5 => ravif::ColorPrimaries::BT470BG,
        6 => ravif::ColorPrimaries::BT601,
        7 => ravif::ColorPrimaries::SMPTE240,
        8 => ravif::ColorPrimaries::GenericFilm,
        9 => ravif::ColorPrimaries::BT2020,
        10 => ravif::ColorPrimaries::XYZ,
        11 => ravif::ColorPrimaries::SMPTE431,
        12 => ravif::ColorPrimaries::SMPTE432,
        22 => ravif::ColorPrimaries::EBU3213,
        _ => ravif::ColorPrimaries::Unspecified,
    }
}

/// Convert a CICP transfer characteristics code point to the ravif enum.
fn cicp_to_transfer_characteristics(tc: u8) -> ravif::TransferCharacteristics {
    match tc {
        1 => ravif::TransferCharacteristics::BT709,
        4 => ravif::TransferCharacteristics::BT470M,
        5 => ravif::TransferCharacteristics::BT470BG,
        6 => ravif::TransferCharacteristics::BT601,
        7 => ravif::TransferCharacteristics::SMPTE240,
        8 => ravif::TransferCharacteristics::Linear,
        9 => ravif::TransferCharacteristics::Log100,
        10 => ravif::TransferCharacteristics::Log100Sqrt10,
        11 => ravif::TransferCharacteristics::IEC61966,
        12 => ravif::TransferCharacteristics::BT1361,
        13 => ravif::TransferCharacteristics::SRGB,
        14 => ravif::TransferCharacteristics::BT2020_10Bit,
        15 => ravif::TransferCharacteristics::BT2020_12Bit,
        16 => ravif::TransferCharacteristics::SMPTE2084,
        18 => ravif::TransferCharacteristics::HLG,
        _ => ravif::TransferCharacteristics::Unspecified,
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
    if let Some(ref xmp_data) = config.xmp {
        enc = enc.with_xmp(xmp_data.clone());
    }
    if let Some(ref icc) = config.icc_profile {
        enc = enc.with_icc_profile(icc.clone());
    }
    if let Some(angle) = config.rotation {
        enc = enc.with_rotation(angle);
    }
    if let Some(axis) = config.mirror {
        enc = enc.with_mirror(axis);
    }
    if let Some((max_cll, max_fall)) = config.content_light_level {
        enc = enc.with_content_light(ravif::ContentLight {
            max_content_light_level: max_cll,
            max_frame_average_light_level: max_fall,
        });
    }
    if let Some(md) = config.mastering_display {
        enc = enc.with_mastering_display(ravif::MasteringDisplay {
            primaries: [
                ravif::ChromaticityPoint {
                    x: md.primaries[0].0,
                    y: md.primaries[0].1,
                },
                ravif::ChromaticityPoint {
                    x: md.primaries[1].0,
                    y: md.primaries[1].1,
                },
                ravif::ChromaticityPoint {
                    x: md.primaries[2].0,
                    y: md.primaries[2].1,
                },
            ],
            white_point: ravif::ChromaticityPoint {
                x: md.white_point.0,
                y: md.white_point.1,
            },
            max_luminance: md.max_luminance,
            min_luminance: md.min_luminance,
        });
    }
    if let Some(cp) = config.color_primaries {
        enc = enc.with_color_primaries(cicp_to_color_primaries(cp));
    }
    if let Some(tc) = config.transfer_characteristics {
        enc = enc.with_transfer_characteristics(cicp_to_transfer_characteristics(tc));
    }
    if let Some(pr) = config.pixel_range {
        enc = enc.with_pixel_range(match pr {
            EncodePixelRange::Full => ravif::PixelRange::Full,
            EncodePixelRange::Limited => ravif::PixelRange::Limited,
        });
    }
    if let Some(ref gm) = config.gain_map {
        enc = enc.with_gain_map(ravif::GainMapData {
            av1_data: gm.av1_data.clone(),
            width: gm.width,
            height: gm.height,
            bit_depth: gm.bit_depth,
            metadata: gm.metadata.clone(),
        });
    }
    #[cfg(feature = "encode-imazen")]
    {
        enc = enc
            .with_qm(config.enable_qm)
            .with_vaq(config.enable_vaq, config.vaq_strength)
            .with_still_image_tuning(config.tune_still_image)
            .with_lossless(config.lossless);
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
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let enc = build_ravif_encoder(config);
    let result = enc
        .encode_rgb(img)
        .map_err(|e| at!(Error::Encode(e.to_string())))?;
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
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let enc = build_ravif_encoder(config);
    let result = enc
        .encode_rgba(img)
        .map_err(|e| at!(Error::Encode(e.to_string())))?;
    Ok(EncodedImage {
        avif_file: result.avif_file,
        color_byte_size: result.color_byte_size,
        alpha_byte_size: result.alpha_byte_size,
    })
}

/// Encode a 16-bit RGB image to AVIF (10-bit AV1)
///
/// Input values should be in full u16 range (0–65535), in the image's native
/// transfer function (typically sRGB gamma). Values are scaled to 10-bit
/// internally before encoding.
///
/// # Arguments
///
/// * `img` - RGB16 image buffer (0–65535)
/// * `config` - Encoder configuration
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_rgb16(
    img: ImgRef<'_, Rgb<u16>>,
    config: &EncoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<EncodedImage> {
    use crate::convert::scale_from_u16;
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let enc = build_ravif_encoder(config);
    let width = img.width();
    let height = img.height();
    let pixels: Vec<[u16; 3]> = img
        .pixels()
        .map(|p| {
            [
                scale_from_u16(p.r, 10),
                scale_from_u16(p.g, 10),
                scale_from_u16(p.b, 10),
            ]
        })
        .collect();
    let pixel_range = match config.pixel_range {
        Some(EncodePixelRange::Limited) => ravif::PixelRange::Limited,
        _ => ravif::PixelRange::Full,
    };
    let result = enc
        .encode_raw_planes_10_bit(
            width,
            height,
            pixels,
            None::<std::iter::Empty<u16>>,
            pixel_range,
            ravif::MatrixCoefficients::Identity,
        )
        .map_err(|e| at!(Error::Encode(e.to_string())))?;
    Ok(EncodedImage {
        avif_file: result.avif_file,
        color_byte_size: result.color_byte_size,
        alpha_byte_size: result.alpha_byte_size,
    })
}

/// Encode a 16-bit RGBA image to AVIF (10-bit AV1)
///
/// Input values should be in full u16 range (0–65535), in the image's native
/// transfer function (typically sRGB gamma). Values are scaled to 10-bit
/// internally before encoding.
///
/// # Arguments
///
/// * `img` - RGBA16 image buffer (0–65535)
/// * `config` - Encoder configuration
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_rgba16(
    img: ImgRef<'_, Rgba<u16>>,
    config: &EncoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<EncodedImage> {
    use crate::convert::scale_from_u16;
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let enc = build_ravif_encoder(config);
    let width = img.width();
    let height = img.height();
    let pixels: Vec<[u16; 3]> = img
        .pixels()
        .map(|p| {
            [
                scale_from_u16(p.r, 10),
                scale_from_u16(p.g, 10),
                scale_from_u16(p.b, 10),
            ]
        })
        .collect();
    let alpha: Vec<u16> = img.pixels().map(|p| scale_from_u16(p.a, 10)).collect();
    let pixel_range = match config.pixel_range {
        Some(EncodePixelRange::Limited) => ravif::PixelRange::Limited,
        _ => ravif::PixelRange::Full,
    };
    let result = enc
        .encode_raw_planes_10_bit(
            width,
            height,
            pixels,
            Some(alpha),
            pixel_range,
            ravif::MatrixCoefficients::Identity,
        )
        .map_err(|e| at!(Error::Encode(e.to_string())))?;
    Ok(EncodedImage {
        avif_file: result.avif_file,
        color_byte_size: result.color_byte_size,
        alpha_byte_size: result.alpha_byte_size,
    })
}

/// A single frame in an animated AVIF sequence
#[derive(Clone)]
pub struct AnimationFrame {
    /// Frame pixel data (RGB8)
    pub pixels: ImgVec<RGB8>,
    /// Duration of this frame in milliseconds
    pub duration_ms: u32,
}

/// A single frame with alpha in an animated AVIF sequence
#[derive(Clone)]
pub struct AnimationFrameRgba {
    /// Frame pixel data (RGBA8)
    pub pixels: ImgVec<RGBA8>,
    /// Duration of this frame in milliseconds
    pub duration_ms: u32,
}

/// Result of animated AVIF encoding
#[non_exhaustive]
#[derive(Clone)]
pub struct EncodedAnimation {
    /// Complete AVIF file bytes
    pub avif_file: Vec<u8>,
    /// Number of frames encoded
    pub frame_count: usize,
    /// Total duration in milliseconds
    pub total_duration_ms: u64,
}

/// Encode a sequence of RGB8 frames into an animated AVIF
///
/// All frames must have the same dimensions. Each frame has its own
/// duration in milliseconds.
///
/// # Arguments
///
/// * `frames` - Sequence of RGB8 frames with durations
/// * `config` - Encoder configuration (quality, speed, etc.)
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_animation_rgb8(
    frames: &[AnimationFrame],
    config: &EncoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<EncodedAnimation> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let enc = build_ravif_encoder(config);

    let ravif_frames: Vec<ravif::AnimFrame<'_>> = frames
        .iter()
        .map(|f| ravif::AnimFrame {
            rgb: f.pixels.as_ref(),
            duration_ms: f.duration_ms,
        })
        .collect();

    let result = enc
        .encode_animation_rgb(&ravif_frames)
        .map_err(|e| at!(Error::Encode(e.to_string())))?;

    Ok(EncodedAnimation {
        avif_file: result.avif_file,
        frame_count: result.frame_count,
        total_duration_ms: result.total_duration_ms,
    })
}

/// Encode a sequence of RGBA8 frames into an animated AVIF
///
/// All frames must have the same dimensions. If any frame has
/// non-opaque alpha, an alpha track is included automatically.
///
/// # Arguments
///
/// * `frames` - Sequence of RGBA8 frames with durations
/// * `config` - Encoder configuration (quality, speed, etc.)
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_animation_rgba8(
    frames: &[AnimationFrameRgba],
    config: &EncoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<EncodedAnimation> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let enc = build_ravif_encoder(config);

    let ravif_frames: Vec<ravif::AnimFrameRgba<'_>> = frames
        .iter()
        .map(|f| ravif::AnimFrameRgba {
            rgba: f.pixels.as_ref(),
            duration_ms: f.duration_ms,
        })
        .collect();

    let result = enc
        .encode_animation_rgba(&ravif_frames)
        .map_err(|e| at!(Error::Encode(e.to_string())))?;

    Ok(EncodedAnimation {
        avif_file: result.avif_file,
        frame_count: result.frame_count,
        total_duration_ms: result.total_duration_ms,
    })
}

/// A single 16-bit RGB frame in an animated AVIF sequence
#[derive(Clone)]
pub struct AnimationFrame16 {
    /// Frame pixel data (RGB16, full 0–65535 range)
    pub pixels: ImgVec<RGB16>,
    /// Duration of this frame in milliseconds
    pub duration_ms: u32,
}

/// A single 16-bit RGBA frame in an animated AVIF sequence
#[derive(Clone)]
pub struct AnimationFrameRgba16 {
    /// Frame pixel data (RGBA16, full 0–65535 range)
    pub pixels: ImgVec<RGBA16>,
    /// Duration of this frame in milliseconds
    pub duration_ms: u32,
}

/// Encode a sequence of 16-bit RGB frames into an animated AVIF (10-bit AV1)
///
/// Input values should be in full u16 range (0–65535), in the image's native
/// transfer function (typically sRGB gamma). Values are scaled to 10-bit
/// internally. All frames must have the same dimensions.
///
/// # Arguments
///
/// * `frames` - Sequence of RGB16 frames with durations (0–65535)
/// * `config` - Encoder configuration (quality, speed, etc.)
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_animation_rgb16(
    frames: &[AnimationFrame16],
    config: &EncoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<EncodedAnimation> {
    use crate::convert::scale_from_u16;
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let enc = build_ravif_encoder(config);

    // Scale each frame from 0–65535 to 10-bit (0–1023)
    let scaled_frames: Vec<ImgVec<RGB16>> = frames
        .iter()
        .map(|f| {
            let scaled: Vec<RGB16> = f
                .pixels
                .buf()
                .iter()
                .map(|p| RGB16 {
                    r: scale_from_u16(p.r, 10),
                    g: scale_from_u16(p.g, 10),
                    b: scale_from_u16(p.b, 10),
                })
                .collect();
            ImgVec::new(scaled, f.pixels.width(), f.pixels.height())
        })
        .collect();

    let ravif_frames: Vec<ravif::AnimFrame16<'_>> = scaled_frames
        .iter()
        .zip(frames.iter())
        .map(|(scaled, orig)| ravif::AnimFrame16 {
            rgb: scaled.as_ref(),
            duration_ms: orig.duration_ms,
        })
        .collect();

    let result = enc
        .encode_animation_rgb16(&ravif_frames)
        .map_err(|e| at!(Error::Encode(e.to_string())))?;

    Ok(EncodedAnimation {
        avif_file: result.avif_file,
        frame_count: result.frame_count,
        total_duration_ms: result.total_duration_ms,
    })
}

/// Encode a sequence of 16-bit RGBA frames into an animated AVIF (10-bit AV1)
///
/// Input values should be in full u16 range (0–65535), in the image's native
/// transfer function (typically sRGB gamma). Values are scaled to 10-bit
/// internally. All frames must have the same dimensions.
///
/// # Arguments
///
/// * `frames` - Sequence of RGBA16 frames with durations (0–65535)
/// * `config` - Encoder configuration (quality, speed, etc.)
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_animation_rgba16(
    frames: &[AnimationFrameRgba16],
    config: &EncoderConfig,
    stop: &(impl Stop + ?Sized),
) -> Result<EncodedAnimation> {
    use crate::convert::scale_from_u16;
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let enc = build_ravif_encoder(config);

    // Scale each frame from 0–65535 to 10-bit (0–1023)
    let scaled_frames: Vec<ImgVec<RGBA16>> = frames
        .iter()
        .map(|f| {
            let scaled: Vec<RGBA16> = f
                .pixels
                .buf()
                .iter()
                .map(|p| RGBA16 {
                    r: scale_from_u16(p.r, 10),
                    g: scale_from_u16(p.g, 10),
                    b: scale_from_u16(p.b, 10),
                    a: scale_from_u16(p.a, 10),
                })
                .collect();
            ImgVec::new(scaled, f.pixels.width(), f.pixels.height())
        })
        .collect();

    let ravif_frames: Vec<ravif::AnimFrameRgba16<'_>> = scaled_frames
        .iter()
        .zip(frames.iter())
        .map(|(scaled, orig)| ravif::AnimFrameRgba16 {
            rgba: scaled.as_ref(),
            duration_ms: orig.duration_ms,
        })
        .collect();

    let result = enc
        .encode_animation_rgba16(&ravif_frames)
        .map_err(|e| at!(Error::Encode(e.to_string())))?;

    Ok(EncodedAnimation {
        avif_file: result.avif_file,
        frame_count: result.frame_count,
        total_duration_ms: result.total_duration_ms,
    })
}
