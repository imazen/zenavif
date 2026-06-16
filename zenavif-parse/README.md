# zenavif-parse [![CI](https://img.shields.io/github/actions/workflow/status/imazen/zenavif-parse/ci.yml?style=flat-square)](https://github.com/imazen/zenavif-parse/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/zenavif-parse?style=flat-square)](https://crates.io/crates/zenavif-parse) [![lib.rs](https://img.shields.io/badge/lib.rs-zenavif--parse-blue?style=flat-square)](https://lib.rs/crates/zenavif-parse) [![docs.rs](https://img.shields.io/docsrs/zenavif-parse?style=flat-square)](https://docs.rs/zenavif-parse) [![license](https://img.shields.io/crates/l/zenavif-parse?style=flat-square)](https://github.com/imazen/zenavif-parse#license)

AVIF container parser (ISOBMFF/MIAF demuxer) that extracts AV1 payloads, alpha channels, grid tiles, and animation frames from AVIF files. Written entirely in safe Rust with fallible allocations throughout.

This is a fork of [kornelski/avif-parse](https://github.com/kornelski/avif-parse), which itself descends from Mozilla's MP4 parser used in Firefox. The upstream crate is battle-tested against untrusted data; this fork adds animation, grid images, and a zero-copy API for our specific use case.

## What this fork adds

**New zero-copy API** — `AvifParser` parses box structure and records byte offsets without copying mdat content. Data access returns `Cow<[u8]>` — borrowed for single-extent items (the common case), owned only when extents must be concatenated.

**Grid image support** — Parses `iref` dimg references and `ImageGrid` property boxes to identify grid tiles. Falls back to calculating grid layout from `ispe` (Image Spatial Extents) properties when no explicit grid box exists.

**Animated AVIF** — Parses `moov`/`trak` boxes, `stts` timing, `stco`/`co64` chunk offsets, and sample tables. Frames are accessed on-demand via iterator or index.

**Resource limits** — `DecodeConfig` caps peak memory, megapixels, animation frame count, and grid tile count during parsing. Limits are checked before allocations, not after.

**Cooperative cancellation** — All parsing paths accept an `enough::Stop` token for cancellation.

**Parsing fixes** — Size-0 box support (ISOBMFF "extends to EOF"), `idat` construction method support (`iloc construction_method=1`), correct `construction_method` handling (upstream guessed based on offset value).

The original `read_avif()` / `AvifData` API is preserved for backwards compatibility, but `AvifParser` is preferred for new code.

## Usage

### Zero-copy parser (recommended)

```rust
use zenavif_parse::AvifParser;

let bytes = std::fs::read("image.avif")?;
let parser = AvifParser::from_bytes(&bytes)?;

// Primary item — zero-copy for single-extent items
let primary = parser.primary_data()?;
decode_av1(&primary)?;

// Alpha channel
if let Some(alpha) = parser.alpha_data() {
    decode_av1(&alpha?)?;
}

if parser.premultiplied_alpha() {
    // divide RGB by A after decoding
}
```

Three constructors, each with a `_with_config` variant for resource limits:

- `from_bytes(&[u8])` — borrows the input; data access is zero-copy
- `from_owned(Vec<u8>)` — takes ownership; returns `AvifParser<'static>`
- `from_reader(impl Read)` — reads into an owned buffer; returns `AvifParser<'static>`

#### What `primary_data()` returns (the byte contract)

`primary_data()` returns `Result<Cow<[u8]>>` — the **raw item-payload bytes** sliced
directly out of the file's `mdat` (or `idat`) box, exactly as stored. No bytes are
synthesized, reordered, or stripped.

- **It is the complete AV1 OBU temporal unit for the primary image, decoder-ready
  as-is.** In a still AVIF the primary item's `mdat` extent holds the full coded OBU
  stream *including the sequence-header OBU* — so you can hand the slice straight to an
  AV1 decoder. (`primary_metadata()` proves this: it parses the sequence header out of
  `primary_data()` directly, with nothing prepended.)
- **The `av1C` config box is NOT prepended.** This crate parses `av1C` only for its
  profile / level / bit-depth / chroma fields (exposed via `av1_config()`); its
  `configOBUs` payload is skipped, because for AVIF the sequence header already lives
  inline in the item data. You do **not** need to reconstruct or prepend anything.
- `Cow::Borrowed` for the common single-extent case (zero copy, borrowed from the input
  buffer); `Cow::Owned` only when an item spans multiple `iloc` extents and must be
  concatenated.

**Grid and animation primaries are the exception — `primary_data()` is not AV1 there:**

- **Grid** (primary item type `grid`): `primary_data()` returns the *grid derivation
  item's* own tiny extent (the `ImageGrid` header bytes), **not** decodable AV1, and
  `primary_metadata()` will error trying to parse it. Detect this with
  `grid_config().is_some()` and decode the tiles instead — see [Grid images](#grid-images)
  below (`grid_tile_count()` + `tile_data(i)`).
- **Animation cover still** (image sequence carrying a `meta` box): `primary_data()`
  returns the still cover image's OBU stream (decoder-ready, as above), independent of
  the animation tracks.
- **Pure image sequence** (no `meta` box, `avis` only): there is no primary still — the
  primary extent is empty and `primary_data()` returns an **empty** `Cow`. Use
  `animation_info()` + `frames()` / `frame(i)` instead.

### Grid images

```rust
if let Some(grid) = parser.grid_config() {
    println!("{}x{} tiles, output {}x{}",
        grid.columns, grid.rows,
        grid.output_width, grid.output_height);
    for i in 0..parser.grid_tile_count() {
        let tile = parser.tile_data(i)?;
        decode_av1(&tile)?;
    }
}
```

### Animated AVIF

```rust
if let Some(info) = parser.animation_info() {
    for frame in parser.frames() {
        let frame = frame?;
        decode_av1(&frame.data)?;
        // display for frame.duration_ms milliseconds
    }
}
```

### AV1 metadata without decoding

`primary_metadata()` returns a `Result<AV1Metadata>` parsed from the primary item's AV1
sequence header (and first frame header) — no AV1 decode:

```rust
let meta = parser.primary_metadata()?;   // -> zenavif_parse::AV1Metadata
println!("{}x{}, {}bpc, chroma {:?}",
    meta.max_frame_width, meta.max_frame_height,
    meta.bit_depth, meta.chroma_subsampling);
// also: meta.seq_profile, meta.monochrome, meta.base_q_idx, meta.lossless
```

### Color / CICP for correct delivery

For color-correct output you need the authoritative color signaling from the **container**
`colr` box (CICP/nclx) or the embedded **ICC** profile — *not* the values inside the AV1
bitstream. AVIF places the canonical color description in the container, and an
`nclx` `colr` overrides anything in the sequence header. Read it with `color_info()`,
which returns `Option<&ColorInformation>`:

```rust
use zenavif_parse::ColorInformation;

match parser.color_info() {
    Some(ColorInformation::Nclx {
        color_primaries,           // u16 — CICP colour primaries (H.273 Table 2; e.g. 1 = BT.709, 9 = BT.2020, 12 = P3-D65)
        transfer_characteristics,  // u16 — CICP transfer (H.273 Table 3; e.g. 13 = sRGB, 16 = PQ, 18 = HLG)
        matrix_coefficients,       // u16 — CICP matrix (H.273 Table 4; e.g. 0 = identity/RGB, 1 = BT.709, 9 = BT.2020-NCL)
        full_range,                // bool — true = full range, false = limited/studio range
    }) => {
        // Build your color transform from CICP (e.g. feed moxcms / your CMS).
        let _ = (color_primaries, transfer_characteristics, matrix_coefficients, full_range);
    }
    Some(ColorInformation::IccProfile(icc)) => {
        // Embedded ICC profile bytes (rICC/prof colour_type) — feed to your CMS.
        let _ = icc; // &[u8]
    }
    None => {
        // No colr box. Per AVIF/MIAF, assume the AV1 sequence header's CICP
        // (available via decode), or fall back to BT.709 / limited range.
    }
}
```

`ColorInformation` is the single type for both cases — a CICP/`nclx` variant with the four
integer/flag fields, or an `IccProfile(Vec<u8>)` variant. (Field name is
`color_primaries`, US spelling.) An AVIF carries **one** of the two, never both, so a
single `match` covers it.

HDR / wide-gamut delivery often also needs the static metadata boxes, each exposed as its
own accessor returning a borrowed `Option`:

- `mastering_display()` → `Option<&MasteringDisplayColourVolume>` (`mdcv`)
- `content_light_level()` → `Option<&ContentLightLevel>` (`clli`, MaxCLL / MaxFALL)
- `content_colour_volume()` → `Option<&ContentColourVolume>` (`cclv`)
- `ambient_viewing()` → `Option<&AmbientViewingEnvironment>` (`amve`)

For Ultra HDR / gain-map workflows, `gain_map_color_info()` returns the gain map item's own
`ColorInformation`, and `gain_map_metadata()` / `gain_map()` expose the tone-mapping
parameters.

### Error and metadata types

- **Result / error type.** Public methods return `zenavif_parse::Result<T>`, which is
  `Result<T, whereat::At<zenavif_parse::Error>>` — the error is wrapped in
  [`whereat::At`](https://docs.rs/whereat) to carry source-location frames. Both
  `zenavif_parse::Error` and `whereat::At<Error>` implement `std::error::Error`, so `?`
  propagates cleanly into `Box<dyn std::error::Error>` / `anyhow::Error` (as the examples
  here do). To inspect the inner enum, use `at.error()` (borrow) or `at.decompose()`
  (consume). `Error` variants include `InvalidData`, `Unsupported`, `UnexpectedEOF`,
  `Io`, `NoMoov`, `OutOfMemory`, `ResourceLimitExceeded`, and `Stopped` (cancellation).
- **Color type.** `color_info()` / `gain_map_color_info()` return
  `Option<&zenavif_parse::ColorInformation>` (enum: `Nclx { .. }` or `IccProfile(Vec<u8>)`).
- **Metadata type.** `primary_metadata()` / `alpha_metadata()` return
  `zenavif_parse::AV1Metadata` (a `#[non_exhaustive]` struct: `still_picture`,
  `max_frame_width`, `max_frame_height`, `bit_depth`, `seq_profile`,
  `chroma_subsampling`, `monochrome`, `base_q_idx`, `lossless`).

### Resource limits

```rust
use zenavif_parse::{AvifParser, DecodeConfig};

let config = DecodeConfig::default()
    .with_peak_memory_limit(64 * 1024 * 1024)   // 64MB
    .with_total_megapixels_limit(128)
    .with_max_animation_frames(500)
    .with_max_grid_tiles(64);

let parser = AvifParser::from_bytes_with_config(
    &bytes, &config, &enough::Unstoppable
)?;
```

Defaults: 1GB peak memory, 512MP total, 10k frames, 1k tiles. Use `DecodeConfig::unlimited()` to disable all limits. The plain `from_bytes` / `from_owned` constructors are **not** unlimited — they apply these same defaults — but for untrusted uploads pass your own tighter `DecodeConfig` via the `_with_config` form.

`&enough::Unstoppable` above is the no-op token. For real cancellation (a request deadline or shutdown), pass any [`enough::Stop`](https://docs.rs/enough); the simplest constructible token is `almost_enough::Stopper` (`cargo add almost-enough`) — `Clone`, with all clones sharing one flag:

```rust
let stopper = almost_enough::Stopper::new();
let watch = stopper.clone();   // hand a clone to a watchdog/deadline thread
// std::thread::spawn(move || { /* on deadline */ watch.cancel(); });
let parser = AvifParser::from_bytes_with_config(&bytes, &config, &stopper)?;
// once cancelled, parsing returns Err(Error::Stopped(..)).
```

### zencodec integration (feature = "zencodec")

The `zencodec` feature enables bidirectional `From` conversions between
`GainMapMetadata` / `GainMapChannel` and their `zencodec::GainMapParams` /
`zencodec::GainMapChannel` counterparts:

```toml
[dependencies]
zenavif-parse = { version = "0.6", features = ["zencodec"] }
zencodec = "0.1"
```

```rust
// zenavif-parse rationals → zencodec f64 domain
let params: zencodec::GainMapParams = zencodec::GainMapParams::from(&metadata);

// zencodec f64 domain → zenavif-parse rationals (continued-fraction encoding)
let metadata: zenavif_parse::GainMapMetadata = zenavif_parse::GainMapMetadata::from(&params);
```

Rational fractions are encoded using the continued-fraction algorithm, matching
libultrahdr's canonical form.

### Legacy API (feature = "eager")

The original `read_avif()` / `AvifData` API and C FFI are behind the `eager` feature flag, off by default.

```toml
[dependencies]
zenavif-parse = { version = "0.6", features = ["eager"] }
```

```rust
use zenavif_parse::read_avif;

let data = read_avif(&mut reader)?;
decode_av1(&data.primary_item)?;
```

## Upstream contributions welcome

All code in this fork is available under the same MPL-2.0 license as the original. The upstream maintainers are welcome to incorporate any or all changes — no attribution to this fork required. If specific features would be useful upstream in a different form, open an issue and we can restructure to fit.

Any copyright claims to changes in this fork are released to and under the upstream license. If changes in this fork are desired upstream by the upstream maintainers, please open an issue requesting a PR.

## Credits

This crate builds directly on work by:

- [Kornel Lesinski](https://github.com/kornelski) — created and maintains [avif-parse](https://github.com/kornelski/avif-parse)
- Mozilla — the original `mp4parse` crate that avif-parse forked from, used in Firefox
- Ralph Giles, Matthew Gregan, Alfredo Yang, Jon Bauman — original mp4parse authors

## Image tech I maintain

| | |
|:--|:--|
| State of the art codecs* | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] ([rav1d-safe] · [zenrav1e] · **zenavif-parse** · [zenavif-serialize]) · [zenjxl] ([jxl-encoder] · [zenjxl-decoder]) · [zentiff] · [zenbitmaps] · [heic] · [zenraw] · [zenpdf] · [ultrahdr] · [mozjpeg-rs] · [webpx] |
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

MPL-2.0 (unchanged from upstream).



### Upstream Contribution

This is a fork of [kornelski/avif-parse](https://github.com/kornelski/avif-parse) (MPL-2.0).
We are willing to release our improvements under the original MPL-2.0
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
