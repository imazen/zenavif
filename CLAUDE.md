# zenavif

Pure Rust AVIF encoder/decoder wrapping rav1d-safe (pure Rust AV1 decoder) and zenavif-parse.

## Quick Commands

```bash
just check        # cargo check
just build        # cargo build --release
just test         # cargo test
just clippy       # cargo clippy with warnings as errors
just fmt          # cargo fmt
just build-encode # cargo build --features encode
just test-encode  # cargo test --features encode
```

## Architecture

### Decoding
- `src/lib.rs` - Public API, re-exports
- `src/error.rs` - Error types with whereat location tracking
- `src/config.rs` - DecoderConfig builder
- `src/image.rs` - DecodedImage enum, ImageInfo metadata
- `src/decoder_managed.rs` - Main decoder (100% safe Rust, rav1d-safe managed API)
- `src/decoder.rs` - Legacy FFI decoder (behind `unsafe-asm` feature gate)
- `src/convert.rs` - Alpha channel handling, unpremultiply

### YUV Conversion
- `src/yuv_convert.rs` - Float SIMD path (AVX2/FMA via archmage)
- `src/yuv_convert_libyuv.rs` - Exact libyuv integer math (BT.709, BT.601)
- `src/yuv_convert_libyuv_simd.rs` - AVX2 SIMD libyuv path
- `src/yuv_convert_libyuv_autovec.rs` - Auto-vectorized libyuv variant
- `src/yuv_convert_fast.rs` - Fast fixed-point integer path
- `src/chroma.rs` - YUV chroma upsampling iterators

### Encoding
- `src/encoder.rs` - AVIF encoding via zenravif (behind `encode` feature; currently disabled — zenravif not yet published)
- `src/encode_plan.rs` - `EncoderConfig::resolve_plan(PlanInput) -> EncodePlan` static resolution + provenance-tagged mirrors of zenravif's quality→quantizer curve and SpeedTweaks tables (`encode` feature)
- `src/sweep.rs` - Sweep planner + byte-identity fingerprint (`__expert` feature); see `docs/VARIANT_GENERATION.md` for the knob audit (dominance/trial/metric), the fingerprint exclusions, and the harness findings
- `examples/sweep_validate.rs` - Empirical axis validation; **re-run whenever bumping the zenravif dep or touching the sweep axes/fingerprint** (`benchmarks/sweep_validate_*.tsv`). Gotcha: encode+decode+score loops in rayon pools need `stack_size(32 MB)`. Per-task stack peaks at ~0.5 MB dominated by rav1d-safe's `rav1d_open` decoder-context construction (gdb-verified 2026-06-11; zenrav1e encode itself needs ≤128 KB even at speed 2/q10/noise — an earlier "partition RDO" attribution was wrong). The overflow mechanism is rayon work-stealing stacking whole task contexts on one worker stack at zensim's internal join points, so 2 MB defaults die probabilistically under load
- `tests/encode_contracts.rs` - Encode-level byte contracts (alpha quality fallback, quantizer mediation, subsampling liveness)

### Additional Source Files
- `src/decode_av1.rs` - AV1 bitstream decoding entry points
- `src/strip_convert.rs` - Strip-based pixel conversion utilities
- `src/detect.rs` - AVIF file detection / sniffing
- `src/codec.rs` - zencodec trait implementations
- `src/zennode_defs.rs` - zennode pipeline node definitions (behind `zennode` feature)
- `src/simd/` - SIMD acceleration modules

## Dependencies

- `rav1d-safe` - Pure Rust AV1 decoder (managed API, no C FFI)
- `zenavif-parse` - AVIF container parser (registry dep)
- `zenravif` / `ravif` - AVIF encoder (optional, `encode` feature)
- `zencodec` - Codec abstraction traits (registry dep)
- `zenpixels` - Pixel buffer types (registry dep)
- `archmage` / `magetypes` - Token-based safe SIMD
- `yuv` - YUV to RGB conversion (supplementary)
- `imgref` - Image buffer type
- `rgb` - RGB pixel types
- `enough` - Cooperative cancellation
- `whereat` - Error location tracking
- `thiserror` - Error derive macro

## Features

- `(default)` - Pure Rust decode only, safe SIMD via archmage
- `encode` - AVIF encoding via zenravif
- `encode-imazen` - Encoding with zenrav1e fork extras (QM, VAQ, still-image, lossless)
- `encode-asm` - Encoding with hand-written assembly (fastest, unsafe)
- `encode-threading` - Encoding with multi-threading
- `unsafe-asm` - Decoding with hand-written assembly via C FFI (fastest, unsafe)
- `zencodec` - zencodec trait integration
- `_dev` - Expose internal YUV modules for profiling (not public API)

## Known Bugs

### zenrav1e lossless is not bit-exact (zenrav1e#9)
`with_lossless(true)` produces ±2 scatter on ~28 % of pixels (noise,
speed 6, 8-bit, identity 4:4:4 full-range — measured via the identity
passthrough, so the AV1 roundtrip is the only transform). The identity
roundtrip tests carry a documented ≤2 tolerance until fixed; likely
related to #8 (lossless speed inversion).


### rav1d-safe Threading Race Condition (RESOLVED)
DisjointMut overlap panic was caused by frame threading. Fix: `max_frame_delay=1`
gives tile parallelism without frame threading. Default threads now 0 (auto-detect).

## TODO: Encoding Enhancements

### Target-Quality Convergence (not yet implemented)
Binary-search-over-quantizer to hit a target perceptual quality score.
Decision needed: Butteraugli vs SSIMULACRA2 (or both).

### Encoding Features (`encode-imazen` feature gate)
All wired through to zenrav1e fork. Benchmarked results (ravif 7265eea):
- `with_qm(true)` - only measurable win (~10% BD-rate). Default enabled.
- `with_vaq()` - hurts quality; psychovisual tune already includes SSIM boost.
- `tune_still_image` - no effect; ravif disables CDEF at high quality levels.
- `with_lossless` - implemented, works.

## Canonical training data + indexes (added 2026-05-20)

**The canonical index for all ML data lives at `~/work/zen/DATA_PROVENANCE.md`.**

Quick paths:
- Trainer input: `/mnt/v/zen/zensim-training/canonical-2026-05-21/`
- Master inventory: `~/work/zen/_ml-inventory-2026-05-20/00-MASTER-SYNTHESIS.md`
- Per-codec picker audit: `~/work/zen/_ml-inventory-2026-05-20/05-per-codec-pickers.md`

## ML/auto-tune status (2026-05-20)

zenavif is the **only zen codec with a production picker.** `EncoderConfig::auto_tune()` (feature `auto-tune`) loads `src/models/rav1e_picker_v0_1_1.bin` (ZNPR v2, ~217 KB) + the two LUT JSONs and returns optimal (speed, quality) knobs for a `QualityTarget`. Reference implementation for any future codec picker wiring.

Training intermediates from earlier 2026-05-04 sweep (`benchmarks/zenavif_picker_v0.{3,4,5}_2026-05-04.bin`) are kept for reproducibility; production wires `v0.1.1`.

The training pipeline that produces a new `rav1e_picker_v*.bin` lives in `~/work/zen/zenanalyze/zentrain/` (Python). See `~/work/zen/_ml-inventory-2026-05-20/05-per-codec-pickers.md` for cross-codec picker design.
