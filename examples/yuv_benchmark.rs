//! Quick comparison: Our SIMD vs yuv crate

use archmage::prelude::*;
use std::time::Instant;
use yuv::{YuvPlanarImage, YuvRange, YuvStandardMatrix, yuv420_to_rgb};
use zenavif::yuv_convert::{YuvMatrix as OurYuvMatrix, YuvRange as OurYuvRange, yuv420_to_rgb8};
use zenavif::yuv_convert_fast::yuv420_to_rgb8_fast;

fn main() {
    let width = 1920;
    let height = 1080;
    let iterations = 100;

    // Prepare test data
    let y_plane = vec![128u8; width * height];
    let uv_size = ((width + 1) / 2) * ((height + 1) / 2);
    let u_plane = vec![128u8; uv_size];
    let v_plane = vec![128u8; uv_size];

    println!("Benchmarking YUV420→RGB conversion at {}x{}", width, height);
    println!("Iterations: {}\n", iterations);

    // Warmup
    for _ in 0..10 {
        let _ = yuv420_to_rgb8(
            &y_plane,
            width,
            &u_plane,
            (width + 1) / 2,
            &v_plane,
            (width + 1) / 2,
            width,
            height,
            OurYuvRange::Full,
            OurYuvMatrix::Bt709,
        );
    }

    // Benchmark our SIMD implementation
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = yuv420_to_rgb8(
            &y_plane,
            width,
            &u_plane,
            (width + 1) / 2,
            &v_plane,
            (width + 1) / 2,
            width,
            height,
            OurYuvRange::Full,
            OurYuvMatrix::Bt709,
        );
    }
    let our_time = start.elapsed();
    let our_avg = our_time.as_micros() as f64 / iterations as f64 / 1000.0;

    // Warmup fast version
    if let Some(token) = Desktop64::summon() {
        for _ in 0..10 {
            let _ = yuv420_to_rgb8_fast(
                token,
                &y_plane,
                width,
                &u_plane,
                (width + 1) / 2,
                &v_plane,
                (width + 1) / 2,
                width,
                height,
            );
        }

        // Benchmark fast integer version
        let start = Instant::now();
        for _ in 0..iterations {
            let _ = yuv420_to_rgb8_fast(
                token,
                &y_plane,
                width,
                &u_plane,
                (width + 1) / 2,
                &v_plane,
                (width + 1) / 2,
                width,
                height,
            );
        }
        let fast_time = start.elapsed();
        let fast_avg = fast_time.as_micros() as f64 / iterations as f64 / 1000.0;

        println!(
            "  zenavif FAST:      {:>8.2} ms  ({:>6.1} Mpixels/s)",
            fast_avg,
            (width * height) as f64 / fast_avg / 1000.0
        );
    }

    // Warmup yuv crate
    let yuv_image = YuvPlanarImage {
        y_plane: &y_plane,
        y_stride: width as u32,
        u_plane: &u_plane,
        u_stride: ((width + 1) / 2) as u32,
        v_plane: &v_plane,
        v_stride: ((width + 1) / 2) as u32,
        width: width as u32,
        height: height as u32,
    };
    let mut rgb = vec![0u8; width * height * 3];
    let rgb_stride = width as u32 * 3;

    for _ in 0..10 {
        yuv420_to_rgb(
            &yuv_image,
            &mut rgb,
            rgb_stride,
            YuvRange::Full,
            YuvStandardMatrix::Bt709,
        )
        .unwrap();
    }

    // Benchmark yuv crate (Balanced mode - default)
    let start = Instant::now();
    for _ in 0..iterations {
        yuv420_to_rgb(
            &yuv_image,
            &mut rgb,
            rgb_stride,
            YuvRange::Full,
            YuvStandardMatrix::Bt709,
        )
        .unwrap();
    }
    let yuv_time = start.elapsed();
    let yuv_avg = yuv_time.as_micros() as f64 / iterations as f64 / 1000.0;

    // Results
    println!("Results:");
    println!(
        "  zenavif SIMD:      {:>8.2} ms  ({:>6.1} Mpixels/s)",
        our_avg,
        (width * height) as f64 / our_avg / 1000.0
    );
    println!(
        "  yuv crate (Bal):   {:>8.2} ms  ({:>6.1} Mpixels/s)",
        yuv_avg,
        (width * height) as f64 / yuv_avg / 1000.0
    );
    println!();

    if our_avg < yuv_avg {
        println!(
            "✅ zenavif is {:.1}x FASTER ({:.1}% speedup)",
            yuv_avg / our_avg,
            (yuv_avg - our_avg) / our_avg * 100.0
        );
    } else {
        println!(
            "❌ zenavif is {:.1}x slower ({:.1}% slower)",
            our_avg / yuv_avg,
            (our_avg - yuv_avg) / yuv_avg * 100.0
        );
    }
}
