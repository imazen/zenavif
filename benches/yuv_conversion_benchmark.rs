//! Benchmark YUV to RGB conversion (SIMD vs scalar)

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use zenavif::yuv_convert::{yuv420_to_rgb8, YuvRange, YuvMatrix};

fn bench_yuv420_conversion(c: &mut Criterion) {
    // Check if SIMD is available
    use archmage::prelude::*;
    let simd_available = Desktop64::summon().is_some();
    if simd_available {
        eprintln!("✓ AVX2/FMA (Desktop64) available - SIMD enabled");
    } else {
        eprintln!("✗ AVX2/FMA not available - using scalar fallback");
    }

    let mut group = c.benchmark_group("yuv420_to_rgb8");

    // Test common resolutions
    let sizes = [
        ("512x256", 512, 256),     // Small (131k pixels)
        ("1920x1080", 1920, 1080), // FHD (2M pixels)
    ];

    for (name, width, height) in sizes {
        // Prepare test data
        let y_plane = vec![128u8; width * height];
        let uv_size = ((width + 1) / 2) * ((height + 1) / 2);
        let u_plane = vec![128u8; uv_size];
        let v_plane = vec![128u8; uv_size];

        group.throughput(criterion::Throughput::Elements((width * height) as u64));

        group.bench_with_input(
            BenchmarkId::from_parameter(name),
            &(width, height),
            |b, &(w, h)| {
                b.iter(|| {
                    yuv420_to_rgb8(
                        black_box(&y_plane),
                        black_box(w),
                        black_box(&u_plane),
                        black_box((w + 1) / 2),
                        black_box(&v_plane),
                        black_box((w + 1) / 2),
                        black_box(w),
                        black_box(h),
                        YuvRange::Full,
                        YuvMatrix::Bt709,
                    )
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_yuv420_conversion);
criterion_main!(benches);
