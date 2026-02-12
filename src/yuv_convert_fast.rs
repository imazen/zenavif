//! Fast YUV to RGB conversion using integer arithmetic
//!
//! Key optimizations:
//! - Fixed-point integer math (much faster than float)
//! - Process 32 pixels at once
//! - Process 2 rows simultaneously for YUV420
//! - Use AVX2 intrinsics for proper SIMD vectorization

// These unsafe fn helpers use SIMD intrinsics that are safe within target_feature context.
#![allow(unsafe_op_in_unsafe_fn)]

use archmage::prelude::*;
use imgref::ImgVec;
use rgb::RGB8;

/// Fast YUV420 to RGB8 using integer arithmetic (optimized path)
#[arcane]
pub fn yuv420_to_rgb8_fast(
    token: Desktop64,
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: usize,
    height: usize,
) -> ImgVec<RGB8> {
    let mut out = vec![RGB8::default(); width * height];

    // BT.709 coefficients in fixed-point (Q13 format: 8192 = 1.0)
    // Values from yuv crate for BT.709 full range
    let y_coef: i16 = 9539; // 1.164 * 8192
    let cr_coef: i16 = 13075; // 1.596 * 8192
    let cb_coef: i16 = 16525; // 2.018 * 8192
    let g_coef_1: i16 = 6660; // For U component (formula subtracts this)
    let g_coef_2: i16 = 3209; // For V component (formula subtracts this)

    // Bias values
    let y_bias: i16 = 16;
    let uv_bias: i16 = 128;

    // Process 2 rows at a time for YUV420
    for y in (0..height).step_by(2) {
        let y0_row = y;
        let y1_row = (y + 1).min(height - 1);
        let chroma_row = y / 2;

        // Process 32 pixels at a time
        for x in (0..width).step_by(32) {
            let pixels_remaining = (width - x).min(32);

            if pixels_remaining < 32 {
                // Handle remaining pixels with scalar code
                for i in 0..pixels_remaining {
                    for row in [y0_row, y1_row] {
                        if row >= height {
                            continue;
                        }
                        let px = x + i;
                        let chroma_x = px / 2;

                        let y_val = y_plane[row * y_stride + px] as i32 - y_bias as i32;
                        let u_val =
                            u_plane[chroma_row * u_stride + chroma_x] as i32 - uv_bias as i32;
                        let v_val =
                            v_plane[chroma_row * v_stride + chroma_x] as i32 - uv_bias as i32;

                        let y_scaled = (y_val * y_coef as i32) >> 13;
                        let r = y_scaled + ((v_val * cr_coef as i32) >> 13);
                        let g =
                            y_scaled - ((v_val * g_coef_1 as i32 + u_val * g_coef_2 as i32) >> 13);
                        let b = y_scaled + ((u_val * cb_coef as i32) >> 13);

                        out[row * width + px] = RGB8 {
                            r: r.clamp(0, 255) as u8,
                            g: g.clamp(0, 255) as u8,
                            b: b.clamp(0, 255) as u8,
                        };
                    }
                }
                continue;
            }

            // SIMD path for 32 pixels
            // Split the output buffer between row 0 and row 1
            let split_point = y1_row * width;
            let (top_rows, bottom_rows) = out.split_at_mut(split_point);
            let row0_out = &mut top_rows[y0_row * width + x..];
            let row1_out = &mut bottom_rows[x..];

            process_32_pixels_420(
                token,
                &y_plane[y0_row * y_stride + x..],
                &y_plane[y1_row * y_stride + x..],
                &u_plane[chroma_row * u_stride + x / 2..],
                &v_plane[chroma_row * v_stride + x / 2..],
                row0_out,
                row1_out,
                y_coef,
                cr_coef,
                cb_coef,
                g_coef_1,
                g_coef_2,
                y_bias,
                uv_bias,
            );
        }
    }

    ImgVec::new(out, width, height)
}

