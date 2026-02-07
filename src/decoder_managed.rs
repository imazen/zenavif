//! AVIF decoder implementation using rav1d-safe managed API
//!
//! This module provides a 100% safe implementation using the managed API.
//! No unsafe code required!

#![deny(unsafe_code)]

use crate::config::DecoderConfig;
use crate::convert::{add_alpha8, add_alpha16};
use crate::error::{Error, Result};
use crate::image::{
    ChromaSampling, ColorPrimaries, ColorRange, DecodedImage, ImageInfo, MatrixCoefficients,
    TransferCharacteristics,
};
use enough::Stop;
use imgref::ImgVec;
use rgb::{ComponentBytes, ComponentSlice, Rgb, Rgba};
use whereat::at;
use yuv::{YuvGrayImage, YuvPlanarImage, YuvRange, YuvStandardMatrix};

// Import managed API from rav1d-safe
use rav1d_safe::src::managed::{
    ColorPrimaries as Rav1dColorPrimaries, ColorRange as Rav1dColorRange,
    Decoder as Rav1dDecoder, Frame, MatrixCoefficients as Rav1dMatrixCoefficients, PixelLayout,
    Planes, Settings, TransferCharacteristics as Rav1dTransferCharacteristics,
};

/// Convert rav1d-safe ColorPrimaries to zenavif ColorPrimaries
fn convert_color_primaries(pri: Rav1dColorPrimaries) -> ColorPrimaries {
    match pri {
        Rav1dColorPrimaries::BT709 => ColorPrimaries::BT709,
        Rav1dColorPrimaries::BT2020 => ColorPrimaries::BT2020,
        Rav1dColorPrimaries::BT601 => ColorPrimaries::BT601,
        Rav1dColorPrimaries::SMPTE240 => ColorPrimaries::SMPTE240,
        _ => ColorPrimaries::UNKNOWN,
    }
}

/// Convert rav1d-safe TransferCharacteristics to zenavif
fn convert_transfer(trc: Rav1dTransferCharacteristics) -> TransferCharacteristics {
    match trc {
        Rav1dTransferCharacteristics::BT709 => TransferCharacteristics::BT709,
        Rav1dTransferCharacteristics::SMPTE2084 => TransferCharacteristics::SMPTE2084,
        Rav1dTransferCharacteristics::HLG => TransferCharacteristics::HLG,
        Rav1dTransferCharacteristics::SRGB => TransferCharacteristics::SRGB,
        _ => TransferCharacteristics::UNKNOWN,
    }
}

/// Convert rav1d-safe MatrixCoefficients to zenavif
fn convert_matrix(mtrx: Rav1dMatrixCoefficients) -> MatrixCoefficients {
    match mtrx {
        Rav1dMatrixCoefficients::Identity => MatrixCoefficients::IDENTITY,
        Rav1dMatrixCoefficients::BT709 => MatrixCoefficients::BT709,
        Rav1dMatrixCoefficients::BT2020NCL => MatrixCoefficients::BT2020_NCL,
        Rav1dMatrixCoefficients::BT601 => MatrixCoefficients::BT601,
        _ => MatrixCoefficients::UNKNOWN,
    }
}

/// Convert rav1d-safe ColorRange to zenavif
fn convert_color_range(range: Rav1dColorRange) -> ColorRange {
    match range {
        Rav1dColorRange::Limited => ColorRange::Limited,
        Rav1dColorRange::Full => ColorRange::Full,
    }
}

/// Convert zenavif MatrixCoefficients to yuv crate's YuvStandardMatrix
fn to_yuv_matrix(mc: MatrixCoefficients) -> YuvStandardMatrix {
    match mc {
        MatrixCoefficients::BT709 => YuvStandardMatrix::Bt709,
        MatrixCoefficients::BT601 | MatrixCoefficients::BT470BG | MatrixCoefficients::FCC => {
            YuvStandardMatrix::Bt601
        }
        MatrixCoefficients::BT2020_NCL | MatrixCoefficients::BT2020_CL => {
            YuvStandardMatrix::Bt2020
        }
        MatrixCoefficients::SMPTE240 => YuvStandardMatrix::Smpte240,
        _ => YuvStandardMatrix::Bt601,
    }
}

