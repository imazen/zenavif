//! AVG (average) bilinear prediction for AV1 decode
//!
//! This is a port of dav1d/rav1d's AVG function from x86 assembly to safe Rust
//! using archmage tokens for runtime CPU feature detection.
//!
//! The AVG operation combines two intermediate 16-bit pixel buffers by averaging
//! them and packing the result back to 8-bit pixels.

#[cfg(target_arch = "x86_64")]
use archmage::{Desktop64, SimdToken, arcane};

#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

#[cfg(target_arch = "wasm32")]
use archmage::{SimdToken, Wasm128Token, arcane};

#[cfg(target_arch = "wasm32")]
use core::arch::wasm32::*;

/// Rounding constant for pmulhrsw: 1024 = (1 << 10)
/// pmulhrsw computes: (a * b + 16384) >> 15
/// With b=1024: (a * 1024 + 16384) >> 15 ≈ (a + 1) >> 1 (with rounding)
const PW_1024: i16 = 1024;

/// AVG operation: average two 16-bit buffers and pack to 8-bit
///
/// # Arguments
/// * `token` - Proof that AVX2+FMA are available
/// * `dst` - Output 8-bit pixel buffer
/// * `dst_stride` - Stride between rows in dst (bytes)
/// * `tmp1` - First 16-bit intermediate buffer (contiguous)
/// * `tmp2` - Second 16-bit intermediate buffer (contiguous)
/// * `w` - Width in pixels (must be multiple of 32 for AVX2)
/// * `h` - Height in rows
#[cfg(target_arch = "x86_64")]
#[arcane]
pub fn avg_8bpc_avx2(
    _token: Desktop64,
    dst: &mut [u8],
    dst_stride: usize,
    tmp1: &[i16],
    tmp2: &[i16],
    w: usize,
    h: usize,
) {
    debug_assert!(
        w.is_multiple_of(32),
        "width must be multiple of 32 for AVX2"
    );
    debug_assert!(tmp1.len() >= w * h, "tmp1 too small");
    debug_assert!(tmp2.len() >= w * h, "tmp2 too small");
    debug_assert!(dst.len() >= (h - 1) * dst_stride + w, "dst too small");

    // Broadcast rounding constant to all lanes
    let round = _mm256_set1_epi16(PW_1024);

    for row in 0..h {
        let tmp1_row = &tmp1[row * w..][..w];
        let tmp2_row = &tmp2[row * w..][..w];
        let dst_row = &mut dst[row * dst_stride..][..w];

        // Process 32 pixels (64 bytes of i16 input → 32 bytes of u8 output) at a time
        let mut col = 0;
        while col + 32 <= w {
            // Convert slices to arrays for safe_unaligned_simd
            // 16 i16 = 256 bits = 32 bytes
            let t1_lo_arr: &[i16; 16] = tmp1_row[col..col + 16].try_into().unwrap();
            let t1_hi_arr: &[i16; 16] = tmp1_row[col + 16..col + 32].try_into().unwrap();
            let t2_lo_arr: &[i16; 16] = tmp2_row[col..col + 16].try_into().unwrap();
            let t2_hi_arr: &[i16; 16] = tmp2_row[col + 16..col + 32].try_into().unwrap();

            // Load 32 i16 values from tmp1 (two 256-bit loads)
            let t1_lo = safe_unaligned_simd::x86_64::_mm256_loadu_si256(t1_lo_arr);
            let t1_hi = safe_unaligned_simd::x86_64::_mm256_loadu_si256(t1_hi_arr);

            // Load 32 i16 values from tmp2
            let t2_lo = safe_unaligned_simd::x86_64::_mm256_loadu_si256(t2_lo_arr);
            let t2_hi = safe_unaligned_simd::x86_64::_mm256_loadu_si256(t2_hi_arr);

            // Add: tmp1 + tmp2
            let sum_lo = _mm256_add_epi16(t1_lo, t2_lo);
            let sum_hi = _mm256_add_epi16(t1_hi, t2_hi);

            // Multiply and round shift: (sum * 1024 + 16384) >> 15
            // This effectively computes (sum + 1) >> 1 with rounding
            let avg_lo = _mm256_mulhrs_epi16(sum_lo, round);
            let avg_hi = _mm256_mulhrs_epi16(sum_hi, round);

            // Pack to unsigned bytes with saturation
            // packuswb interleaves oddly, need permute to fix lane order
            let packed = _mm256_packus_epi16(avg_lo, avg_hi);
            // Fix AVX2 lane interleaving: [0,1,4,5,2,3,6,7] → [0,1,2,3,4,5,6,7]
            let result = _mm256_permute4x64_epi64(packed, 0b11_01_10_00);

            // Store 32 bytes to dst - use [u8; 32] array
            let dst_arr: &mut [u8; 32] = (&mut dst_row[col..col + 32]).try_into().unwrap();
            safe_unaligned_simd::x86_64::_mm256_storeu_si256(dst_arr, result);

            col += 32;
        }

        // Handle remaining pixels with scalar fallback
        while col < w {
            let sum = tmp1_row[col].wrapping_add(tmp2_row[col]);
            // Same rounding as pmulhrsw: (sum * 1024 + 16384) >> 15
            let avg = ((sum as i32 * 1024 + 16384) >> 15).clamp(0, 255) as u8;
            dst_row[col] = avg;
            col += 1;
        }
    }
}

