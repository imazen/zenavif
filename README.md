# zenavif [![CI](https://img.shields.io/github/actions/workflow/status/imazen/zenavif/ci.yml?style=flat-square&label=CI)](https://github.com/imazen/zenavif/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/zenavif?style=flat-square)](https://crates.io/crates/zenavif) [![lib.rs](https://img.shields.io/crates/v/zenavif?style=flat-square&label=lib.rs&color=blue)](https://lib.rs/crates/zenavif) [![docs.rs](https://img.shields.io/docsrs/zenavif?style=flat-square)](https://docs.rs/zenavif) [![MSRV](https://img.shields.io/badge/MSRV-1.93-blue?style=flat-square)](https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field) [![license](https://img.shields.io/badge/license-AGPL--3.0%20%2F%20Commercial-blue?style=flat-square)](#license)

Pure Rust AVIF image codec. Decodes and encodes AVIF images using
[rav1d-safe](https://github.com/imazen/rav1d-safe) (AV1 decoder) and
[zenavif-parse](https://crates.io/crates/zenavif-parse) (AVIF container parser).

This repository is a cargo workspace hosting three published crates (the
container crates were absorbed with full histories on 2026-07-16):

| Crate | Directory | What it does |
|---|---|---|
| [zenavif](https://crates.io/crates/zenavif) | repo root | AVIF codec (decode + encode) |
| [zenavif-parse](https://crates.io/crates/zenavif-parse) | `zenavif-parse/` | AVIF/ISOBMFF container parser (demuxer) |
| [zenavif-serialize](https://crates.io/crates/zenavif-serialize) | `zenavif-serialize/` | AVIF container serializer (muxer) |

Release tags are crate-prefixed (`zenavif-vX.Y.Z`, `zenavif-parse-vX.Y.Z`,
`zenavif-serialize-vX.Y.Z`); historical tags from the absorbed repos carry
their lineage prefixes (`avif-parse-v*`, `mp4parse-v*`). zenavif-parse's
corpus tests read two git submodules — run `git submodule update --init`
once per checkout.

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

## Quick start

```toml
[dependencies]
zenavif = "0.1.7"               # decode only (pure-Rust, safe)
# zenavif = { version = "0.1.7", features = ["encode"] }   # add AVIF encoding
```

`decode` returns a [`PixelBuffer`](https://docs.rs/zenpixels) (re-exported as
`zenavif::PixelBuffer`). It carries width, height, stride, bit depth, and color
metadata — it is **not** a flat `Vec<u8>`. See *Getting RGBA8 pixels* below to
get bytes.

```rust
use zenavif::decode;

let avif_data = std::fs::read("image.avif").unwrap();
let image = decode(&avif_data).unwrap();
println!("{}x{}", image.width(), image.height());
```

### Getting RGBA8 pixels

To pull tightly-packed 8-bit RGBA bytes out of the decoded buffer (regardless of
the source's channel order, subsampling, or bit depth), normalize with
[`zenpixels-convert`](https://crates.io/crates/zenpixels-convert):

```toml
# Cargo.toml — add alongside zenavif
zenpixels-convert = { version = "0.2.13", features = ["rgb"] }
```

```rust
use zenavif::decode;
use zenpixels_convert::PixelBufferConvertTypedExt; // brings to_rgba8() into scope

let image = decode(&avif_data).unwrap();
let rgba = image.to_rgba8();                  // PixelBuffer<Rgba<u8>>, always 8-bit RGBA
let (w, h) = (rgba.width(), rgba.height());
let bytes: Vec<u8> = rgba.copy_to_contiguous_bytes(); // w * h * 4, no row padding
```

`to_rgb8()`, `to_gray8()`, and `to_bgra8()` are also available. If you instead
want to keep the buffer as `imgref` rows for re-encoding, add the `imgref`
feature and use `try_as_imgref::<rgb::Rgba<u8>>()` (returns `None` unless the
buffer is exactly RGBA8 — `to_rgba8()` is the conversion that never fails).

> **Bit depth:** the decoder outputs the AV1 bitstream's native depth, so a
> 10-bit file yields a **16-bit** `PixelBuffer`. `to_rgba8()` downscales to 8-bit
> for you; if you want the decoder itself to emit 8-bit, set
> `DecoderConfig::new().prefer_8bit(true)` (see *Decoder output depth*).

### Custom configuration

```rust
use zenavif::{decode_with, DecoderConfig};
use enough::Unstoppable;

let config = DecoderConfig::new()
    .threads(4)
    .apply_grain(true)
    .frame_size_limit(8192 * 8192); // tighten the 120 MP default — see note below

let avif_data = std::fs::read("image.avif").unwrap();
let image = decode_with(&avif_data, &config, &Unstoppable).unwrap();
```

> **Server safety — `frame_size_limit` defaults to 120 MP (`120_000_000`).** The
> bare `decode()` entry point and a default `DecoderConfig` enforce this cap
> pre-flight, before any frame allocation, so an out-of-range frame fails fast
> instead of forcing a large allocation. 120 MP admits ~108 MP phone photos
> (`12000 * 9000`) while still bounding untrusted input by default. To tighten,
> pass `.frame_size_limit(max_w * max_h)` sized to what your service accepts
> (e.g. `8192 * 8192` ≈ 67 MP); to opt out entirely, pass
> `.frame_size_limit(0)` (unbounded).

### Errors and cancellation

`decode*`/`encode*` return `Result<_, whereat::At<Error>>`. The
[`At`](https://docs.rs/whereat) wrapper records the source location for
server-side logs — read it with `.location()` (returns `Option<&Location>`, with
`file()`/`line()`); call `.error()` (borrow) or `.decompose().0` (owned) to get
the inner `Error` enum to match on, and map it to an HTTP status — malformed input
(`Error::Parse`, `Error::Decode`, `ValidationError`) is a client `4xx`, while a
resource trip from your `frame_size_limit` should be a `413`/`422`.

Cancellation differs by direction, by design:

- **Decode** takes `&(impl enough::Stop)` — pass `&enough::Unstoppable` for no
  cancellation, or a real, thread-safe stopper from the
  [`almost-enough`](https://crates.io/crates/almost-enough) crate (`cargo add
  almost-enough`): `let stop = almost_enough::Stopper::new();`, then
  `decode_with(bytes, &config, &stop)?`, and call `stop.cancel()` (it is `Clone`)
  from a deadline/disconnect watcher thread.
- **Encode** takes an `almost_enough::StopToken` (the encoder's backend uses the
  `almost_enough` crate) — build one with `StopToken::new(enough::Unstoppable)`
  for the no-op case, as the encode examples below show.

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
use zenavif::{EncoderConfig, encode_with, decode, Unstoppable};
use almost_enough::StopToken;

let image = decode(&std::fs::read("input.avif").unwrap()).unwrap();

let config = EncoderConfig::new()
    .quality(80.0)   // 1.0 (worst) to 100.0 (best)
    .speed(4);       // 1 (slowest) to 10 (fastest)

// StopToken::new(Unstoppable) is the no-op token; pass a real stopper to cancel.
let encoded = encode_with(&image, &config, StopToken::new(Unstoppable)).unwrap();
std::fs::write("output.avif", &encoded.avif_file).unwrap();
```

`encode_with` takes a decoded `&PixelBuffer`. If you have raw pixels in hand,
use `encode_rgb8` / `encode_rgba8` / `encode_rgb16` / `encode_rgba16`, which take
an `imgref::ImgRef` of the matching `rgb` pixel type (e.g.
`encode_rgb8(img, &config, StopToken::new(Unstoppable))`).

## Encoder configuration guide

### Speed vs quality tradeoffs

Speed controls how much time the encoder spends optimizing. Higher speeds
produce slightly larger files but encode much faster. Quality is comparable
across speeds — the main tradeoff is encode time vs file size, not visual quality.

<!-- crates.io:skip-start -->
Measured on a 512×512 photographic image (CID22 corpus), q80, 8-bit
([full sweep data](benchmarks/avif_encode_fine_sweep_2026-04-16.tsv) ·
[methodology](benchmarks/README.md)):

| Speed | Encode time | File size | Compression ratio | zensim |
|------:|:----------:|:---------:|:-----------------:|:------:|
| 1 | 1.1s | 55.9K | 14.1x | 85.4 |
| 2 | 1.1s | 55.9K | 14.1x | 85.4 |
| 4 | 0.8s | 56.5K | 13.9x | 85.5 |
| 6 | 0.2s | 56.8K | 13.8x | 85.5 |
| 10 | 78ms | 59.0K | 13.3x | 85.4 |
<!-- crates.io:skip-end -->

Speed 4 is a good default. Speed 6 gives 4x faster encoding with identical quality.
Speed 10 is best for real-time/interactive use — still good quality at ~80ms per frame.
Speed 1-2 produce marginally smaller files but take 5-14x longer than speed 4.

### Quality parameter

The `quality` parameter maps to an AV1 quantizer index. The default is **75**
(high quality); the reference points below bracket the useful range:

| Quality | Use case | Typical compression |
|--------:|----------|:-------------------:|
| 30 | Thumbnails, previews | 100-120x |
| 50 | Web images (aggressive) | 40-45x |
| 65 | Web images (balanced) | 22-25x |
| 80 | High quality | 12-14x |
| 95 | Near-lossless | 5-6x |
| 100 | Lossless | 2-3x |

### Quantization matrices (QM)

With the `encode-imazen` feature, quantization matrices are enabled by default.
QM applies frequency-dependent quantization weights that save **5-12% file size**
at q≤50 and roughly break even on size at high quality. Quality impact is small:
within ±0.4 zensim of QM-off from q=70 onward, and ≤2 zensim points at low q
(measured across 5 CID22 test images at speed 6).

QM is gracefully disabled for lossless encoding (`with_lossless(true)`); at
quality=100 the underlying encoder also detects that all selected QM levels
collapse to identity and clears the QM signaling, producing output equivalent
to QM-off.

```rust,ignore
// QM is on by default. To disable:
let config = EncoderConfig::new()
    .quality(80.0)
    .with_qm(false);
```

### TX RDO (quality-priority, optional)

At speed ≥ 6 the encoder skips the per-block transform-type RDO search
and uses DCT-DCT only for intra blocks. Forcing the full search back on
is an opt-in quality knob:

```rust,ignore
let config = EncoderConfig::new()
    .quality(95.0)
    .with_qm(true)
    .with_rdo_tx_decision(Some(true)); // archival / one-shot encode
```

<!-- crates.io:skip-start -->
Measured trade on a 63-image stills corpus (CID22, speed 6, with QM on):

| Config | Mean BD-Rate vs upstream rav1e | Encode time |
|---|---|---|
| `with_qm(true)` (default) | −10.1 % | 1.0× |
| `with_qm(true).with_rdo_tx_decision(Some(true))` | −10.3 % | ~3.0× |
<!-- crates.io:skip-end -->

Mean gain over QM-only is small (~0.2 % BD-rate), but per-image gains
range up to −31 %. Recommended only for archival / one-shot encodes,
not bulk web pipelines.

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
| `zenav1-svt` | EXPERIMENTAL encoding via svtav1-rs (pure Rust SVT-AV1 port; 8- and 10-bit 4:2:0 stills incl. HDR CICP + clli/mdcv; dims arbitrary at any speed on the colour path, and with alpha/gray multiples of 64 below speed 5 else multiples of 8; 10-bit alpha/gray at speed ≥ 7; sibling path dep) |
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

## License

Dual-licensed: [AGPL-3.0](https://github.com/imazen/zenavif/blob/main/LICENSE-AGPL3) or [commercial](https://github.com/imazen/zenavif/blob/main/LICENSE-COMMERCIAL).

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

See [LICENSE-COMMERCIAL](https://github.com/imazen/zenavif/blob/main/LICENSE-COMMERCIAL) for details.

## AI-Generated Code Notice

Developed with AI assistance (Claude, Anthropic). Not all code manually reviewed — review critical paths before production use.

## Image tech I maintain

| | |
|:--|:--|
| **Codecs** ¹ | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · **zenavif** · [zenjxl] · [zenjxl-decoder] · [jxl-encoder] · [zenbitmaps] · [heic] · [zentiff] · [zenpdf] · [zensvg] · [zenjp2] · [zenraw] · [ultrahdr] |
| Codec internals | [zenrav1e] · [rav1d-safe] · [zenravif] · [zenavif-parse] · [zenavif-serialize] |
| Compression | [zenflate] · [zenzop] · [zenzstd] |
| Processing | [zenresize] · [zenquant] · [zenblend] · [zenfilters] · [zensally] · [zentone] |
| Pixels & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] · [zenyuv] |
| Pipeline & framework | [zenpipe] · [zencodec] · [zencodecs] · [zenlayout] · [zennode] · [zenwasm] · [zentract] |
| Metrics | [zensim] · [fast-ssim2] · [butteraugli] · [zenmetrics] · [resamplescope-rs] |
| Pickers & ML | [zenanalyze] · [zenpredict] · [zenpicker] · [zenanalyze-api] |
| Test corpora | [codec-corpus] · [imazen-26] |
| Products | [Imageflow] image engine ([.NET][imageflow-dotnet] · [Node][imageflow-node] · [Go][imageflow-go]) · [Imageflow Server] · [ImageResizer] (C#) |

<sub>¹ pure-Rust, `#![forbid(unsafe_code)]` codecs, as of 2026</sub>

### General Rust awesomeness

[zenbench] · [archmage] · [magetypes] · [enough] · [whereat] · [cargo-copter] · [zenutils]

[Open source](https://www.imazen.io/open-source) · [@imazen](https://github.com/imazen) · [@lilith](https://github.com/lilith) · [lib.rs/~lilith](https://lib.rs/~lilith)

[zenjpeg]: https://github.com/imazen/zenjpeg
[zenpng]: https://github.com/imazen/zenpng
[zenwebp]: https://github.com/imazen/zenwebp
[zengif]: https://github.com/imazen/zengif
[zenjxl]: https://github.com/imazen/zenjxl
[zenjxl-decoder]: https://github.com/imazen/zenjxl-decoder
[jxl-encoder]: https://github.com/imazen/jxl-encoder
[zenbitmaps]: https://github.com/imazen/zenbitmaps
[heic]: https://github.com/imazen/heic
[zentiff]: https://github.com/imazen/zenextras
[zenpdf]: https://github.com/imazen/zenextras
[zensvg]: https://github.com/imazen/zenextras
[zenjp2]: https://github.com/imazen/zenextras
[zenraw]: https://github.com/imazen/zenraw
[ultrahdr]: https://github.com/imazen/ultrahdr
[zenrav1e]: https://github.com/imazen/zenrav1e
[rav1d-safe]: https://github.com/imazen/rav1d-safe
[zenravif]: https://github.com/imazen/cavif-rs
[zenavif-parse]: https://github.com/imazen/zenavif
[zenavif-serialize]: https://github.com/imazen/zenavif
[zenflate]: https://github.com/imazen/zenflate
[zenzop]: https://github.com/imazen/zenzop
[zenzstd]: https://github.com/imazen/zenzstd
[zenresize]: https://github.com/imazen/zenresize
[zenquant]: https://github.com/imazen/zenquant
[zenblend]: https://github.com/imazen/zenblend
[zenfilters]: https://github.com/imazen/zenpipe
[zensally]: https://github.com/imazen/zensally
[zentone]: https://github.com/imazen/zentone
[zenpixels]: https://github.com/imazen/zenpixels
[zenpixels-convert]: https://github.com/imazen/zenpixels
[linear-srgb]: https://github.com/imazen/linear-srgb
[garb]: https://github.com/imazen/garb
[zenyuv]: https://github.com/imazen/zenjpeg
[zenpipe]: https://github.com/imazen/zenpipe
[zencodec]: https://github.com/imazen/zencodec
[zencodecs]: https://github.com/imazen/zenpipe
[zenlayout]: https://github.com/imazen/zenpipe
[zennode]: https://github.com/imazen/zennode
[zenwasm]: https://github.com/imazen/zenwasm
[zentract]: https://github.com/imazen/zentract
[zensim]: https://github.com/imazen/zensim
[fast-ssim2]: https://github.com/imazen/fast-ssim2
[butteraugli]: https://github.com/imazen/butteraugli
[zenmetrics]: https://github.com/imazen/zenmetrics
[resamplescope-rs]: https://github.com/imazen/resamplescope-rs
[zenanalyze]: https://github.com/imazen/zenanalyze
[zenpredict]: https://github.com/imazen/zenanalyze
[zenpicker]: https://github.com/imazen/zenanalyze
[zenanalyze-api]: https://github.com/imazen/zenanalyze
[codec-corpus]: https://github.com/imazen/codec-corpus
[imazen-26]: https://github.com/imazen/imazen-26
[zenbench]: https://github.com/imazen/zenbench
[archmage]: https://github.com/imazen/archmage
[magetypes]: https://github.com/imazen/archmage
[enough]: https://github.com/imazen/enough
[whereat]: https://github.com/lilith/whereat
[cargo-copter]: https://github.com/imazen/cargo-copter
[zenutils]: https://github.com/imazen/zenutils
[Imageflow]: https://github.com/imazen/imageflow
[Imageflow Server]: https://github.com/imazen/imageflow-dotnet-server
[ImageResizer]: https://github.com/imazen/resizer
[imageflow-dotnet]: https://github.com/imazen/imageflow-dotnet
[imageflow-node]: https://github.com/imazen/imageflow-node
[imageflow-go]: https://github.com/imazen/imageflow-go
