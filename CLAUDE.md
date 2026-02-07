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

## Recent Changes (2026-02-06)

### ✅ Managed API Migration Complete

The managed decoder (`src/decoder_managed.rs`) is now fully functional and is the default. Key accomplishments:

1. **Fixed all compilation errors** (51 → 0)
   - Fixed enum variant names (Mono → Monochrome, etc.)
   - Fixed `ImageInfo` construction with all required fields
   - Added `to_yuv_matrix()` helper for YUV color space conversion
   - Fixed error handling with proper `map_err` usage

2. **Implemented complete YUV to RGB conversion**
   - Proper row-iterator-based approach matching chroma.rs API
   - Support for all chroma subsampling modes (420, 422, 444, Monochrome)
   - Both 8-bit and 16-bit (10/12-bit) conversion paths

3. **Implemented alpha channel handling**
   - Creates RGBA images when alpha is present
   - Properly handles premultiplied alpha
   - Uses rav1d-safe's zero-copy managed API

4. **100% safe Rust**
   - No unsafe code in the managed decoder
   - `#![deny(unsafe_code)]` at module level
   - Uses rav1d-safe's managed API exclusively

### Build Status

- `cargo build --no-default-features --features managed` ✅ SUCCESS
- `cargo build --release` (default features) ✅ SUCCESS  
- `cargo test --features managed` ✅ 7/7 PASS

### Next Steps

From the original handoff document, remaining tasks:

1. **Remove C FFI dependencies** - Update Cargo.toml to ensure managed feature doesn't pull in c-ffi
2. **Delete/rename old decoder** - Add conditional compilation to decoder.rs or rename to decoder_ffi.rs
3. **Integration tests** - Download AVIF test vectors and create comprehensive test suite
4. **CI configuration** - Set up GitHub Actions workflows
5. **Performance optimization** - Profile and optimize managed decoder

The core implementation is complete and functional!
