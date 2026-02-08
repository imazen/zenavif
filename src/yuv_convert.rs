//! YUV to RGB color space conversion
//!
//! Implements standard color space conversions for AVIF/AV1 images.
//! Includes SIMD-optimized paths using AVX2/FMA when available.
//!
//! References:
//! - ITU-R BT.601 (SD video)
//! - ITU-R BT.709 (HD video)
//! - ITU-R BT.2020 (UHD video)

use archmage::prelude::*;
use imgref::ImgVec;
use magetypes::simd::f32x8;
use rgb::{RGB8, RGB16, RGBA8, RGBA16};

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

/// Convert YUV420 to RGB8 with bilinear chroma upsampling
///
/// Automatically dispatches to SIMD (AVX2/FMA) or scalar implementation.
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
    if let Some(token) = Desktop64::summon() {
        yuv420_to_rgb8_simd(
            token, y_plane, y_stride, u_plane, u_stride, v_plane, v_stride, width, height, range,
            matrix,
        )
    } else {
        yuv420_to_rgb8_scalar(
            y_plane, y_stride, u_plane, u_stride, v_plane, v_stride, width, height, range,
            matrix,
        )
    }
}

/// SIMD implementation of YUV420 to RGB8 conversion (AVX2/FMA)
#[arcane]
fn yuv420_to_rgb8_simd(
    token: Desktop64,
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
    // Pad width to multiple of 8 for SIMD processing
    let simd_width = (width + 7) & !7;
    let mut out = vec![RGB8::default(); simd_width * height];

    // Get conversion coefficients
    let (kr, kb) = matrix_coefficients(matrix);
    let kg = 1.0 - kr - kb;

    // Chroma dimensions
    let chroma_width = (width + 1) / 2;
    let chroma_height = (height + 1) / 2;

    // Process row by row
    for y_pos in 0..height {
        let row_start = y_pos * simd_width;

        // Process 8 pixels at a time with SIMD
        let mut x_pos = 0;
        while x_pos + 8 <= width {
            // Gather Y values for 8 pixels
            let y_idx = y_pos * y_stride + x_pos;
            let mut y_vals = [0f32; 8];
            for i in 0..8 {
                y_vals[i] = y_plane[y_idx + i] as f32;
            }

            // Vectorized chroma sampling for 8 pixels
            let (u_vec, v_vec) = bilinear_chroma_sample_x8(
                token,
                x_pos,
                y_pos,
                chroma_width,
                chroma_height,
                u_plane,
                u_stride,
                v_plane,
                v_stride,
            );

            // Load Y values into SIMD vector
            let y_vec = f32x8::from_array(token, y_vals);

            // Convert YUV to RGB using SIMD
            let (r_vec, g_vec, b_vec) =
                yuv_to_rgb_simd(token, y_vec, u_vec, v_vec, kr, kg, kb, range);

            // Convert f32 vectors to u8 and store
            let r_vals = r_vec.to_array();
            let g_vals = g_vec.to_array();
            let b_vals = b_vec.to_array();

            for i in 0..8 {
                out[row_start + x_pos + i] = RGB8 {
                    r: (r_vals[i].round().clamp(0.0, 255.0)) as u8,
                    g: (g_vals[i].round().clamp(0.0, 255.0)) as u8,
                    b: (b_vals[i].round().clamp(0.0, 255.0)) as u8,
                };
            }

            x_pos += 8;
        }

        // Handle remaining pixels with scalar code
        while x_pos < width {
            let y_val = y_plane[y_pos * y_stride + x_pos] as f32;
            let (u_val, v_val) = bilinear_chroma_sample(
                token,
                x_pos,
                y_pos,
                chroma_width,
                chroma_height,
                u_plane,
                u_stride,
                v_plane,
                v_stride,
            );

            let (r, g, b) = yuv_to_rgb(y_val, u_val, v_val, kr, kg, kb, range);
            out[row_start + x_pos] = RGB8 { r, g, b };

            x_pos += 1;
        }
    }

    // If width was padded, crop to actual width
    if width == simd_width {
        ImgVec::new(out, width, height)
    } else {
        // Copy only the actual pixels (excluding padding)
        let mut cropped = vec![RGB8::default(); width * height];
        for y in 0..height {
            let src_start = y * simd_width;
            let dst_start = y * width;
            cropped[dst_start..dst_start + width]
                .copy_from_slice(&out[src_start..src_start + width]);
        }
        ImgVec::new(cropped, width, height)
    }
}

