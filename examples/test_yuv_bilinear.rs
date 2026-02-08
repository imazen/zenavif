//! Test yuv crate bilinear vs our float SIMD

use zenavif::yuv_convert::{yuv420_to_rgb8, YuvRange, YuvMatrix};
use yuv::{yuv420_to_rgb_bilinear, YuvPlanarImage, YuvRange as YuvCrateRange, YuvStandardMatrix};

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

    // Convert with our float SIMD (uses bilinear)
    let float_result = yuv420_to_rgb8(
        &y_plane, width,
        &u_plane, width / 2,
        &v_plane, width / 2,
        width, height,
        YuvRange::Full,
        YuvMatrix::Bt709,
    );

    // Convert with yuv crate bilinear
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
    yuv420_to_rgb_bilinear(&yuv_image, &mut yuv_crate_rgb, (width * 3) as u32, 
                           YuvCrateRange::Full, YuvStandardMatrix::Bt709).unwrap();

    // Compare
    let mut max_diff = 0i32;
    let mut total_diff = 0i64;
    let mut pixels_different = 0;

    for i in 0..(width * height) {
        let float = float_result.buf()[i];
        let yuv_r = yuv_crate_rgb[i * 3];
        let yuv_g = yuv_crate_rgb[i * 3 + 1];
        let yuv_b = yuv_crate_rgb[i * 3 + 2];
        
        let diff_r = (float.r as i32 - yuv_r as i32).abs();
        let diff_g = (float.g as i32 - yuv_g as i32).abs();
        let diff_b = (float.b as i32 - yuv_b as i32).abs();
        
        let max_channel_diff = diff_r.max(diff_g).max(diff_b);
        max_diff = max_diff.max(max_channel_diff);
        total_diff += (diff_r + diff_g + diff_b) as i64;
        
        if max_channel_diff > 1 {
            pixels_different += 1;
        }
    }

    println!("Comparison: Our Float SIMD vs yuv crate bilinear");
    println!("  Max difference: {}", max_diff);
    println!("  Avg difference: {:.3}", total_diff as f64 / (width * height * 3) as f64);
    println!("  Pixels with >1 diff: {} ({:.2}%)", 
             pixels_different,
             100.0 * pixels_different as f64 / (width * height) as f64);

    if max_diff <= 1 {
        println!("\n✅ EXCELLENT: Differences within ±1 (rounding only)");
    } else if max_diff <= 2 {
        println!("\n✅ GOOD: Small differences (±{}), likely rounding", max_diff);
    } else if pixels_different < (width * height) / 100 {
        println!("\n⚠️  ACCEPTABLE: <1% pixels differ by more than 1");
    } else {
        println!("\n❌ POOR: Significant differences detected");
    }
}