/// Convert zenavif ColorRange to yuv crate's YuvRange
fn to_yuv_range(range: ColorRange) -> YuvRange {
    match range {
        ColorRange::Full => YuvRange::Full,
        ColorRange::Limited => YuvRange::Limited,
    }
}

/// Convert rav1d-safe PixelLayout to zenavif ChromaSampling
fn convert_chroma_sampling(layout: PixelLayout) -> ChromaSampling {
    match layout {
        PixelLayout::I400 => ChromaSampling::Monochrome,
        PixelLayout::I420 => ChromaSampling::Cs420,
        PixelLayout::I422 => ChromaSampling::Cs422,
        PixelLayout::I444 => ChromaSampling::Cs444,
    }
}

/// Managed decoder wrapper - 100% safe!
pub struct ManagedAvifDecoder {
    decoder: Rav1dDecoder,
    avif_data: avif_parse::AvifData,
}

impl ManagedAvifDecoder {
    /// Create new decoder with AVIF data and configuration
    pub fn new(data: &[u8], config: &DecoderConfig) -> Result<Self> {
        let mut cursor = std::io::Cursor::new(data);
        let avif_data =
            avif_parse::read_avif(&mut cursor).map_err(|e| at(Error::from(e)))?;

        let settings = Settings {
            threads: config.threads,
            apply_grain: config.apply_grain,
            frame_size_limit: config.frame_size_limit,
            ..Default::default()
        };

        let decoder = Rav1dDecoder::with_settings(settings).map_err(|_e| {
            at(Error::Decode {
                code: -1,
                msg: "Failed to create decoder",
            })
        })?;

        Ok(Self { decoder, avif_data })
    }

    /// Decode the primary image and optionally alpha channel
    pub fn decode(&mut self, stop: &impl Stop) -> Result<DecodedImage> {
        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        let primary_frame = self
            .decoder
            .decode(&self.avif_data.primary_item)
            .map_err(|_e| {
                at(Error::Decode {
                    code: -1,
                    msg: "Failed to decode primary frame",
                })
            })?
            .ok_or_else(|| {
                at(Error::Decode {
                    code: -1,
                    msg: "No frame returned from decoder",
                })
            })?;

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        let alpha_frame = if let Some(ref alpha_data) = self.avif_data.alpha_item {
            Some(
                self.decoder
                    .decode(alpha_data)
                    .map_err(|_e| {
                        at(Error::Decode {
                            code: -1,
                            msg: "Failed to decode alpha frame",
                        })
                    })?
                    .ok_or_else(|| {
                        at(Error::Decode {
                            code: -1,
                            msg: "No alpha frame returned",
                        })
                    })?,
            )
        } else {
            None
        };

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        self.convert_to_image(primary_frame, alpha_frame, stop)
    }

    /// Convert rav1d Frame to zenavif DecodedImage
    fn convert_to_image(
        &self,
        primary: Frame,
        alpha: Option<Frame>,
        stop: &impl Stop,
    ) -> Result<DecodedImage> {
        let width = primary.width() as usize;
        let height = primary.height() as usize;
        let bit_depth = primary.bit_depth();
        let layout = primary.pixel_layout();

        let color = primary.color_info();
        let has_alpha = alpha.is_some();
        let info = ImageInfo {
            width: width as u32,
            height: height as u32,
            bit_depth,
            has_alpha,
            premultiplied_alpha: self.avif_data.premultiplied_alpha,
            monochrome: matches!(layout, PixelLayout::I400),
            color_primaries: convert_color_primaries(color.primaries),
            transfer_characteristics: convert_transfer(color.transfer_characteristics),
            matrix_coefficients: convert_matrix(color.matrix_coefficients),
            color_range: convert_color_range(color.color_range),
            chroma_sampling: convert_chroma_sampling(layout),
        };

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        match bit_depth {
            8 => self.convert_8bit(primary, alpha, info, stop),
            10 | 12 => self.convert_16bit(primary, alpha, info, stop),
            _ => Err(at(Error::Decode {
                code: -1,
                msg: "Unsupported bit depth",
            })),
        }
    }

