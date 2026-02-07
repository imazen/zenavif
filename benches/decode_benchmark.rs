//! Benchmarks for zenavif decoder

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use zenavif::{decode_with, DecoderConfig};
use enough::Unstoppable;

// Include test images as bytes
const SMALL_IMAGE: &[u8] = include_bytes!("../tests/vectors/libavif/white_1x1.avif");
const MEDIUM_IMAGE: &[u8] = include_bytes!("../tests/vectors/libavif/abc_color_irot_alpha_irot.avif");

fn benchmark_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode");

    // Small image - single threaded
    group.bench_function("small_managed_1thread", |b| {
        let config = DecoderConfig::new().threads(1);
        b.iter(|| {
            let result = decode_with(black_box(SMALL_IMAGE), &config, &Unstoppable);
            black_box(result)
        });
    });

    // Medium image - single threaded
    group.bench_function("medium_managed_1thread", |b| {
        let config = DecoderConfig::new().threads(1);
        b.iter(|| {
            let result = decode_with(black_box(MEDIUM_IMAGE), &config, &Unstoppable);
            black_box(result)
        });
    });

    group.finish();
}

criterion_group!(benches, benchmark_decode);
criterion_main!(benches);
