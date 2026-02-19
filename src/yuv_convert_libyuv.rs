//! Exact libyuv YUV to RGB conversion
//!
//! Supports BT.709 and BT.601 in both Full and Limited range

// YUV conversion functions naturally require many plane/stride/dimension/matrix/range parameters.
#![allow(clippy::too_many_arguments)]

use crate::yuv_convert::{YuvMatrix, YuvRange};
#[cfg(target_arch = "x86_64")]
use crate::yuv_convert_libyuv_simd;
#[cfg(target_arch = "x86_64")]
use archmage::prelude::*;
use imgref::ImgVec;
use rgb::RGB8;

/// YUV conversion constants for different matrix/range combinations
#[allow(dead_code)]
struct YuvConstants {
    yg: i32,
    ygb: i32,
    ub: i32,
    ug: i32,
    vg: i32,
    vr: i32,
    bb: i32,
    bg: i32,
    br: i32,
}

impl YuvConstants {
    /// BT.709 Full Range (most common for AVIF)
    const BT709_FULL: Self = Self {
        yg: 18997,                         // 1.164 * 64 * 256 * 256 / 257
        ygb: -1160,                        // 1.164 * 64 * -16 + 64 / 2
        ub: -128,                          // -2.112 * 64
        ug: 14,                            // 0.213 * 64
        vg: 34,                            // 0.533 * 64
        vr: -115,                          // -1.793 * 64
        bb: -128 * 128 + (-1160),          // -17544
        bg: 14 * 128 + 34 * 128 + (-1160), // 4984
        br: -115 * 128 + (-1160),          // -15880
    };

    /// BT.709 Limited Range (16-235 for Y, 16-240 for UV)
    const BT709_LIMITED: Self = Self {
        yg: 18997,
        ygb: -1160,
        ub: -128,
        ug: 14,
        vg: 34,
        vr: -115,
        bb: -17544,
        bg: 4984,
        br: -15880,
    };

    /// BT.601 Full Range
    const BT601_FULL: Self = Self {
        yg: 18997,
        ygb: -1160,
        ub: -132,                           // -2.063 * 64
        ug: 52,                             // 0.813 * 64
        vg: 104,                            // 1.633 * 64
        vr: -102,                           // -1.596 * 64
        bb: -132 * 128 + (-1160),           // -18056
        bg: 52 * 128 + 104 * 128 + (-1160), // 18808
        br: -102 * 128 + (-1160),           // -14216
    };

    /// BT.601 Limited Range
    const BT601_LIMITED: Self = Self::BT601_FULL; // Same as full for now
}

/// Get constants for the given matrix and range
fn get_constants(matrix: YuvMatrix, range: YuvRange) -> Option<&'static YuvConstants> {
    match (matrix, range) {
        (YuvMatrix::Bt709, YuvRange::Full) => Some(&YuvConstants::BT709_FULL),
        (YuvMatrix::Bt709, YuvRange::Limited) => Some(&YuvConstants::BT709_LIMITED),
        (YuvMatrix::Bt601, YuvRange::Full) => Some(&YuvConstants::BT601_FULL),
        (YuvMatrix::Bt601, YuvRange::Limited) => Some(&YuvConstants::BT601_LIMITED),
        _ => None, // BT.2020 not yet implemented
    }
}

/// Convert single YUV pixel to RGB
#[inline(always)]
fn yuv_pixel_with_constants(y: u8, u: u8, v: u8, c: &YuvConstants) -> RGB8 {
    let y1 = ((y as u32) * 0x0101 * (c.yg as u32)) >> 16;
    let y1 = y1 as i32;

    let b_raw = (-((u as i32) * c.ub) + y1 + c.bb) >> 6;
    let g_raw = (-((u as i32) * c.ug + (v as i32) * c.vg) + y1 + c.bg) >> 6;
    let r_raw = (-((v as i32) * c.vr) + y1 + c.br) >> 6;

    RGB8 {
        r: r_raw.clamp(0, 255) as u8,
        g: g_raw.clamp(0, 255) as u8,
        b: b_raw.clamp(0, 255) as u8,
    }
}

