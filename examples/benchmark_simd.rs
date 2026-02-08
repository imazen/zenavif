//! Benchmark SIMD vs scalar libyuv

use zenavif::yuv_convert::{YuvRange, YuvMatrix};
use zenavif::yuv_convert_libyuv;
use zenavif::yuv_convert_libyuv_simd;
use archmage::prelude::*;
use std::time::Instant;

fn main() {
    let Some(token) = Desktop64::summon() else {
        println!("AVX2 not available");
        return;
    };
    
    let width = 1920;
    let height = 1080;
    
    // Create test data
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
    
    println!("Benchmarking {}x{} YUV420 to RGB8", width, height);
    println!();
    
    // Warm up
    let _ = yuv_convert_libyuv::yuv420_to_rgb8(
        &y_plane, width, &u_plane, width/2, &v_plane, width/2,
        width, height, YuvRange::Full, YuvMatrix::Bt709).unwrap();
    let _ = yuv_convert_libyuv_simd::yuv420_to_rgb8_simd(
        token,
        &y_plane, width, &u_plane, width/2, &v_plane, width/2,
        width, height, YuvRange::Full, YuvMatrix::Bt709).unwrap();
    
    // Benchmark scalar
    let iterations = 100;
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = yuv_convert_libyuv::yuv420_to_rgb8(
            &y_plane, width, &u_plane, width/2, &v_plane, width/2,
            width, height, YuvRange::Full, YuvMatrix::Bt709).unwrap();
    }
    let scalar_time = start.elapsed();
    let scalar_ms = scalar_time.as_secs_f64() * 1000.0 / iterations as f64;
    
    // Benchmark SIMD
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = yuv_convert_libyuv_simd::yuv420_to_rgb8_simd(
            token,
            &y_plane, width, &u_plane, width/2, &v_plane, width/2,
            width, height, YuvRange::Full, YuvMatrix::Bt709).unwrap();
    }
    let simd_time = start.elapsed();
    let simd_ms = simd_time.as_secs_f64() * 1000.0 / iterations as f64;
    
    println!("Scalar:  {:.3} ms  ({:.1} Mpixels/s)", 
             scalar_ms, (width * height) as f64 / scalar_ms / 1000.0);
    println!("SIMD:    {:.3} ms  ({:.1} Mpixels/s)", 
             simd_ms, (width * height) as f64 / simd_ms / 1000.0);
    println!();
    println!("Speedup: {:.2}x", scalar_ms / simd_ms);
    
    // Verify accuracy
    let scalar_result = yuv_convert_libyuv::yuv420_to_rgb8(
        &y_plane, width, &u_plane, width/2, &v_plane, width/2,
        width, height, YuvRange::Full, YuvMatrix::Bt709).unwrap();
    let simd_result = yuv_convert_libyuv_simd::yuv420_to_rgb8_simd(
        token,
        &y_plane, width, &u_plane, width/2, &v_plane, width/2,
        width, height, YuvRange::Full, YuvMatrix::Bt709).unwrap();
    
    let mut mismatches = 0;
    for i in 0..(width * height) {
        let s = scalar_result.buf()[i];
        let v = simd_result.buf()[i];
        if s.r != v.r || s.g != v.g || s.b != v.b {
            mismatches += 1;
            if mismatches <= 5 {
                println!("Mismatch at pixel {}: scalar=({},{},{}), simd=({},{},{})",
                         i, s.r, s.g, s.b, v.r, v.g, v.b);
            }
        }
    }
    
    if mismatches > 0 {
        println!("\n❌ {} pixels differ!", mismatches);
    } else {
        println!("\n✅ SIMD matches scalar perfectly!");
    }
}
