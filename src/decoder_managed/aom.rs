//! The `zenav1-aom`-gated still/grid decode path.
//!
//! Everything in this file is behind `#[cfg(feature = "zenav1-aom")]` via
//! the `mod aom;` declaration in [`super`], so the items themselves carry no
//! feature gates.

use super::ManagedAvifDecoder;
use crate::cicp_resolve::ResolvedMatrix;
use crate::convert::{add_alpha8, add_alpha16, downscale_to_8bit, scale_pixels_to_u16};
use crate::error::{Error, Result};
use crate::image::{
    ChromaSampling, ColorPrimaries, ColorRange, ImageInfo, MatrixCoefficients,
    TransferCharacteristics,
};
use crate::yuv_convert::{YuvMatrix as OurYuvMatrix, YuvRange as OurYuvRange};
use enough::Stop;
use rgb::{Rgb, Rgba};
use whereat::at;
use zenpixels::PixelBuffer;

/// The aom-backed still decode path (`DecoderConfig::decode_backend =
/// Zenav1Aom`). Item payloads decode through `zenav1-aom` instead of rav1d-safe;
/// conversion runs the SAME in-house `yuv_convert` kernel family the rav1d
/// path uses (one canonical recipe → byte-identical output by construction,
/// pinned by `tests/product_aom_backend.rs`). Scope: non-grid stills;
/// grid images and animation return honest `Unsupported` until the aom
/// inter/grid envelopes land.
impl ManagedAvifDecoder {
    fn decode_item_aom(
        &self,
        data: &[u8],
        stop: &(impl Stop + ?Sized),
    ) -> Result<aom_decode::frame::FrameDecode> {
        let config = crate::DecoderConfig {
            frame_size_limit: self.frame_size_limit,
            alloc_pref: self.alloc_pref,
            ..crate::DecoderConfig::default()
        };
        let aom_config = crate::decode_av1::aom_config_from(&config).with_stop(&stop);
        aom_decode::frame::decode_frame_obus_with(data, &aom_config)
            .map_err(crate::decode_av1::map_aom_error)
    }

    pub(super) fn decode_full_aom(
        &mut self,
        stop: &(impl Stop + ?Sized),
    ) -> Result<(PixelBuffer, ImageInfo)> {
        if self.parser.grid_config().is_some() {
            let pixels = self.decode_grid_aom(stop)?;
            let info = self.probe_info()?;
            return Ok((pixels, info));
        }
        let primary_data = self
            .parser
            .primary_data()
            .map_err(|e| e.map_error(Error::Parse))?;
        let fd = self.decode_item_aom(&primary_data, stop)?;
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;
        let fd_alpha = if let Some(alpha_result) = self.parser.alpha_data() {
            let alpha_data = alpha_result.map_err(|e| e.map_error(Error::Parse))?;
            Some(self.decode_item_aom(&alpha_data, stop)?)
        } else {
            None
        };
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;
        self.convert_aom_to_image(fd, fd_alpha, stop)
    }

