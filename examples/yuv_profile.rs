//! Profile our YUV conversion to find bottlenecks

use std::time::Instant;
use zenavif::yuv_convert::{yuv420_to_rgb8, YuvRange, YuvMatrix};

fn main() {
    let width = 1920;
    let height = 1080;
    let iterations = 1000;

    // Prepare test data
    let y_plane = vec![128u8; width * height];
    let uv_size = ((width + 1) / 2) * ((height + 1) / 2);
    let u_plane = vec![128u8; uv_size];
    let v_plane = vec![128u8; uv_size];

    // Warmup
    for _ in 0..10 {
        let _ = yuv420_to_rgb8(
            &y_plane, width,
            &u_plane, (width + 1) / 2,
            &v_plane, (width + 1) / 2,
            width, height,
            YuvRange::Full,
            YuvMatrix::Bt709,
        );
    }

    println!("Running {} iterations...", iterations);
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = yuv420_to_rgb8(
            &y_plane, width,
            &u_plane, (width + 1) / 2,
            &v_plane, (width + 1) / 2,
            width, height,
            YuvRange::Full,
            YuvMatrix::Bt709,
        );
    }
    let elapsed = start.elapsed();
    
    println!("Total: {:.2}s", elapsed.as_secs_f64());
    println!("Average: {:.2}ms per frame", elapsed.as_millis() as f64 / iterations as f64);
    println!("Throughput: {:.1} Mpixels/s", (width * height) as f64 * iterations as f64 / elapsed.as_secs_f64() / 1_000_000.0);
    
    println!("\nRun with: cargo flamegraph --example yuv_profile --release --features managed");
}
