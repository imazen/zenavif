//! AVIF image metadata types

pub use zenavif_parse::{
    CleanAperture, ContentLightLevel, ImageMirror, ImageRotation, MasteringDisplayColourVolume,
    PixelAspectRatio,
};

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
    /// ICC color profile from the container's `colr` box, if present
    pub icc_profile: Option<Vec<u8>>,
    /// Image rotation from the container's `irot` property
    pub rotation: Option<ImageRotation>,
    /// Image mirror from the container's `imir` property
    pub mirror: Option<ImageMirror>,
    /// Clean aperture (crop) from the container's `clap` property
    pub clean_aperture: Option<CleanAperture>,
    /// Pixel aspect ratio from the container's `pasp` property
    pub pixel_aspect_ratio: Option<PixelAspectRatio>,
    /// Content light level from the container's `clli` property
    pub content_light_level: Option<ContentLightLevel>,
    /// Mastering display colour volume from the container's `mdcv` property
    pub mastering_display: Option<MasteringDisplayColourVolume>,
    /// EXIF metadata (TIFF header onwards, AVIF offset prefix stripped)
    pub exif: Option<Vec<u8>>,
    /// XMP metadata (raw XML)
    pub xmp: Option<Vec<u8>>,
}

/// A single decoded frame from an animated AVIF sequence.
#[derive(Debug)]
pub struct DecodedFrame {
    /// Decoded pixel data for this frame.
    pub pixels: zencodec_types::PixelData,
    /// Duration of this frame in milliseconds.
    pub duration_ms: u32,
}

/// Metadata about a decoded animation.
#[derive(Debug, Clone)]
pub struct DecodedAnimationInfo {
    /// Number of frames in the animation.
    pub frame_count: usize,
    /// Number of times to loop (0 = infinite).
    pub loop_count: u32,
    /// Whether the animation has alpha.
    pub has_alpha: bool,
    /// Media timescale (ticks per second) of the color track.
    pub timescale: u32,
}

/// A fully decoded animation: frames + metadata.
#[derive(Debug)]
pub struct DecodedAnimation {
    /// Decoded frames in presentation order.
    pub frames: Vec<DecodedFrame>,
    /// Animation metadata.
    pub info: DecodedAnimationInfo,
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
            icc_profile: None,
            rotation: None,
            mirror: None,
            clean_aperture: None,
            pixel_aspect_ratio: None,
            content_light_level: None,
            mastering_display: None,
            exif: None,
            xmp: None,
        }
    }
}
