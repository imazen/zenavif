//! Check accuracy of fast integer YUV conversion

use zenavif::yuv_convert::{yuv420_to_rgb8, YuvRange, YuvMatrix};
use zenavif::yuv_convert_fast::yuv420_to_rgb8_fast;
use archmage::prelude::*;
use yuv::{yuv420_to_rgb, YuvPlanarImage, YuvRange as YuvCrateRange, YuvStandardMatrix};

fn main() {
    let width = 256;
    let height = 256;

    // Create test pattern with varying Y, U, V values
    let mut y_plane = vec![0u8; width * height];
    let uv_size = ((width + 1) / 2) * ((height + 1) / 2);
    let mut u_plane = vec![0u8; uv_size];
    let mut v_plane = vec![0u8; uv_size];

    // Fill with gradient pattern
    for y in 0..height {
        for x in 0..width {
            y_plane[y * width + x] = ((x + y) % 256) as u8;
        }
    }
    for y in 0..height/2 {
        for x in 0..width/2 {
            u_plane[y * width/2 + x] = ((x * 2) % 256) as u8;
            v_plane[y * width/2 + x] = ((y * 2) % 256) as u8;
        }
    }

    // Convert with our float SIMD (reference)
    let float_result = yuv420_to_rgb8(
        &y_plane, width,
        &u_plane, width / 2,
        &v_plane, width / 2,
        width, height,
        YuvRange::Full,
        YuvMatrix::Bt709,
    );

    // Convert with our fast integer SIMD
    let fast_result = if let Some(token) = Desktop64::summon() {
        yuv420_to_rgb8_fast(
            token,
            &y_plane, width,
            &u_plane, width / 2,
            &v_plane, width / 2,
            width, height,
        )
    } else {
        panic!("AVX2 not available");
    };

    // Convert with yuv crate
    let yuv_image = YuvPlanarImage {
        y_plane: &y_plane,
        y_stride: width as u32,
        u_plane: &u_plane,
        u_stride: (width / 2) as u32,
        v_plane: &v_plane,
        v_stride: (width / 2) as u32,
        width: width as u32,
        height: height as u32,
    };
    let mut yuv_crate_rgb = vec![0u8; width * height * 3];
    yuv420_to_rgb(&yuv_image, &mut yuv_crate_rgb, (width * 3) as u32, 
                  YuvCrateRange::Full, YuvStandardMatrix::Bt709).unwrap();

    // Compare fast vs float
    let mut max_diff_float = 0i32;
    let mut total_diff_float = 0i64;
    let mut pixels_different_float = 0;

    for i in 0..(width * height) {
        let fast = fast_result.buf()[i];
        let float = float_result.buf()[i];
        
        let diff_r = (fast.r as i32 - float.r as i32).abs();
        let diff_g = (fast.g as i32 - float.g as i32).abs();
        let diff_b = (fast.b as i32 - float.b as i32).abs();
        
        let max_channel_diff = diff_r.max(diff_g).max(diff_b);
        max_diff_float = max_diff_float.max(max_channel_diff);
        total_diff_float += (diff_r + diff_g + diff_b) as i64;
        
        if max_channel_diff > 1 {
            pixels_different_float += 1;
        }
    }

    // Compare fast vs yuv crate
    let mut max_diff_yuv = 0i32;
    let mut total_diff_yuv = 0i64;
    let mut pixels_different_yuv = 0;

    for i in 0..(width * height) {
        let fast = fast_result.buf()[i];
        let yuv_r = yuv_crate_rgb[i * 3];
        let yuv_g = yuv_crate_rgb[i * 3 + 1];
        let yuv_b = yuv_crate_rgb[i * 3 + 2];
        
        let diff_r = (fast.r as i32 - yuv_r as i32).abs();
        let diff_g = (fast.g as i32 - yuv_g as i32).abs();
        let diff_b = (fast.b as i32 - yuv_b as i32).abs();
        
        let max_channel_diff = diff_r.max(diff_g).max(diff_b);
        max_diff_yuv = max_diff_yuv.max(max_channel_diff);
        total_diff_yuv += (diff_r + diff_g + diff_b) as i64;
        
        if max_channel_diff > 1 {
            pixels_different_yuv += 1;
        }
    }

    println!("Accuracy Check ({}x{}, {} pixels)", width, height, width * height);
    println!();
    println!("Fast Integer vs Float SIMD:");
    println!("  Max difference: {}", max_diff_float);
    println!("  Avg difference: {:.3}", total_diff_float as f64 / (width * height * 3) as f64);
    println!("  Pixels with >1 diff: {} ({:.2}%)", 
             pixels_different_float, 
             100.0 * pixels_different_float as f64 / (width * height) as f64);
    println!();
    println!("Fast Integer vs yuv crate:");
    println!("  Max difference: {}", max_diff_yuv);
    println!("  Avg difference: {:.3}", total_diff_yuv as f64 / (width * height * 3) as f64);
    println!("  Pixels with >1 diff: {} ({:.2}%)", 
             pixels_different_yuv,
             100.0 * pixels_different_yuv as f64 / (width * height) as f64);

    if max_diff_float <= 1 && max_diff_yuv <= 1 {
        println!();
        println!("✅ PASS: All differences within ±1 (rounding tolerance)");
    } else if max_diff_float <= 2 && max_diff_yuv <= 2 {
        println!();
        println!("⚠️  ACCEPTABLE: Small differences (±2), likely rounding");
    } else {
        println!();
        println!("❌ FAIL: Significant differences detected");
    }
}
