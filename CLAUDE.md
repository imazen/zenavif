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
- `src/encoder.rs` - AVIF encoding via zenravif (behind `encode` feature)

### Integration
- `src/zencodec.rs` - zencodec-types trait implementations

## Dependencies

- `rav1d-safe` - Pure Rust AV1 decoder (managed API, no C FFI)
- `zenavif-parse` - AVIF container parser (path dep)
- `zenravif` / `ravif` - AVIF encoder (optional, `encode` feature)
- `zencodec-types` - Codec abstraction traits (path dep)
- `zenpixels` - Pixel buffer types (path dep)
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
- `zencodec` - zencodec-types trait integration
- `_dev` - Expose internal YUV modules for profiling (not public API)

## Known Bugs

### rav1d-safe Threading Race Condition
DisjointMut overlap panic with multi-threaded decoding (`cdef.rs` / `cdef_apply.rs`).
Default is `threads: 1` as workaround. Upstream rav1d-safe issue.

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
