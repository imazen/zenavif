//! Enum-to-enum CICP mapping between rav1d-safe's managed types, zenavif's
//! own [`crate::image`] types, and the `yuv` crate's range type.
//!
//! Pure translation: no decoder state, no I/O, no math.

use crate::image::{
    ChromaSampling, ColorPrimaries, ColorRange, MatrixCoefficients, TransferCharacteristics,
};
use crate::yuv_convert::YuvRange as OurYuvRange;
use rav1d_safe::src::managed::{
    ColorPrimaries as Rav1dColorPrimaries, ColorRange as Rav1dColorRange,
    MatrixCoefficients as Rav1dMatrixCoefficients, PixelLayout,
    TransferCharacteristics as Rav1dTransferCharacteristics,
};
use yuv::YuvRange;

/// Convert rav1d-safe ColorPrimaries to zenavif ColorPrimaries
pub(super) fn convert_color_primaries(pri: Rav1dColorPrimaries) -> ColorPrimaries {
    match pri {
        Rav1dColorPrimaries::BT709 => ColorPrimaries::BT709,
        Rav1dColorPrimaries::BT2020 => ColorPrimaries::BT2020,
        Rav1dColorPrimaries::BT601 => ColorPrimaries::BT601,
        Rav1dColorPrimaries::SMPTE240 => ColorPrimaries::SMPTE240,
        _ => ColorPrimaries::UNKNOWN,
    }
}

/// Convert rav1d-safe TransferCharacteristics to zenavif
pub(super) fn convert_transfer(trc: Rav1dTransferCharacteristics) -> TransferCharacteristics {
    match trc {
        Rav1dTransferCharacteristics::BT709 => TransferCharacteristics::BT709,
        Rav1dTransferCharacteristics::SMPTE2084 => TransferCharacteristics::SMPTE2084,
        Rav1dTransferCharacteristics::HLG => TransferCharacteristics::HLG,
        Rav1dTransferCharacteristics::SRGB => TransferCharacteristics::SRGB,
        _ => TransferCharacteristics::UNKNOWN,
    }
}

/// Convert rav1d-safe MatrixCoefficients to zenavif
pub(super) fn convert_matrix(mtrx: Rav1dMatrixCoefficients) -> MatrixCoefficients {
    match mtrx {
        Rav1dMatrixCoefficients::Identity => MatrixCoefficients::IDENTITY,
        Rav1dMatrixCoefficients::BT709 => MatrixCoefficients::BT709,
        Rav1dMatrixCoefficients::BT2020NCL => MatrixCoefficients::BT2020_NCL,
        Rav1dMatrixCoefficients::BT601 => MatrixCoefficients::BT601,
        _ => MatrixCoefficients::UNKNOWN,
    }
}

/// Convert rav1d-safe ColorRange to zenavif
pub(super) fn convert_color_range(range: Rav1dColorRange) -> ColorRange {
    match range {
        Rav1dColorRange::Limited => ColorRange::Limited,
        Rav1dColorRange::Full => ColorRange::Full,
    }
}

// The former `to_yuv_matrix` / `to_our_yuv_matrix` blind converters
// (`_ => Bt601` on identity/CL/ICtCp/unspecified — silent chroma
// corruption, imazen/zenavif#15) are replaced by
// `crate::cicp_resolve::resolve`, which resolves H.273 code points
// honestly (identity passthrough, hint for unspecified, derivation for
// MC=12/13, loud errors for unimplemented math).

/// Convert zenavif ColorRange to our YuvRange
pub(super) fn to_our_yuv_range(cr: ColorRange) -> OurYuvRange {
    match cr {
        ColorRange::Limited => OurYuvRange::Limited,
        ColorRange::Full => OurYuvRange::Full,
    }
}

/// Convert zenavif ColorRange to yuv crate's YuvRange
pub(super) fn to_yuv_range(range: ColorRange) -> YuvRange {
    match range {
        ColorRange::Full => YuvRange::Full,
        ColorRange::Limited => YuvRange::Limited,
    }
}

/// Convert rav1d-safe PixelLayout to zenavif ChromaSampling
pub(super) fn convert_chroma_sampling(layout: PixelLayout) -> ChromaSampling {
    match layout {
        PixelLayout::I400 => ChromaSampling::Monochrome,
        PixelLayout::I420 => ChromaSampling::Cs420,
        PixelLayout::I422 => ChromaSampling::Cs422,
        PixelLayout::I444 => ChromaSampling::Cs444,
    }
}
