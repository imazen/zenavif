//! Auto-vectorized libyuv using chunks_exact
//!
//! Test if compiler can auto-vectorize with better hints

#![allow(clippy::too_many_arguments)]

use crate::yuv_convert::{YuvMatrix, YuvRange};
use imgref::ImgVec;
use rgb::RGB8;

const YG: i32 = 18997;
const YGB: i32 = -1160;
const UB: i32 = -128;
const UG: i32 = 14;
const VG: i32 = 34;
const VR: i32 = -115;
const BB: i32 = UB * 128 + YGB;
const BG: i32 = UG * 128 + VG * 128 + YGB;
const BR: i32 = VR * 128 + YGB;

#[inline(always)]
fn yuv_pixel(y: u8, u: u8, v: u8) -> RGB8 {
    let y1 = ((y as u32) * 0x0101 * (YG as u32)) >> 16;
    let y1 = y1 as i32;

    let b_raw = (-((u as i32) * UB) + y1 + BB) >> 6;
    let g_raw = (-((u as i32) * UG + (v as i32) * VG) + y1 + BG) >> 6;
    let r_raw = (-((v as i32) * VR) + y1 + BR) >> 6;

    RGB8 {
        r: r_raw.clamp(0, 255) as u8,
        g: g_raw.clamp(0, 255) as u8,
        b: b_raw.clamp(0, 255) as u8,
    }
}

/// Auto-vectorizable version using chunks_exact
pub fn yuv420_to_rgb8_autovec(
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: usize,
    height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
) -> Option<ImgVec<RGB8>> {
    if !matches!((range, matrix), (YuvRange::Full, YuvMatrix::Bt709)) {
        return None;
    }

    let mut out = vec![RGB8::default(); width * height];

    for y in (0..height).step_by(2) {
        let y0 = y;
        let y1 = (y + 1).min(height - 1);
        let chroma_y = y / 2;

        // Process both rows
        for row in [y0, y1] {
            if row >= height {
                continue;
            }

            let y_row = &y_plane[row * y_stride..][..width];
            let u_row = &u_plane[chroma_y * u_stride..][..width / 2];
            let v_row = &v_plane[chroma_y * v_stride..][..width / 2];
            let out_row = &mut out[row * width..][..width];

            // Process 8 pixels at a time with chunks_exact
            let chunks = y_row.chunks_exact(8);
            let u_chunks = u_row.chunks_exact(4);
            let v_chunks = v_row.chunks_exact(4);
            let out_chunks = out_row.chunks_exact_mut(8);

            for (((y_chunk, u_chunk), v_chunk), out_chunk) in
                chunks.zip(u_chunks).zip(v_chunks).zip(out_chunks)
            {
                // Process 8 pixels (compiler should auto-vectorize this loop)
                for i in 0..8 {
                    let chroma_i = i / 2;
                    out_chunk[i] = yuv_pixel(y_chunk[i], u_chunk[chroma_i], v_chunk[chroma_i]);
                }
            }

            // Handle remainder
            let remainder_start = (width / 8) * 8;
            for x in remainder_start..width {
                let chroma_x = x / 2;
                out_row[x] = yuv_pixel(y_row[x], u_row[chroma_x], v_row[chroma_x]);
            }
        }
    }

    Some(ImgVec::new(out, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autovec() {
        let width = 16;
        let height = 16;

        let y_plane = vec![180u8; width * height];
        let u_plane = vec![100u8; (width / 2) * (height / 2)];
        let v_plane = vec![150u8; (width / 2) * (height / 2)];

        let result = yuv420_to_rgb8_autovec(
            &y_plane,
            width,
            &u_plane,
            width / 2,
            &v_plane,
            width / 2,
            width,
            height,
            YuvRange::Full,
            YuvMatrix::Bt709,
        )
        .unwrap();

        for pixel in result.buf() {
            assert_eq!(pixel.r, 230);
            assert_eq!(pixel.g, 185);
            assert_eq!(pixel.b, 135);
        }
    }
}