/// AVG operation using wasm128 SIMD — processes 8 pixels at a time
///
/// Synthesizes pmulhrsw from i32x4_extmul + add + shift + narrow since
/// WebAssembly SIMD128 has no direct pmulhrsw equivalent.
#[cfg(target_arch = "wasm32")]
#[arcane]
pub fn avg_8bpc_wasm128(
    _token: Wasm128Token,
    dst: &mut [u8],
    dst_stride: usize,
    tmp1: &[i16],
    tmp2: &[i16],
    w: usize,
    h: usize,
) {
    debug_assert!(tmp1.len() >= w * h, "tmp1 too small");
    debug_assert!(tmp2.len() >= w * h, "tmp2 too small");
    debug_assert!(dst.len() >= (h - 1) * dst_stride + w, "dst too small");

    let round_const = i32x4_splat(16384);
    let zero = i16x8_splat(0);
    let pw_1024 = i16x8_splat(PW_1024);

    for row in 0..h {
        let tmp1_row = &tmp1[row * w..][..w];
        let tmp2_row = &tmp2[row * w..][..w];
        let dst_row = &mut dst[row * dst_stride..][..w];

        let mut col = 0;
        // Process 8 pixels at a time (128-bit: 8 x i16)
        while col + 8 <= w {
            let t1_arr: &[i16; 8] = tmp1_row[col..col + 8].try_into().unwrap();
            let t2_arr: &[i16; 8] = tmp2_row[col..col + 8].try_into().unwrap();

            let t1 = safe_unaligned_simd::wasm32::v128_load(t1_arr);
            let t2 = safe_unaligned_simd::wasm32::v128_load(t2_arr);

            // sum = tmp1 + tmp2
            let sum = i16x8_add(t1, t2);

            // Synthesize pmulhrsw(sum, 1024):
            // result = (sum * 1024 + 16384) >> 15
            // Use widening multiply to get full 32-bit products
            let prod_lo = i32x4_extmul_low_i16x8(sum, pw_1024);
            let prod_hi = i32x4_extmul_high_i16x8(sum, pw_1024);

            // Add rounding constant
            let rounded_lo = i32x4_add(prod_lo, round_const);
            let rounded_hi = i32x4_add(prod_hi, round_const);

            // Arithmetic right shift by 15
            let shifted_lo = i32x4_shr(rounded_lo, 15);
            let shifted_hi = i32x4_shr(rounded_hi, 15);

            // Narrow i32x4 → i16x8 (signed saturation)
            let narrowed = i16x8_narrow_i32x4(shifted_lo, shifted_hi);

            // Pack i16x8 → u8x16 (unsigned saturation), low 8 bytes are the result
            let packed = u8x16_narrow_i16x8(narrowed, zero);

            // Store low 8 bytes
            let val = i64x2_extract_lane::<0>(packed);
            let bytes = val.to_ne_bytes();
            dst_row[col..col + 8].copy_from_slice(&bytes);

            col += 8;
        }

        // Scalar fallback for remaining pixels
        while col < w {
            let sum = tmp1_row[col].wrapping_add(tmp2_row[col]);
            let avg = ((sum as i32 * 1024 + 16384) >> 15).clamp(0, 255) as u8;
            dst_row[col] = avg;
            col += 1;
        }
    }
}

/// Scalar fallback for AVG operation (for testing and non-AVX2 systems)
pub fn avg_8bpc_scalar(
    dst: &mut [u8],
    dst_stride: usize,
    tmp1: &[i16],
    tmp2: &[i16],
    w: usize,
    h: usize,
) {
    for row in 0..h {
        let tmp1_row = &tmp1[row * w..][..w];
        let tmp2_row = &tmp2[row * w..][..w];
        let dst_row = &mut dst[row * dst_stride..][..w];

        for col in 0..w {
            // Add as i16 (wrapping, same as paddw)
            let sum = tmp1_row[col].wrapping_add(tmp2_row[col]);
            // pmulhrsw equivalent: (sum * 1024 + 16384) >> 15, treating sum as signed
            // This is an arithmetic (sign-extending) right shift
            let avg = ((sum as i32 * 1024 + 16384) >> 15).clamp(0, 255) as u8;
            dst_row[col] = avg;
        }
    }
}

