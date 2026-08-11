//! Decoded-frame -> pixel-buffer driver.
//!
//! [`ManagedAvifDecoder::convert_to_image`] is the one place a rav1d `Frame`
//! (plus an optional alpha frame) becomes a [`PixelBuffer`]: it applies the
//! CICP precedence, builds the [`ImageInfo`], dispatches by depth / sampling
//! / matrix into the [`super::plane_convert`] kernels, then crops, rescales
//! and attaches alpha.

use super::ManagedAvifDecoder;
use super::cicp_map::{
    convert_chroma_sampling, convert_color_primaries, convert_color_range, convert_matrix,
    convert_transfer, to_yuv_range,
};
use super::plane_convert::{
    ConvertCtx, convert_8bit_identity, convert_8bit_monochrome, convert_8bit_monochrome_gray,
    convert_8bit_planar, convert_16bit_identity, convert_16bit_monochrome,
    convert_16bit_monochrome_gray, convert_16bit_planar,
};
use crate::cicp_resolve::ResolvedMatrix;
use crate::convert::{add_alpha8, add_alpha16, downscale_to_8bit, scale_pixels_to_u16};
use crate::error::{Error, Result};
use crate::image::{ChromaSampling, ColorPrimaries, ImageInfo, TransferCharacteristics};
use enough::Stop;
use rav1d_safe::src::managed::{Frame, PixelLayout, Planes};
use whereat::at;
use zenpixels::PixelBuffer;

impl ManagedAvifDecoder {
    /// Crop an image to the specified dimensions
    fn crop_image(
        image: PixelBuffer,
        width: usize,
        height: usize,
        alloc_pref: crate::alloc_util::AllocPref,
    ) -> Result<PixelBuffer> {
        let descriptor = image.descriptor();
        let bpp = descriptor.bytes_per_pixel();
        let src_w = image.width() as usize;
        let src_h = image.height() as usize;
        let copy_w = width.min(src_w);
        let copy_bytes = copy_w * bpp;

        let alloc_size = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(bpp))
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        // Full crop destination, sized from the display dimensions → fallible
        // by default.
        let mut data = crate::alloc_util::alloc_filled(alloc_pref, true, 0u8, alloc_size)?;
        let src = image.as_slice();
        for y in 0..height.min(src_h) {
            let src_row = src.row(y as u32);
            let dst_start = y * width * bpp;
            data[dst_start..dst_start + copy_bytes].copy_from_slice(&src_row[..copy_bytes]);
        }