/// SIMD helper: Bilinear chroma sample for 8 consecutive pixels
#[rite]
fn bilinear_chroma_sample_x8(
    token: Desktop64,
    x_start: usize,
    y: usize,
    chroma_width: usize,
    chroma_height: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
) -> (f32x8, f32x8) {
    // Calculate chroma y position (same for all 8 pixels in this row)
    let chroma_y_raw = (y as f32 + 0.5) * 0.5 - 0.5;
    let chroma_y = chroma_y_raw.max(0.0).min(chroma_height as f32 - 1.0);
    let cy0 = chroma_y.floor() as usize;
    let cy1 = (cy0 + 1).min(chroma_height - 1);
    let fy = chroma_y - cy0 as f32;
    let fy1 = 1.0 - fy;

    // Process 8 pixels
    let mut u_vals = [0f32; 8];
    let mut v_vals = [0f32; 8];

    for i in 0..8 {
        let x = x_start + i;

        // Calculate chroma x position
        let chroma_x_raw = (x as f32 + 0.5) * 0.5 - 0.5;
        let chroma_x = chroma_x_raw.max(0.0).min(chroma_width as f32 - 1.0);
        let cx0 = chroma_x.floor() as usize;
        let cx1 = (cx0 + 1).min(chroma_width - 1);
        let fx = chroma_x - cx0 as f32;
        let fx1 = 1.0 - fx;

        // Load 4 surrounding chroma samples
        let u00 = u_plane[cy0 * u_stride + cx0] as f32;
        let u01 = u_plane[cy0 * u_stride + cx1] as f32;
        let u10 = u_plane[cy1 * u_stride + cx0] as f32;
        let u11 = u_plane[cy1 * u_stride + cx1] as f32;

        let v00 = v_plane[cy0 * v_stride + cx0] as f32;
        let v01 = v_plane[cy0 * v_stride + cx1] as f32;
        let v10 = v_plane[cy1 * v_stride + cx0] as f32;
        let v11 = v_plane[cy1 * v_stride + cx1] as f32;

        // Bilinear interpolation
        u_vals[i] = u00 * fx1 * fy1 + u01 * fx * fy1 + u10 * fx1 * fy + u11 * fx * fy;
        v_vals[i] = v00 * fx1 * fy1 + v01 * fx * fy1 + v10 * fx1 * fy + v11 * fx * fy;
    }

    (f32x8::from_array(token, u_vals), f32x8::from_array(token, v_vals))
}

/// SIMD helper: Bilinear chroma sample for a single pixel
#[rite]
fn bilinear_chroma_sample(
    _token: Desktop64,
    x: usize,
    y: usize,
    chroma_width: usize,
    chroma_height: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
) -> (f32, f32) {
    // Map luma position to chroma position (with 0.5 offset for centering)
    let chroma_x_raw = (x as f32 + 0.5) * 0.5 - 0.5;
    let chroma_y_raw = (y as f32 + 0.5) * 0.5 - 0.5;

    // Clamp to valid range BEFORE calculating floor
    let chroma_x = chroma_x_raw.max(0.0).min(chroma_width as f32 - 1.0);
    let chroma_y = chroma_y_raw.max(0.0).min(chroma_height as f32 - 1.0);

    // Get the 4 surrounding chroma samples
    let cx0 = chroma_x.floor() as usize;
    let cy0 = chroma_y.floor() as usize;
    let cx1 = (cx0 + 1).min(chroma_width - 1);
    let cy1 = (cy0 + 1).min(chroma_height - 1);

    // Interpolation weights
    let fx = chroma_x - cx0 as f32;
    let fy = chroma_y - cy0 as f32;
    let fx1 = 1.0 - fx;
    let fy1 = 1.0 - fy;

    // Sample the 4 surrounding chroma values
    let u00 = u_plane[cy0 * u_stride + cx0] as f32;
    let u01 = u_plane[cy0 * u_stride + cx1] as f32;
    let u10 = u_plane[cy1 * u_stride + cx0] as f32;
    let u11 = u_plane[cy1 * u_stride + cx1] as f32;

    let v00 = v_plane[cy0 * v_stride + cx0] as f32;
    let v01 = v_plane[cy0 * v_stride + cx1] as f32;
    let v10 = v_plane[cy1 * v_stride + cx0] as f32;
    let v11 = v_plane[cy1 * v_stride + cx1] as f32;

    // Bilinear interpolation
    let u_val = u00 * fx1 * fy1 + u01 * fx * fy1 + u10 * fx1 * fy + u11 * fx * fy;
    let v_val = v00 * fx1 * fy1 + v01 * fx * fy1 + v10 * fx1 * fy + v11 * fx * fy;

    (u_val, v_val)
}

