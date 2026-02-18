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
#[cfg(target_arch = "x86_64")]
use magetypes::simd::f32x8;
use rgb::RGB8;

#[cfg(target_arch = "wasm32")]
use archmage::Wasm128Token;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use core::arch::wasm32::*;

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
    #[cfg(target_arch = "x86_64")]
    if let Some(token) = Desktop64::summon() {
        return yuv420_to_rgb8_simd(
            token, y_plane, y_stride, u_plane, u_stride, v_plane, v_stride, width, height, range,
            matrix,
        );
    }

    #[cfg(target_arch = "wasm32")]
    if let Some(token) = Wasm128Token::summon() {
        return yuv420_to_rgb8_wasm128(
            token, y_plane, y_stride, u_plane, u_stride, v_plane, v_stride, width, height, range,
            matrix,
        );
    }

    yuv420_to_rgb8_scalar(
        y_plane, y_stride, u_plane, u_stride, v_plane, v_stride, width, height, range, matrix,
    )
}

/// SIMD implementation of YUV420 to RGB8 conversion (AVX2/FMA)
#[cfg(target_arch = "x86_64")]
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

            // Clamp and round using SIMD
            let zero = f32x8::splat(token, 0.0);
            let max_val = f32x8::splat(token, 255.0);
            let r_clamped = r_vec.clamp(zero, max_val).round();
            let g_clamped = g_vec.clamp(zero, max_val).round();
            let b_clamped = b_vec.clamp(zero, max_val).round();

            // Convert to u8 and store
            let r_vals = r_clamped.to_array();
            let g_vals = g_clamped.to_array();
            let b_vals = b_clamped.to_array();

            for i in 0..8 {
                out[row_start + x_pos + i] = RGB8 {
                    r: r_vals[i] as u8,
                    g: g_vals[i] as u8,
                    b: b_vals[i] as u8,
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
#[cfg(target_arch = "x86_64")]
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

    // Gather data for 8 pixels
    let mut fx_vals = [0f32; 8];
    let mut u00_vals = [0f32; 8];
    let mut u01_vals = [0f32; 8];
    let mut u10_vals = [0f32; 8];
    let mut u11_vals = [0f32; 8];
    let mut v00_vals = [0f32; 8];
    let mut v01_vals = [0f32; 8];
    let mut v10_vals = [0f32; 8];
    let mut v11_vals = [0f32; 8];

    for i in 0..8 {
        let x = x_start + i;

        // Calculate chroma x position
        let chroma_x_raw = (x as f32 + 0.5) * 0.5 - 0.5;
        let chroma_x = chroma_x_raw.max(0.0).min(chroma_width as f32 - 1.0);
        let cx0 = chroma_x.floor() as usize;
        let cx1 = (cx0 + 1).min(chroma_width - 1);
        fx_vals[i] = chroma_x - cx0 as f32;

        // Load 4 surrounding chroma samples
        u00_vals[i] = u_plane[cy0 * u_stride + cx0] as f32;
        u01_vals[i] = u_plane[cy0 * u_stride + cx1] as f32;
        u10_vals[i] = u_plane[cy1 * u_stride + cx0] as f32;
        u11_vals[i] = u_plane[cy1 * u_stride + cx1] as f32;

        v00_vals[i] = v_plane[cy0 * v_stride + cx0] as f32;
        v01_vals[i] = v_plane[cy0 * v_stride + cx1] as f32;
        v10_vals[i] = v_plane[cy1 * v_stride + cx0] as f32;
        v11_vals[i] = v_plane[cy1 * v_stride + cx1] as f32;
    }

    // Load into SIMD vectors
    let fx = f32x8::from_array(token, fx_vals);
    let u00 = f32x8::from_array(token, u00_vals);
    let u01 = f32x8::from_array(token, u01_vals);
    let u10 = f32x8::from_array(token, u10_vals);
    let u11 = f32x8::from_array(token, u11_vals);
    let v00 = f32x8::from_array(token, v00_vals);
    let v01 = f32x8::from_array(token, v01_vals);
    let v10 = f32x8::from_array(token, v10_vals);
    let v11 = f32x8::from_array(token, v11_vals);

    // Precompute weights
    let one = f32x8::splat(token, 1.0);
    let fx1 = one - fx;
    let fy_vec = f32x8::splat(token, fy);
    let fy1_vec = f32x8::splat(token, 1.0 - fy);

    // Bilinear interpolation using FMA: u00*(1-fx)*(1-fy) + u01*fx*(1-fy) + u10*(1-fx)*fy + u11*fx*fy
    // Rearrange: ((u00*(1-fx) + u01*fx)*(1-fy)) + ((u10*(1-fx) + u11*fx)*fy)
    let u_top = u01.mul_add(fx, u00 * fx1); // u00*(1-fx) + u01*fx
    let u_bot = u11.mul_add(fx, u10 * fx1); // u10*(1-fx) + u11*fx
    let u_result = u_bot.mul_add(fy_vec, u_top * fy1_vec); // u_top*(1-fy) + u_bot*fy

    let v_top = v01.mul_add(fx, v00 * fx1);
    let v_bot = v11.mul_add(fx, v10 * fx1);
    let v_result = v_bot.mul_add(fy_vec, v_top * fy1_vec);

    (u_result, v_result)
}

/// SIMD helper: Bilinear chroma sample for a single pixel
#[cfg(target_arch = "x86_64")]
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
#[cfg(target_arch = "x86_64")]
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

/// wasm128 SIMD implementation of YUV420 to RGB8 conversion
///
/// Processes 4 pixels at a time using f32x4 wasm SIMD intrinsics.
/// Same bilinear chroma upsampling as the AVX2 path but at half width.
#[cfg(target_arch = "wasm32")]
#[arcane]
fn yuv420_to_rgb8_wasm128(
    _token: Wasm128Token,
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

    let chroma_width = (width + 1) / 2;
    let chroma_height = (height + 1) / 2;

    // Precompute conversion coefficients
    let vr = 2.0 * (1.0 - kr);
    let ug = -2.0 * kb * (1.0 - kb) / kg;
    let vg = -2.0 * kr * (1.0 - kr) / kg;
    let ub = 2.0 * (1.0 - kb);

    let vr_vec = f32x4_splat(vr);
    let ug_vec = f32x4_splat(ug);
    let vg_vec = f32x4_splat(vg);
    let ub_vec = f32x4_splat(ub);
    let scale_255 = f32x4_splat(255.0);
    let zero_v = f32x4_splat(0.0);
    let max_255 = f32x4_splat(255.0);

    // Range normalization constants
    let (y_offset, y_scale, uv_center, uv_scale) = match range {
        YuvRange::Full => (
            f32x4_splat(0.0),
            f32x4_splat(1.0 / 255.0),
            f32x4_splat(128.0),
            f32x4_splat(1.0 / 255.0),
        ),
        YuvRange::Limited => (
            f32x4_splat(16.0),
            f32x4_splat(1.0 / 219.0),
            f32x4_splat(128.0),
            f32x4_splat(1.0 / 224.0),
        ),
    };

    for y_pos in 0..height {
        let row_start = y_pos * width;

        // Chroma y position (same for all pixels in this row)
        let chroma_y_raw = (y_pos as f32 + 0.5) * 0.5 - 0.5;
        let chroma_y = chroma_y_raw.max(0.0).min(chroma_height as f32 - 1.0);
        let cy0 = chroma_y.floor() as usize;
        let cy1 = (cy0 + 1).min(chroma_height - 1);
        let fy = chroma_y - cy0 as f32;

        let mut x_pos = 0;
        // Process 4 pixels at a time
        while x_pos + 4 <= width {
            // Gather Y values
            let y_idx = y_pos * y_stride + x_pos;
            let y_arr = [
                y_plane[y_idx] as f32,
                y_plane[y_idx + 1] as f32,
                y_plane[y_idx + 2] as f32,
                y_plane[y_idx + 3] as f32,
            ];
            let y_vec = f32x4(y_arr[0], y_arr[1], y_arr[2], y_arr[3]);

            // Gather chroma samples for bilinear interpolation
            let mut u_vals = [0f32; 4];
            let mut v_vals = [0f32; 4];
            for i in 0..4 {
                let x = x_pos + i;
                let chroma_x_raw = (x as f32 + 0.5) * 0.5 - 0.5;
                let chroma_x = chroma_x_raw.max(0.0).min(chroma_width as f32 - 1.0);
                let cx0 = chroma_x.floor() as usize;
                let cx1 = (cx0 + 1).min(chroma_width - 1);
                let fx = chroma_x - cx0 as f32;
                let fx1 = 1.0 - fx;
                let fy1 = 1.0 - fy;

                let u00 = u_plane[cy0 * u_stride + cx0] as f32;
                let u01 = u_plane[cy0 * u_stride + cx1] as f32;
                let u10 = u_plane[cy1 * u_stride + cx0] as f32;
                let u11 = u_plane[cy1 * u_stride + cx1] as f32;
                u_vals[i] = u00 * fx1 * fy1 + u01 * fx * fy1 + u10 * fx1 * fy + u11 * fx * fy;

                let v00 = v_plane[cy0 * v_stride + cx0] as f32;
                let v01 = v_plane[cy0 * v_stride + cx1] as f32;
                let v10 = v_plane[cy1 * v_stride + cx0] as f32;
                let v11 = v_plane[cy1 * v_stride + cx1] as f32;
                v_vals[i] = v00 * fx1 * fy1 + v01 * fx * fy1 + v10 * fx1 * fy + v11 * fx * fy;
            }

            let u_vec = f32x4(u_vals[0], u_vals[1], u_vals[2], u_vals[3]);
            let v_vec = f32x4(v_vals[0], v_vals[1], v_vals[2], v_vals[3]);

            // Normalize YUV
            let y_norm = f32x4_mul(f32x4_sub(y_vec, y_offset), y_scale);
            let u_norm = f32x4_mul(f32x4_sub(u_vec, uv_center), uv_scale);
            let v_norm = f32x4_mul(f32x4_sub(v_vec, uv_center), uv_scale);

            // Convert to RGB: R = Y + Vr*V, G = Y + Ug*U + Vg*V, B = Y + Ub*U
            let r = f32x4_add(y_norm, f32x4_mul(v_norm, vr_vec));
            let g = f32x4_add(f32x4_add(y_norm, f32x4_mul(u_norm, ug_vec)), f32x4_mul(v_norm, vg_vec));
            let b = f32x4_add(y_norm, f32x4_mul(u_norm, ub_vec));

            // Scale to [0..255], clamp, round
            let r_scaled = f32x4_nearest(f32x4_max(f32x4_min(f32x4_mul(r, scale_255), max_255), zero_v));
            let g_scaled = f32x4_nearest(f32x4_max(f32x4_min(f32x4_mul(g, scale_255), max_255), zero_v));
            let b_scaled = f32x4_nearest(f32x4_max(f32x4_min(f32x4_mul(b, scale_255), max_255), zero_v));

            // Extract and store
            let r0 = f32x4_extract_lane::<0>(r_scaled) as u8;
            let r1 = f32x4_extract_lane::<1>(r_scaled) as u8;
            let r2 = f32x4_extract_lane::<2>(r_scaled) as u8;
            let r3 = f32x4_extract_lane::<3>(r_scaled) as u8;

            let g0 = f32x4_extract_lane::<0>(g_scaled) as u8;
            let g1 = f32x4_extract_lane::<1>(g_scaled) as u8;
            let g2 = f32x4_extract_lane::<2>(g_scaled) as u8;
            let g3 = f32x4_extract_lane::<3>(g_scaled) as u8;

            let b0 = f32x4_extract_lane::<0>(b_scaled) as u8;
            let b1 = f32x4_extract_lane::<1>(b_scaled) as u8;
            let b2 = f32x4_extract_lane::<2>(b_scaled) as u8;
            let b3 = f32x4_extract_lane::<3>(b_scaled) as u8;

            out[row_start + x_pos] = RGB8 { r: r0, g: g0, b: b0 };
            out[row_start + x_pos + 1] = RGB8 { r: r1, g: g1, b: b1 };
            out[row_start + x_pos + 2] = RGB8 { r: r2, g: g2, b: b2 };
            out[row_start + x_pos + 3] = RGB8 { r: r3, g: g3, b: b3 };

            x_pos += 4;
        }

        // Scalar fallback for remaining pixels
        while x_pos < width {
            let y_val = y_plane[y_pos * y_stride + x_pos] as f32;

            let chroma_x_raw = (x_pos as f32 + 0.5) * 0.5 - 0.5;
            let chroma_x = chroma_x_raw.max(0.0).min(chroma_width as f32 - 1.0);
            let cx0 = chroma_x.floor() as usize;
            let cx1 = (cx0 + 1).min(chroma_width - 1);
            let fx = chroma_x - cx0 as f32;
            let fx1 = 1.0 - fx;
            let fy1 = 1.0 - fy;

            let u00 = u_plane[cy0 * u_stride + cx0] as f32;
            let u01 = u_plane[cy0 * u_stride + cx1] as f32;
            let u10 = u_plane[cy1 * u_stride + cx0] as f32;
            let u11 = u_plane[cy1 * u_stride + cx1] as f32;
            let u_val = u00 * fx1 * fy1 + u01 * fx * fy1 + u10 * fx1 * fy + u11 * fx * fy;

            let v00 = v_plane[cy0 * v_stride + cx0] as f32;
            let v01 = v_plane[cy0 * v_stride + cx1] as f32;
            let v10 = v_plane[cy1 * v_stride + cx0] as f32;
            let v11 = v_plane[cy1 * v_stride + cx1] as f32;
            let v_val = v00 * fx1 * fy1 + v01 * fx * fy1 + v10 * fx1 * fy + v11 * fx * fy;

            let (r, g, b) = yuv_to_rgb(y_val, u_val, v_val, kr, kg, kb, range);
            out[row_start + x_pos] = RGB8 { r, g, b };

            x_pos += 1;
        }
    }

    ImgVec::new(out, width, height)
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
