//! Cache-optimal strip-based YUV→RGB conversion.
//!
//! [`StripConverter`] holds decoded YUV frames from rav1d-safe and converts
//! them to RGB(A) in cache-friendly strips rather than allocating a full-frame
//! output buffer. For a 4K image this eliminates a ~33 MB RGBA allocation and
//! reduces the working set from >1 MB to ~330 KB (16 rows × 4K width).
//!
//! The converter fuses chroma upsampling, YUV→RGB matrix, alpha attachment,
//! and unpremultiply into a single pass per strip, keeping the output data
//! hot in L1/L2 cache.

#![deny(unsafe_code)]

use crate::convert::{add_alpha8, scale_pixels_to_u16, add_alpha16};
use crate::error::{Error, Result};
use crate::image::{ChromaSampling, ColorRange};
use crate::yuv_convert::{self, YuvMatrix, YuvRange};
use rgb::{Rgb, Rgba};
use whereat::at;
use zenpixels::{PixelBuffer, PixelDescriptor};

use rav1d_safe::src::managed::{Frame, Planes};

/// Holds decoded YUV frames and converts strips on demand.
///
/// For 8-bit images, strips are converted using the custom SIMD-capable
/// YUV conversion functions with correct bilinear chroma upsampling at
/// strip boundaries (the full plane data is always available).
///
/// For 16-bit images, the full frame is converted at construction time
/// and strips are sliced from the result. This is a fallback until
/// 16-bit strip conversion is implemented.
pub(crate) struct StripConverter {
    state: ConversionState,
    descriptor: PixelDescriptor,
    display_width: usize,
    display_height: usize,
}

enum ConversionState {
    /// 8-bit: hold decoded frames, convert strips on demand.
    Frames8 {
        primary: Frame,
        alpha: Option<Frame>,
        chroma_sampling: ChromaSampling,
        yuv_range: YuvRange,
        yuv_matrix: YuvMatrix,
        alpha_range: ColorRange,
        premultiplied: bool,
        buffer_width: usize,
        buffer_height: usize,
    },
    /// 16-bit or monochrome: fully converted, slice strips.
    FullPixels(PixelBuffer),
}

impl StripConverter {
    /// Create a strip converter from decoded frames.
    ///
    /// For 8-bit color images, the frames are held and converted per-strip.
    /// For 16-bit or monochrome, the full frame is converted immediately
    /// (falling back to the existing conversion pipeline).
    pub fn new(
        primary: Frame,
        alpha: Option<Frame>,
        chroma_sampling: ChromaSampling,
        yuv_range: YuvRange,
        yuv_matrix: YuvMatrix,
        alpha_range: ColorRange,
        premultiplied: bool,
        display_width: usize,
        display_height: usize,
        buffer_width: usize,
        buffer_height: usize,
        descriptor: PixelDescriptor,
    ) -> Self {
        let bit_depth = primary.bit_depth();

        if bit_depth == 8 && !matches!(chroma_sampling, ChromaSampling::Monochrome) {
            StripConverter {
                state: ConversionState::Frames8 {
                    primary,
                    alpha,
                    chroma_sampling,
                    yuv_range,
                    yuv_matrix,
                    alpha_range,
                    premultiplied,
                    buffer_width,
                    buffer_height,
                },
                descriptor,
                display_width,
                display_height,
            }
        } else {
            // Fallback: can't do strip conversion for this format yet.
            // Caller should use `new_from_pixels` instead.
            // This branch shouldn't be reached if callers check properly.
            panic!(
                "StripConverter::new called for unsupported format: bit_depth={}, chroma={:?}. \
                 Use new_from_pixels for 16-bit and monochrome images.",
                bit_depth, chroma_sampling
            );
        }
    }

    /// Create a strip converter from already-converted pixels.
    ///
    /// Used for 16-bit and monochrome images where strip conversion
    /// is not yet implemented.
    pub fn new_from_pixels(pixels: PixelBuffer) -> Self {
        let w = pixels.width() as usize;
        let h = pixels.height() as usize;
        let desc = pixels.descriptor();
        StripConverter {
            state: ConversionState::FullPixels(pixels),
            descriptor: desc,
            display_width: w,
            display_height: h,
        }
    }