        PixelBuffer::from_vec(data, width as u32, height as u32, descriptor).map_err(|_| {
            at!(Error::Decode {
                code: -1,
                msg: "failed to create cropped buffer",
            })
        })
    }

    pub(super) fn convert_to_image(
        &self,
        primary: Frame,
        alpha: Option<Frame>,
        stop: &(impl Stop + ?Sized),
    ) -> Result<(PixelBuffer, ImageInfo)> {
        let width = primary.width() as usize;
        let height = primary.height() as usize;
        let bit_depth = primary.bit_depth();
        let layout = primary.pixel_layout();

        let av1_color = primary.color_info();
        let has_alpha = alpha.is_some();

        // CICP precedence (per MIAF ISO 23000-22 Amd 2):
        //   container colr box > AV1 bitstream > AVIF defaults (1/13/6/full)
        //
        // Matrix coefficients and color range always come from AV1 bitstream
        // because they govern YUV→RGB conversion before any ICC profile applies.
        let matrix_coefficients = convert_matrix(av1_color.matrix_coefficients);
        let color_range = convert_color_range(av1_color.color_range);

        let (color_primaries, transfer_characteristics, icc_profile) =
            match self.parser.color_info() {
                Some(zenavif_parse::ColorInformation::Nclx {
                    color_primaries: cp,
                    transfer_characteristics: tc,
                    ..
                }) => (
                    ColorPrimaries(*cp as u8),
                    TransferCharacteristics(*tc as u8),
                    None,
                ),
                Some(zenavif_parse::ColorInformation::IccProfile(icc)) => {
                    // ICC overrides CP and TC for color management, but we
                    // still populate those fields from AV1 as a fallback
                    (
                        convert_color_primaries(av1_color.primaries),
                        convert_transfer(av1_color.transfer_characteristics),
                        Some(icc.clone()),
                    )
                }
                None => (
                    convert_color_primaries(av1_color.primaries),
                    convert_transfer(av1_color.transfer_characteristics),
                    None,
                ),
            };

        let info = ImageInfo {
            width: width as u32,
            height: height as u32,
            bit_depth,
            has_alpha,
            premultiplied_alpha: self.parser.premultiplied_alpha(),
            monochrome: matches!(layout, PixelLayout::I400),
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            color_range,
            chroma_sampling: convert_chroma_sampling(layout),
            icc_profile,
            rotation: self.parser.rotation().cloned(),
            mirror: self.parser.mirror().cloned(),
            clean_aperture: self.parser.clean_aperture().cloned(),
            pixel_aspect_ratio: self.parser.pixel_aspect_ratio().cloned(),
            content_light_level: self.parser.content_light_level().cloned(),
            mastering_display: self.parser.mastering_display().cloned(),
            exif: self
                .parser
                .exif()
                .and_then(|r| r.ok())
                .map(|c| c.into_owned()),
            xmp: self
                .parser
                .xmp()
                .and_then(|r| r.ok())
                .map(|c| c.into_owned()),
            gain_map: self.extract_gain_map(),
            // Depth map extraction requires zenavif-parse > 0.4.0 (not yet published).
            depth_map: None,
        };

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let info_clone = info.clone();
        let resolved = self.resolved_matrix_for(&info)?;
        let mut pixels = match bit_depth {
            8 => self.convert_8bit(primary, alpha, info, resolved, stop),
            10 | 12 => self.convert_16bit(primary, alpha, info, resolved, stop),
            _ => Err(at!(Error::Unsupported(
                "unsupported bit depth (AV1 spec only defines 8/10/12)"
            ))),
        }?;

        if self.prefer_8bit && bit_depth > 8 {
            pixels = downscale_to_8bit(pixels);
        }

        Ok((pixels, info_clone))
    }

    /// Convert 8-bit frame to RGB using yuv crate bulk conversion (zero-copy)
    fn convert_8bit(
        &self,
        primary: Frame,
        alpha: Option<Frame>,
        info: ImageInfo,
        resolved: ResolvedMatrix,
        stop: &(impl Stop + ?Sized),
    ) -> Result<PixelBuffer> {
        let Planes::Depth8(planes) = primary.planes() else {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "frame reports 8-bit depth but the decoder produced 16-bit planes; \
                       bit depth is inconsistent between the sequence header and the frame",
            }));
        };

        // Use buffer dimensions for YUV conversion (actual buffer size)
        // Then crop to displayed dimensions if needed
        let buffer_width = planes.y().width();
        let buffer_height = planes.y().height();
        let display_width = info.width as usize;
        let display_height = info.height as usize;
        let needs_crop = buffer_width != display_width || buffer_height != display_height;
        let has_alpha = alpha.is_some();
        let yuv_range = to_yuv_range(info.color_range);
        // Placeholder on the identity path: the identity converter never
        // reads `ctx.matrix` (the branch below routes around it).
        let buffer_pixel_count = buffer_width
            .checked_mul(buffer_height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;

        let ctx = ConvertCtx {
            buffer_width,
            buffer_height,
            buffer_pixel_count,
            has_alpha,
            yuv_range,
            alloc_pref: self.alloc_pref,
        };
        let mut image = match (info.chroma_sampling, resolved) {
            (ChromaSampling::Monochrome, _) if self.native_gray && !ctx.has_alpha => {
                convert_8bit_monochrome_gray(&planes, ctx)?
            }
            (ChromaSampling::Monochrome, _) => convert_8bit_monochrome(&planes, ctx)?,
            (ChromaSampling::Cs444, ResolvedMatrix::Identity) => {
                convert_8bit_identity(&planes, ctx)?
            }
            (_, ResolvedMatrix::Identity) => {
                return Err(at!(Error::Unsupported(
                    "matrix_coefficients=0 (identity/GBR) requires 4:4:4 chroma; \
                     subsampled identity has no defined reconstruction"
                )));
            }
            (sampling, _) => convert_8bit_planar(&planes, sampling, &info, resolved, ctx)?,
        };

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        // Crop to display dimensions if needed
        if needs_crop {
            image = Self::crop_image(image, display_width, display_height, self.alloc_pref)?;
        }

        // Handle alpha channel if present
        if let Some(alpha_frame) = alpha {
            let Planes::Depth8(alpha_planes) = alpha_frame.planes() else {
                return Err(at!(Error::Decode {
                    code: -1,
                    msg: "Expected 8-bit alpha plane",
                }));
            };

            let alpha_range = convert_color_range(alpha_frame.color_info().color_range);

            add_alpha8(
                &mut image,
                alpha_planes.y().rows(),
                display_width,
                display_height,
                alpha_range,
                self.parser.premultiplied_alpha(),
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
        resolved: ResolvedMatrix,
        stop: &(impl Stop + ?Sized),
    ) -> Result<PixelBuffer> {
        let Planes::Depth16(planes) = primary.planes() else {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "Expected 16-bit planes",
            }));
        };

        // Use buffer dimensions for YUV conversion (actual buffer size)
        // Then crop to displayed dimensions if needed
        let buffer_width = planes.y().width();
        let buffer_height = planes.y().height();
        let display_width = info.width as usize;
        let display_height = info.height as usize;
        let needs_crop = buffer_width != display_width || buffer_height != display_height;
        let has_alpha = alpha.is_some();
        let yuv_range = to_yuv_range(info.color_range);
        let buffer_pixel_count = buffer_width
            .checked_mul(buffer_height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;

        let ctx = ConvertCtx {
            buffer_width,
            buffer_height,
            buffer_pixel_count,
            has_alpha,
            yuv_range,
            alloc_pref: self.alloc_pref,
        };
        let mut image = match (info.chroma_sampling, resolved) {
            (ChromaSampling::Monochrome, _) if self.native_gray && !ctx.has_alpha => {
                convert_16bit_monochrome_gray(&planes, info.bit_depth, ctx)?
            }
            (ChromaSampling::Monochrome, _) => {
                convert_16bit_monochrome(&planes, info.bit_depth, ctx)?
            }
            (ChromaSampling::Cs444, ResolvedMatrix::Identity) => {
                convert_16bit_identity(&planes, info.bit_depth, ctx)?
            }
            (_, ResolvedMatrix::Identity) => {
                return Err(at!(Error::Unsupported(
                    "matrix_coefficients=0 (identity/GBR) requires 4:4:4 chroma; \
                     subsampled identity has no defined reconstruction"
                )));
            }
            (sampling, _) => {
                convert_16bit_planar(&planes, sampling, info.bit_depth, resolved, ctx)?
            }
        };

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        // Scale from native bit depth (e.g. 0–1023 for 10-bit) to full u16 (0–65535).
        // Must happen before alpha attachment so unpremultiply uses correct 16-bit range.
        scale_pixels_to_u16(&mut image, info.bit_depth);

        // Crop to display dimensions if needed
        if needs_crop {
            image = Self::crop_image(image, display_width, display_height, self.alloc_pref)?;
        }

        // Handle alpha channel if present
        if let Some(alpha_frame) = alpha {
            let Planes::Depth16(alpha_planes) = alpha_frame.planes() else {
                return Err(at!(Error::Decode {
                    code: -1,
                    msg: "Expected 16-bit alpha plane",
                }));
            };

            let alpha_range = convert_color_range(alpha_frame.color_info().color_range);

            add_alpha16(
                &mut image,
                alpha_planes.y().rows(),
                display_width,
                display_height,
                alpha_range,
                info.bit_depth,
                self.parser.premultiplied_alpha(),
            )?;
        }

        Ok(image)
    }
}
