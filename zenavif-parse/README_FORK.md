# zenavif-parse Fork

This fork extends [kornelski/avif-parse](https://github.com/kornelski/avif-parse) with full AVIF 1.2 feature support for the [zenavif](https://github.com/imazen/zenavif) decoder.

Upstream merge base: v2.0.0. All upstream tests pass. All files in the AOM test suite and link-u animated samples parse correctly.

## Two Parsing APIs

### Eager: `read_avif` (deprecated)

**Deprecated.** `AvifParser` is a strict superset — use it instead.

Reads the entire file and copies all mdat data into memory. Requires the `eager` feature flag.

```rust
use zenavif_parse::read_avif;
use std::io::BufReader;
use std::fs::File;

let mut f = BufReader::new(File::open("image.avif")?);
let data = read_avif(&mut f)?;
av1_decode(&data.primary_item)?;
```

### Zero-Copy: `AvifParser`

Parses metadata without copying pixel data. Returns `Cow<[u8]>` — borrowed for single-extent items, owned (concatenated) for multi-extent items.

```rust
use zenavif_parse::{AvifParser, DecodeConfig};
use enough::Unstoppable;

let bytes = std::fs::read("image.avif")?;
let config = DecodeConfig::default();
let parser = AvifParser::from_bytes_with_config(&bytes, &config, &Unstoppable)?;

let primary = parser.primary_data()?;  // Cow<[u8]>
av1_decode(&primary)?;

if let Some(alpha) = parser.alpha_data() {
    av1_decode(&alpha?)?;
}
```

Three constructors (`from_bytes`, `from_owned`, `from_reader`), each with a `_with_config` variant accepting `DecodeConfig` + `Stop` for resource limits and cooperative cancellation.

### Animated AVIF

```rust
if let Some(info) = parser.animation_info() {
    for frame in parser.frames() {
        let frame = frame?;
        av1_decode(&frame.data)?;
        // frame.alpha_data is present if the animation has a separate alpha track
    }
}
```

### Grid Images

```rust
if let Some(grid) = parser.grid_config() {
    for i in 0..parser.grid_tile_count() {
        let tile = parser.tile_data(i)?;
        av1_decode(&tile)?;
    }
}
```

### HDR Gain Maps (tmap)

```rust
if let Some(meta) = parser.gain_map_metadata() {
    let gain_map_av1 = parser.gain_map_data().unwrap()?;
    let alt_color = parser.gain_map_color_info();
    // Use meta + gain_map_av1 + alt_color to reconstruct HDR
}
```

### Resource Limits and Cancellation

```rust
use zenavif_parse::{AvifParser, DecodeConfig};

let config = DecodeConfig::default()
    .with_peak_memory_limit(64 * 1024 * 1024)
    .with_total_megapixels_limit(128)
    .with_max_animation_frames(500)
    .with_max_grid_tiles(64);

let parser = AvifParser::from_bytes_with_config(&bytes, &config, &stop)?;
```

## Changes vs Upstream

**Parsing features:**
- Zero-copy `AvifParser` with `Cow`-based data access
- Grid images (iref dimg, ispe dimensions, tile ordering)
- Animated AVIF (moov/trak, alpha tracks, stts timing, loop count)
- HDR gain maps (tmap derived image items, ISO 21496-1 metadata)
- HDR metadata: clli, mdcv, cclv, amve property boxes
- EXIF and XMP metadata via cdsc references
- Entity groups (grpl/altr box parsing)
- idat construction method (iloc construction_method=1)
- Cooperative cancellation via the `enough::Stop` trait
- Resource limits (memory, megapixels, frames, tiles)

**Feature flags:**
- `eager` — enables deprecated `read_avif` / `AvifData` API
- `c_api` — C FFI bindings

**Backwards compatibility:**
- All upstream tests pass
- Default behavior is strict parsing

## Testing

```bash
cargo test --all-features
```

Tests require git submodules `av1-avif` and `link-u-samples` for corpus tests.

## License

MPL-2.0 (same as upstream)

## Credits

Original: https://github.com/kornelski/avif-parse
Fork: Extended for the zenavif project
