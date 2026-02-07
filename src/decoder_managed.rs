//! AVIF decoder implementation using rav1d-safe managed API
//!
//! This module provides a 100% safe implementation using the managed API.
//! No unsafe code required!

#![deny(unsafe_code)]

use crate::chroma::{yuv_420, yuv_422, yuv_444};
use crate::config::DecoderConfig;
use crate::convert::{add_alpha8, add_alpha16};
use crate::error::{Error, Result};
use crate::image::{
    ChromaSampling, ColorPrimaries, ColorRange, DecodedImage, ImageInfo, MatrixCoefficients,
    TransferCharacteristics,
};
use enough::Stop;
use imgref::ImgVec;
use rgb::{RGB, prelude::*};
use whereat::at;
use yuv::YUV;
use yuv::color::{Depth, Range};
use yuv::convert::RGBConvert;

// Import managed API from rav1d-safe
use rav1d_safe::src::managed::{
    self as rav1d_managed, ColorInfo as Rav1dColorInfo, ColorPrimaries as Rav1dColorPrimaries,
    ColorRange as Rav1dColorRange, Decoder as Rav1dDecoder, Frame, 
    MatrixCoefficients as Rav1dMatrixCoefficients, PixelLayout, Planes, Settings,
    TransferCharacteristics as Rav1dTransferCharacteristics,
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

/// Convert zenavif MatrixCoefficients to yuv crate's MatrixCoefficients
fn to_yuv_matrix(mc: MatrixCoefficients) -> yuv::color::MatrixCoefficients {
    match mc {
        MatrixCoefficients::IDENTITY => yuv::color::MatrixCoefficients::Identity,
        MatrixCoefficients::BT709 => yuv::color::MatrixCoefficients::BT709,
        MatrixCoefficients::FCC => yuv::color::MatrixCoefficients::FCC,
        MatrixCoefficients::BT470BG => yuv::color::MatrixCoefficients::BT470BG,
        MatrixCoefficients::BT601 => yuv::color::MatrixCoefficients::BT601,
        MatrixCoefficients::SMPTE240 => yuv::color::MatrixCoefficients::SMPTE240,
        MatrixCoefficients::YCGCO => yuv::color::MatrixCoefficients::YCgCo,
        MatrixCoefficients::BT2020_NCL => yuv::color::MatrixCoefficients::BT2020NCL,
        MatrixCoefficients::BT2020_CL => yuv::color::MatrixCoefficients::BT2020CL,
        _ => yuv::color::MatrixCoefficients::BT601, // Default fallback
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
        // Parse AVIF container
        let mut cursor = std::io::Cursor::new(data);
        let avif_data = avif_parse::read_avif(&mut cursor)
            .map_err(|e| at(Error::from(e)))?;

        // Create managed decoder with settings
        let settings = Settings {
            threads: config.threads,
            apply_grain: config.apply_grain,
            frame_size_limit: config.frame_size_limit,
            ..Default::default()
        };

        let decoder = Rav1dDecoder::with_settings(settings)
            .map_err(|e| at(Error::Decode {
                code: -1,
                msg: "Failed to create decoder",
            }))?;

        Ok(Self { decoder, avif_data })
    }

    /// Decode the primary image and optionally alpha channel
    pub fn decode(&mut self, stop: &impl Stop) -> Result<DecodedImage> {
        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Decode primary item
        let primary_frame = self.decoder
            .decode(&self.avif_data.primary_item)
            .map_err(|e| at(Error::Decode {
                code: -1,
                msg: "Failed to decode primary frame",
            }))?
            .ok_or_else(|| at(Error::Decode {
                code: -1,
                msg: "No frame returned from decoder",
            }))?;

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Decode alpha if present
        let alpha_frame = if let Some(ref alpha_data) = self.avif_data.alpha_item {
            Some(self.decoder
                .decode(alpha_data)
                .map_err(|e| at(Error::Decode {
                    code: -1,
                    msg: "Failed to decode alpha frame",
                }))?
                .ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "No alpha frame returned",
                }))?)
        } else {
            None
        };

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Convert to DecodedImage
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

        // Get color info
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

        // Convert based on bit depth
        match bit_depth {
            8 => self.convert_8bit(primary, alpha, info, stop),
            10 | 12 => self.convert_16bit(primary, alpha, info, stop),
            _ => Err(at(Error::Decode {
                code: -1,
                msg: "Unsupported bit depth",
            })),
        }
    }

    /// Convert 8-bit frame to RGB
    /// Convert 8-bit frame to RGB
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

        let width = info.width as usize;
        let height = info.height as usize;

        // Get Y, U, V planes
        let y_plane = planes.y();
        let u_plane = planes.u();
        let v_plane = planes.v();

        // Convert YUV to RGB based on chroma sampling
        let yuv_range = match info.color_range {
            ColorRange::Full => Range::Full,
            ColorRange::Limited => Range::Limited,
        };
        let conv = RGBConvert::<u8>::new(yuv_range, to_yuv_matrix(info.matrix_coefficients))
            .map_err(|e| at(Error::ColorConversion(e)))?;

        let has_alpha = alpha.is_some();

        let mut image = match info.chroma_sampling {
            ChromaSampling::Cs420 => {
                let u = u_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing U plane for 420",
                }))?;
                let v = v_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing V plane for 420",
                }))?;

                let px_iter = yuv_420(y_plane.rows(), u.rows(), v.rows());

                if has_alpha {
                    let mut out = Vec::with_capacity(width * height);
                    out.extend(px_iter.map(|px| conv.to_rgb(px).with_alpha(255)));
                    DecodedImage::Rgba8(ImgVec::new(out, width, height))
                } else {
                    let mut out = Vec::with_capacity(width * height);
                    out.extend(px_iter.map(|px| conv.to_rgb(px)));
                    DecodedImage::Rgb8(ImgVec::new(out, width, height))
                }
            }
            ChromaSampling::Cs422 => {
                let u = u_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing U plane for 422",
                }))?;
                let v = v_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing V plane for 422",
                }))?;

                let px_iter = yuv_422(y_plane.rows(), u.rows(), v.rows());

                if has_alpha {
                    let mut out = Vec::with_capacity(width * height);
                    out.extend(px_iter.map(|px| conv.to_rgb(px).with_alpha(255)));
                    DecodedImage::Rgba8(ImgVec::new(out, width, height))
                } else {
                    let mut out = Vec::with_capacity(width * height);
                    out.extend(px_iter.map(|px| conv.to_rgb(px)));
                    DecodedImage::Rgb8(ImgVec::new(out, width, height))
                }
            }
            ChromaSampling::Cs444 => {
                let u = u_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing U plane for 444",
                }))?;
                let v = v_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing V plane for 444",
                }))?;

                let px_iter = yuv_444(y_plane.rows(), u.rows(), v.rows());

                if has_alpha {
                    let mut out = Vec::with_capacity(width * height);
                    out.extend(px_iter.map(|px| conv.to_rgb(px).with_alpha(255)));
                    DecodedImage::Rgba8(ImgVec::new(out, width, height))
                } else {
                    let mut out = Vec::with_capacity(width * height);
                    out.extend(px_iter.map(|px| conv.to_rgb(px)));
                    DecodedImage::Rgb8(ImgVec::new(out, width, height))
                }
            }
            ChromaSampling::Monochrome => {
                // Grayscale - create RGB from Y only
                if has_alpha {
                    let mut rgb_data = Vec::with_capacity(width * height);
                    for row in y_plane.rows() {
                        for &y in row.iter().take(width) {
                            let gray = conv.to_luma(y);
                            rgb_data.push(RGB::new(gray, gray, gray).with_alpha(255));
                        }
                    }
                    DecodedImage::Rgba8(ImgVec::new(rgb_data, width, height))
                } else {
                    let mut rgb_data = Vec::with_capacity(width * height);
                    for row in y_plane.rows() {
                        for &y in row.iter().take(width) {
                            let gray = conv.to_luma(y);
                            rgb_data.push(RGB::new(gray, gray, gray));
                        }
                    }
                    DecodedImage::Rgb8(ImgVec::new(rgb_data, width, height))
                }
            }
        };

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Handle alpha channel if present
        if let Some(alpha_frame) = alpha {
            // Extract alpha plane and add to image
            let Planes::Depth8(alpha_planes) = alpha_frame.planes() else {
                return Err(at(Error::Decode {
                    code: -1,
                    msg: "Expected 8-bit alpha plane",
                }));
            };

            let alpha_y = alpha_planes.y();
            let alpha_range = if matches!(alpha_frame.color_info().color_range, Rav1dColorRange::Full) {
                Range::Full
            } else {
                Range::Limited
            };

            let alpha_conv = RGBConvert::<u8>::new(alpha_range, yuv::color::MatrixCoefficients::Identity)
                .map_err(|e| at(Error::ColorConversion(e)))?;

            add_alpha8(
                &mut image,
                alpha_y.rows(),
                width,
                height,
                alpha_conv,
                self.avif_data.premultiplied_alpha,
            )?;
        }

        Ok(image)
    }

    /// Convert 10/12-bit frame to RGB
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

        let width = info.width as usize;
        let height = info.height as usize;

        // Get Y, U, V planes
        let y_plane = planes.y();
        let u_plane = planes.u();
        let v_plane = planes.v();

        // Convert YUV to RGB based on chroma sampling
        let yuv_range = match info.color_range {
            ColorRange::Full => Range::Full,
            ColorRange::Limited => Range::Limited,
        };
        let yuv_depth = match info.bit_depth {
            10 => Depth::Depth10,
            12 => Depth::Depth12,
            _ => Depth::Depth16,
        };
        let conv = RGBConvert::<u16>::new(yuv_range, to_yuv_matrix(info.matrix_coefficients), yuv_depth)
            .map_err(|e| at(Error::ColorConversion(e)))?;

        let has_alpha = alpha.is_some();

        let mut image = match info.chroma_sampling {
            ChromaSampling::Cs420 => {
                let u = u_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing U plane for 420",
                }))?;
                let v = v_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing V plane for 420",
                }))?;

                let px_iter = yuv_420(y_plane.rows(), u.rows(), v.rows());

                if has_alpha {
                    let mut out = Vec::with_capacity(width * height);
                    out.extend(px_iter.map(|px| conv.to_rgb(px).with_alpha(0xFFFF)));
                    DecodedImage::Rgba16(ImgVec::new(out, width, height))
                } else {
                    let mut out = Vec::with_capacity(width * height);
                    out.extend(px_iter.map(|px| conv.to_rgb(px)));
                    DecodedImage::Rgb16(ImgVec::new(out, width, height))
                }
            }
            ChromaSampling::Cs422 => {
                let u = u_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing U plane for 422",
                }))?;
                let v = v_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing V plane for 422",
                }))?;

                let px_iter = yuv_422(y_plane.rows(), u.rows(), v.rows());

                if has_alpha {
                    let mut out = Vec::with_capacity(width * height);
                    out.extend(px_iter.map(|px| conv.to_rgb(px).with_alpha(0xFFFF)));
                    DecodedImage::Rgba16(ImgVec::new(out, width, height))
                } else {
                    let mut out = Vec::with_capacity(width * height);
                    out.extend(px_iter.map(|px| conv.to_rgb(px)));
                    DecodedImage::Rgb16(ImgVec::new(out, width, height))
                }
            }
            ChromaSampling::Cs444 => {
                let u = u_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing U plane for 444",
                }))?;
                let v = v_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing V plane for 444",
                }))?;

                let px_iter = yuv_444(y_plane.rows(), u.rows(), v.rows());

                if has_alpha {
                    let mut out = Vec::with_capacity(width * height);
                    out.extend(px_iter.map(|px| conv.to_rgb(px).with_alpha(0xFFFF)));
                    DecodedImage::Rgba16(ImgVec::new(out, width, height))
                } else {
                    let mut out = Vec::with_capacity(width * height);
                    out.extend(px_iter.map(|px| conv.to_rgb(px)));
                    DecodedImage::Rgb16(ImgVec::new(out, width, height))
                }
            }
            ChromaSampling::Monochrome => {
                // Grayscale - create RGB from Y only
                if has_alpha {
                    let mut rgb_data = Vec::with_capacity(width * height);
                    for row in y_plane.rows() {
                        for &y in row.iter().take(width) {
                            let gray = conv.to_luma(y);
                            rgb_data.push(RGB::new(gray, gray, gray).with_alpha(0xFFFF));
                        }
                    }
                    DecodedImage::Rgba16(ImgVec::new(rgb_data, width, height))
                } else {
                    let mut rgb_data = Vec::with_capacity(width * height);
                    for row in y_plane.rows() {
                        for &y in row.iter().take(width) {
                            let gray = conv.to_luma(y);
                            rgb_data.push(RGB::new(gray, gray, gray));
                        }
                    }
                    DecodedImage::Rgb16(ImgVec::new(rgb_data, width, height))
                }
            }
        };

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Handle alpha channel if present
        if let Some(alpha_frame) = alpha {
            // Extract alpha plane and add to image
            let Planes::Depth16(alpha_planes) = alpha_frame.planes() else {
                return Err(at(Error::Decode {
                    code: -1,
                    msg: "Expected 16-bit alpha plane",
                }));
            };

            let alpha_y = alpha_planes.y();
            let alpha_range = if matches!(alpha_frame.color_info().color_range, Rav1dColorRange::Full) {
                Range::Full
            } else {
                Range::Limited
            };

            let alpha_conv = RGBConvert::<u16>::new(alpha_range, yuv::color::MatrixCoefficients::Identity, yuv_depth)
                .map_err(|e| at(Error::ColorConversion(e)))?;

            add_alpha16(
                &mut image,
                alpha_y.rows(),
                width,
                height,
                alpha_conv,
                self.avif_data.premultiplied_alpha,
            )?;
        }

        Ok(image)
    }
}
