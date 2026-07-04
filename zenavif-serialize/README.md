# zenavif-serialize [![CI](https://img.shields.io/github/actions/workflow/status/imazen/zenavif-serialize/ci.yml?style=flat-square&label=CI)](https://github.com/imazen/zenavif-serialize/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/zenavif-serialize?style=flat-square)](https://crates.io/crates/zenavif-serialize) [![lib.rs](https://img.shields.io/crates/v/zenavif-serialize?style=flat-square&label=lib.rs&color=blue)](https://lib.rs/crates/zenavif-serialize) [![docs.rs](https://img.shields.io/docsrs/zenavif-serialize?style=flat-square)](https://docs.rs/zenavif-serialize) [![MSRV](https://img.shields.io/badge/MSRV-1.93-blue?style=flat-square)](https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field) [![license](https://img.shields.io/crates/l/zenavif-serialize?style=flat-square)](#license)

AVIF container serializer (muxer) in pure Rust. It wraps already-compressed AV1 bitstreams into MPEG/HEIF/MIAF/ISO-BMFF boxes for still images, animations, and grid layouts — it does **not** encode pixels itself. `#![forbid(unsafe_code)]`, and depends only on `arrayvec` and `whereat` (uses `std::io` for output).

Pair it with an AV1 encoder such as [zenrav1e](https://lib.rs/zenrav1e) for a pure-Rust AVIF encoding path.

## Quick start

```toml
[dependencies]
zenavif-serialize = "0.1.4"
```

Compress your pixels with an AV1 encoder first, then wrap the single-keyframe bitstream in one call:

```rust
// `color_av1` is a raw AV1 OBU bitstream for ONE keyframe, sequence header in-band.
let avif_bytes = zenavif_serialize::serialize_to_vec(
    &color_av1,       // AV1 bitstream
    None,             // alpha plane (optional; separately-encoded monochrome AV1)
    width, height, 8, // dimensions and bit depth (8, 10, or 12)
);
```

That's the whole job for a plain still image. The sections below add color
signaling, transforms, metadata, animation, and grids.

## Features

- **Still images** with optional alpha channel (separate monochrome AV1 plane)
- **Animated AVIF** with per-frame durations and keyframe control
- **Grid/tiled images** (up to 256x256 tiles) for large images
- **HDR metadata** — content light level (clli) and mastering display color volume (mdcv)
- **Transforms** — rotation, mirror, clean aperture crop, pixel aspect ratio
- **Color spaces** — full CICP support (BT.709, BT.2020, Display P3, PQ, HLG, etc.)
- **ICC profiles**, EXIF, and XMP metadata embedding
- **8/10/12-bit** depth
- **Pure safe Rust** — `#![forbid(unsafe_code)]`; uses `std::io` (not `no_std`)

## Configuring color, transforms & metadata

```rust
use zenavif_serialize::{Aviffy, constants::{ColorPrimaries, TransferCharacteristics}};

let avif_bytes = Aviffy::new()
    .set_color_primaries(ColorPrimaries::Bt2020)
    .set_transfer_characteristics(TransferCharacteristics::Smpte2084)
    .set_content_light_level(1000, 400) // MaxCLL, MaxFALL in cd/m² (nits)
    .set_rotation(1) // 90 degrees CCW
    .to_vec(&color_av1, alpha_av1.as_deref(), width, height, 10);
```

> **Server note:** `to_vec` and `serialize_to_vec` return a `Vec<u8>` directly
> and **panic** if box construction fails — most easily when `depth_bits` is not
> `8`, `10`, or `12`. On a request path that handles untrusted dimensions/depth,
> prefer `Aviffy::write`, which returns the crate's
> `Result<(), whereat::At<SerializeError>>` and surfaces the failure
> (e.g. `SerializeError::InvalidInput`) — with a `file:line` source location for
> structured logs — instead of panicking:
>
> ```rust
> let mut out = Vec::new();
> Aviffy::new().write(&mut out, &color_av1, None, width, height, depth_bits)?;
> // On error: `e.error()` -> &SerializeError, `e.location()` -> the file:line.
> ```

## AV1 input contract

For a muxer the byte-level input contract is the whole job, so it's worth stating
exactly what `color_av1_data` must be. These are traced from the source, not
assumed:

- **Pass the raw AV1 OBU bitstream for a single keyframe, with the sequence
  header OBU in-band.** The serializer copies the bytes verbatim into the image
  data extent with no parsing or validation (`IlocExtent { data: color_av1_data }`,
  `src/lib.rs:598`), and the still path stores no separate sequence header — the
  `av1C` box it writes is the 4-byte config record only, no `configOBUs`
  (`src/boxes.rs`, length `BASIC_BOX_SIZE + 4`). Decoders read the sequence
  header from the payload, so it must be present there.
- **No external framing** — no length prefix, no Annex-B start codes. Bytes go in
  exactly as the AV1 encoder emitted them for one frame.
- **Dimensions and `depth_bits` are passed as arguments, not parsed** (→ `ispe`
  and `pixi`), and must match how the bitstream was encoded. `depth_bits` must be
  `8`, `10`, or `12` (`src/lib.rs:536`).
- **The `av1C` config record is built from the builder settings, not extracted
  from the bitstream** — `set_monochrome` / `set_chroma_subsampling` /
  `set_seq_profile` plus the depth determine its fields. Strict decoders (Chrome)
  validate `av1C` against the in-band sequence header, so those settings must
  agree with how the frame was encoded.
- **Optional `alpha_av1_data`** is a separately-encoded **monochrome** AV1
  bitstream for the alpha plane (`rav1e`'s `Cs400` / "YUV400"), under the same
  raw-OBU contract.

The animation and grid builders differ deliberately: they take the sequence
header separately (`AnimatedImage::serialize(..., color_seq_header, ...)`,
`set_color_config`) rather than in-band, which is why only the still path expects
it inside the payload.

### Color signaling

When neither an ICC profile nor any CICP color field is set, the `colr` box is
**omitted entirely** (`src/lib.rs`: `colr` is written only when
`self.colr != ColrBox::default()`) and decoders fall back to the in-band sequence
header's signaling. Setting any of `set_color_primaries` /
`set_transfer_characteristics` / `set_matrix_coefficients` / `set_full_color_range`
writes an nclx `colr`; `set_icc_profile` writes a `prof` `colr` and takes
precedence over nclx (the two are mutually exclusive per spec).

`set_content_light_level(max_content_light_level, max_pic_average_light_level)`
takes **MaxCLL then MaxFALL, both in cd/m² (nits)** — not the raw `clli`-box
integers (`src/lib.rs:343`).

### Signature reference

```rust
// Free function — infallible Vec path (panics if box construction fails):
pub fn serialize_to_vec(
    color_av1_data: &[u8],
    alpha_av1_data: Option<&[u8]>,
    width: u32,
    height: u32,
    depth_bits: u8,
) -> Vec<u8>;

// Builder — same parameters, after configuring color/transforms/metadata:
Aviffy::new()/* ...setters... */.to_vec(
    color_av1_data: &[u8],
    alpha_av1_data: Option<&[u8]>,
    width: u32,
    height: u32,
    depth_bits: u8,
) -> Vec<u8>;

// Fallible streaming path — same parameters, writes into any io::Write:
pub fn serialize<W: std::io::Write>(
    into_output: W,
    color_av1_data: &[u8],
    alpha_av1_data: Option<&[u8]>,
    width: u32,
    height: u32,
    depth_bits: u8,
) -> zenavif_serialize::Result<()>; // = Result<(), whereat::At<SerializeError>>
// `Aviffy::write` has the identical signature after configuration.
```

The `Vec` paths (`serialize_to_vec`, `Aviffy::to_vec`) panic if box construction
fails. On a request path with untrusted dimensions/depth, use the fallible
`serialize` / `Aviffy::write`, which return a `SerializeError`
(`InvalidInput` / `Io` / `Oom`) carrying a `file:line` location instead.

### Animation

```rust
use zenavif_serialize::{Av1CBox, animated::{AnimatedImage, AnimFrame}};

let mut anim = AnimatedImage::new();
anim.set_timescale(1000);       // ticks per second; durations below are in ticks
anim.set_color_config(av1c);    // av1c: Av1CBox describing the color track

let frames = vec![
    AnimFrame::new(&frame0_av1, 33).with_sync(true), // sync = keyframe
    AnimFrame::new(&frame1_av1, 33),
    AnimFrame::new(&frame2_av1, 33),
];

// seq_header: &[u8] — the AV1 sequence header OBU, passed separately here.
let avif_bytes = anim.serialize(width, height, &frames, &seq_header, None);
```

### Grid (tiled)

```rust
use zenavif_serialize::{Av1CBox, grid::GridImage};

let mut grid = GridImage::new();
grid.set_color_config(av1c);    // av1c: Av1CBox describing the tiles

let avif_bytes = grid.serialize(
    2, 2,           // rows x columns
    2048, 2048,     // output dimensions
    1024, 1024,     // tile dimensions
    &[&tile0, &tile1, &tile2, &tile3],
    None,           // alpha tiles (optional)
)?;                 // GridImage::serialize returns io::Result<Vec<u8>>
```

## Compatibility

Output is tested against three independent AVIF parsers: [avif-parse](https://lib.rs/avif-parse), [zenavif-parse](https://lib.rs/zenavif-parse), and [mp4parse](https://lib.rs/mp4parse) (Mozilla). Browser compatibility has not been independently verified.

## Fork of avif-serialize

Forked from [avif-serialize](https://lib.rs/avif-serialize) v0.8.8 by Kornel Lesiński, rebased on upstream as of 2026-02-14.

Changes from upstream:

- **Animation** — `AnimatedImage` builder with per-frame durations, keyframe control, alpha track (`animated.rs`)
- **Grid/tiled images** — `GridImage` builder for tile-based encoding up to 256x256 (`grid.rs`)
- **Transforms** — rotation (irot), mirror (imir), clean aperture crop (clap), pixel aspect ratio (pasp)
- **Metadata** — ICC profile, EXIF, and XMP embedding as separate items with item references
- **Builder API** — `Aviffy` builder with `#[non_exhaustive]` types for forward compatibility

Original still-image serialization code is largely unchanged.

## License

BSD-3-Clause. Original code copyright Cloudflare, Inc. Fork additions copyright Imazen LLC.

This is a fork of [kornelski/avif-serialize](https://github.com/kornelski/avif-serialize) (BSD-3-Clause). We're happy to release these improvements under the original BSD-3-Clause license if upstream wants to take over their maintenance — we'd rather contribute back than maintain a parallel codebase. Open an issue or reach out.

## Image tech I maintain

| | |
|:--|:--|
| **Codecs** ¹ | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] · [zenjxl] · [zenbitmaps] · [heic] · [zentiff] · [zenpdf] · [zensvg] · [zenjp2] · [zenraw] · [ultrahdr] |
| Codec internals | [zenjxl-decoder] · [jxl-encoder] · [zenrav1e] · [rav1d-safe] · [zenavif-parse] · **zenavif-serialize** |
| Compression | [zenflate] · [zenzop] · [zenzstd] |
| Processing | [zenresize] · [zenquant] · [zenblend] · [zenfilters] · [zensally] · [zentone] |
| Pixels & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] |
| Pipeline & framework | [zenpipe] · [zencodec] · [zencodecs] · [zenlayout] · [zennode] · [zenwasm] · [zentract] |
| Metrics | [zensim] · [fast-ssim2] · [butteraugli] · [zenmetrics] · [resamplescope-rs] |
| Pickers & ML | [zenanalyze] · [zenpredict] · [zenpicker] |
| Products | [Imageflow] image engine ([.NET][imageflow-dotnet] · [Node][imageflow-node] · [Go][imageflow-go]) · [Imageflow Server] · [ImageResizer] (C#) |

<sub>¹ pure-Rust, `#![forbid(unsafe_code)]` codecs, as of 2026</sub>

### General Rust awesomeness

[zenbench] · [archmage] · [magetypes] · [enough] · [whereat] · [cargo-copter]

[Open source](https://www.imazen.io/open-source) · [@imazen](https://github.com/imazen) · [@lilith](https://github.com/lilith) · [lib.rs/~lilith](https://lib.rs/~lilith)

[zenjpeg]: https://github.com/imazen/zenjpeg
[zenpng]: https://github.com/imazen/zenpng
[zenwebp]: https://github.com/imazen/zenwebp
[zengif]: https://github.com/imazen/zengif
[zenavif]: https://github.com/imazen/zenavif
[zenjxl]: https://github.com/imazen/zenjxl
[zenbitmaps]: https://github.com/imazen/zenbitmaps
[heic]: https://github.com/imazen/heic
[zentiff]: https://github.com/imazen/zentiff
[zenpdf]: https://github.com/imazen/zenpdf
[zensvg]: https://github.com/imazen/zenextras
[zenjp2]: https://github.com/imazen/zenextras
[zenraw]: https://github.com/imazen/zenraw
[ultrahdr]: https://github.com/imazen/ultrahdr
[zenjxl-decoder]: https://github.com/imazen/zenjxl-decoder
[jxl-encoder]: https://github.com/imazen/jxl-encoder
[zenrav1e]: https://github.com/imazen/zenrav1e
[rav1d-safe]: https://github.com/imazen/rav1d-safe
[zenavif-parse]: https://github.com/imazen/zenavif-parse
[zenflate]: https://github.com/imazen/zenflate
[zenzop]: https://github.com/imazen/zenzop
[zenzstd]: https://github.com/imazen/zenzstd
[zenresize]: https://github.com/imazen/zenresize
[zenquant]: https://github.com/imazen/zenquant
[zenblend]: https://github.com/imazen/zenblend
[zenfilters]: https://github.com/imazen/zenfilters
[zensally]: https://github.com/imazen/zensally
[zentone]: https://github.com/imazen/zentone
[zenpixels]: https://github.com/imazen/zenpixels
[zenpixels-convert]: https://github.com/imazen/zenpixels
[linear-srgb]: https://github.com/imazen/linear-srgb
[garb]: https://github.com/imazen/garb
[zenpipe]: https://github.com/imazen/zenpipe
[zencodec]: https://github.com/imazen/zencodec
[zencodecs]: https://github.com/imazen/zencodecs
[zenlayout]: https://github.com/imazen/zenlayout
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
[zenbench]: https://github.com/imazen/zenbench
[archmage]: https://github.com/imazen/archmage
[magetypes]: https://github.com/imazen/archmage
[enough]: https://github.com/imazen/enough
[whereat]: https://github.com/lilith/whereat
[cargo-copter]: https://github.com/imazen/cargo-copter
[Imageflow]: https://github.com/imazen/imageflow
[Imageflow Server]: https://github.com/imazen/imageflow-dotnet-server
[ImageResizer]: https://github.com/imazen/resizer
[imageflow-dotnet]: https://github.com/imazen/imageflow-dotnet
[imageflow-node]: https://github.com/imazen/imageflow-node
[imageflow-go]: https://github.com/imazen/imageflow-go
