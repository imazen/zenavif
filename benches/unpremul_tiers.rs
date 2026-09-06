//! Per-kernel NEON-vs-forced-scalar for `unpremultiply8`.
//!
//! The existing tier bench covers YUV->RGB only. `unpremultiply8` runs once per
//! row on every alpha-bearing AVIF (buffered and streaming paths) and divides
//! by the pixel's own alpha, so no integer-SIMD form exists and the scalar loop
//! cannot vectorize. That made it invisible to an end-to-end decode number.
//!
//! Run: `cargo bench --features _dev --bench unpremul_tiers`

use rgb::Rgba;
use zenbench::prelude::*;

#[cfg(target_arch = "aarch64")]
type TierToken = archmage::NeonToken;
#[cfg(target_arch = "x86_64")]
type TierToken = archmage::X64V3Token;

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
const TIER_NAME: &str = if cfg!(target_arch = "aarch64") {
    "neon"
} else {
    "v3(avx2)"
};

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
fn set_simd(on: bool) -> bool {
    // `dangerously_disable_token_process_wide` is inherent on every token
    // type (no `SimdToken` trait import needed on any arch).
    TierToken::dangerously_disable_token_process_wide(!on).is_ok()
}
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn set_simd(_on: bool) -> bool {
    false
}

fn bench(suite: &mut Suite) {
    assert!(
        set_simd(true) && set_simd(false),
        "benchmark requires toggleable SIMD"
    );
    set_simd(true);
    for width in [1usize, 17, 64, 512, 1920, 4096] {
        let row: &'static [Rgba<u8>] = Box::leak(
            (0..width)
                .map(|i| Rgba {
                    r: (i % 251) as u8,
                    g: (i % 199) as u8,
                    b: (i % 173) as u8,
                    // Full alpha range; most pixels take the divide branch.
                    a: (i % 256) as u8,
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let mut scalar = row.to_vec();
        assert!(set_simd(false));
        zenavif::simd::unpremultiply8_dispatch(&mut scalar);
        let mut neon = row.to_vec();
        assert!(set_simd(true));
        zenavif::simd::unpremultiply8_dispatch(&mut neon);
        assert_eq!(neon, scalar, "tier parity at width {width}");
        suite.compare(format!("unpremultiply8/{width}px"), move |g| {
            g.throughput(Throughput::Bytes((width * 4) as u64));
            for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                g.bench(arm, move |b| {
                    // Buffer built in with_input so the clone is not timed.
                    b.with_input(move || {
                        assert!(set_simd(simd));
                        row.to_vec()
                    })
                    .run(move |mut r| {
                        zenavif::simd::unpremultiply8_dispatch(&mut r);
                        r
                    })
                });
            }
        });
    }
    set_simd(true);
}

zenbench::main!(bench);