/// SIMD helper: Convert YUV to RGB for 8 pixels at once
#[rite]
fn yuv_to_rgb_simd(
    token: Desktop64,
    y: f32x8,
    u: f32x8,
    v: f32x8,
    kr: f32,
    kg: f32,
    kb: f32,
    range: YuvRange,
) -> (f32x8, f32x8, f32x8) {
    // Normalize to [0..1] range based on color range
    let (y_norm, u_norm, v_norm) = match range {
        YuvRange::Full => {
            // Full range: Y, U, V are all in [0..255]
            let scale = f32x8::splat(token, 1.0 / 255.0);
            let center = f32x8::splat(token, 128.0);

            let y_norm = y * scale;
            let u_norm = (u - center) * scale;
            let v_norm = (v - center) * scale;
            (y_norm, u_norm, v_norm)
        }
        YuvRange::Limited => {
            // Limited range: Y in [16..235], UV in [16..240]
            let y_offset = f32x8::splat(token, 16.0);
            let uv_center = f32x8::splat(token, 128.0);
            let y_scale = f32x8::splat(token, 1.0 / 219.0);
            let uv_scale = f32x8::splat(token, 1.0 / 224.0);

            let y_norm = (y - y_offset) * y_scale;
            let u_norm = (u - uv_center) * uv_scale;
            let v_norm = (v - uv_center) * uv_scale;
            (y_norm, u_norm, v_norm)
        }
    };

    // Calculate conversion coefficients
    let vr = 2.0 * (1.0 - kr);
    let ug = -2.0 * kb * (1.0 - kb) / kg;
    let vg = -2.0 * kr * (1.0 - kr) / kg;
    let ub = 2.0 * (1.0 - kb);

    // Broadcast coefficients to SIMD vectors
    let vr_vec = f32x8::splat(token, vr);
    let ug_vec = f32x8::splat(token, ug);
    let vg_vec = f32x8::splat(token, vg);
    let ub_vec = f32x8::splat(token, ub);

    // Convert to RGB using FMA instructions
    // R = Y + Vr * V
    let r = v_norm.mul_add(vr_vec, y_norm);

    // G = Y + Ug * U + Vg * V
    let g_temp = u_norm * ug_vec;
    let g = v_norm.mul_add(vg_vec, g_temp + y_norm);

    // B = Y + Ub * U
    let b = u_norm.mul_add(ub_vec, y_norm);

    // Scale back to [0..255] range
    let scale_255 = f32x8::splat(token, 255.0);
    let r_scaled = r * scale_255;
    let g_scaled = g * scale_255;
    let b_scaled = b * scale_255;

    (r_scaled, g_scaled, b_scaled)
}

/// Scalar implementation of YUV420 to RGB8 conversion with bilinear chroma upsampling
fn yuv420_to_rgb8_scalar(
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

    // Chroma dimensions (half of luma for 4:2:0)
    let chroma_width = (width + 1) / 2;
    let chroma_height = (height + 1) / 2;

    for y in 0..height {
        for x in 0..width {
            // Get Y value
            let y_val = y_plane[y * y_stride + x] as f32;

            // Bilinear chroma upsampling
            // Map luma position to chroma position (with 0.5 offset for centering)
            let chroma_x_raw = (x as f32 + 0.5) * 0.5 - 0.5;
            let chroma_y_raw = (y as f32 + 0.5) * 0.5 - 0.5;

            // Clamp to valid range BEFORE calculating floor
            let chroma_x = chroma_x_raw.max(0.0).min(chroma_width as f32 - 1.0);
            let chroma_y = chroma_y_raw.max(0.0).min(chroma_height as f32 - 1.0);

            // Get the 4 surrounding chroma samples
            let cx0 = chroma_x.floor() as usize;
            let cy0 = chroma_y.floor() as usize;
            let cx1 = (cx0 + 1).min(chroma_width - 1);
            let cy1 = (cy0 + 1).min(chroma_height - 1);

            // Interpolation weights (now guaranteed to be in [0, 1])
            let fx = chroma_x - cx0 as f32;
            let fy = chroma_y - cy0 as f32;
            let fx1 = 1.0 - fx;
            let fy1 = 1.0 - fy;

            // Sample the 4 surrounding chroma values
            let u00 = u_plane[cy0 * u_stride + cx0] as f32;
            let u01 = u_plane[cy0 * u_stride + cx1] as f32;
            let u10 = u_plane[cy1 * u_stride + cx0] as f32;
            let u11 = u_plane[cy1 * u_stride + cx1] as f32;

            let v00 = v_plane[cy0 * v_stride + cx0] as f32;
            let v01 = v_plane[cy0 * v_stride + cx1] as f32;
            let v10 = v_plane[cy1 * v_stride + cx0] as f32;
            let v11 = v_plane[cy1 * v_stride + cx1] as f32;

            // Bilinear interpolation
            let u_val = u00 * fx1 * fy1 + u01 * fx * fy1 + u10 * fx1 * fy + u11 * fx * fy;
            let v_val = v00 * fx1 * fy1 + v01 * fx * fy1 + v10 * fx1 * fy + v11 * fx * fy;

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
fn yuv_to_rgb(y: f32, u: f32, v: f32, kr: f32, kg: f32, kb: f32, range: YuvRange) -> (u8, u8, u8) {
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
