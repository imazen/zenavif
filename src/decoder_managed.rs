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
use rgb::prelude::*;
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
        _ => ColorPrimaries::Unspecified,
    }
}

/// Convert rav1d-safe TransferCharacteristics to zenavif
fn convert_transfer(trc: Rav1dTransferCharacteristics) -> TransferCharacteristics {
    match trc {
        Rav1dTransferCharacteristics::BT709 => TransferCharacteristics::BT709,
        Rav1dTransferCharacteristics::SMPTE2084 => TransferCharacteristics::SMPTE2084,
        Rav1dTransferCharacteristics::HLG => TransferCharacteristics::HLG,
        Rav1dTransferCharacteristics::SRGB => TransferCharacteristics::SRGB,
        _ => TransferCharacteristics::Unspecified,
    }
}

/// Convert rav1d-safe MatrixCoefficients to zenavif
fn convert_matrix(mtrx: Rav1dMatrixCoefficients) -> MatrixCoefficients {
    match mtrx {
        Rav1dMatrixCoefficients::Identity => MatrixCoefficients::Identity,
        Rav1dMatrixCoefficients::BT709 => MatrixCoefficients::BT709,
        Rav1dMatrixCoefficients::BT2020NCL => MatrixCoefficients::BT2020NCL,
        Rav1dMatrixCoefficients::BT601 => MatrixCoefficients::BT601,
        _ => MatrixCoefficients::Unspecified,
    }
}

/// Convert rav1d-safe ColorRange to zenavif
fn convert_color_range(range: Rav1dColorRange) -> ColorRange {
    match range {
        Rav1dColorRange::Limited => ColorRange::Limited,
        Rav1dColorRange::Full => ColorRange::Full,
    }
}

/// Convert rav1d-safe PixelLayout to zenavif ChromaSampling
fn convert_chroma_sampling(layout: PixelLayout) -> ChromaSampling {
    match layout {
        PixelLayout::I400 => ChromaSampling::Mono,
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
            .map_err(|e| at(Error::Parse(format!("Failed to parse AVIF: {}", e))))?;

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
        stop.check()?;

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

        stop.check()?;

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

        stop.check()?;

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
        let info = ImageInfo {
            width,
            height,
            bit_depth,
            chroma_sampling: convert_chroma_sampling(layout),
            primaries: convert_color_primaries(color.primaries),
            transfer: convert_transfer(color.transfer_characteristics),
            matrix: convert_matrix(color.matrix_coefficients),
            full_range: matches!(color.color_range, Rav1dColorRange::Full),
        };

        stop.check()?;

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

        let width = info.width;
        let height = info.height;

        // Get Y, U, V planes
        let y_plane = planes.y();
        let u_plane = planes.u();
        let v_plane = planes.v();

        // Convert YUV to RGB based on chroma sampling
        let yuv_depth = Depth::Depth8;
        let yuv_range = if info.full_range { Range::Full } else { Range::Limited };

        let rgb_img = match info.chroma_sampling {
            ChromaSampling::Cs420 => {
                let u = u_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing U plane for 420",
                }))?;
                let v = v_plane.ok_or_else(|| at(Error::Decode {
                    code: -1,
                    msg: "Missing V plane for 420",
                }))?;

                yuv_420(
                    y_plane.as_slice(),
                    y_plane.stride(),
                    u.as_slice(),
                    u.stride(),
                    v.as_slice(),
                    v.stride(),
                    width,
                    height,
                    yuv_depth,
                    yuv_range,
                    stop,
                )?
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

                yuv_422(
                    y_plane.as_slice(),
                    y_plane.stride(),
                    u.as_slice(),
                    u.stride(),
                    v.as_slice(),
                    v.stride(),
                    width,
                    height,
                    yuv_depth,
                    yuv_range,
                    stop,
                )?
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

                yuv_444(
                    y_plane.as_slice(),
                    y_plane.stride(),
                    u.as_slice(),
                    u.stride(),
                    v.as_slice(),
                    v.stride(),
                    width,
                    height,
                    yuv_depth,
                    yuv_range,
                    stop,
                )?
            }
            ChromaSampling::Mono => {
                // Grayscale - create RGB from Y only
                let mut rgb_data = Vec::with_capacity(width * height * 3);
                for row in y_plane.rows() {
                    for &y in &row[..width] {
                        rgb_data.push(y);
                        rgb_data.push(y);
                        rgb_data.push(y);
                    }
                }
                ImgVec::new(rgb_data, width, height)
            }
        };

        stop.check()?;

        // Handle alpha channel if present
        if let Some(alpha_frame) = alpha {
            let alpha_img = self.extract_alpha_8bit(alpha_frame, width, height)?;
            let rgba_img = add_alpha8(rgb_img, alpha_img, self.avif_data.premultiplied_alpha)?;
            Ok(DecodedImage::Rgba8(rgba_img).with_info(info))
        } else {
            Ok(DecodedImage::Rgb8(rgb_img).with_info(info))
        }
    }

    /// Extract alpha plane from 8-bit frame
    fn extract_alpha_8bit(&self, alpha_frame: Frame, width: usize, height: usize) -> Result<ImgVec<u8>> {
        let Planes::Depth8(planes) = alpha_frame.planes() else {
            return Err(at(Error::Decode {
                code: -1,
                msg: "Expected 8-bit alpha planes",
            }));
        };

        let y_plane = planes.y();
        let mut alpha_data = Vec::with_capacity(width * height);

        for row in y_plane.rows() {
            alpha_data.extend_from_slice(&row[..width]);
        }

        Ok(ImgVec::new(alpha_data, width, height))
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

        let width = info.width;
        let height = info.height;

        // Get Y, U, V planes
        let y_plane = planes.y();
        let u_plane = planes.u();
        let v_plane = planes.v();

        // For 16-bit, we need to handle the conversion differently
        // Similar logic to 8-bit but with u16 data
        // This is simplified - full implementation would use yuv crate properly

        stop.check()?;

        // For now, return error - full 16-bit support needs more work
        Err(at(Error::Decode {
            code: -1,
            msg: "16-bit RGB conversion not yet implemented in managed decoder",
        }))
    }
}
