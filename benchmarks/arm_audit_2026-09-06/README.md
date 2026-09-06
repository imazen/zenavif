# AVIF ARM audit, 2026-09-06

Coverage: three YUV420 conversion sizes, six unpremultiplication widths,
and three full-decode fixtures. AVIF encoding and optional AV1 backends are
not covered by this report. No production codec implementation changed.

Apple M4 Pro, macOS, Rust 1.98.0 / LLVM 22. Four build/Rayon/OMP threads,
`nice -n 19`, runtime dispatch without `target-cpu=native`. Codec baseline
`2ebca1b4`; rav1d-safe is pinned to `66f58fa6a64c689998721cc5cdb16a4698e26eec`.

## Conversion

The old YUV benchmark used zenbench's sequential `criterion_compat` API.
The updated benchmark uses native interleaved comparisons and untimed token
configuration. It fails explicitly when the tier cannot be toggled.

| YUV420 size | NEON mean | Forced scalar mean |
|---|---:|---:|
| 64×64 | 13.83 µs | 12.78 µs |
| 512×256 | 257.92 µs | 253.79 µs |
| 1920×1080 | 3.72 ms | 3.70 ms |

The latter two confidence intervals cross zero. The small case favors scalar
by 7.45% in the paired analysis. Both tier bodies use auto-vectorized integer
loops: [assembly excerpts](yuv-auto-vectorized.asm) show vector `mul.4s`,
`mla.4s`, and clamping in both dispatch branches. This is not absent ARM
vectorization. Full data: [YUV log](avif-yuv-tiers.log).

Unpremultiplication favors NEON at widths 17, 64, 512, 1920 and 4096. At one
pixel, both use scalar arithmetic and the NEON entry has a small dispatch cost
(20.3 versus 19.7 ns). Several scalar-arm runs have high variance; use the
paired intervals in [the log](avif-unpremul-tiers.log), not ratios of unrelated
runs. Untimed exact-output assertions passed at every width.

## Full decode

| Fixture | Native SIMD mean | Forced scalar mean |
|---|---:|---:|
| kodim03_yuv420_8bpc.avif | 6.50 ms | 13.48 ms |
| extended_pixi.avif | 111.32 µs | 112.03 µs |
| clap_irot_imir_non_essential.avif | 100.35 µs | 116.29 µs |

These fixtures are loaded from `tests/vectors/libavif`, with missing files or
decode errors failing explicitly. Decoding uses one thread. All three produced
identical pixel bytes and dimensions across tiers before timing. The middle
comparison is not statistically distinguishable. See [decode log](avif-decode-tiers.log).

The pinned rav1d-safe source returns to scalar when its ARM token is disabled
(for example `mc_arm::avg_dispatch_inner`); the old backend benchmark's blanket
ARM skip no longer describes this source. The measured AVIF decode comparison
also demonstrates that disabling the token succeeds for these fixtures.

Clippy passed for the library and all three modified benchmarks with
`-D warnings`; scoped formatting passed. Reproduce with
`just arm-tiers-macos tier_isolation`, `just arm-tiers-macos unpremul_tiers`,
and `just arm-tiers-macos decode_benchmark`. These results do not calibrate
quality settings or establish performance across other AVIF configurations.