#[rite]
fn process_32_pixels_420(
    _token: Desktop64,
    y0: &[u8],
    y1: &[u8],
    u: &[u8],
    v: &[u8],
    out0: &mut [RGB8],
    out1: &mut [RGB8],
    y_coef: i16,
    cr_coef: i16,
    cb_coef: i16,
    g_coef_1: i16,
    g_coef_2: i16,
    y_bias: i16,
    uv_bias: i16,
) {
    use core::arch::x86_64::*;

    // Take only the 32 pixels we're processing
    let out0 = &mut out0[..32];
    let out1 = &mut out1[..32];

    unsafe {
        // Load 32 Y values for each row
        let y0_vals = _mm256_loadu_si256(y0.as_ptr() as *const __m256i);
        let y1_vals = _mm256_loadu_si256(y1.as_ptr() as *const __m256i);

        // Load 16 U and V values (half resolution for 4:2:0)
        let u_vals = _mm_loadu_si128(u.as_ptr() as *const __m128i);
        let v_vals = _mm_loadu_si128(v.as_ptr() as *const __m128i);

        // Broadcast UV bias and Y bias
        let y_corr = _mm256_set1_epi8(y_bias as i8);
        let uv_corr = _mm256_set1_epi16(((uv_bias << 2) | (uv_bias >> 6)) as i16);

        // Broadcast coefficients
        let v_y_coef = _mm256_set1_epi16(y_coef);
        let v_cr_coef = _mm256_set1_epi16(cr_coef);
        let v_cb_coef = _mm256_set1_epi16(cb_coef);
        let v_g_coef_1 = _mm256_set1_epi16(g_coef_1);
        let v_g_coef_2 = _mm256_set1_epi16(g_coef_2);

        // Subtract Y bias
        let y0_sub = _mm256_subs_epu8(y0_vals, y_corr);
        let y1_sub = _mm256_subs_epu8(y1_vals, y_corr);

        // Expand chroma from 16 to 32 values using shuffle
        // Create a shuffle mask that duplicates each byte: [0,0,1,1,2,2,...]
        let shuf_expand = _mm256_setr_epi8(
            0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
            13, 14, 14, 15, 15,
        );

        // Broadcast 128-bit chroma to both lanes of 256-bit register
        let u_256 = _mm256_inserti128_si256::<1>(_mm256_castsi128_si256(u_vals), u_vals);
        let v_256 = _mm256_inserti128_si256::<1>(_mm256_castsi128_si256(v_vals), v_vals);

        // Expand each chroma sample to cover 2 pixels
        let u_expanded = _mm256_shuffle_epi8(u_256, shuf_expand);
        let v_expanded = _mm256_shuffle_epi8(v_256, shuf_expand);

        // Expand u8 to i16 by unpacking (creates 10-bit representation)
        let y0_lo = expand_u8_to_i16_lo(y0_sub);
        let y0_hi = expand_u8_to_i16_hi(y0_sub);
        let y1_lo = expand_u8_to_i16_lo(y1_sub);
        let y1_hi = expand_u8_to_i16_hi(y1_sub);

        let u_lo = expand_u8_to_i16_lo(u_expanded);
        let u_hi = expand_u8_to_i16_hi(u_expanded);
        let v_lo = expand_u8_to_i16_lo(v_expanded);
        let v_hi = expand_u8_to_i16_hi(v_expanded);

        // Subtract UV bias
        let u_lo = _mm256_sub_epi16(u_lo, uv_corr);
        let u_hi = _mm256_sub_epi16(u_hi, uv_corr);
        let v_lo = _mm256_sub_epi16(v_lo, uv_corr);
        let v_hi = _mm256_sub_epi16(v_hi, uv_corr);

        // Process low 16 pixels of row 0
        let (r0_lo, g0_lo, b0_lo) = yuv_to_rgb_i16(
            y0_lo, u_lo, v_lo, v_y_coef, v_cr_coef, v_cb_coef, v_g_coef_1, v_g_coef_2,
        );

        // Process high 16 pixels of row 0
        let (r0_hi, g0_hi, b0_hi) = yuv_to_rgb_i16(
            y0_hi, u_hi, v_hi, v_y_coef, v_cr_coef, v_cb_coef, v_g_coef_1, v_g_coef_2,
        );

        // Process low 16 pixels of row 1
        let (r1_lo, g1_lo, b1_lo) = yuv_to_rgb_i16(
            y1_lo, u_lo, v_lo, v_y_coef, v_cr_coef, v_cb_coef, v_g_coef_1, v_g_coef_2,
        );

        // Process high 16 pixels of row 1
        let (r1_hi, g1_hi, b1_hi) = yuv_to_rgb_i16(
            y1_hi, u_hi, v_hi, v_y_coef, v_cr_coef, v_cb_coef, v_g_coef_1, v_g_coef_2,
        );

        // Pack i16 back to u8 with saturation
        let r0 = _mm256_packus_epi16(r0_lo, r0_hi);
        let g0 = _mm256_packus_epi16(g0_lo, g0_hi);
        let b0 = _mm256_packus_epi16(b0_lo, b0_hi);

        let r1 = _mm256_packus_epi16(r1_lo, r1_hi);
        let g1 = _mm256_packus_epi16(g1_lo, g1_hi);
        let b1 = _mm256_packus_epi16(b1_lo, b1_hi);

        // Deinterleave and store RGB values
        store_rgb_row(out0, r0, g0, b0);
        store_rgb_row(out1, r1, g1, b1);
    }
}

#[inline(always)]
unsafe fn expand_u8_to_i16_lo(v: __m256i) -> __m256i {
    use core::arch::x86_64::*;
    let v_dup = _mm256_unpacklo_epi8(v, v);
    _mm256_srli_epi16::<6>(v_dup)
}

#[inline(always)]
unsafe fn expand_u8_to_i16_hi(v: __m256i) -> __m256i {
    use core::arch::x86_64::*;
    let v_dup = _mm256_unpackhi_epi8(v, v);
    _mm256_srli_epi16::<6>(v_dup)
}