    /// Grid (tiled) AVIF via the aom backend: every grid cell is an
    /// independently-coded AV1 still — decode each item with zenav1-aom,
    /// convert with the shared aom conversion path, and byte-stitch into
    /// the grid canvas exactly like the rav1d grid path (same
    /// `stitch_tile_images` helper). (Container `grid` items are unrelated
    /// to AV1 bitstream tiles, which the decoder handles internally.)
    fn decode_grid_aom(&mut self, stop: &(impl Stop + ?Sized)) -> Result<PixelBuffer> {
        let grid_config = self
            .parser
            .grid_config()
            .ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Expected grid config but found none",
                })
            })?
            .clone();
        self.reject_grid_alpha()?;
        let rows = grid_config.rows as usize;
        let cols = grid_config.columns as usize;
        let tile_count = self.parser.grid_tile_count();
        if tile_count != rows * cols {
            return Err(at!(Error::Malformed(
                "Tile count doesn't match grid dimensions"
            )));
        }
        if tile_count == 0 {
            return Err(at!(Error::Malformed("No tiles to stitch")));
        }
        let mut tile_images = Vec::new();
        let mut tile_dims = (0usize, 0usize);
        for i in 0..tile_count {
            stop.check().map_err(|e| at!(Error::Cancelled(e)))?;
            let tile_data = self
                .parser
                .tile_data(i)
                .map_err(|e| e.map_error(Error::Parse))?;
            let fd = self.decode_item_aom(&tile_data, stop)?;
            if i == 0 {
                tile_dims = (fd.width, fd.height);
            }
            let (img, _info) = self.convert_aom_to_image(fd, None, stop)?;
            tile_images.push(img);
        }
        let output_width = if grid_config.output_width > 0 {
            grid_config.output_width as usize
        } else {
            tile_dims.0 * cols
        };
        let output_height = if grid_config.output_height > 0 {
            grid_config.output_height as usize
        } else {
            tile_dims.1 * rows
        };
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;
        self.stitch_tile_images(tile_images, cols, output_width, output_height)
    }

    /// Mirror of `convert_to_image` over an aom `FrameDecode`: identical CICP
    /// precedence (container colr > AV1 bitstream > AVIF defaults; MC + range
    /// always from the bitstream), identical `ImageInfo` shape, and the same
    /// in-house kernels — driven through the depth-generic u16 entries
    /// (`FrameDecode` planes are u16 at every depth; the canonical recipe is
    /// depth-parameterized, so d=8 through the u16 kernels is byte-identical
    /// to the u8 kernels the rav1d path runs).
    pub(super) fn convert_aom_to_image(
        &self,
        fd: aom_decode::frame::FrameDecode,
        fd_alpha: Option<aom_decode::frame::FrameDecode>,
        stop: &(impl Stop + ?Sized),
    ) -> Result<(PixelBuffer, ImageInfo)> {
        let (width, height) = (fd.width, fd.height);
        let bit_depth = fd.bit_depth as u8;
        if !matches!(bit_depth, 8 | 10 | 12) {
            return Err(at!(Error::Unsupported(
                "unsupported bit depth (AV1 spec only defines 8/10/12)"
            )));
        }
        let has_alpha = fd_alpha.is_some();
        let chroma_sampling = if fd.monochrome {
            ChromaSampling::Monochrome
        } else {
            match (fd.subsampling_x, fd.subsampling_y) {
                (0, 0) => ChromaSampling::Cs444,
                (1, 0) => ChromaSampling::Cs422,
                _ => ChromaSampling::Cs420,
            }
        };
        let matrix_coefficients = MatrixCoefficients(fd.matrix_coefficients as u8);
        let color_range = if fd.full_range {
            ColorRange::Full
        } else {
            ColorRange::Limited
        };
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
                Some(zenavif_parse::ColorInformation::IccProfile(icc)) => (
                    ColorPrimaries(fd.color_primaries as u8),
                    TransferCharacteristics(fd.transfer_characteristics as u8),
                    Some(icc.clone()),
                ),
                None => (
                    ColorPrimaries(fd.color_primaries as u8),
                    TransferCharacteristics(fd.transfer_characteristics as u8),
                    None,
                ),
            };
        let info = ImageInfo {
            width: width as u32,
            height: height as u32,
            bit_depth,
            has_alpha,
            premultiplied_alpha: self.parser.premultiplied_alpha(),
            monochrome: fd.monochrome,
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            color_range,
            chroma_sampling,
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
            depth_map: None,
        };
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let our_range = match color_range {
            ColorRange::Full => OurYuvRange::Full,
            ColorRange::Limited => OurYuvRange::Limited,
        };
        let wide_out = bit_depth > 8;
        let mut image = if fd.monochrome {
            self.aom_mono_to_buffer(&fd, our_range, bit_depth, wide_out, has_alpha)?
        } else {
            let resolved = self.resolved_matrix_for(&info)?;
            match resolved {
                ResolvedMatrix::Identity if chroma_sampling == ChromaSampling::Cs444 => {
                    aom_identity_to_buffer(&fd, our_range, bit_depth, wide_out)?
                }
                ResolvedMatrix::Identity => {
                    return Err(at!(Error::Unsupported(
                        "matrix_coefficients=0 (identity/GBR) requires 4:4:4 chroma; \
                         subsampled identity has no defined reconstruction"
                    )));
                }
                _ => {
                    let matrix = resolved
                        .to_our()
                        .expect("identity guarded before planar conversion");
                    let sampling = match chroma_sampling {
                        ChromaSampling::Cs444 => crate::yuv_convert::ChromaSubsampling::Cs444,
                        ChromaSampling::Cs422 => crate::yuv_convert::ChromaSubsampling::Cs422,
                        _ => crate::yuv_convert::ChromaSubsampling::Cs420,
                    };
                    aom_planar_to_buffer(
                        &fd,
                        sampling,
                        our_range,
                        matrix,
                        bit_depth,
                        wide_out,
                        has_alpha,
                        self.alloc_pref,
                    )?
                }
            }
        };
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        // Scale native-depth values (0..2^bd-1) to full u16 — same order as
        // the rav1d path: before alpha attachment so unpremultiply runs on
        // the correct 16-bit range.
        if wide_out {
            scale_pixels_to_u16(&mut image, bit_depth);
        }

        if let Some(af) = fd_alpha {
            if af.width != width || af.height != height {
                return Err(at!(Error::Unsupported(
                    "alpha item dimensions do not match the primary item"
                )));
            }
            let alpha_range = if af.full_range {
                ColorRange::Full
            } else {
                ColorRange::Limited
            };
            let premul = self.parser.premultiplied_alpha();
            if wide_out {
                add_alpha16(
                    &mut image,
                    af.y.chunks(af.width),
                    width,
                    height,
                    alpha_range,
                    af.bit_depth as u8,
                    premul,
                )?;
            } else {
                // Narrow the u16-carried 8-bit alpha samples to the u8 rows
                // `add_alpha8` consumes (exact: bd8 samples are <= 255).
                let a8: Vec<u8> = af.y.iter().map(|&s| s as u8).collect();
                add_alpha8(
                    &mut image,
                    a8.chunks(af.width),
                    width,
                    height,
                    alpha_range,
                    premul,
                )?;
            }
        }

        if self.prefer_8bit && bit_depth > 8 {
            image = downscale_to_8bit(image);
        }
        Ok((image, info))
    }

    fn aom_mono_to_buffer(
        &self,
        fd: &aom_decode::frame::FrameDecode,
        range: OurYuvRange,
        bit_depth: u8,
        wide_out: bool,
        has_alpha: bool,
    ) -> Result<PixelBuffer> {
        let (w, h) = (fd.width, fd.height);
        let px = w.checked_mul(h).ok_or_else(|| at!(Error::OutOfMemory))?;
        macro_rules! mono {
            ($pix:ty) => {{
                let mut out = crate::alloc_util::alloc_filled(
                    self.alloc_pref,
                    true,
                    <$pix as Default>::default(),
                    px,
                )?;
                crate::yuv_convert::yuv400_to_rgbx_strip::<u16, $pix>(
                    &fd.y, w, w, 0, h, range, bit_depth, &mut out,
                );
                PixelBuffer::from_pixels(out, w as u32, h as u32)
                    .map(Into::into)
                    .map_err(|_| at!(Error::OutOfMemory))
            }};
        }
        match (wide_out, self.native_gray && !has_alpha, has_alpha) {
            (false, true, _) => mono!(rgb::Gray<u8>),
            (true, true, _) => mono!(rgb::Gray<u16>),
            (false, _, true) => mono!(Rgba<u8>),
            (false, _, false) => mono!(Rgb<u8>),
            (true, _, true) => mono!(rgb::Rgba<u16>),
            (true, _, false) => mono!(rgb::Rgb<u16>),
        }
    }
}

