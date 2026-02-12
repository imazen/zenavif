//! Benchmark auto-vectorization vs manual SIMD

use archmage::prelude::*;
use std::time::Instant;
use zenavif::yuv_convert::{YuvMatrix, YuvRange};
use zenavif::yuv_convert_libyuv;
use zenavif::yuv_convert_libyuv_autovec;
use zenavif::yuv_convert_libyuv_simd;

fn main() {
    let width = 1920;
    let height = 1080;

    let mut y_plane = vec![0u8; width * height];
    let mut u_plane = vec![0u8; (width / 2) * (height / 2)];
    let mut v_plane = vec![0u8; (width / 2) * (height / 2)];

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
    println!("Testing auto-vectorization vs manual SIMD");
    println!();

    // Warm up
    let _ = yuv_convert_libyuv::yuv420_to_rgb8(
        &y_plane,
        width,
        &u_plane,
        width / 2,
        &v_plane,
        width / 2,
        width,
        height,
        YuvRange::Full,
        YuvMatrix::Bt709,
    );

    let _ = yuv_convert_libyuv_autovec::yuv420_to_rgb8_autovec(
        &y_plane,
        width,
        &u_plane,
        width / 2,
        &v_plane,
        width / 2,
        width,
        height,
        YuvRange::Full,
        YuvMatrix::Bt709,
    );

    if let Some(token) = Desktop64::summon() {
        let _ = yuv_convert_libyuv_simd::yuv420_to_rgb8_simd(
            token,
            &y_plane,
            width,
            &u_plane,
            width / 2,
            &v_plane,
            width / 2,
            width,
            height,
            YuvRange::Full,
            YuvMatrix::Bt709,
        );
    }

    // Benchmark
    let iterations = 100;

    // Scalar
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = yuv_convert_libyuv::yuv420_to_rgb8(
            &y_plane,
            width,
            &u_plane,
            width / 2,
            &v_plane,
            width / 2,
            width,
            height,
            YuvRange::Full,
            YuvMatrix::Bt709,
        );
    }
    let scalar_ms = start.elapsed().as_secs_f64() * 1000.0 / iterations as f64;

    // Auto-vectorized
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = yuv_convert_libyuv_autovec::yuv420_to_rgb8_autovec(
            &y_plane,
            width,
            &u_plane,
            width / 2,
            &v_plane,
            width / 2,
            width,
            height,
            YuvRange::Full,
            YuvMatrix::Bt709,
        );
    }
    let autovec_ms = start.elapsed().as_secs_f64() * 1000.0 / iterations as f64;

    // Manual SIMD
    let simd_ms = if let Some(token) = Desktop64::summon() {
        let start = Instant::now();
        for _ in 0..iterations {
            let _ = yuv_convert_libyuv_simd::yuv420_to_rgb8_simd(
                token,
                &y_plane,
                width,
                &u_plane,
                width / 2,
                &v_plane,
                width / 2,
                width,
                height,
                YuvRange::Full,
                YuvMatrix::Bt709,
            );
        }
        Some(start.elapsed().as_secs_f64() * 1000.0 / iterations as f64)
    } else {
        None
    };

    let mpix = (width * height) as f64 / 1000.0;

    println!(
        "Scalar:        {:.3} ms ({:.1} Mpixels/s)",
        scalar_ms,
        mpix / scalar_ms
    );
    println!(
        "Auto-vec:      {:.3} ms ({:.1} Mpixels/s) - {:.2}x vs scalar",
        autovec_ms,
        mpix / autovec_ms,
        scalar_ms / autovec_ms
    );

    if let Some(simd_ms) = simd_ms {
        println!(
            "Manual SIMD:   {:.3} ms ({:.1} Mpixels/s) - {:.2}x vs scalar, {:.2}x vs autovec",
            simd_ms,
            mpix / simd_ms,
            scalar_ms / simd_ms,
            autovec_ms / simd_ms
        );
    }

    // Verify accuracy
    let scalar_result = yuv_convert_libyuv::yuv420_to_rgb8(
        &y_plane,
        width,
        &u_plane,
        width / 2,
        &v_plane,
        width / 2,
        width,
        height,
        YuvRange::Full,
        YuvMatrix::Bt709,
    )
    .unwrap();

    let autovec_result = yuv_convert_libyuv_autovec::yuv420_to_rgb8_autovec(
        &y_plane,
        width,
        &u_plane,
        width / 2,
        &v_plane,
        width / 2,
        width,
        height,
        YuvRange::Full,
        YuvMatrix::Bt709,
    )
    .unwrap();

    let mut mismatches = 0;
    for i in 0..(width * height) {
        let s = scalar_result.buf()[i];
        let a = autovec_result.buf()[i];
        if s.r != a.r || s.g != a.g || s.b != a.b {
            mismatches += 1;
        }
    }

    println!();
    if mismatches == 0 {
        println!("✅ Auto-vec matches scalar perfectly!");
    } else {
        println!("❌ {} pixels differ", mismatches);
    }
}
