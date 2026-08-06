//! Stateless YUV-plane -> pixel-buffer kernels.
//!
//! Every helper here takes decoded plane views plus a [`ConvertCtx`] and
//! produces a [`PixelBuffer`]; none of them know about the decoder, the
//! container, or the parser. The per-depth / per-sampling dispatch that
//! picks among them lives in [`super::frame_convert`].

use super::cicp_map::to_our_yuv_range;
use crate::cicp_resolve::ResolvedMatrix;
use crate::error::{Error, Result};
use crate::image::{ChromaSampling, ImageInfo};
use crate::yuv_convert::{self, YuvMatrix as OurYuvMatrix, YuvRange as OurYuvRange};
use rav1d_safe::src::managed::{Planes8, Planes16};
use rgb::{Rgb, Rgba};
use whereat::at;
use yuv::{YuvPlanarImage, YuvRange};
use zenpixels::PixelBuffer;

/// Common context shared across every YUV→RGB(A) helper in this module.
///
/// Bundling these together keeps the helper signatures inside clippy's
/// `too_many_arguments` limit and makes it harder to wire up the wrong
/// dimension to the wrong call site.
#[derive(Clone, Copy)]
pub(super) struct ConvertCtx {
    /// Frame width in samples (matches the AV1 buffer, not the cropped display).
    pub(super) buffer_width: usize,
    /// Frame height in samples.
    pub(super) buffer_height: usize,
    /// `buffer_width * buffer_height`, pre-checked for overflow by the caller.
    pub(super) buffer_pixel_count: usize,
    /// Whether the primary frame carries a sibling alpha plane.
    pub(super) has_alpha: bool,
    /// `yuv` crate's color range (Tv/Pc).
    pub(super) yuv_range: YuvRange,
    /// Allocation-fallibility preference for the output buffers allocated by
    /// the YUV→RGB(A) helpers. The full-image `out` buffers are sized from the
    /// (untrusted) AV1 frame dimensions, so they default to fallible; the
    /// per-row `rgb_row` scratch is width-bounded and defaults to infallible.
    pub(super) alloc_pref: crate::alloc_util::AllocPref,
}

/// Map the ctx's yuv-crate range to the in-house kernel range.
fn ctx_our_range(ctx: &ConvertCtx) -> OurYuvRange {
    match ctx.yuv_range {
        YuvRange::Full => OurYuvRange::Full,
        YuvRange::Limited => OurYuvRange::Limited,
    }
}

impl ConvertCtx {
    pub(super) fn dims(&self) -> (u32, u32) {
        (self.buffer_width as u32, self.buffer_height as u32)
    }
}

/// Native Gray8 output for alpha-free monochrome (imazen/zenavif#5):
/// 1 byte/pixel instead of the 3-4x RGB expansion. Range expansion goes
/// through the same `yuv` crate kernel as the RGB path (per-row scratch),
/// so gray output is bit-identical to the R channel of an RGB decode.
pub(super) fn convert_8bit_monochrome_gray(
    planes: &Planes8<'_>,
    ctx: ConvertCtx,
) -> Result<PixelBuffer> {
    let y_view = planes.y();
    let (w, h) = ctx.dims();
    let hu = h as usize;
    let mut out = crate::alloc_util::alloc_filled(
        ctx.alloc_pref,
        true,
        rgb::Gray::<u8>::new(0),
        ctx.buffer_pixel_count,
    )?;
    crate::yuv_convert::yuv400_to_rgbx_strip::<u8, rgb::Gray<u8>>(
        y_view.as_slice(),
        y_view.stride(),
        w as usize,
        0,
        hu,
        ctx_our_range(&ctx),
        8,
        &mut out,
    );
    PixelBuffer::from_pixels(out, w, h)
        .map(Into::into)
        .map_err(|_| at!(Error::OutOfMemory))
}

