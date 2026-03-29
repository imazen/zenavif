# zenavif ![CI](https://img.shields.io/github/actions/workflow/status/imazen/zenavif/ci.yml?branch=main&style=for-the-badge) ![MSRV](https://img.shields.io/badge/MSRV-1.93-blue?style=for-the-badge) ![License](https://img.shields.io/badge/License-AGPL%2FCommercial-blue?style=for-the-badge)

Pure Rust AVIF image codec. Decodes and encodes AVIF images using
[rav1d-safe](https://github.com/memorysafety/rav1d) (AV1 decoder) and
[zenavif-parse](https://crates.io/crates/zenavif-parse) (AVIF container parser).

## What it does

- Decodes 8/10/12-bit AVIF with all chroma subsampling modes (4:2:0, 4:2:2, 4:4:4, monochrome)
- Handles alpha channels (straight and premultiplied)
- Supports full and limited color range, HDR color spaces (BT.2020, P3, etc.)
- Preserves EXIF, XMP, rotation, mirror, clean aperture, pixel aspect ratio, HDR metadata
- Decodes animated AVIF sequences with per-frame timing
- Decodes gain maps (ISO 21496-1) and depth auxiliary images from AVIF containers
- Encodes AVIF with optional gain map embedding via `GainMapConfig` (requires `encode` feature)
- Encodes AVIF via [zenravif](https://github.com/imazen/cavif-rs) (optional `encode` feature; requires local zenravif path dep — not yet published to crates.io)
- 100% safe Rust by default. Zero `unsafe` in the decode path.
- Cooperative cancellation via the [`enough`](https://crates.io/crates/enough) crate

## Quick Start

```rust
use zenavif::decode;

let avif_data = std::fs::read("image.avif").unwrap();
let image = decode(&avif_data).unwrap();
println!("{}x{}", image.width(), image.height());
```

### Custom configuration

```rust
use zenavif::{decode_with, DecoderConfig};
use enough::Unstoppable;

let config = DecoderConfig::new()
    .threads(4)
    .apply_grain(true)
    .frame_size_limit(8192 * 8192);

let avif_data = std::fs::read("image.avif").unwrap();
let image = decode_with(&avif_data, &config, &Unstoppable).unwrap();
```

### Animation

```rust
let avif_data = std::fs::read("animation.avif").unwrap();
let animation = zenavif::decode_animation(&avif_data).unwrap();
for frame in &animation.frames {
    println!("{}x{} frame, {}ms",
        frame.pixels.width(), frame.pixels.height(), frame.duration_ms);
}
```

### Encoding (requires `encode` feature)

```rust,ignore
use zenavif::{encode, decode};

let image = decode(&std::fs::read("input.avif").unwrap()).unwrap();
let encoded = zenavif::encode(&image).unwrap();
std::fs::write("output.avif", &encoded.avif_file).unwrap();
```

## Features

| Feature | Description |
|---|---|
| *(default)* | Pure Rust decode via rav1d-safe. No unsafe code. |
| `encode` | AVIF encoding via zenravif (pure Rust) |
| `encode-asm` | Encoding with hand-written assembly (fastest, uses unsafe) |
| `encode-threading` | Multi-threaded encoding |
| `encode-imazen` | Encoding with zenrav1e fork extras (QM, lossless) |
| `unsafe-asm` | Decoding with hand-written assembly via C FFI (fastest, uses unsafe) |
| `zencodec` | Integration with [zencodec](https://crates.io/crates/zencodec) trait hierarchy |
| `zennode` | Pipeline node definitions for [zennode](https://github.com/imazen/zennode) graph engine |

## Building

```bash
# Default safe decoder
cargo build --release

# With encoding
cargo build --release --features encode

# Fast assembly decoder (uses unsafe + C FFI)
cargo build --release --features unsafe-asm

# Run tests
cargo test

# Run with test vectors
just download-vectors
just test-integration
```

## Credits

This project builds on excellent work by others:

- **[rav1d](https://github.com/memorysafety/rav1d)** (BSD-2-Clause) — Pure Rust AV1 decoder (Rust port of [dav1d](https://code.videolan.org/videolan/dav1d)). Provides the AV1 decoding backend via its managed safe API.

- **[zenavif-parse](https://crates.io/crates/zenavif-parse)** (MIT/Apache-2.0) — AVIF container parser for extracting image items and metadata from the ISOBMFF container.

- **[yuv](https://crates.io/crates/yuv)** (MIT) — YUV to RGB color conversion.

- **[libavif](https://github.com/AOMediaCodec/libavif)** (BSD-2-Clause) — Reference AVIF implementation used for pixel-level verification and behavioral reference.

## Limitations

- The `encode` feature requires a local path dependency on zenravif, which is not yet published to crates.io.
- This crate is not yet published to crates.io.

## License

Dual-licensed: [AGPL-3.0](LICENSE-AGPL3) or [commercial](LICENSE-COMMERCIAL).

I've maintained and developed open-source image server software — and the 40+
library ecosystem it depends on — full-time since 2011. Fifteen years of
continual maintenance, backwards compatibility, support, and the (very rare)
security patch. That kind of stability requires sustainable funding, and
dual-licensing is how we make it work without venture capital or rug-pulls.
Support sustainable and secure software; swap patch tuesday for patch leap-year.

[Our open-source products](https://www.imazen.io/open-source)

**Your options:**

- **Startup license** — $1 if your company has under $1M revenue and fewer
  than 5 employees. [Get a key →](https://www.imazen.io/pricing)
- **Commercial subscription** — Governed by the Imazen Site-wide Subscription
  License v1.1 or later. Apache 2.0-like terms, no source-sharing requirement.
  Sliding scale by company size.
  [Pricing & 60-day free trial →](https://www.imazen.io/pricing)
- **AGPL v3** — Free and open. Share your source if you distribute.

See [LICENSE-COMMERCIAL](LICENSE-COMMERCIAL) for details.

## AI-Generated Code Notice

Developed with AI assistance (Claude, Anthropic). Not all code manually reviewed — review critical paths before production use.
