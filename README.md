# zenavif

[![CI](https://github.com/imazen/zenavif/workflows/CI/badge.svg)](https://github.com/imazen/zenavif/actions)
[![Crates.io](https://img.shields.io/crates/v/zenavif.svg)](https://crates.io/crates/zenavif)
[![Documentation](https://docs.rs/zenavif/badge.svg)](https://docs.rs/zenavif)
[![License](https://img.shields.io/crates/l/zenavif.svg)](https://github.com/imazen/zenavif#license)

Pure Rust AVIF image decoder powered by [rav1d](https://github.com/memorysafety/rav1d).

## Features

- **100% Safe Rust** - Default `managed` feature uses zero unsafe code
- **Fast** - Optional `asm` feature uses hand-written assembly for maximum performance  
- **Comprehensive** - Supports 8/10/12-bit, all chroma subsampling modes, alpha channel
- **Ergonomic API** - Simple decode functions with detailed error types
- **Cancellable** - Built-in cooperative cancellation support

## Quick Start

```rust
use zenavif::decode;

let avif_data = std::fs::read("image.avif")?;
let image = decode(&avif_data)?;

match image {
    DecodedImage::Rgb8(img) => {
        println!("RGB8 image: {}x{}", img.width(), img.height());
    }
    DecodedImage::Rgba8(img) => {
        println!("RGBA8 image: {}x{}", img.width(), img.height());
    }
    _ => {}
}
```

## Features

### `managed` (default)

100% safe Rust implementation using [rav1d-safe](https://github.com/memorysafety/rav1d)'s managed API. No unsafe code in the entire decode path. Enforced by `#![deny(unsafe_code)]` at module level.

### `asm`

High-performance implementation using hand-written assembly. Uses C FFI for maximum speed. Best for production workloads where performance is critical.

## Configuration

```rust
use zenavif::{decode_with, DecoderConfig};
use enough::Unstoppable;

let config = DecoderConfig::new()
    .threads(4)               // Use 4 threads (0 = auto-detect)
    .apply_grain(true)        // Apply film grain
    .frame_size_limit(8192 * 8192); // Max 8K resolution

let image = decode_with(&avif_data, &config, &Unstoppable)?;
```

## Supported Formats

- ✅ 8-bit, 10-bit, 12-bit color depth
- ✅ 4:2:0, 4:2:2, 4:4:4 chroma subsampling
- ✅ Monochrome (grayscale)
- ✅ Alpha channel (straight and premultiplied)
- ✅ Full and limited color range
- ✅ HDR color spaces (BT.2020, P3, etc.)
- ❌ Animated AVIF (use real AV1 video instead)
- ❌ Grid-based collages

## Building

```bash
# Safe managed API (default)
cargo build --release

# Fast assembly version
cargo build --release --no-default-features --features asm

# Run tests
cargo test

# Run with test vectors
just download-vectors
just test-integration
```

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
