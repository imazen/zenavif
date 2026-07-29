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
const TIER_NAME: &str = if cfg!(target_arch = "aarch64") { "neon" } else { "v3(avx2)" };

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
fn set_simd(on: bool) -> bool {
    use archmage::SimdToken;
    TierToken::dangerously_disable_token_process_wide(!on).is_ok()
}
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn set_simd(_on: bool) -> bool { false }

fn bench(suite: &mut Suite) {
    if !set_simd(true) || !set_simd(false) {
        eprintln!("[unpremul_tiers] SIMD tier not toggleable here. Skipping.");
        return;
    }
    set_simd(true);
    for width in [1920usize, 512] {
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
        suite.compare(&format!("unpremultiply8/{width}px"), move |g| {
            g.throughput(Throughput::Bytes((width * 4) as u64));
            for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                g.bench(arm, move |b| {
                    // Buffer built in with_input so the clone is not timed.
                    b.with_input(move || { set_simd(simd); row.to_vec() })
                        .run(move |mut r| { zenavif::simd::unpremultiply8_dispatch(&mut r); r })
                });
            }
        });
    }
    set_simd(true);
}

zenbench::main!(bench);