/// Runtime-dispatched AVG function
///
/// Automatically selects AVX2, wasm128, or scalar implementation based on CPU features.
pub fn avg_8bpc(dst: &mut [u8], dst_stride: usize, tmp1: &[i16], tmp2: &[i16], w: usize, h: usize) {
    #[cfg(target_arch = "x86_64")]
    if let Some(token) = Desktop64::summon() {
        avg_8bpc_avx2(token, dst, dst_stride, tmp1, tmp2, w, h);
        return;
    }

    #[cfg(target_arch = "wasm32")]
    if let Some(token) = Wasm128Token::summon() {
        avg_8bpc_wasm128(token, dst, dst_stride, tmp1, tmp2, w, h);
        return;
    }

    avg_8bpc_scalar(dst, dst_stride, tmp1, tmp2, w, h);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brute-force test: verify AVX2 matches scalar for all possible i16 input combinations
    /// in a reasonable subset
    #[test]
    fn test_avg_avx2_matches_scalar() {
        // Test various input values spanning the typical range
        let test_values: Vec<i16> = vec![
            0,
            1,
            2,
            127,
            128,
            255,
            256,
            511,
            512,
            1023,
            1024,
            2047,
            2048,
            4095,
            4096,
            8191,
            8192,
            16383,
            16384,
            -1,
            -128,
            -256,
            -512,
            -1024,
            -2048,
            -4096,
            i16::MIN,
            i16::MAX,
        ];

        let w = 64; // Multiple of 32 for AVX2
        let h = 2;

        let mut tmp1 = vec![0i16; w * h];
        let mut tmp2 = vec![0i16; w * h];
        let mut dst_avx2 = vec![0u8; w * h];
        let mut dst_scalar = vec![0u8; w * h];

        // Test all combinations of test values
        for &v1 in &test_values {
            for &v2 in &test_values {
                // Fill buffers with test values
                tmp1.fill(v1);
                tmp2.fill(v2);
                dst_avx2.fill(0);
                dst_scalar.fill(0);

                // Run both implementations
                avg_8bpc_scalar(&mut dst_scalar, w, &tmp1, &tmp2, w, h);
                avg_8bpc(&mut dst_avx2, w, &tmp1, &tmp2, w, h);

                // Compare results
                assert_eq!(
                    dst_avx2,
                    dst_scalar,
                    "Mismatch for v1={}, v2={}: avx2={:?} scalar={:?}",
                    v1,
                    v2,
                    &dst_avx2[..8],
                    &dst_scalar[..8]
                );
            }
        }
    }

    /// Test with random-ish patterns to catch edge cases
    #[test]
    fn test_avg_varying_data() {
        let w = 128;
        let h = 4;

        let tmp1: Vec<i16> = (0..w * h).map(|i| ((i * 37) % 8192) as i16).collect();
        let tmp2: Vec<i16> = (0..w * h)
            .map(|i| ((i * 73 + 1000) % 8192) as i16)
            .collect();

        let mut dst_avx2 = vec![0u8; w * h];
        let mut dst_scalar = vec![0u8; w * h];

        avg_8bpc_scalar(&mut dst_scalar, w, &tmp1, &tmp2, w, h);
        avg_8bpc(&mut dst_avx2, w, &tmp1, &tmp2, w, h);

        assert_eq!(
            dst_avx2, dst_scalar,
            "Results differ for varying data pattern"
        );
    }

    /// Test that the rounding is correct
    /// pmulhrsw(a, b) = (a * b + 16384) >> 15 (signed)
    #[test]
    fn test_avg_rounding() {
        let w = 32;
        let h = 1;

        // Test sum = 1: (1 * 1024 + 16384) >> 15 = 17408 >> 15 = 0
        let tmp1 = vec![1i16; w];
        let tmp2 = vec![0i16; w];
        let mut dst = vec![255u8; w];
        avg_8bpc(&mut dst, w, &tmp1, &tmp2, w, h);
        assert_eq!(dst[0], 0, "sum=1 should round to 0");

        // Test sum = 32: (32 * 1024 + 16384) >> 15 = 49152 >> 15 = 1
        let tmp1 = vec![16i16; w];
        let tmp2 = vec![16i16; w];
        let mut dst = vec![0u8; w];
        avg_8bpc(&mut dst, w, &tmp1, &tmp2, w, h);
        assert_eq!(dst[0], 1, "sum=32 should round to 1");

        // Test sum = 510: (510 * 1024 + 16384) >> 15 = 538624 >> 15 = 16
        let tmp1 = vec![255i16; w];
        let tmp2 = vec![255i16; w];
        let mut dst = vec![0u8; w];
        avg_8bpc(&mut dst, w, &tmp1, &tmp2, w, h);
        assert_eq!(dst[0], 16, "sum=510 should give 16");

        // Test typical bilinear values (around 128 * 64 = 8192 range)
        // sum = 16384: (16384 * 1024 + 16384) >> 15 = 16793600 >> 15 = 512 → clamped to 255
        let tmp1 = vec![8192i16; w];
        let tmp2 = vec![8192i16; w];
        let mut dst = vec![0u8; w];
        avg_8bpc(&mut dst, w, &tmp1, &tmp2, w, h);
        assert_eq!(dst[0], 255, "sum=16384 should saturate to 255");
    }
}