/// Identity (MC=0, 4:4:4) reorder for the aom path: planes are (G,B,R);
/// limited range expands to full, exactly like `convert_8bit_identity` /
/// `convert_16bit_identity` on the rav1d path.
fn aom_identity_to_buffer(
    fd: &aom_decode::frame::FrameDecode,
    range: OurYuvRange,
    bit_depth: u8,
    wide_out: bool,
) -> Result<PixelBuffer> {
    let (w, h) = (fd.width, fd.height);
    let px = w.checked_mul(h).ok_or_else(|| at!(Error::OutOfMemory))?;
    let limited = matches!(range, OurYuvRange::Limited);
    let max = (1u32 << bit_depth) - 1;
    let smin = 16u32 << (bit_depth - 8);
    let span = 219u32 << (bit_depth - 8);
    let expand = |v: u16| -> u16 {
        if limited {
            let c = (v as u32).saturating_sub(smin).min(span);
            ((c * max + span / 2) / span) as u16
        } else {
            v
        }
    };
    if wide_out {
        // Native-depth output (range-expanded within 0..2^bd-1) — the caller's
        // scale_pixels_to_u16 widens, exactly like the rav1d identity path.
        let mut out = vec![rgb::Rgb::<u16> { r: 0, g: 0, b: 0 }; px];
        for (i, o) in out.iter_mut().enumerate() {
            *o = rgb::Rgb {
                r: expand(fd.v[i]),
                g: expand(fd.y[i]),
                b: expand(fd.u[i]),
            };
        }
        PixelBuffer::from_pixels(out, w as u32, h as u32)
            .map(Into::into)
            .map_err(|_| at!(Error::OutOfMemory))
    } else {
        let mut out = vec![Rgb::<u8> { r: 0, g: 0, b: 0 }; px];
        for (i, o) in out.iter_mut().enumerate() {
            *o = Rgb {
                r: expand(fd.v[i]) as u8,
                g: expand(fd.y[i]) as u8,
                b: expand(fd.u[i]) as u8,
            };
        }
        PixelBuffer::from_pixels(out, w as u32, h as u32)
            .map(Into::into)
            .map_err(|_| at!(Error::OutOfMemory))
    }
}