/// Native Gray16 output for alpha-free 10/12-bit monochrome.
/// Values are native-depth; the caller's `scale_pixels_to_u16` expands.
pub(super) fn convert_16bit_monochrome_gray(
    planes: &Planes16<'_>,
    bit_depth: u8,
    ctx: ConvertCtx,
) -> Result<PixelBuffer> {
    let y_view = planes.y();
    let (w, h) = ctx.dims();
    let mut out = crate::alloc_util::alloc_filled(
        ctx.alloc_pref,
        true,
        rgb::Gray::<u16>::new(0),
        ctx.buffer_pixel_count,
    )?;
    crate::yuv_convert::yuv400_to_rgbx_strip::<u16, rgb::Gray<u16>>(
        y_view.as_slice(),
        y_view.stride(),
        w as usize,
        0,
        h as usize,
        ctx_our_range(&ctx),
        bit_depth,
        &mut out,
    );
    PixelBuffer::from_pixels(out, w, h)
        .map(Into::into)
        .map_err(|_| at!(Error::OutOfMemory))
}

/// 8-bit monochrome YUV→RGB(A) dispatch. `has_alpha` selects RGBA vs RGB output.
pub(super) fn convert_8bit_monochrome(
    planes: &Planes8<'_>,
    ctx: ConvertCtx,
) -> Result<PixelBuffer> {
    let y_view = planes.y();
    let (w, h) = ctx.dims();
    let our_range = ctx_our_range(&ctx);
    if ctx.has_alpha {
        let mut out = crate::alloc_util::alloc_filled(
            ctx.alloc_pref,
            true,
            Rgba {
                r: 0u8,
                g: 0,
                b: 0,
                a: 255,
            },
            ctx.buffer_pixel_count,
        )?;
        crate::yuv_convert::yuv400_to_rgbx_strip::<u8, Rgba<u8>>(
            y_view.as_slice(),
            y_view.stride(),
            w as usize,
            0,
            h as usize,
            our_range,
            8,
            &mut out,
        );
        PixelBuffer::from_pixels(out, w, h)
            .map(Into::into)
            .map_err(|_| at!(Error::OutOfMemory))
    } else {
        let mut out = crate::alloc_util::alloc_filled(
            ctx.alloc_pref,
            true,
            Rgb { r: 0u8, g: 0, b: 0 },
            ctx.buffer_pixel_count,
        )?;
        crate::yuv_convert::yuv400_to_rgbx_strip::<u8, Rgb<u8>>(
            y_view.as_slice(),
            y_view.stride(),
            w as usize,
            0,
            h as usize,
            our_range,
            8,
            &mut out,
        );
        PixelBuffer::from_pixels(out, w, h)
            .map(Into::into)
            .map_err(|_| at!(Error::OutOfMemory))
    }
}

/// Identity (MC=0) 8-bit conversion: AV1 planes carry G,B,R — output is
/// a reorder plus range expansion, no matrix math (H.273; the GBR
/// convention zenravif's own `rgb_to_8_bit_gbr` writes). 4:4:4 only —
/// callers guard. `ctx.matrix` is deliberately unread; alpha (when
/// present) is attached by the caller afterwards, so this emits RGB(A)
/// with a placeholder A like the planar paths do.
pub(super) fn convert_8bit_identity(planes: &Planes8<'_>, ctx: ConvertCtx) -> Result<PixelBuffer> {
    let g_view = planes.y();
    let b_view = planes
        .u()
        .ok_or_else(|| at!(Error::Malformed("Identity content missing plane 1 (B)")))?;
    let r_view = planes
        .v()
        .ok_or_else(|| at!(Error::Malformed("Identity content missing plane 2 (R)")))?;

    let (w, h) = (ctx.buffer_width, ctx.buffer_height);
    let limited = matches!(ctx.yuv_range, YuvRange::Limited);
    // Limited range on identity content uses the luma range (16–235)
    // on all three planes (H.273 full-range flag semantics).
    #[inline]
    fn expand_limited(v: u8) -> u8 {
        let c = u32::from(v.saturating_sub(16)).min(219);
        ((c * 255 + 109) / 219) as u8
    }
    let map = |v: u8| if limited { expand_limited(v) } else { v };

    let rows = g_view.rows().zip(b_view.rows()).zip(r_view.rows()).take(h);
    if ctx.has_alpha {
        let mut out: Vec<rgb::Rgba<u8>> =
            crate::alloc_util::vec_with_capacity(ctx.alloc_pref, true, ctx.buffer_pixel_count)?;
        for ((g_row, b_row), r_row) in rows {
            for x in 0..w {
                out.push(rgb::Rgba {
                    r: map(r_row[x]),
                    g: map(g_row[x]),
                    b: map(b_row[x]),
                    a: 255, // attached by the caller afterwards
                });
            }
        }
        PixelBuffer::from_pixels(out, w as u32, h as u32)
            .map(Into::into)
            .map_err(|_| at!(Error::OutOfMemory))
    } else {
        let mut out: Vec<Rgb<u8>> =
            crate::alloc_util::vec_with_capacity(ctx.alloc_pref, true, ctx.buffer_pixel_count)?;
        for ((g_row, b_row), r_row) in rows {
            for x in 0..w {
                out.push(Rgb {
                    r: map(r_row[x]),
                    g: map(g_row[x]),
                    b: map(b_row[x]),
                });
            }
        }
        PixelBuffer::from_pixels(out, w as u32, h as u32)
            .map(Into::into)
            .map_err(|_| at!(Error::OutOfMemory))
    }
}