    pub fn descriptor(&self) -> PixelDescriptor {
        self.descriptor
    }

    pub fn display_width(&self) -> usize {
        self.display_width
    }

    pub fn display_height(&self) -> usize {
        self.display_height
    }

    /// Returns true if this converter does actual strip conversion
    /// (vs. slicing from a fully-converted buffer).
    pub fn is_true_streaming(&self) -> bool {
        matches!(self.state, ConversionState::Frames8 { .. })
    }

    /// Compute optimal strip height for cache efficiency.
    ///
    /// Targets keeping the working set (YUV input + RGB output for the strip)
    /// within L2 cache (~256 KB). For YUV420, strip height must be even.
    pub fn optimal_strip_height(&self) -> usize {
        let width = self.display_width;
        let bpp = self.descriptor.bytes_per_pixel();
        // Bytes per row of output
        let row_bytes_out = width * bpp;
        // Approximate bytes per row of input (Y + U/2 + V/2 for 420)
        let row_bytes_in = width + width; // conservative: Y + U + V at worst
        let total_per_row = row_bytes_out + row_bytes_in;

        // Target ~256 KB working set (fits in L2)
        let target_bytes = 256 * 1024;
        let mut h = if total_per_row > 0 {
            target_bytes / total_per_row
        } else {
            16
        };

        // Clamp to reasonable bounds
        h = h.clamp(2, 64);

        // For YUV420, strip height must be even (2 luma rows per chroma row)
        if let ConversionState::Frames8 {
            chroma_sampling: ChromaSampling::Cs420,
            ..
        } = &self.state
        {
            h &= !1; // round down to even
            if h == 0 {
                h = 2;
            }
        }

        h
    }

