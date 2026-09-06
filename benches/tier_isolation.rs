//! SIMD-tier isolation: the native top tier vs the same code forced to scalar.
//!
//! zenavif's SIMD is the YUV->RGB conversion in `src/yuv_convert.rs`
//! (`#[magetypes(v3, neon, wasm128, scalar)]`). `yuv_conversion_benchmark.rs`
//! is titled "SIMD vs scalar" but never runs a scalar arm — it only probes
//! `Desktop64::summon()` and prints whether AVX2 is present. On aarch64 that
//! probe always fails, so that bench prints
//! "AVX2/FMA not available - using scalar fallback" while NEON is in fact
//! running, and reports a single unlabelled number.
//!
//! This bench actually disables the token and measures both arms.
//!
//! Run: `cargo bench --bench tier_isolation --features _dev`
//! Do NOT build with `-C target-cpu=native`: that pins the tier at compile
//! time, after which it cannot be disabled and this benchmark fails explicitly.

use zenavif::yuv_convert::{YuvMatrix, YuvRange, yuv420_to_rgb8};
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
fn set_simd(enabled: bool) -> bool {
    // `dangerously_disable_token_process_wide` is inherent on every token
    // type (no `SimdToken` trait import needed on any arch).
    TierToken::dangerously_disable_token_process_wide(!enabled).is_ok()
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn set_simd(_enabled: bool) -> bool {
    false
}

/// Varied planes, not a flat fill. The existing bench uses `vec![128u8; ..]`
/// for every plane, which is a constant image — fine for throughput of a
/// branch-free kernel, but it exercises none of the value-dependent paths and
/// makes the result easy to over-trust.
fn planes(w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let uv = w.div_ceil(2) * h.div_ceil(2);
    let mut y = vec![0u8; w * h];
    let mut u = vec![0u8; uv];
    let mut v = vec![0u8; uv];
    let mut s = 0x9e37_79b9u32;
    let mut next = move || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 24) as u8
    };
    for p in y.iter_mut() {
        *p = next();
    }
    for p in u.iter_mut() {
        *p = next();
    }
    for p in v.iter_mut() {
        *p = next();
    }
    (y, u, v)
}

fn bench_tiers(suite: &mut Suite) {
    assert!(
        set_simd(true) && set_simd(false),
        "build with _dev and runtime SIMD dispatch"
    );
    set_simd(true);
    for &(name, w, h) in &[
        ("64x64", 64usize, 64usize),
        ("512x256", 512, 256),
        ("1920x1080", 1920, 1080),
    ] {
        let (yp, up, vp) = planes(w, h);
        suite.compare(format!("yuv420_to_rgb8/{name}"), |group| {
            group.throughput(Throughput::Elements((w * h) as u64));
            for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                let (yp, up, vp) = (yp.clone(), up.clone(), vp.clone());
                group.bench(arm, move |b| {
                    b.with_input(move || assert!(set_simd(simd))).run(|_| {
                        yuv420_to_rgb8(
                            black_box(&yp),
                            w,
                            black_box(&up),
                            w.div_ceil(2),
                            black_box(&vp),
                            w.div_ceil(2),
                            w,
                            h,
                            YuvRange::Full,
                            YuvMatrix::Bt709,
                        )
                    })
                });
            }
        });
    }
}

zenbench::main!(bench_tiers);