/// Identity (MC=0) 10/12-bit conversion — see [`convert_8bit_identity`].
/// Outputs native-bit-depth values; the caller's `scale_pixels_to_u16`
/// expands to full u16 afterwards (same contract as the planar paths).
pub(super) fn convert_16bit_identity(
    planes: &Planes16<'_>,
    bit_depth: u8,
    ctx: ConvertCtx,
) -> Result<PixelBuffer> {
    let g_view = planes.y();
    let b_view = planes
        .u()
        .ok_or_else(|| at!(Error::Malformed("Identity content missing plane 1 (B)")))?;
    let r_view = planes
        .v()
        .ok_or_else(|| at!(Error::Malformed("Identity content missing plane 2 (R)")))?;

    let (w, h) = (ctx.buffer_width, ctx.buffer_height);
    let limited = matches!(ctx.yuv_range, YuvRange::Limited);
    let max = (1u32 << bit_depth) - 1;
    // Studio range scaled by bit depth: min = 16<<(d-8), span = 219<<(d-8).
    let smin = 16u32 << (bit_depth - 8);
    let span = 219u32 << (bit_depth - 8);
    let map = |v: u16| -> u16 {
        if limited {
            let c = u32::from(v).saturating_sub(smin).min(span);
            ((c * max + span / 2) / span) as u16
        } else {
            v
        }
    };

    let rows = g_view.rows().zip(b_view.rows()).zip(r_view.rows()).take(h);
    if ctx.has_alpha {
        let mut out: Vec<rgb::Rgba<u16>> =
            crate::alloc_util::vec_with_capacity(ctx.alloc_pref, true, ctx.buffer_pixel_count)?;
        for ((g_row, b_row), r_row) in rows {
            for x in 0..w {
                out.push(rgb::Rgba {
                    r: map(r_row[x]),
                    g: map(g_row[x]),
                    b: map(b_row[x]),
                    a: max as u16, // attached by the caller afterwards
                });
            }
        }
        PixelBuffer::from_pixels(out, w as u32, h as u32)
            .map(Into::into)
            .map_err(|_| at!(Error::OutOfMemory))
    } else {
        let mut out: Vec<Rgb<u16>> =
            crate::alloc_util::vec_with_capacity(ctx.alloc_pref, true, ctx.buffer_pixel_count)?;
        for ((g_row, b_row), r_row) in rows {
            for x in 0..w {
                out.push(Rgb {
                    r: map(r_row[x]),
                    g: map(g_row[x]),
                    b: map(b_row[x]),
                });
            }
        }
        PixelBuffer::from_pixels(out, w as u32, h as u32)
            .map(Into::into)
            .map_err(|_| at!(Error::OutOfMemory))
    }
}

