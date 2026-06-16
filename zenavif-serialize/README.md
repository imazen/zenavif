# zenavif-serialize [![CI](https://img.shields.io/github/actions/workflow/status/imazen/zenavif-serialize/ci.yml?branch=main&style=flat-square)](https://github.com/imazen/zenavif-serialize/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/zenavif-serialize?style=flat-square)](https://crates.io/crates/zenavif-serialize) [![lib.rs](https://img.shields.io/badge/lib.rs-zenavif--serialize-blue?style=flat-square)](https://lib.rs/crates/zenavif-serialize) [![docs.rs](https://img.shields.io/docsrs/zenavif-serialize?style=flat-square)](https://docs.rs/zenavif-serialize) [![license](https://img.shields.io/crates/l/zenavif-serialize?style=flat-square)](https://github.com/imazen/zenavif-serialize#license)

AVIF container serializer (muxer) in pure Rust. Creates MPEG/HEIF/MIAF/ISO-BMFF boxes for still images, animations, and grid layouts. Only dependency is `arrayvec`.

Together with [zenrav1e](https://lib.rs/zenrav1e), it enables pure-Rust AVIF encoding.

## Fork of avif-serialize

Forked from [avif-serialize](https://lib.rs/avif-serialize) v0.8.8 by Kornel Lesinski. Rebased on upstream as of 2026-02-14.

Changes from upstream (+2,188 lines, -55 lines):

- **Animation** — `AnimatedImage` builder with per-frame durations, keyframe control, alpha track (`animated.rs`, 795 lines)
- **Grid/tiled images** — `GridImage` builder for tile-based encoding up to 256x256 (`grid.rs`, 695 lines)
- **Transforms** — rotation (irot), mirror (imir), clean aperture crop (clap), pixel aspect ratio (pasp)
- **Metadata** — ICC profile, EXIF, and XMP embedding as separate items with item references
- **Builder API** — `Aviffy` builder with `#[non_exhaustive]` types for forward compatibility

Original still-image serialization code is largely unchanged.

## Features

- **Still images** with optional alpha channel (separate monochrome AV1 plane)
- **Animated AVIF** with per-frame durations and keyframe control
- **Grid/tiled images** (up to 256x256 tiles) for large images
- **HDR metadata** — content light level (clli) and mastering display color volume (mdcv)
- **Transforms** — rotation, mirror, clean aperture crop, pixel aspect ratio
- **Color spaces** — full CICP support (BT.709, BT.2020, Display P3, PQ, HLG, etc.)
- **ICC profiles**, EXIF, and XMP metadata embedding
- **8/10/12-bit** depth
- **`no_std` compatible** (with `alloc`)

## Usage

Add to `Cargo.toml`:

```toml
[dependencies]
zenavif-serialize = "0.1"
```

### Still image (minimal)

Compress your pixels with an AV1 encoder first, then wrap the bitstream:

```rust
let avif_bytes = zenavif_serialize::serialize_to_vec(
    &color_av1_data,  // AV1 bitstream
    None,             // alpha (optional)
    width, height, 8, // dimensions and bit depth
);
```

### Still image (configured)

```rust
use zenavif_serialize::{Aviffy, ColorPrimaries, TransferCharacteristics};

let avif_bytes = Aviffy::new()
    .set_color_primaries(ColorPrimaries::Bt2020)
    .set_transfer_characteristics(TransferCharacteristics::Smpte2084)
    .set_content_light_level(1000, 400)
    .set_rotation(1) // 90 degrees CCW
    .to_vec(&color_av1, alpha_av1.as_deref(), width, height, 10);
```

> **Server note:** `to_vec` and `serialize_to_vec` return a `Vec<u8>` directly
> and **panic** if box construction fails — most easily when `depth_bits` is not
> `8`, `10`, or `12`. On a request path that handles untrusted dimensions/depth,
> prefer `Aviffy::write`, which returns `io::Result<()>` and surfaces the failure
> (e.g. `io::ErrorKind::InvalidInput`) instead of panicking:
>
> ```rust
> let mut out = Vec::new();
> Aviffy::new().write(&mut out, &color_av1, None, width, height, depth_bits)?;
> ```

### Animation

```rust
use zenavif_serialize::{AnimatedImage, AnimFrame};

let mut anim = AnimatedImage::new();
anim.set_timescale(1000); // milliseconds
anim.set_color_config(av1c);

let frames = vec![
    AnimFrame::new(&frame0_av1, 33).with_sync(true),
    AnimFrame::new(&frame1_av1, 33),
    AnimFrame::new(&frame2_av1, 33),
];

let avif_bytes = anim.serialize(width, height, &frames, &seq_header, None);
```

### Grid (tiled)

```rust
use zenavif_serialize::GridImage;

let mut grid = GridImage::new();
grid.set_color_config(av1c);

let avif_bytes = grid.serialize(
    2, 2,           // rows x columns
    2048, 2048,     // output dimensions
    1024, 1024,     // tile dimensions
    &[&tile0, &tile1, &tile2, &tile3],
    None,           // alpha tiles (optional)
)?;
```

## Compatibility

Output is tested against three independent AVIF parsers: [avif-parse](https://lib.rs/avif-parse), [zenavif-parse](https://lib.rs/zenavif-parse), and [mp4parse](https://lib.rs/mp4parse) (Mozilla). Browser compatibility has not been independently verified.

## Image tech I maintain

| | |
|:--|:--|
| State of the art codecs* | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] ([rav1d-safe] · [zenrav1e] · [zenavif-parse] · **zenavif-serialize**) · [zenjxl] ([jxl-encoder] · [zenjxl-decoder]) · [zentiff] · [zenbitmaps] · [heic] · [zenraw] · [zenpdf] · [ultrahdr] · [mozjpeg-rs] · [webpx] |
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

BSD-3-Clause. Original code copyright Cloudflare, Inc. Fork additions copyright Imazen LLC.



### Upstream Contribution

This is a fork of [kornelski/avif-serialize](https://github.com/kornelski/avif-serialize) (BSD-3-Clause).
We are willing to release our improvements under the original BSD-3-Clause
license if upstream takes over maintenance of those improvements. We'd rather
contribute back than maintain a parallel codebase. Open an issue or reach out.

[zenjpeg]: https://github.com/imazen/zenjpeg
[zenpng]: https://github.com/imazen/zenpng
[zenwebp]: https://github.com/imazen/zenwebp
[zengif]: https://github.com/imazen/zengif
[zenavif]: https://github.com/imazen/zenavif
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
