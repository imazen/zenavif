//! Whole AVIF decode comparisons with runtime SIMD enabled and disabled.

use enough::Unstoppable;
use zenavif::{DecoderConfig, decode_with};
use zenbench::prelude::*;

#[cfg(target_arch = "aarch64")]
type TierToken = archmage::NeonToken;
#[cfg(target_arch = "x86_64")]
type TierToken = archmage::X64V3Token;

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
fn benchmark_decode(suite: &mut Suite) {
    for name in [
        "kodim03_yuv420_8bpc.avif",
        "extended_pixi.avif",
        "clap_irot_imir_non_essential.avif",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/vectors/libavif")
            .join(name);
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let config = DecoderConfig::new().threads(1);
        TierToken::dangerously_disable_token_process_wide(false).unwrap();
        let simd = decode_with(&data, &config, &Unstoppable).unwrap();
        TierToken::dangerously_disable_token_process_wide(true).unwrap();
        let scalar = decode_with(&data, &config, &Unstoppable).unwrap();
        assert_eq!(simd.width(), scalar.width());
        assert_eq!(simd.height(), scalar.height());
        assert_eq!(
            simd.copy_to_contiguous_bytes(),
            scalar.copy_to_contiguous_bytes(),
            "decoded pixel parity: {name}"
        );
        let pixels = u64::from(simd.width()) * u64::from(simd.height());
        TierToken::dangerously_disable_token_process_wide(false).unwrap();
        suite.compare(format!("decode_tiers/{name}"), |g| {
            g.throughput(Throughput::Elements(pixels));
            for (label, enabled) in [("native_simd", true), ("forced_scalar", false)] {
                let data = data.clone();
                g.bench(label, move |b| {
                    b.with_input(move || {
                        TierToken::dangerously_disable_token_process_wide(!enabled).unwrap();
                    })
                    .run(|_| {
                        decode_with(
                            black_box(&data),
                            &DecoderConfig::new().threads(1),
                            &Unstoppable,
                        )
                        .unwrap()
                    })
                });
            }
        });
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn benchmark_decode(_: &mut Suite) {
    panic!("this tier comparison requires an ARM64 or x86-64 host");
}

zenbench::main!(benchmark_decode);
