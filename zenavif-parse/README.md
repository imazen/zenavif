# zenavif-parse

[![CI](https://github.com/imazen/zenavif-parse/actions/workflows/ci.yml/badge.svg)](https://github.com/imazen/zenavif-parse/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/zenavif-parse.svg)](https://crates.io/crates/zenavif-parse)
[![docs.rs](https://docs.rs/zenavif-parse/badge.svg)](https://docs.rs/zenavif-parse)
[![MSRV](https://img.shields.io/badge/MSRV-1.92-blue.svg)](https://blog.rust-lang.org/2025/05/15/Rust-1.92.0.html)
[![license](https://img.shields.io/crates/l/zenavif-parse.svg)](LICENSE)

AVIF container parser (ISOBMFF/MIAF demuxer) that extracts AV1 payloads, alpha channels, grid tiles, and animation frames from AVIF files. Written entirely in safe Rust with fallible allocations throughout.

This is a fork of [kornelski/avif-parse](https://github.com/kornelski/avif-parse), which itself descends from Mozilla's MP4 parser used in Firefox. The upstream crate is battle-tested against untrusted data; this fork extends it with the features needed for a complete AVIF decoder.

## What changed from avif-parse

The upstream `avif-parse` handles single still images well. This fork adds everything else an AVIF decoder needs:

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

```rust
let meta = parser.primary_metadata()?;
println!("{}x{}, {}bpc, chroma {:?}",
    meta.max_frame_width, meta.max_frame_height,
    meta.bit_depth, meta.chroma_subsampling);
```

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

Defaults: 1GB peak memory, 512MP total, 10k frames, 1k tiles. Use `DecodeConfig::unlimited()` to disable all limits.

### Legacy API (feature = "eager")

The original `read_avif()` / `AvifData` API and C FFI are behind the `eager` feature flag, off by default.

```toml
[dependencies]
zenavif-parse = { version = "0.3", features = ["eager"] }
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

## License

MPL-2.0 (unchanged from upstream).

This crate doesn't include an AV1 decoder. For full AVIF decoding, see [zenavif](https://github.com/imazen/zenavif) which pairs this parser with [rav1d](https://github.com/memorysafety/rav1d).