/// 8-bit planar (Cs420/Cs422/Cs444) YUV→RGB(A) dispatch.
///
/// `has_alpha` selects RGBA (yuv crate bilinear/standard paths) vs RGB
/// (our `yuv_convert` SIMD paths). `info` supplies `color_range` and
/// `matrix_coefficients` for the RGB path.
pub(super) fn convert_8bit_planar(
    planes: &Planes8<'_>,
    sampling: ChromaSampling,
    info: &ImageInfo,
    resolved: ResolvedMatrix,
    ctx: ConvertCtx,
) -> Result<PixelBuffer> {
    let y_view = planes.y();
    let u_view = planes
        .u()
        .ok_or_else(|| at!(Error::Malformed(
            "decoded frame declares chroma subsampling but the decoder returned no U plane; \
             the bitstream's sequence header and its coded planes disagree",
        )))?;
    let v_view = planes
        .v()
        .ok_or_else(|| at!(Error::Malformed(
            "decoded frame declares chroma subsampling but the decoder returned no V plane; \
             the bitstream's sequence header and its coded planes disagree",
        )))?;

    // Every real matrix maps in-house now (FCC/SMPTE-240M/derived via
    // explicit Kr,Kb); identity never reaches here (guarded upstream).
    let our_matrix = resolved
        .to_our()
        .expect("identity guarded before planar conversion");
    let our_range = to_our_yuv_range(info.color_range);
    if ctx.has_alpha {
        convert_8bit_planar_rgba_inhouse(
            &y_view, &u_view, &v_view, sampling, ctx, our_range, our_matrix,
        )
    } else {
        convert_8bit_planar_rgb(
            &y_view, &u_view, &v_view, sampling, ctx, our_range, our_matrix,
        )
    }
}

/// Decode 8-bit YUV planar to RGBA via the in-house SIMD kernels — the
/// SAME unified kernels the no-alpha RGB path uses, with an RGBA store
/// (alpha 255; the caller's `add_alpha8` overwrites it). One kernel for
/// both paths guarantees identical color payloads decode identically with
/// and without an alpha item — byte-for-byte, by construction.
fn convert_8bit_planar_rgba_inhouse(
    y_view: &rav1d_safe::src::managed::PlaneView8<'_>,
    u_view: &rav1d_safe::src::managed::PlaneView8<'_>,
    v_view: &rav1d_safe::src::managed::PlaneView8<'_>,
    sampling: ChromaSampling,
    ctx: ConvertCtx,
    our_range: OurYuvRange,
    our_matrix: OurYuvMatrix,
) -> Result<PixelBuffer> {
    let buffer_width = ctx.buffer_width;
    let buffer_height = ctx.buffer_height;
    let mut out = crate::alloc_util::alloc_filled(
        ctx.alloc_pref,
        true,
        Rgba {
            r: 0u8,
            g: 0,
            b: 0,
            a: 255,
        },
        ctx.buffer_pixel_count,
    )?;
    match sampling {
        ChromaSampling::Cs420 => yuv_convert::yuv420_to_rgba8_strip(
            y_view.as_slice(),
            y_view.stride(),
            u_view.as_slice(),
            u_view.stride(),
            v_view.as_slice(),
            v_view.stride(),
            buffer_width,
            buffer_height,
            0,
            buffer_height,
            our_range,
            our_matrix,
            &mut out,
        ),
        ChromaSampling::Cs422 => yuv_convert::yuv422_to_rgba8_strip(
            y_view.as_slice(),
            y_view.stride(),
            u_view.as_slice(),
            u_view.stride(),
            v_view.as_slice(),
            v_view.stride(),
            buffer_width,
            0,
            buffer_height,
            our_range,
            our_matrix,
            &mut out,
        ),
        ChromaSampling::Cs444 => yuv_convert::yuv444_to_rgba8_strip(
            y_view.as_slice(),
            y_view.stride(),
            u_view.as_slice(),
            u_view.stride(),
            v_view.as_slice(),
            v_view.stride(),
            buffer_width,
            0,
            buffer_height,
            our_range,
            our_matrix,
            &mut out,
        ),
        ChromaSampling::Monochrome => {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "Monochrome should not reach chroma conversion",
            }));
        }
    }
    let (w, h) = ctx.dims();
    PixelBuffer::from_pixels(out, w, h)
        .map(Into::into)
        .map_err(|_| at!(Error::OutOfMemory))
}