/// Planar YUV -> RGB(A) for the aom path via the depth-generic u16 kernel
/// entries (same canonical recipe as the rav1d path's kernels).
#[expect(clippy::too_many_arguments, reason = "internal conversion helper")]
fn aom_planar_to_buffer(
    fd: &aom_decode::frame::FrameDecode,
    sampling: crate::yuv_convert::ChromaSubsampling,
    range: OurYuvRange,
    matrix: OurYuvMatrix,
    bit_depth: u8,
    wide_out: bool,
    has_alpha: bool,
    alloc_pref: crate::alloc_util::AllocPref,
) -> Result<PixelBuffer> {
    let (w, h) = (fd.width, fd.height);
    let px = w.checked_mul(h).ok_or_else(|| at!(Error::OutOfMemory))?;
    macro_rules! planar {
        ($pix:ty) => {{
            let mut out = crate::alloc_util::alloc_filled(
                alloc_pref,
                true,
                <$pix as Default>::default(),
                px,
            )?;
            crate::yuv_convert::yuv16_to_rgbx_strip::<$pix>(
                sampling,
                &fd.y,
                w,
                &fd.u,
                fd.width_uv,
                &fd.v,
                fd.width_uv,
                w,
                h,
                0,
                h,
                range,
                matrix,
                bit_depth,
                &mut out,
            );
            PixelBuffer::from_pixels(out, w as u32, h as u32)
                .map(Into::into)
                .map_err(|_| at!(Error::OutOfMemory))
        }};
    }
    match (wide_out, has_alpha) {
        (false, false) => planar!(Rgb<u8>),
        (false, true) => planar!(Rgba<u8>),
        (true, false) => planar!(rgb::Rgb<u16>),
        (true, true) => planar!(rgb::Rgba<u16>),
    }
}
