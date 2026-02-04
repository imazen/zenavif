//! Decoded image types and metadata

use imgref::ImgVec;
use rgb::{Rgb, Rgba};

/// A decoded AVIF image
#[derive(Debug)]
#[non_exhaustive]
pub enum DecodedImage {
    /// 8-bit RGB image
    Rgb8(ImgVec<Rgb<u8>>),
    /// 8-bit RGBA image
    Rgba8(ImgVec<Rgba<u8>>),
    /// 16-bit RGB image (10/12-bit expanded to 16-bit)
    Rgb16(ImgVec<Rgb<u16>>),
    /// 16-bit RGBA image (10/12-bit expanded to 16-bit)
    Rgba16(ImgVec<Rgba<u16>>),
    /// 8-bit grayscale image
    Gray8(ImgVec<u8>),
    /// 16-bit grayscale image
    Gray16(ImgVec<u16>),
}

impl DecodedImage {
    /// Get the width of the decoded image
    pub fn width(&self) -> usize {
        match self {
            DecodedImage::Rgb8(img) => img.width(),
            DecodedImage::Rgba8(img) => img.width(),
            DecodedImage::Rgb16(img) => img.width(),
            DecodedImage::Rgba16(img) => img.width(),
            DecodedImage::Gray8(img) => img.width(),
            DecodedImage::Gray16(img) => img.width(),
        }
    }

    /// Get the height of the decoded image
    pub fn height(&self) -> usize {
        match self {
            DecodedImage::Rgb8(img) => img.height(),
            DecodedImage::Rgba8(img) => img.height(),
            DecodedImage::Rgb16(img) => img.height(),
            DecodedImage::Rgba16(img) => img.height(),
            DecodedImage::Gray8(img) => img.height(),
            DecodedImage::Gray16(img) => img.height(),
        }
    }

    /// Returns true if the image has an alpha channel
    pub fn has_alpha(&self) -> bool {
        matches!(self, DecodedImage::Rgba8(_) | DecodedImage::Rgba16(_))
    }

    /// Returns true if the image is grayscale (monochrome)
    pub fn is_grayscale(&self) -> bool {
        matches!(self, DecodedImage::Gray8(_) | DecodedImage::Gray16(_))
    }

    /// Returns the bit depth of the image (8 or 16)
    pub fn bit_depth(&self) -> u8 {
        match self {
            DecodedImage::Rgb8(_) | DecodedImage::Rgba8(_) | DecodedImage::Gray8(_) => 8,
            DecodedImage::Rgb16(_) | DecodedImage::Rgba16(_) | DecodedImage::Gray16(_) => 16,
        }
    }
}

/// Chroma subsampling format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaSampling {
    /// 4:2:0 - chroma is half resolution in both dimensions
    Cs420,
    /// 4:2:2 - chroma is half resolution horizontally
    Cs422,
    /// 4:4:4 - no chroma subsampling
    Cs444,
    /// Monochrome (no chroma)
    Monochrome,
}

/// Color primaries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorPrimaries(pub u8);

impl ColorPrimaries {
    pub const BT709: Self = Self(1);
    pub const UNKNOWN: Self = Self(2);
    pub const BT470M: Self = Self(4);
    pub const BT470BG: Self = Self(5);
    pub const BT601: Self = Self(6);
    pub const SMPTE240: Self = Self(7);
    pub const FILM: Self = Self(8);
    pub const BT2020: Self = Self(9);
    pub const XYZ: Self = Self(10);
    pub const SMPTE431: Self = Self(11);
    pub const SMPTE432: Self = Self(12);
    pub const EBU3213: Self = Self(22);
}

/// Transfer characteristics (gamma curve)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransferCharacteristics(pub u8);

impl TransferCharacteristics {
    pub const BT709: Self = Self(1);
    pub const UNKNOWN: Self = Self(2);
    pub const BT470M: Self = Self(4);
    pub const BT470BG: Self = Self(5);
    pub const BT601: Self = Self(6);
    pub const SMPTE240: Self = Self(7);
    pub const LINEAR: Self = Self(8);
    pub const LOG100: Self = Self(9);
    pub const LOG100_SQRT10: Self = Self(10);
    pub const IEC61966: Self = Self(11);
    pub const BT1361: Self = Self(12);
    pub const SRGB: Self = Self(13);
    pub const BT2020_10BIT: Self = Self(14);
    pub const BT2020_12BIT: Self = Self(15);
    pub const SMPTE2084: Self = Self(16);
    pub const SMPTE428: Self = Self(17);
    pub const HLG: Self = Self(18);
}

/// Matrix coefficients for YUV to RGB conversion
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MatrixCoefficients(pub u8);

impl MatrixCoefficients {
    pub const IDENTITY: Self = Self(0);
    pub const BT709: Self = Self(1);
    pub const UNKNOWN: Self = Self(2);
    pub const FCC: Self = Self(4);
    pub const BT470BG: Self = Self(5);
    pub const BT601: Self = Self(6);
    pub const SMPTE240: Self = Self(7);
    pub const YCGCO: Self = Self(8);
    pub const BT2020_NCL: Self = Self(9);
    pub const BT2020_CL: Self = Self(10);
    pub const SMPTE2085: Self = Self(11);
    pub const CHROMAT_NCL: Self = Self(12);
    pub const CHROMAT_CL: Self = Self(13);
    pub const ICTCP: Self = Self(14);
}

/// Color range
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorRange {
    /// Limited/studio range (Y: 16-235, UV: 16-240 for 8-bit)
    #[default]
    Limited,
    /// Full range (0-255 for 8-bit)
    Full,
}

/// Metadata about the decoded image
#[derive(Debug, Clone)]
pub struct ImageInfo {
    /// Image width in pixels
    pub width: u32,
    /// Image height in pixels
    pub height: u32,
    /// Original bit depth (8, 10, or 12)
    pub bit_depth: u8,
    /// Whether the image has an alpha channel
    pub has_alpha: bool,
    /// Whether alpha is premultiplied
    pub premultiplied_alpha: bool,
    /// Whether the image is monochrome
    pub monochrome: bool,
    /// Color primaries
    pub color_primaries: ColorPrimaries,
    /// Transfer characteristics
    pub transfer_characteristics: TransferCharacteristics,
    /// Matrix coefficients
    pub matrix_coefficients: MatrixCoefficients,
    /// Color range (limited or full)
    pub color_range: ColorRange,
    /// Chroma subsampling
    pub chroma_sampling: ChromaSampling,
}

impl Default for ImageInfo {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            bit_depth: 8,
            has_alpha: false,
            premultiplied_alpha: false,
            monochrome: false,
            color_primaries: ColorPrimaries::default(),
            transfer_characteristics: TransferCharacteristics::default(),
            matrix_coefficients: MatrixCoefficients::default(),
            color_range: ColorRange::default(),
            chroma_sampling: ChromaSampling::Cs420,
        }
    }
}