    /// Convert a strip of rows, writing into the provided `PixelBuffer`.
    ///
    /// The buffer must have dimensions `(display_width, strip_height)` and
    /// a compatible descriptor.
    pub fn convert_strip(
        &self,
        y_start: usize,
        strip_height: usize,
        out_buf: &mut PixelBuffer,
    ) -> Result<()> {
        match &self.state {
            ConversionState::Frames8 {
                primary,
                alpha,
                chroma_sampling,
                yuv_range,
                yuv_matrix,
                alpha_range,
                premultiplied,
                buffer_width: _,
                buffer_height,
            } => {
                self.convert_strip_8bit(
                    primary,
                    alpha.as_ref(),
                    *chroma_sampling,
                    *yuv_range,
                    *yuv_matrix,
                    *alpha_range,
                    *premultiplied,
                    *buffer_height,
                    y_start,
                    strip_height,
                    out_buf,
                )
            }
            ConversionState::FullPixels(pixels) => {
                // Copy the strip from the full pixel buffer
                let bpp = pixels.descriptor().bytes_per_pixel();
                let src_slice = pixels.as_slice();
                let mut dst = out_buf.as_slice_mut();
                let width = self.display_width;
                for row in 0..strip_height {
                    let src_row = src_slice.row((y_start + row) as u32);
                    let dst_row = dst.row_mut(row as u32);
                    let copy_bytes = width * bpp;
                    dst_row[..copy_bytes].copy_from_slice(&src_row[..copy_bytes]);
                }
                Ok(())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn convert_strip_8bit(
        &self,
        primary: &Frame,
        alpha: Option<&Frame>,
        chroma_sampling: ChromaSampling,
        yuv_range: YuvRange,
        yuv_matrix: YuvMatrix,
        alpha_range: ColorRange,
        premultiplied: bool,
        buffer_height: usize,
        y_start: usize,
        strip_height: usize,
        out_buf: &mut PixelBuffer,
    ) -> Result<()> {
        let Planes::Depth8(planes) = primary.planes() else {
            return Err(at(Error::Decode {
                code: -1,
                msg: "Expected 8-bit planes",
            }));
        };

        let y_view = planes.y();
        let width = self.display_width;

        let has_alpha = alpha.is_some();

        if has_alpha {
            // Convert YUV → RGBA8 (alpha=255), then overwrite alpha channel
            let mut img = out_buf.try_as_imgref_mut::<Rgba<u8>>().ok_or_else(|| {
                at(Error::Unsupported("expected RGBA8 buffer for alpha image"))
            })?;
            let out_rgba = img.buf_mut();

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

            match chroma_sampling {
                ChromaSampling::Cs420 => yuv_convert::yuv420_to_rgba8_strip(
                    y_view.as_slice(),
                    y_view.stride(),
                    u_view.as_slice(),
                    u_view.stride(),
                    v_view.as_slice(),
                    v_view.stride(),
                    width,
                    buffer_height,
                    y_start,
                    strip_height,
                    yuv_range,
                    yuv_matrix,
                    out_rgba,
                ),
                ChromaSampling::Cs422 => yuv_convert::yuv422_to_rgba8_strip(
                    y_view.as_slice(),
                    y_view.stride(),
                    u_view.as_slice(),
                    u_view.stride(),
                    v_view.as_slice(),
                    v_view.stride(),
                    width,
                    y_start,
                    strip_height,
                    yuv_range,
                    yuv_matrix,
                    out_rgba,
                ),
                ChromaSampling::Cs444 => yuv_convert::yuv444_to_rgba8_strip(
                    y_view.as_slice(),
                    y_view.stride(),
                    u_view.as_slice(),
                    u_view.stride(),
                    v_view.as_slice(),
                    v_view.stride(),
                    width,
                    y_start,
                    strip_height,
                    yuv_range,
                    yuv_matrix,
                    out_rgba,
                ),
                ChromaSampling::Monochrome => {
                    return Err(at(Error::Decode {
                        code: -1,
                        msg: "Monochrome should not reach strip chroma conversion",
                    }));
                }
            }

            // Fuse alpha attachment while RGBA data is hot in cache
            if let Some(alpha_frame) = alpha {
                let Planes::Depth8(alpha_planes) = alpha_frame.planes() else {
                    return Err(at(Error::Decode {
                        code: -1,
                        msg: "Expected 8-bit alpha plane",
                    }));
                };
                let alpha_y = alpha_planes.y();

                for row in 0..strip_height {
                    let src_y = y_start + row;
                    if src_y >= alpha_y.height() {
                        break;
                    }
                    let alpha_row = alpha_y.row(src_y);
                    let out_row = &mut out_rgba[row * width..(row + 1) * width];
                    for (px, &a) in out_row.iter_mut().zip(alpha_row.iter()) {
                        px.a = match alpha_range {
                            ColorRange::Full => a,
                            ColorRange::Limited => limited_to_full_8(a),
                        };
                    }
                    if premultiplied {
                        crate::convert::unpremultiply8(out_row);
                    }
                }
            }
        } else {
            // Convert YUV → RGB8
            let mut img = out_buf.try_as_imgref_mut::<Rgb<u8>>().ok_or_else(|| {
                at(Error::Unsupported("expected RGB8 buffer for non-alpha image"))
            })?;
            let out_rgb = img.buf_mut();

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

            match chroma_sampling {
                ChromaSampling::Cs420 => yuv_convert::yuv420_to_rgb8_strip(
                    y_view.as_slice(),
                    y_view.stride(),
                    u_view.as_slice(),
                    u_view.stride(),
                    v_view.as_slice(),
                    v_view.stride(),
                    width,
                    buffer_height,
                    y_start,
                    strip_height,
                    yuv_range,
                    yuv_matrix,
                    out_rgb,
                ),
                ChromaSampling::Cs422 => yuv_convert::yuv422_to_rgb8_strip(
                    y_view.as_slice(),
                    y_view.stride(),
                    u_view.as_slice(),
                    u_view.stride(),
                    v_view.as_slice(),
                    v_view.stride(),
                    width,
                    y_start,
                    strip_height,
                    yuv_range,
                    yuv_matrix,
                    out_rgb,
                ),
                ChromaSampling::Cs444 => yuv_convert::yuv444_to_rgb8_strip(
                    y_view.as_slice(),
                    y_view.stride(),
                    u_view.as_slice(),
                    u_view.stride(),
                    v_view.as_slice(),
                    v_view.stride(),
                    width,
                    y_start,
                    strip_height,
                    yuv_range,
                    yuv_matrix,
                    out_rgb,
                ),
                ChromaSampling::Monochrome => {
                    return Err(at(Error::Decode {
                        code: -1,
                        msg: "Monochrome should not reach strip chroma conversion",
                    }));
                }
            }
        }

        Ok(())
    }
}

/// Scale a limited-range Y value to full range (8-bit).
/// Duplicated from convert.rs to avoid making it pub.
#[inline]
fn limited_to_full_8(y: u8) -> u8 {
    let y = y as i32;
    ((y - 16).max(0) * 255 / 219).min(255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yuv_convert::{YuvMatrix, YuvRange};
    use rgb::RGB8;

    /// Verify that strip conversion produces identical output to full-frame.
    #[test]
    fn strip_matches_full_frame_420() {
        let width = 24;
        let height = 16;
        let chroma_w = width / 2;
        let chroma_h = height / 2;

        // Generate test YUV data
        let mut y_plane = vec![0u8; width * height];
        let mut u_plane = vec![128u8; chroma_w * chroma_h];
        let mut v_plane = vec![128u8; chroma_w * chroma_h];

        for y in 0..height {
            for x in 0..width {
                y_plane[y * width + x] = ((x * 10 + y * 7) % 256) as u8;
            }
        }
        for y in 0..chroma_h {
            for x in 0..chroma_w {
                u_plane[y * chroma_w + x] = ((x * 13 + y * 11 + 64) % 256) as u8;
                v_plane[y * chroma_w + x] = ((x * 17 + y * 3 + 96) % 256) as u8;
            }
        }

        // Full-frame conversion
        let full = crate::yuv_convert::yuv420_to_rgb8(
            &y_plane, width, &u_plane, chroma_w, &v_plane, chroma_w, width, height,
            YuvRange::Full, YuvMatrix::Bt709,
        );

        // Strip conversion with various strip heights
        for strip_h in [2, 4, 6, 8, 16] {
            let mut strip_result = vec![RGB8::default(); width * height];
            let mut y_start = 0;
            while y_start < height {
                let h = strip_h.min(height - y_start);
                let strip_out = &mut strip_result[y_start * width..(y_start + h) * width];
                crate::yuv_convert::yuv420_to_rgb8_strip(
                    &y_plane, width, &u_plane, chroma_w, &v_plane, chroma_w,
                    width, height, y_start, h, YuvRange::Full, YuvMatrix::Bt709,
                    strip_out,
                );
                y_start += h;
            }

            assert_eq!(
                full.buf(),
                strip_result.as_slice(),
                "Strip conversion (h={strip_h}) doesn't match full-frame"
            );
        }
    }

    /// Verify RGBA8 strip produces correct alpha=255.
    #[test]
    fn rgba_strip_has_alpha_255() {
        let width = 8;
        let height = 4;
        let y_plane = vec![128u8; width * height];
        let u_plane = vec![128u8; (width / 2) * (height / 2)];
        let v_plane = vec![128u8; (width / 2) * (height / 2)];

        let mut out = vec![Rgba { r: 0, g: 0, b: 0, a: 0 }; width * height];
        crate::yuv_convert::yuv420_to_rgba8_strip(
            &y_plane, width, &u_plane, width / 2, &v_plane, width / 2,
            width, height, 0, height, YuvRange::Full, YuvMatrix::Bt709,
            &mut out,
        );

        for px in &out {
            assert_eq!(px.a, 255, "Alpha should be 255");
        }
    }
}