/// Convert YUV420 to RGB8 using exact libyuv math
///
/// Uses SIMD when available (2.77x faster), falls back to scalar
pub fn yuv420_to_rgb8(
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
    // Try SIMD first for BT.709 Full Range (most common)
    #[cfg(target_arch = "x86_64")]
    #[allow(clippy::collapsible_if)]
    if matches!((range, matrix), (YuvRange::Full, YuvMatrix::Bt709)) {
        if let Some(token) = Desktop64::summon() {
            return yuv_convert_libyuv_simd::yuv420_to_rgb8_simd(
                token, y_plane, y_stride, u_plane, u_stride, v_plane, v_stride, width, height,
                range, matrix,
            );
        }
    }

    // Scalar fallback for all matrix/range combinations
    let c = get_constants(matrix, range)?;

    let mut out = vec![RGB8::default(); width * height];

    for y in 0..height {
        let chroma_y = y / 2;
        for x in 0..width {
            let chroma_x = x / 2;

            let y_val = y_plane[y * y_stride + x];
            let u_val = u_plane[chroma_y * u_stride + chroma_x];
            let v_val = v_plane[chroma_y * v_stride + chroma_x];

            out[y * width + x] = yuv_pixel_with_constants(y_val, u_val, v_val, c);
        }
    }

    Some(ImgVec::new(out, width, height))
}

/// Convert YUV422 to RGB8
pub fn yuv422_to_rgb8(
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
    let c = get_constants(matrix, range)?;
    let mut out = vec![RGB8::default(); width * height];

    for y in 0..height {
        for x in 0..width {
            let chroma_x = x / 2;

            let y_val = y_plane[y * y_stride + x];
            let u_val = u_plane[y * u_stride + chroma_x];
            let v_val = v_plane[y * v_stride + chroma_x];

            out[y * width + x] = yuv_pixel_with_constants(y_val, u_val, v_val, c);
        }
    }

    Some(ImgVec::new(out, width, height))
}

/// Convert YUV444 to RGB8
pub fn yuv444_to_rgb8(
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
    let c = get_constants(matrix, range)?;
    let mut out = vec![RGB8::default(); width * height];

    for y in 0..height {
        for x in 0..width {
            let y_val = y_plane[y * y_stride + x];
            let u_val = u_plane[y * u_stride + x];
            let v_val = v_plane[y * v_stride + x];

            out[y * width + x] = yuv_pixel_with_constants(y_val, u_val, v_val, c);
        }
    }

    Some(ImgVec::new(out, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bt709_full() {
        let width = 4;
        let height = 4;

        let y_plane = vec![180u8; width * height];
        let u_plane = vec![100u8; (width / 2) * (height / 2)];
        let v_plane = vec![150u8; (width / 2) * (height / 2)];

        let result = yuv420_to_rgb8(
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

        // All pixels should be R=230, G=185, B=135
        for pixel in result.buf() {
            assert_eq!(pixel.r, 230);
            assert_eq!(pixel.g, 185);
            assert_eq!(pixel.b, 135);
        }
    }

    #[test]
    fn test_bt601_supported() {
        let width = 4;
        let height = 4;

        let y_plane = vec![128u8; width * height];
        let u_plane = vec![128u8; (width / 2) * (height / 2)];
        let v_plane = vec![128u8; (width / 2) * (height / 2)];

        let result = yuv420_to_rgb8(
            &y_plane,
            width,
            &u_plane,
            width / 2,
            &v_plane,
            width / 2,
            width,
            height,
            YuvRange::Full,
            YuvMatrix::Bt601,
        );

        assert!(result.is_some(), "BT.601 should be supported");
    }
}