    /// Convert 8-bit frame to RGB using yuv crate bulk conversion (zero-copy)
    fn convert_8bit(
        &self,
        primary: Frame,
        alpha: Option<Frame>,
        info: ImageInfo,
        stop: &impl Stop,
    ) -> Result<DecodedImage> {
        let Planes::Depth8(planes) = primary.planes() else {
            return Err(at(Error::Decode {
                code: -1,
                msg: "Expected 8-bit planes",
            }));
        };

        // Use PlaneView dimensions instead of info metadata
        // The PlaneView height has been corrected to match actual buffer size
        let width = planes.y().width();
        let height = planes.y().height();
        let has_alpha = alpha.is_some();
        let yuv_range = to_yuv_range(info.color_range);
        let matrix = to_yuv_matrix(info.matrix_coefficients);
        let pixel_count = width * height;

        let mut image = match info.chroma_sampling {
            ChromaSampling::Monochrome => {
                let y_view = planes.y();
                let gray = YuvGrayImage {
                    y_plane: y_view.as_slice(),
                    y_stride: y_view.stride() as u32,
                    width: width as u32,
                    height: height as u32,
                };

                if has_alpha {
                    let mut out = vec![Rgba { r: 0u8, g: 0, b: 0, a: 255 }; pixel_count];
                    let rgb_stride = width as u32 * 4;
                    yuv::yuv400_to_rgba(&gray, out.as_mut_slice().as_bytes_mut(), rgb_stride, yuv_range, matrix)
                        .map_err(|e| at(Error::ColorConversion(e)))?;
                    DecodedImage::Rgba8(ImgVec::new(out, width, height))
                } else {
                    let mut out = vec![Rgb { r: 0u8, g: 0, b: 0 }; pixel_count];
                    let rgb_stride = width as u32 * 3;
                    yuv::yuv400_to_rgb(&gray, out.as_mut_slice().as_bytes_mut(), rgb_stride, yuv_range, matrix)
                        .map_err(|e| at(Error::ColorConversion(e)))?;
                    DecodedImage::Rgb8(ImgVec::new(out, width, height))
                }
            }
            sampling => {
                let y_view = planes.y();
                let u_view = planes.u().ok_or_else(|| {
                    at(Error::Decode {
                        code: -1,
                        msg: "Missing U plane",
                    })
                })?;
                let v_view = planes.v().ok_or_else(|| {
                    at(Error::Decode {
                        code: -1,
                        msg: "Missing V plane",
                    })
                })?;

                let planar = YuvPlanarImage {
                    y_plane: y_view.as_slice(),
                    y_stride: y_view.stride() as u32,
                    u_plane: u_view.as_slice(),
                    u_stride: u_view.stride() as u32,
                    v_plane: v_view.as_slice(),
                    v_stride: v_view.stride() as u32,
                    width: width as u32,
                    height: height as u32,
                };

                if has_alpha {
                    let mut out = vec![Rgba { r: 0u8, g: 0, b: 0, a: 255 }; pixel_count];
                    let rgb_stride = width as u32 * 4;
                    match sampling {
                        ChromaSampling::Cs420 => yuv::yuv420_to_rgba(&planar, out.as_mut_slice().as_bytes_mut(), rgb_stride, yuv_range, matrix),
                        ChromaSampling::Cs422 => yuv::yuv422_to_rgba(&planar, out.as_mut_slice().as_bytes_mut(), rgb_stride, yuv_range, matrix),
                        ChromaSampling::Cs444 => yuv::yuv444_to_rgba(&planar, out.as_mut_slice().as_bytes_mut(), rgb_stride, yuv_range, matrix),
                        ChromaSampling::Monochrome => unreachable!(),
                    }
                    .map_err(|e| at(Error::ColorConversion(e)))?;
                    DecodedImage::Rgba8(ImgVec::new(out, width, height))
                } else {
                    let mut out = vec![Rgb { r: 0u8, g: 0, b: 0 }; pixel_count];
                    let rgb_stride = width as u32 * 3;
                    match sampling {
                        ChromaSampling::Cs420 => yuv::yuv420_to_rgb(&planar, out.as_mut_slice().as_bytes_mut(), rgb_stride, yuv_range, matrix),
                        ChromaSampling::Cs422 => yuv::yuv422_to_rgb(&planar, out.as_mut_slice().as_bytes_mut(), rgb_stride, yuv_range, matrix),
                        ChromaSampling::Cs444 => yuv::yuv444_to_rgb(&planar, out.as_mut_slice().as_bytes_mut(), rgb_stride, yuv_range, matrix),
                        ChromaSampling::Monochrome => unreachable!(),
                    }
                    .map_err(|e| at(Error::ColorConversion(e)))?;
                    DecodedImage::Rgb8(ImgVec::new(out, width, height))
                }
            }
        };

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Handle alpha channel if present
        if let Some(alpha_frame) = alpha {
            let Planes::Depth8(alpha_planes) = alpha_frame.planes() else {
                return Err(at(Error::Decode {
                    code: -1,
                    msg: "Expected 8-bit alpha plane",
                }));
            };

            let alpha_range = convert_color_range(alpha_frame.color_info().color_range);

            add_alpha8(
                &mut image,
                alpha_planes.y().rows(),
                width,
                height,
                alpha_range,
                self.avif_data.premultiplied_alpha,
            )?;
        }

        Ok(image)
    }

