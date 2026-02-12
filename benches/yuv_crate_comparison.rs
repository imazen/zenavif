//! Benchmark: Our SIMD vs yuv crate (Balanced and Professional modes)

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, black_box, criterion_group, criterion_main};
use yuv::{YuvPlanarImage, YuvRange, YuvStandardMatrix, yuv420_to_rgb};
use zenavif::yuv_convert::{YuvMatrix as OurYuvMatrix, YuvRange as OurYuvRange, yuv420_to_rgb8};

fn prepare_test_data(width: usize, height: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let y_plane = vec![128u8; width * height];
    let uv_size = ((width + 1) / 2) * ((height + 1) / 2);
    let u_plane = vec![128u8; uv_size];
    let v_plane = vec![128u8; uv_size];
    (y_plane, u_plane, v_plane)
}

fn bench_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("yuv420_comparison");

    // Test at 1920x1080 (FHD - realistic size)
    let width = 1920;
    let height = 1080;
    let (y_plane, u_plane, v_plane) = prepare_test_data(width, height);

    group.throughput(criterion::Throughput::Elements((width * height) as u64));

    // Our SIMD implementation
    group.bench_function("zenavif_simd", |b| {
        b.iter(|| {
            yuv420_to_rgb8(
                black_box(&y_plane),
                black_box(width),
                black_box(&u_plane),
                black_box((width + 1) / 2),
                black_box(&v_plane),
                black_box((width + 1) / 2),
                black_box(width),
                black_box(height),
                OurYuvRange::Full,
                OurYuvMatrix::Bt709,
            )
        });
    });

    // yuv crate (default Balanced mode)
    group.bench_function("yuv_crate_balanced", |b| {
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

        b.iter(|| {
            yuv420_to_rgb(
                black_box(&yuv_image),
                black_box(&mut rgb),
                black_box(rgb_stride),
                black_box(YuvRange::Full),
                black_box(YuvStandardMatrix::Bt709),
            )
            .unwrap();
        });
    });

    group.finish();
}

criterion_group!(benches, bench_comparison);
criterion_main!(benches);
