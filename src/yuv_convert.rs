//! YUV to RGB color space conversion
//!
//! Implements standard color space conversions for AVIF/AV1 images.
//!
//! References:
//! - ITU-R BT.601 (SD video)
//! - ITU-R BT.709 (HD video)
//! - ITU-R BT.2020 (UHD video)

use rgb::{RGB8, RGBA8, RGB16, RGBA16};
use imgref::ImgVec;

/// YUV color range
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvRange {
    /// Limited/studio range: Y [16..235], UV [16..240] for 8-bit
    Limited,
    /// Full range: Y [0..255], UV [0..255] for 8-bit
    Full,
}

/// YUV matrix coefficients (color space)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvMatrix {
    /// ITU-R BT.601 (SD video, NTSC/PAL)
    Bt601,
    /// ITU-R BT.709 (HD video)
    Bt709,
    /// ITU-R BT.2020 (UHD video, HDR)
    Bt2020,
}

/// Chroma subsampling format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaSubsampling {
    /// 4:4:4 - no subsampling
    Cs444,
    /// 4:2:2 - horizontal subsampling
    Cs422,
    /// 4:2:0 - horizontal and vertical subsampling
    Cs420,
}

/// Convert YUV420 to RGB8
///
/// # Arguments
/// * `y_plane` - Luma plane (full resolution)
/// * `y_stride` - Y plane stride in bytes
/// * `u_plane` - U chroma plane (half resolution)
/// * `u_stride` - U plane stride in bytes
/// * `v_plane` - V chroma plane (half resolution)
/// * `v_stride` - V plane stride in bytes
/// * `width` - Image width
/// * `height` - Image height
/// * `range` - Color range (Limited or Full)
/// * `matrix` - Matrix coefficients (BT.601, BT.709, or BT.2020)
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
) -> ImgVec<RGB8> {
    let mut out = vec![RGB8::default(); width * height];

    // Get conversion coefficients
    let (kr, kb) = matrix_coefficients(matrix);
    let kg = 1.0 - kr - kb;

    for y in 0..height {
        for x in 0..width {
            // Get Y value
            let y_val = y_plane[y * y_stride + x] as f32;

            // Get UV values (with chroma upsampling)
            // For 4:2:0, chroma is at half resolution
            let u_x = x / 2;
            let u_y = y / 2;
            let u_val = u_plane[u_y * u_stride + u_x] as f32;
            let v_val = v_plane[u_y * v_stride + u_x] as f32;

            // Convert to RGB
            let (r, g, b) = yuv_to_rgb(y_val, u_val, v_val, kr, kg, kb, range);

            out[y * width + x] = RGB8 { r, g, b };
        }
    }

    ImgVec::new(out, width, height)
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
) -> ImgVec<RGB8> {
    let mut out = vec![RGB8::default(); width * height];

    let (kr, kb) = matrix_coefficients(matrix);
    let kg = 1.0 - kr - kb;

    for y in 0..height {
        for x in 0..width {
            let y_val = y_plane[y * y_stride + x] as f32;

            // For 4:2:2, chroma is at half horizontal resolution
            let u_x = x / 2;
            let u_val = u_plane[y * u_stride + u_x] as f32;
            let v_val = v_plane[y * v_stride + u_x] as f32;

            let (r, g, b) = yuv_to_rgb(y_val, u_val, v_val, kr, kg, kb, range);

            out[y * width + x] = RGB8 { r, g, b };
        }
    }

    ImgVec::new(out, width, height)
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
) -> ImgVec<RGB8> {
    let mut out = vec![RGB8::default(); width * height];

    let (kr, kb) = matrix_coefficients(matrix);
    let kg = 1.0 - kr - kb;

    for y in 0..height {
        for x in 0..width {
            let y_val = y_plane[y * y_stride + x] as f32;
            let u_val = u_plane[y * u_stride + x] as f32;
            let v_val = v_plane[y * v_stride + x] as f32;

            let (r, g, b) = yuv_to_rgb(y_val, u_val, v_val, kr, kg, kb, range);

            out[y * width + x] = RGB8 { r, g, b };
        }
    }

    ImgVec::new(out, width, height)
}

