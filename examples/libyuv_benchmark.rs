//! Benchmark libyuv exact implementation

use zenavif::yuv_convert::{yuv420_to_rgb8, YuvRange, YuvMatrix};
use zenavif::yuv_convert_libyuv::yuv420_to_rgb8 as yuv420_to_rgb8_libyuv;
use std::time::Instant;

fn main() {
    // Test with realistic 1920x1080 frame
    let width = 1920;
    let height = 1080;

    // Create test data with some variation
    let mut y_plane = vec![0u8; width * height];
    let mut u_plane = vec![0u8; (width/2) * (height/2)];
    let mut v_plane = vec![0u8; (width/2) * (height/2)];

    for (i, val) in y_plane.iter_mut().enumerate() {
        *val = ((i * 17) % 256) as u8;
    }
    for (i, val) in u_plane.iter_mut().enumerate() {
        *val = ((i * 37 + 64) % 256) as u8;
    }
    for (i, val) in v_plane.iter_mut().enumerate() {
        *val = ((i * 53 + 128) % 256) as u8;
    }

    println!("Benchmarking {}x{} YUV420 to RGB8 conversion", width, height);
    println!();

    let range = YuvRange::Full;
    let matrix = YuvMatrix::Bt709;

    // Warm up
    let _ = yuv420_to_rgb8_libyuv(&y_plane, width, &u_plane, width/2,
                                   &v_plane, width/2, width, height, range, matrix).unwrap();
    let _ = yuv420_to_rgb8(&y_plane, width, &u_plane, width/2, &v_plane, width/2,
                           width, height, range, matrix);

    // Benchmark libyuv scalar
    let iterations = 100;
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = yuv420_to_rgb8_libyuv(
            &y_plane, width,
            &u_plane, width / 2,
            &v_plane, width / 2,
            width, height,
            range, matrix,
        );
    }
    let libyuv_time = start.elapsed();
    let libyuv_ms = libyuv_time.as_secs_f64() * 1000.0 / iterations as f64;

    // Benchmark float SIMD
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = yuv420_to_rgb8(
            &y_plane, width,
            &u_plane, width / 2,
            &v_plane, width / 2,
            width, height,
            range, matrix,
        );
    }
    let float_time = start.elapsed();
    let float_ms = float_time.as_secs_f64() * 1000.0 / iterations as f64;

    println!("libyuv scalar:  {:.3} ms  ({:.1} Mpixels/s)",
             libyuv_ms, (width * height) as f64 / libyuv_ms / 1000.0);
    println!("Float SIMD:     {:.3} ms  ({:.1} Mpixels/s)",
             float_ms, (width * height) as f64 / float_ms / 1000.0);
    println!();
    println!("Speedup: {:.2}x", libyuv_ms / float_ms);

    // Compare accuracy
    let libyuv_result = yuv420_to_rgb8_libyuv(
        &y_plane, width, &u_plane, width/2, &v_plane, width/2, width, height, range, matrix)
        .expect("libyuv conversion failed");
    let float_result = yuv420_to_rgb8(
        &y_plane, width, &u_plane, width/2, &v_plane, width/2,
        width, height, range, matrix);

    let mut max_diff = 0i32;
    let mut total_diff = 0i64;
    for i in 0..(width * height) {
        let lib = libyuv_result.buf()[i];
        let flt = float_result.buf()[i];
        let diff_r = (lib.r as i32 - flt.r as i32).abs();
        let diff_g = (lib.g as i32 - flt.g as i32).abs();
        let diff_b = (lib.b as i32 - flt.b as i32).abs();
        max_diff = max_diff.max(diff_r).max(diff_g).max(diff_b);
        total_diff += (diff_r + diff_g + diff_b) as i64;
    }

    println!();
    println!("Accuracy comparison (libyuv exact vs float SIMD):");
    println!("  Max difference: {}", max_diff);
    println!("  Avg difference: {:.3}", total_diff as f64 / (width * height * 3) as f64);
}