#[inline(always)]
unsafe fn yuv_to_rgb_i16(
    y: __m256i,
    u: __m256i,
    v: __m256i,
    y_coef: __m256i,
    cr_coef: __m256i,
    cb_coef: __m256i,
    g_coef_1: __m256i,
    g_coef_2: __m256i,
) -> (__m256i, __m256i, __m256i) {
    use core::arch::x86_64::*;

    // Scale Y with luma coefficient
    let y_scaled = _mm256_mulhrs_epi16(y, y_coef);

    // Compute color components
    // Note: g_coef_1 applies to V, g_coef_2 applies to U (opposite of variable names!)
    let v_cr = _mm256_mulhrs_epi16(v, cr_coef);
    let u_cb = _mm256_mulhrs_epi16(u, cb_coef);
    let v_g = _mm256_mulhrs_epi16(v, g_coef_1);
    let u_g = _mm256_mulhrs_epi16(u, g_coef_2);

    let r = _mm256_add_epi16(y_scaled, v_cr);
    let b = _mm256_add_epi16(y_scaled, u_cb);
    let g = _mm256_sub_epi16(y_scaled, _mm256_add_epi16(v_g, u_g));

    (r, g, b)
}

#[inline(always)]
unsafe fn store_rgb_row(out: &mut [RGB8], r: __m256i, g: __m256i, b: __m256i) {
    use core::arch::x86_64::*;

    // For now, use simple array extraction to debug
    // TODO: Optimize with shuffle-based interleaving once accuracy is verified
    let mut r_arr = [0u8; 32];
    let mut g_arr = [0u8; 32];
    let mut b_arr = [0u8; 32];

    _mm256_storeu_si256(r_arr.as_mut_ptr() as *mut __m256i, r);
    _mm256_storeu_si256(g_arr.as_mut_ptr() as *mut __m256i, g);
    _mm256_storeu_si256(b_arr.as_mut_ptr() as *mut __m256i, b);

    for i in 0..32 {
        out[i] = RGB8 {
            r: r_arr[i],
            g: g_arr[i],
            b: b_arr[i],
        };
    }
}

/// Interleave planar R, G, B into packed RGB using AVX2 shuffles
/// Input: 32 R values, 32 G values, 32 B values (each in a 256-bit register)
/// Output: 3x 256-bit registers containing 96 bytes of interleaved RGBRGBRGB...
///
/// Ported from yuv crate's avx2_interleave_rgb
#[inline(always)]
#[allow(dead_code)]
unsafe fn interleave_rgb_avx2(r: __m256i, g: __m256i, b: __m256i) -> (__m256i, __m256i, __m256i) {
    use core::arch::x86_64::*;

    // Shuffle masks to rearrange bytes for RGB interleaving
    let sh_b = _mm256_setr_epi8(
        0, 11, 6, 1, 12, 7, 2, 13, 8, 3, 14, 9, 4, 15, 10, 5, 0, 11, 6, 1, 12, 7, 2, 13, 8, 3, 14,
        9, 4, 15, 10, 5,
    );
    let sh_g = _mm256_setr_epi8(
        5, 0, 11, 6, 1, 12, 7, 2, 13, 8, 3, 14, 9, 4, 15, 10, 5, 0, 11, 6, 1, 12, 7, 2, 13, 8, 3,
        14, 9, 4, 15, 10,
    );
    let sh_r = _mm256_setr_epi8(
        10, 5, 0, 11, 6, 1, 12, 7, 2, 13, 8, 3, 14, 9, 4, 15, 10, 5, 0, 11, 6, 1, 12, 7, 2, 13, 8,
        3, 14, 9, 4, 15,
    );

    // Apply shuffles to each color channel
    let b0 = _mm256_shuffle_epi8(r, sh_b);
    let g0 = _mm256_shuffle_epi8(g, sh_g);
    let r0 = _mm256_shuffle_epi8(b, sh_r);

    // Blend masks for selecting bytes from each shuffled vector
    // -1 (0xFF) selects from second source, 0 selects from first source
    let m0 = _mm256_setr_epi8(
        0, -1, 0, 0, -1, 0, 0, -1, 0, 0, -1, 0, 0, -1, 0, 0, 0, -1, 0, 0, -1, 0, 0, -1, 0, 0, -1,
        0, 0, -1, 0, 0,
    );
    let m1 = _mm256_setr_epi8(
        0, 0, -1, 0, 0, -1, 0, 0, -1, 0, 0, -1, 0, 0, -1, 0, 0, 0, -1, 0, 0, -1, 0, 0, -1, 0, 0,
        -1, 0, 0, -1, 0,
    );

    // Blend the shuffled values to create RGB pattern
    let p0 = _mm256_blendv_epi8(_mm256_blendv_epi8(b0, g0, m0), r0, m1);
    let p1 = _mm256_blendv_epi8(_mm256_blendv_epi8(g0, r0, m0), b0, m1);
    let p2 = _mm256_blendv_epi8(_mm256_blendv_epi8(r0, b0, m0), g0, m1);

    // Permute lanes to get final RGB layout
    let rgb0 = _mm256_permute2x128_si256::<0x20>(p0, p1); // 0x20 = 32
    let rgb1 = _mm256_permute2x128_si256::<0x30>(p2, p0); // 0x30 = 48
    let rgb2 = _mm256_permute2x128_si256::<0x31>(p1, p2); // 0x31 = 49

    (rgb0, rgb1, rgb2)
}