/// Get matrix coefficients (Kr, Kb) for the specified color space
fn matrix_coefficients(matrix: YuvMatrix) -> (f32, f32) {
    match matrix {
        // ITU-R BT.601 (SD)
        YuvMatrix::Bt601 => (0.299, 0.114),
        // ITU-R BT.709 (HD)
        YuvMatrix::Bt709 => (0.2126, 0.0722),
        // ITU-R BT.2020 (UHD)
        YuvMatrix::Bt2020 => (0.2627, 0.0593),
    }
}

/// Convert YUV to RGB using the given matrix coefficients
///
/// Formula for Full range:
/// ```text
/// R = Y + Vr * (V - 128)
/// G = Y + Ug * (U - 128) + Vg * (V - 128)
/// B = Y + Ub * (U - 128)
///
/// where:
/// Vr = 2 * (1 - Kr)
/// Ug = -2 * Kb * (1 - Kb) / Kg
/// Vg = -2 * Kr * (1 - Kr) / Kg
/// Ub = 2 * (1 - Kb)
/// ```
fn yuv_to_rgb(
    y: f32,
    u: f32,
    v: f32,
    kr: f32,
    kg: f32,
    kb: f32,
    range: YuvRange,
) -> (u8, u8, u8) {
    // Normalize to [0..1] range based on color range
    let (y_norm, u_norm, v_norm) = match range {
        YuvRange::Full => {
            // Full range: Y, U, V are all in [0..255]
            // Center U and V around 0
            let y = y / 255.0;
            let u = (u - 128.0) / 255.0;
            let v = (v - 128.0) / 255.0;
            (y, u, v)
        }
        YuvRange::Limited => {
            // Limited range: Y in [16..235], UV in [16..240]
            let y = (y - 16.0) / 219.0;
            let u = (u - 128.0) / 224.0;
            let v = (v - 128.0) / 224.0;
            (y, u, v)
        }
    };

    // Calculate conversion coefficients
    let vr = 2.0 * (1.0 - kr);
    let ug = -2.0 * kb * (1.0 - kb) / kg;
    let vg = -2.0 * kr * (1.0 - kr) / kg;
    let ub = 2.0 * (1.0 - kb);

    // Convert to RGB
    let r = y_norm + vr * v_norm;
    let g = y_norm + ug * u_norm + vg * v_norm;
    let b = y_norm + ub * u_norm;

    // Clamp and convert to u8
    let r = (r * 255.0).round().clamp(0.0, 255.0) as u8;
    let g = (g * 255.0).round().clamp(0.0, 255.0) as u8;
    let b = (b * 255.0).round().clamp(0.0, 255.0) as u8;

    (r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yuv_to_rgb_gray() {
        // YUV (128, 128, 128) should be gray (128, 128, 128)
        let (r, g, b) = yuv_to_rgb(128.0, 128.0, 128.0, 0.299, 0.587, 0.114, YuvRange::Full);
        assert_eq!(r, 128);
        assert_eq!(g, 128);
        assert_eq!(b, 128);
    }

    #[test]
    fn test_yuv_to_rgb_black() {
        // YUV (0, 128, 128) should be black (0, 0, 0)
        let (r, g, b) = yuv_to_rgb(0.0, 128.0, 128.0, 0.299, 0.587, 0.114, YuvRange::Full);
        assert_eq!(r, 0);
        assert_eq!(g, 0);
        assert_eq!(b, 0);
    }

    #[test]
    fn test_yuv_to_rgb_white() {
        // YUV (255, 128, 128) should be white (255, 255, 255)
        let (r, g, b) = yuv_to_rgb(255.0, 128.0, 128.0, 0.299, 0.587, 0.114, YuvRange::Full);
        assert_eq!(r, 255);
        assert_eq!(g, 255);
        assert_eq!(b, 255);
    }
}
