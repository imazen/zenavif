# avif-parse Fork

This fork extends [kornelski/avif-parse](https://github.com/kornelski/avif-parse) v1.4.0 with full AVIF feature support: grid images, animated AVIF, idat construction, and a zero-copy `AvifParser` API.

All upstream tests pass. All files in the AOM test suite and link-u animated samples parse correctly.

## Two Parsing APIs

### Eager: `read_avif`

Reads the entire file and copies all mdat data into memory. This is the original upstream API.

```rust
use avif_parse::read_avif;
use std::io::BufReader;
use std::fs::File;

let mut f = BufReader::new(File::open("image.avif")?);
let data = read_avif(&mut f)?;
av1_decode(&data.primary_item)?;
if let Some(alpha) = &data.alpha_item {
    av1_decode(alpha)?;
}
```

For lenient parsing (skip non-critical validation errors):

```rust
use avif_parse::{read_avif_with_options, ParseOptions};

let data = read_avif_with_options(&mut f, &ParseOptions { lenient: true })?;
```

### Zero-Copy: `AvifParser`

Parses metadata without copying pixel data. Returns `Cow<[u8]>` — borrowed for single-extent items, owned (concatenated) for multi-extent items.

```rust
use avif_parse::AvifParser;

let bytes = std::fs::read("image.avif")?;
let parser = AvifParser::from_bytes(&bytes)?;

let primary = parser.primary_data()?;  // Cow<[u8]>
av1_decode(&primary)?;

if let Some(alpha) = parser.alpha_data() {
    av1_decode(&alpha?)?;
}
```

Three constructors:

- `from_bytes(&[u8])` — borrows the buffer; data access is zero-copy for single-extent items
- `from_owned(Vec<u8>)` — takes ownership; returns `AvifParser<'static>`
- `from_reader(impl Read)` — reads into an owned buffer; returns `AvifParser<'static>`

All three have `_with_config` variants that accept a `DecodeConfig` for resource limits.

### Animated AVIF

```rust
let parser = AvifParser::from_bytes(&bytes)?;

if let Some(info) = parser.animation_info() {
    for i in 0..info.frame_count {
        let frame = parser.frame(i)?;
        av1_decode(&frame.data)?;
        // display for frame.duration_ms milliseconds
    }
}

// Or iterate:
for frame in parser.frames() {
    let frame = frame?;
    av1_decode(&frame.data)?;
}
```

### Grid Images

```rust
let parser = AvifParser::from_bytes(&bytes)?;

if let Some(grid) = parser.grid_config() {
    for i in 0..parser.grid_tile_count() {
        let tile = parser.tile_data(i)?;
        av1_decode(&tile)?;
    }
}
```

### AV1 Bitstream Metadata

Extract sequence header info (dimensions, bit depth, chroma) without a full decode:

```rust
let meta = parser.primary_metadata()?;
println!("{}x{}, {} bit", meta.image_width, meta.image_height, meta.bit_depth);
```

### Resource Limits

`DecodeConfig` caps memory, megapixels, animation frames, and grid tiles during parsing:

```rust
use avif_parse::{AvifParser, DecodeConfig};

let config = DecodeConfig::default()
    .with_peak_memory_limit(64 * 1024 * 1024)
    .with_total_megapixels_limit(128)
    .with_max_animation_frames(500)
    .with_max_grid_tiles(64);

let parser = AvifParser::from_bytes_with_config(&bytes, config)?;
```

Use `DecodeConfig::unlimited()` to disable all limits.

### Conversion

`parser.to_avif_data()` converts to the eager `AvifData` type for interop with code that uses `read_avif`.

## Changes vs Upstream

**New types and functions:**
- `AvifParser<'data>` — zero-copy parser with `Cow`-based data access
- `FrameRef<'a>` — animation frame with `Cow<'a, [u8]>` data and duration
- `AnimationInfo` — frame count and loop count
- `DecodeConfig` — resource limits builder
- `AV1Metadata` — parsed AV1 sequence header
- `read_avif_with_config()` — eager parse with resource limits

**Parsing improvements:**
- Size=0 box support (ISOBMFF "extends to EOF")
- Lenient parsing mode via `ParseOptions`
- Grid image parsing (iref dimg references, ispe dimensions)
- Animated AVIF parsing (moov/trak, stts timing, stco/co64 offsets)
- idat construction method support (iloc construction_method=1)
- Correct construction_method handling (upstream guessed based on offset)

**Backwards compatibility:**
- `read_avif()` and `AvifData` unchanged
- Default behavior is strict parsing
- All upstream tests pass

## Testing

```bash
cargo test
```

Tests require the git submodules `av1-avif` and `link-u-samples` for corpus tests.

## License

MPL-2.0 (same as upstream)

## Credits

Original: https://github.com/kornelski/avif-parse
Fork: Extended support for the zenavif project