/// Decode 8-bit YUV planar to RGB via our `yuv_convert` SIMD path.
fn convert_8bit_planar_rgb(
    y_view: &rav1d_safe::src::managed::PlaneView8<'_>,
    u_view: &rav1d_safe::src::managed::PlaneView8<'_>,
    v_view: &rav1d_safe::src::managed::PlaneView8<'_>,
    sampling: ChromaSampling,
    ctx: ConvertCtx,
    our_range: OurYuvRange,
    our_matrix: OurYuvMatrix,
) -> Result<PixelBuffer> {
    let buffer_width = ctx.buffer_width;
    let buffer_height = ctx.buffer_height;
    let result = match sampling {
        ChromaSampling::Cs420 => yuv_convert::yuv420_to_rgb8(
            y_view.as_slice(),
            y_view.stride(),
            u_view.as_slice(),
            u_view.stride(),
            v_view.as_slice(),
            v_view.stride(),
            buffer_width,
            buffer_height,
            our_range,
            our_matrix,
        ),
        ChromaSampling::Cs422 => yuv_convert::yuv422_to_rgb8(
            y_view.as_slice(),
            y_view.stride(),
            u_view.as_slice(),
            u_view.stride(),
            v_view.as_slice(),
            v_view.stride(),
            buffer_width,
            buffer_height,
            our_range,
            our_matrix,
        ),
        ChromaSampling::Cs444 => yuv_convert::yuv444_to_rgb8(
            y_view.as_slice(),
            y_view.stride(),
            u_view.as_slice(),
            u_view.stride(),
            v_view.as_slice(),
            v_view.stride(),
            buffer_width,
            buffer_height,
            our_range,
            our_matrix,
        ),
        ChromaSampling::Monochrome => {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "Monochrome should not reach chroma conversion",
            }));
        }
    };

    Ok(PixelBuffer::from_imgvec(result).into())
}

/// 16-bit (10/12) monochrome YUV→RGB(A) dispatch. `bit_depth` selects
/// the y010/y012/y016 conversion; `ctx.has_alpha` selects RGBA vs RGB.
pub(super) fn convert_16bit_monochrome(
    planes: &Planes16<'_>,
    bit_depth: u8,
    ctx: ConvertCtx,
) -> Result<PixelBuffer> {
    let y_view = planes.y();
    let (w, h) = ctx.dims();
    let our_range = ctx_our_range(&ctx);
    if ctx.has_alpha {
        let mut out = crate::alloc_util::alloc_filled(
            ctx.alloc_pref,
            true,
            Rgba {
                r: 0u16,
                g: 0,
                b: 0,
                a: 0,
            },
            ctx.buffer_pixel_count,
        )?;
        crate::yuv_convert::yuv400_to_rgbx_strip::<u16, Rgba<u16>>(
            y_view.as_slice(),
            y_view.stride(),
            w as usize,
            0,
            h as usize,
            our_range,
            bit_depth,
            &mut out,
        );
        PixelBuffer::from_pixels(out, w, h)
            .map(Into::into)
            .map_err(|_| at!(Error::OutOfMemory))
    } else {
        let mut out = crate::alloc_util::alloc_filled(
            ctx.alloc_pref,
            true,
            Rgb {
                r: 0u16,
                g: 0,
                b: 0,
            },
            ctx.buffer_pixel_count,
        )?;
        crate::yuv_convert::yuv400_to_rgbx_strip::<u16, Rgb<u16>>(
            y_view.as_slice(),
            y_view.stride(),
            w as usize,
            0,
            h as usize,
            our_range,
            bit_depth,
            &mut out,
        );
        PixelBuffer::from_pixels(out, w, h)
            .map(Into::into)
            .map_err(|_| at!(Error::OutOfMemory))
    }
}

