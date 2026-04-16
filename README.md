# zenavif ![CI](https://img.shields.io/github/actions/workflow/status/imazen/zenavif/ci.yml?style=flat-square&label=CI) ![crates.io](https://img.shields.io/crates/v/zenavif?style=flat-square) [![lib.rs](https://img.shields.io/crates/v/zenavif?style=flat-square&label=lib.rs&color=blue)](https://lib.rs/crates/zenavif) ![docs.rs](https://img.shields.io/docsrs/zenavif?style=flat-square) ![license](https://img.shields.io/crates/l/zenavif?style=flat-square)

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
- Encodes AVIF via [zenravif](https://lib.rs/crates/zenravif) (optional `encode` feature)
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

### Encoding with custom config

```rust,ignore
use zenavif::{EncoderConfig, encode_rgb8};
use almost_enough::Unstoppable;

let config = EncoderConfig::new()
    .quality(80.0)   // 1.0 (worst) to 100.0 (best)
    .speed(4);       // 1 (slowest) to 10 (fastest)

let encoded = encode_rgb8(img.as_ref(), &config, Unstoppable.into_token()).unwrap();
std::fs::write("output.avif", &encoded.avif_file).unwrap();
```

## Encoder configuration guide

### Speed vs quality tradeoffs

Speed controls how much time the encoder spends optimizing. Higher speeds
produce slightly larger files but encode much faster. Quality is comparable
across speeds — the main tradeoff is encode time vs file size, not visual quality.

Measured on a 512×512 photographic image (CID22 corpus), q80, 8-bit
([full sweep data](benchmarks/avif_encode_fine_sweep_2026-04-16.tsv)):

| Speed | Encode time | File size | Compression ratio | zensim |
|------:|:----------:|:---------:|:-----------------:|:------:|
| 1 | 1.1s | 55.9K | 14.1x | 85.4 |
| 2 | 1.1s | 55.9K | 14.1x | 85.4 |
| 4 | 0.8s | 56.5K | 13.9x | 85.5 |
| 6 | 0.2s | 56.8K | 13.8x | 85.5 |
| 10 | 78ms | 59.0K | 13.3x | 85.4 |

Speed 4 is a good default. Speed 6 gives 4x faster encoding with identical quality.
Speed 10 is best for real-time/interactive use — still good quality at ~80ms per frame.
Speed 1-2 produce marginally smaller files but take 5-14x longer than speed 4.

### Quality parameter

The `quality` parameter maps to an AV1 quantizer index:

| Quality | Use case | Typical compression |
|--------:|----------|:-------------------:|
| 30 | Thumbnails, previews | 100-120x |
| 50 | Web images (aggressive) | 40-45x |
| 65 | Web images (balanced) | 22-25x |
| 80 | High quality (default) | 12-14x |
| 95 | Near-lossless | 5-6x |
| 100 | Lossless | 2-3x |

### Quantization matrices (QM)

With the `encode-imazen` feature, quantization matrices are enabled by default.
QM applies frequency-dependent quantization weights that save **9-13% file size**
with negligible quality impact (<1 zensim point at all speeds and quality levels).

QM is automatically disabled for lossless encoding (quality 100).

```rust,ignore
// QM is on by default. To disable:
let config = EncoderConfig::new()
    .quality(80.0)
    .with_qm(false);
```

### Bit depth

The encoder matches output bit depth to input type by default:

- `encode_rgb8` / `encode_rgba8` → 8-bit AV1
- `encode_rgb16` / `encode_rgba16` → 10-bit AV1

Override with `.bit_depth(EncodeBitDepth::Ten)` if you want 10-bit output
from 8-bit input (slightly better quality at the cost of larger files and
wider decoder compatibility requirements).

### Decoder output depth

The decoder outputs at the AV1 bitstream's native bit depth. Files encoded
at 10-bit (common from other encoders that default to 10-bit) produce 16-bit
`PixelBuffer` output. Use `prefer_8bit(true)` to downscale to 8-bit:

```rust,ignore
let config = DecoderConfig::new().prefer_8bit(true);
let image = decode_with(&avif_data, &config, &Unstoppable).unwrap();
// image is Rgb8 even if the AV1 bitstream was 10-bit
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

- The `encode` feature is not yet available from a crates.io build (zenravif path dep needs wiring).

## Image tech I maintain

| | |
|:--|:--|
| State of the art codecs* | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · **zenavif** ([rav1d-safe] · [zenrav1e] · [zenavif-parse] · [zenavif-serialize]) · [zenjxl] ([jxl-encoder] · [zenjxl-decoder]) · [zentiff] · [zenbitmaps] · [heic] · [zenraw] · [zenpdf] · [ultrahdr] · [mozjpeg-rs] · [webpx] |
| Compression | [zenflate] · [zenzop] |
| Processing | [zenresize] · [zenfilters] · [zenquant] · [zenblend] |
| Metrics | [zensim] · [fast-ssim2] · [butteraugli] · [resamplescope-rs] · [codec-eval] · [codec-corpus] |
| Pixel types & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] |
| Pipeline | [zenpipe] · [zencodec] · [zencodecs] · [zenlayout] · [zennode] |
| ImageResizer | [ImageResizer] (C#) — 24M+ NuGet downloads across all packages |
| [Imageflow][] | Image optimization engine (Rust) — [.NET][imageflow-dotnet] · [node][imageflow-node] · [go][imageflow-go] — 9M+ NuGet downloads across all packages |
| [Imageflow Server][] | [The fast, safe image server](https://www.imazen.io/) (Rust+C#) — 552K+ NuGet downloads, deployed by Fortune 500s and major brands |

<sub>* as of 2026</sub>

### General Rust awesomeness

[archmage] · [magetypes] · [enough] · [whereat] · [zenbench] · [cargo-copter]

[And other projects](https://www.imazen.io/open-source) · [GitHub @imazen](https://github.com/imazen) · [GitHub @lilith](https://github.com/lilith) · [lib.rs/~lilith](https://lib.rs/~lilith) · [NuGet](https://www.nuget.org/profiles/imazen) (over 30 million downloads / 87 packages)

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

[zenjpeg]: https://github.com/imazen/zenjpeg
[zenpng]: https://github.com/imazen/zenpng
[zenwebp]: https://github.com/imazen/zenwebp
[zengif]: https://github.com/imazen/zengif
[zenjxl]: https://github.com/imazen/zenjxl
[zentiff]: https://github.com/imazen/zentiff
[zenbitmaps]: https://github.com/imazen/zenbitmaps
[heic]: https://github.com/imazen/heic-decoder-rs
[zenraw]: https://github.com/imazen/zenraw
[zenpdf]: https://github.com/imazen/zenpdf
[ultrahdr]: https://github.com/imazen/ultrahdr
[jxl-encoder]: https://github.com/imazen/jxl-encoder
[zenjxl-decoder]: https://github.com/imazen/zenjxl-decoder
[rav1d-safe]: https://github.com/imazen/rav1d-safe
[zenrav1e]: https://github.com/imazen/zenrav1e
[mozjpeg-rs]: https://github.com/imazen/mozjpeg-rs
[zenavif-parse]: https://github.com/imazen/zenavif-parse
[zenavif-serialize]: https://github.com/imazen/zenavif-serialize
[webpx]: https://github.com/imazen/webpx
[zenflate]: https://github.com/imazen/zenflate
[zenzop]: https://github.com/imazen/zenzop
[zenresize]: https://github.com/imazen/zenresize
[zenfilters]: https://github.com/imazen/zenfilters
[zenquant]: https://github.com/imazen/zenquant
[zenblend]: https://github.com/imazen/zenblend
[zensim]: https://github.com/imazen/zensim
[fast-ssim2]: https://github.com/imazen/fast-ssim2
[butteraugli]: https://github.com/imazen/butteraugli
[zenpixels]: https://github.com/imazen/zenpixels
[zenpixels-convert]: https://github.com/imazen/zenpixels
[linear-srgb]: https://github.com/imazen/linear-srgb
[garb]: https://github.com/imazen/garb
[zenpipe]: https://github.com/imazen/zenpipe
[zencodec]: https://github.com/imazen/zencodec
[zencodecs]: https://github.com/imazen/zencodecs
[zenlayout]: https://github.com/imazen/zenlayout
[zennode]: https://github.com/imazen/zennode
[Imageflow]: https://github.com/imazen/imageflow
[Imageflow Server]: https://github.com/imazen/imageflow-server
[imageflow-dotnet]: https://github.com/imazen/imageflow-dotnet
[imageflow-node]: https://github.com/imazen/imageflow-node
[imageflow-go]: https://github.com/imazen/imageflow-go
[ImageResizer]: https://github.com/imazen/resizer
[archmage]: https://github.com/imazen/archmage
[magetypes]: https://github.com/imazen/archmage
[enough]: https://github.com/imazen/enough
[whereat]: https://github.com/lilith/whereat
[zenbench]: https://github.com/imazen/zenbench
[cargo-copter]: https://github.com/imazen/cargo-copter
[resamplescope-rs]: https://github.com/imazen/resamplescope-rs
[codec-eval]: https://github.com/imazen/codec-eval
[codec-corpus]: https://github.com/imazen/codec-corpus
