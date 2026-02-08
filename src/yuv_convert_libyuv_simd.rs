//! SIMD-optimized libyuv YUV to RGB conversion using AVX2
//!
//! Safety: All intrinsics are protected by archmage's token system.
//! The Desktop64 token proves AVX2 is available, making operations safe.
//! This module uses #![forbid(unsafe_code)] - all SIMD is safe via #[arcane].

#![forbid(unsafe_code)]

use imgref::ImgVec;
use rgb::RGB8;
use crate::yuv_convert::{YuvRange, YuvMatrix};
use archmage::prelude::*;  // Includes core::arch and safe_unaligned_simd
use safe_unaligned_simd::x86_64::_mm_loadl_epi64;

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

/// Convert YUV420 to RGB8 using AVX2 SIMD
/// 
/// Safety: Token-gated via #[arcane] - all SIMD operations are safe
#[arcane]
pub fn yuv420_to_rgb8_simd(
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
) -> Option<ImgVec<RGB8>> {
    if !matches!((range, matrix), (YuvRange::Full, YuvMatrix::Bt709)) {
        return None;
    }
    
    let mut out = vec![RGB8::default(); width * height];
    
    for y in (0..height).step_by(2) {
        let y0 = y;
        let y1 = (y + 1).min(height - 1);
        let chroma_y = y / 2;
        
        let mut x = 0;
        
        while x + 8 <= width {
            process_8_pixels_avx2(
                token,
                &y_plane[y0 * y_stride + x..],
                &u_plane[chroma_y * u_stride + x/2..],
                &v_plane[chroma_y * v_stride + x/2..],
                &mut out[y0 * width + x..],
            );
            
            if y1 < height {
                process_8_pixels_avx2(
                    token,
                    &y_plane[y1 * y_stride + x..],
                    &u_plane[chroma_y * u_stride + x/2..],
                    &v_plane[chroma_y * v_stride + x/2..],
                    &mut out[y1 * width + x..],
                );
            }
            x += 8;
        }
        
        while x < width {
            for row in [y0, y1] {
                if row >= height {
                    continue;
                }
                let chroma_x = x / 2;
                let y_val = y_plane[row * y_stride + x];
                let u_val = u_plane[chroma_y * u_stride + chroma_x];
                let v_val = v_plane[chroma_y * v_stride + chroma_x];
                out[row * width + x] = yuv_pixel(y_val, u_val, v_val);
            }
            x += 1;
        }
    }
    
    Some(ImgVec::new(out, width, height))
}