/// 16-bit (10/12) planar (Cs420/Cs422/Cs444) YUV→RGB(A) dispatch.
pub(super) fn convert_16bit_planar(
    planes: &Planes16<'_>,
    sampling: ChromaSampling,
    bit_depth: u8,
    resolved: ResolvedMatrix,
    ctx: ConvertCtx,
) -> Result<PixelBuffer> {
    let y_view = planes.y();
    let u_view = planes
        .u()
        .ok_or_else(|| at!(Error::Malformed(
            "decoded frame declares chroma subsampling but the decoder returned no U plane; \
             the bitstream's sequence header and its coded planes disagree",
        )))?;
    let v_view = planes
        .v()
        .ok_or_else(|| at!(Error::Malformed(
            "decoded frame declares chroma subsampling but the decoder returned no V plane; \
             the bitstream's sequence header and its coded planes disagree",
        )))?;

    let (w, h) = ctx.dims();
    let planar = YuvPlanarImage {
        y_plane: y_view.as_slice(),
        y_stride: y_view.stride() as u32,
        u_plane: u_view.as_slice(),
        u_stride: u_view.stride() as u32,
        v_plane: v_view.as_slice(),
        v_stride: v_view.stride() as u32,
        width: w,
        height: h,
    };

    // Every real matrix maps in-house (FCC/SMPTE-240M/derived via
    // explicit Kr,Kb); identity never reaches here (guarded upstream).
    let our_matrix = resolved
        .to_our()
        .expect("identity guarded before planar conversion");
    let our_range = ctx_our_range(&ctx);
    convert_16bit_planar_inhouse(&planar, sampling, bit_depth, ctx, our_range, our_matrix)
}

/// 16-bit planar → RGB(A)16 via the in-house unified kernels (native-depth
/// output; RGBA alpha = the native ceiling, exactly like the yuv-crate
/// path). Same kernels as every other depth/output — byte-identity across
/// RGB-vs-RGBA and full-vs-strip is structural.
fn convert_16bit_planar_inhouse(
    planar: &YuvPlanarImage<'_, u16>,
    sampling: ChromaSampling,
    bit_depth: u8,
    ctx: ConvertCtx,
    our_range: OurYuvRange,
    our_matrix: OurYuvMatrix,
) -> Result<PixelBuffer> {
    let our_sampling = match sampling {
        ChromaSampling::Cs420 => crate::yuv_convert::ChromaSubsampling::Cs420,
        ChromaSampling::Cs422 => crate::yuv_convert::ChromaSubsampling::Cs422,
        ChromaSampling::Cs444 => crate::yuv_convert::ChromaSubsampling::Cs444,
        ChromaSampling::Monochrome => {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "Monochrome should not reach chroma conversion",
            }));
        }
    };
    let (w, h) = ctx.dims();
    let bw = ctx.buffer_width;
    let bh = ctx.buffer_height;
    if ctx.has_alpha {
        let mut out = crate::alloc_util::alloc_filled(
            ctx.alloc_pref,
            true,
            Rgba {
                r: 0u16,
                g: 0,
                b: 0,
                a: 0,
            },
            ctx.buffer_pixel_count,
        )?;
        crate::yuv_convert::yuv16_to_rgbx_strip::<Rgba<u16>>(
            our_sampling,
            planar.y_plane,
            planar.y_stride as usize,
            planar.u_plane,
            planar.u_stride as usize,
            planar.v_plane,
            planar.v_stride as usize,
            bw,
            bh,
            0,
            bh,
            our_range,
            our_matrix,
            bit_depth,
            &mut out,
        );
        PixelBuffer::from_pixels(out, w, h)
            .map(Into::into)
            .map_err(|_| at!(Error::OutOfMemory))
    } else {
        let mut out = crate::alloc_util::alloc_filled(
            ctx.alloc_pref,
            true,
            Rgb {
                r: 0u16,
                g: 0,
                b: 0,
            },
            ctx.buffer_pixel_count,
        )?;
        crate::yuv_convert::yuv16_to_rgbx_strip::<Rgb<u16>>(
            our_sampling,
            planar.y_plane,
            planar.y_stride as usize,
            planar.u_plane,
            planar.u_stride as usize,
            planar.v_plane,
            planar.v_stride as usize,
            bw,
            bh,
            0,
            bh,
            our_range,
            our_matrix,
            bit_depth,
            &mut out,
        );
        PixelBuffer::from_pixels(out, w, h)
            .map(Into::into)
            .map_err(|_| at!(Error::OutOfMemory))
    }
}