    /// Convert 10/12-bit frame to RGB using yuv crate bulk conversion (zero-copy)
    fn convert_16bit(
        &self,
        primary: Frame,
        alpha: Option<Frame>,
        info: ImageInfo,
        stop: &impl Stop,
    ) -> Result<DecodedImage> {
        let Planes::Depth16(planes) = primary.planes() else {
            return Err(at(Error::Decode {
                code: -1,
                msg: "Expected 16-bit planes",
            }));
        };

        // Use PlaneView dimensions instead of info metadata
        // The PlaneView height has been corrected to match actual buffer size
        let width = planes.y().width();
        let height = planes.y().height();
        let has_alpha = alpha.is_some();
        let yuv_range = to_yuv_range(info.color_range);
        let matrix = to_yuv_matrix(info.matrix_coefficients);
        let pixel_count = width * height;

        let mut image = match info.chroma_sampling {
            ChromaSampling::Monochrome => {
                let y_view = planes.y();
                let gray = YuvGrayImage {
                    y_plane: y_view.as_slice(),
                    y_stride: y_view.stride() as u32,
                    width: width as u32,
                    height: height as u32,
                };

                if has_alpha {
                    let mut out = vec![Rgba { r: 0u16, g: 0, b: 0, a: 0xFFFF }; pixel_count];
                    let rgb_stride = width as u32 * 4;
                    match info.bit_depth {
                        10 => yuv::y010_to_rgba10(&gray, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        12 => yuv::y012_to_rgba12(&gray, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        _ => yuv::y016_to_rgba16(&gray, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                    }
                    .map_err(|e| at(Error::ColorConversion(e)))?;
                    DecodedImage::Rgba16(ImgVec::new(out, width, height))
                } else {
                    let mut out = vec![Rgb { r: 0u16, g: 0, b: 0 }; pixel_count];
                    let rgb_stride = width as u32 * 3;
                    match info.bit_depth {
                        10 => yuv::y010_to_rgb10(&gray, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        12 => yuv::y012_to_rgb12(&gray, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        _ => yuv::y016_to_rgb16(&gray, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                    }
                    .map_err(|e| at(Error::ColorConversion(e)))?;
                    DecodedImage::Rgb16(ImgVec::new(out, width, height))
                }
            }
            sampling => {
                let y_view = planes.y();
                let u_view = planes.u().ok_or_else(|| {
                    at(Error::Decode {
                        code: -1,
                        msg: "Missing U plane",
                    })
                })?;
                let v_view = planes.v().ok_or_else(|| {
                    at(Error::Decode {
                        code: -1,
                        msg: "Missing V plane",
                    })
                })?;

                let planar = YuvPlanarImage {
                    y_plane: y_view.as_slice(),
                    y_stride: y_view.stride() as u32,
                    u_plane: u_view.as_slice(),
                    u_stride: u_view.stride() as u32,
                    v_plane: v_view.as_slice(),
                    v_stride: v_view.stride() as u32,
                    width: width as u32,
                    height: height as u32,
                };

                if has_alpha {
                    let mut out = vec![Rgba { r: 0u16, g: 0, b: 0, a: 0xFFFF }; pixel_count];
                    let rgb_stride = width as u32 * 4;
                    match (info.bit_depth, sampling) {
                        (10, ChromaSampling::Cs420) => yuv::i010_to_rgba10(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (10, ChromaSampling::Cs422) => yuv::i210_to_rgba10(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (10, ChromaSampling::Cs444) => yuv::i410_to_rgba10(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (12, ChromaSampling::Cs420) => yuv::i012_to_rgba12(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (12, ChromaSampling::Cs422) => yuv::i212_to_rgba12(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (12, ChromaSampling::Cs444) => yuv::i412_to_rgba12(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (_, ChromaSampling::Cs420) => yuv::i016_to_rgba16(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (_, ChromaSampling::Cs422) => yuv::i216_to_rgba16(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (_, ChromaSampling::Cs444) => yuv::i416_to_rgba16(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (_, ChromaSampling::Monochrome) => unreachable!(),
                    }
                    .map_err(|e| at(Error::ColorConversion(e)))?;
                    DecodedImage::Rgba16(ImgVec::new(out, width, height))
                } else {
                    let mut out = vec![Rgb { r: 0u16, g: 0, b: 0 }; pixel_count];
                    let rgb_stride = width as u32 * 3;
                    match (info.bit_depth, sampling) {
                        (10, ChromaSampling::Cs420) => yuv::i010_to_rgb10(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (10, ChromaSampling::Cs422) => yuv::i210_to_rgb10(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (10, ChromaSampling::Cs444) => yuv::i410_to_rgb10(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (12, ChromaSampling::Cs420) => yuv::i012_to_rgb12(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (12, ChromaSampling::Cs422) => yuv::i212_to_rgb12(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (12, ChromaSampling::Cs444) => yuv::i412_to_rgb12(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (_, ChromaSampling::Cs420) => yuv::i016_to_rgb16(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (_, ChromaSampling::Cs422) => yuv::i216_to_rgb16(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (_, ChromaSampling::Cs444) => yuv::i416_to_rgb16(&planar, out.as_mut_slice().as_mut_slice(), rgb_stride, yuv_range, matrix),
                        (_, ChromaSampling::Monochrome) => unreachable!(),
                    }
                    .map_err(|e| at(Error::ColorConversion(e)))?;
                    DecodedImage::Rgb16(ImgVec::new(out, width, height))
                }
            }
        };

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Handle alpha channel if present
        if let Some(alpha_frame) = alpha {
            let Planes::Depth16(alpha_planes) = alpha_frame.planes() else {
                return Err(at(Error::Decode {
                    code: -1,
                    msg: "Expected 16-bit alpha plane",
                }));
            };

            let alpha_range = convert_color_range(alpha_frame.color_info().color_range);

            add_alpha16(
                &mut image,
                alpha_planes.y().rows(),
                width,
                height,
                alpha_range,
                info.bit_depth as u8,
                self.avif_data.premultiplied_alpha,
            )?;
        }

        Ok(image)
    }
}
