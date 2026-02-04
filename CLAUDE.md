# zenavif

Pure Rust AVIF decoder wrapping rav1d (pure Rust AV1 decoder) and avif-parse.

## Quick Commands

```bash
just check   # cargo check
just build   # cargo build --release
just test    # cargo test
just clippy  # cargo clippy with warnings as errors
just fmt     # cargo fmt
```

## Architecture

- `src/lib.rs` - Public API, re-exports
- `src/error.rs` - Error types with whereat location tracking
- `src/config.rs` - DecoderConfig builder
- `src/image.rs` - DecodedImage enum, ImageInfo metadata
- `src/decoder.rs` - AvifDecoder wrapping rav1d FFI
- `src/convert.rs` - Alpha channel handling, unpremultiply
- `src/chroma.rs` - YUV chroma upsampling iterators

## Dependencies

- `rav1d` - Pure Rust AV1 decoder (C FFI interface)
- `avif-parse` - AVIF container parser
- `yuv` - YUV to RGB conversion
- `imgref` - Image buffer type
- `rgb` - RGB pixel types
- `enough` - Cooperative cancellation
- `whereat` - Error location tracking
- `thiserror` - Error derive macro

## Known Bugs

(none yet)

## Investigation Notes

(none yet)
