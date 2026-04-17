//! Benchmarks for zenavif decoder

use enough::Unstoppable;
use std::hint::black_box;
use zenavif::{DecoderConfig, decode_with};
use zenbench::criterion_compat::*;
use zenbench::{criterion_group, criterion_main};

fn load_test_image(name: &str) -> Option<Vec<u8>> {
    let path = format!(
        "{}/tests/vectors/libavif/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).ok()
}

fn benchmark_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode");

    // Small image - single threaded
    if let Some(small_image) = load_test_image("white_1x1.avif") {
        group.bench_function("small_managed_1thread", |b| {
            let config = DecoderConfig::new().threads(1);
            b.iter(|| {
                let result = decode_with(black_box(&small_image), &config, &Unstoppable);
                black_box(result)
            });
        });
    }

    // Medium image - single threaded
    if let Some(medium_image) = load_test_image("abc_color_irot_alpha_irot.avif") {
        group.bench_function("medium_managed_1thread", |b| {
            let config = DecoderConfig::new().threads(1);
            b.iter(|| {
                let result = decode_with(black_box(&medium_image), &config, &Unstoppable);
                black_box(result)
            });
        });
    }

    group.finish();
}

criterion_group!(benches, benchmark_decode);
criterion_main!(benches);