/// Process 8 pixels using AVX2
///
/// Safety: Token proves AVX2 is available. #[rite] enables target_feature,
/// making all intrinsics safe to call without unsafe blocks (Rust 1.85+).
#[rite]
fn process_8_pixels_avx2(
    _token: Desktop64,  // Token proves safety
    y: &[u8],
    u: &[u8],
    v: &[u8],
    out: &mut [RGB8],
) {
        let yg_vec = _mm256_set1_epi32(YG);
        let ub_vec = _mm256_set1_epi32(UB);
        let ug_vec = _mm256_set1_epi32(UG);
        let vg_vec = _mm256_set1_epi32(VG);
        let vr_vec = _mm256_set1_epi32(VR);
        let bb_vec = _mm256_set1_epi32(BB);
        let bg_vec = _mm256_set1_epi32(BG);
        let br_vec = _mm256_set1_epi32(BR);
        let c0x0101 = _mm256_set1_epi32(0x0101);

        // Load and convert Y, U, V to i32
        // safe_unaligned_simd provides safe array-based intrinsics via prelude
        // For slices smaller than required, pad with zeros to meet alignment requirements
        let mut y_padded = [0u8; 16];
        y_padded[..8].copy_from_slice(&y[..8]);
        let y_vals = _mm_loadl_epi64(&y_padded);  // Loads lower 8 bytes
        let y_8xi32 = _mm256_cvtepu8_epi32(y_vals);

        let u_arr: &[u8; 4] = (&u[..4]).try_into().unwrap();
        let v_arr: &[u8; 4] = (&v[..4]).try_into().unwrap();
        let u_vals_4 = _mm_cvtsi32_si128(u32::from_le_bytes(*u_arr) as i32);
        let v_vals_4 = _mm_cvtsi32_si128(u32::from_le_bytes(*v_arr) as i32);
        let u_dup = _mm_unpacklo_epi8(u_vals_4, u_vals_4);
        let v_dup = _mm_unpacklo_epi8(v_vals_4, v_vals_4);
        let u_8xi32 = _mm256_cvtepu8_epi32(u_dup);
        let v_8xi32 = _mm256_cvtepu8_epi32(v_dup);
        
        // y1 = (y * 0x0101 * YG) >> 16
        let y1 = _mm256_srai_epi32(_mm256_mullo_epi32(_mm256_mullo_epi32(y_8xi32, c0x0101), yg_vec), 16);
        
        // RGB computation
        let b_i32 = _mm256_srai_epi32(_mm256_add_epi32(_mm256_sub_epi32(y1, _mm256_mullo_epi32(u_8xi32, ub_vec)), bb_vec), 6);
        let g_i32 = _mm256_srai_epi32(_mm256_add_epi32(_mm256_sub_epi32(y1, _mm256_add_epi32(_mm256_mullo_epi32(u_8xi32, ug_vec), _mm256_mullo_epi32(v_8xi32, vg_vec))), bg_vec), 6);
        let r_i32 = _mm256_srai_epi32(_mm256_add_epi32(_mm256_sub_epi32(y1, _mm256_mullo_epi32(v_8xi32, vr_vec)), br_vec), 6);
        
        // Pack i32 -> i16 -> u8 with lane fix
        let zero = _mm256_setzero_si256();
        let r_i16_lane = _mm256_packs_epi32(r_i32, zero);
        let g_i16_lane = _mm256_packs_epi32(g_i32, zero);
        let b_i16_lane = _mm256_packs_epi32(b_i32, zero);
        
        // Fix lane order with permute
        let perm = _mm256_setr_epi32(0, 1, 4, 5, 2, 3, 6, 7);
        let r_i16 = _mm256_permutevar8x32_epi32(r_i16_lane, perm);
        let g_i16 = _mm256_permutevar8x32_epi32(g_i16_lane, perm);
        let b_i16 = _mm256_permutevar8x32_epi32(b_i16_lane, perm);
        
        // Pack to u8 with saturation
        let r_u8 = _mm256_packus_epi16(r_i16, zero);
        let g_u8 = _mm256_packus_epi16(g_i16, zero);
        let b_u8 = _mm256_packus_epi16(b_i16, zero);
        
        // Extract low 64 bits (8 bytes)
        let r_64 = _mm256_extract_epi64(r_u8, 0);
        let g_64 = _mm256_extract_epi64(g_u8, 0);
        let b_64 = _mm256_extract_epi64(b_u8, 0);
        
        // Write to output
        for i in 0..8 {
            out[i] = RGB8 {
                r: ((r_64 >> (i * 8)) & 0xFF) as u8,
                g: ((g_64 >> (i * 8)) & 0xFF) as u8,
                b: ((b_64 >> (i * 8)) & 0xFF) as u8,
            };
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_simd_matches_scalar() {
        if let Some(token) = Desktop64::summon() {
            let width = 16;
            let height = 16;
            
            let y_plane = vec![180u8; width * height];
            let u_plane = vec![100u8; (width/2) * (height/2)];
            let v_plane = vec![150u8; (width/2) * (height/2)];
            
            let result = yuv420_to_rgb8_simd(
                token,
                &y_plane, width,
                &u_plane, width/2,
                &v_plane, width/2,
                width, height,
                YuvRange::Full,
                YuvMatrix::Bt709,
            ).unwrap();
            
            for (i, pixel) in result.buf().iter().enumerate() {
                assert_eq!(pixel.r, 230, "R at {}", i);
                assert_eq!(pixel.g, 185, "G at {}", i);
                assert_eq!(pixel.b, 135, "B at {}", i);
            }
        }
    }
}
