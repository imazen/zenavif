# zenavif-serialize

[![crates.io](https://img.shields.io/crates/v/zenavif-serialize.svg)](https://crates.io/crates/zenavif-serialize)
[![docs.rs](https://docs.rs/zenavif-serialize/badge.svg)](https://docs.rs/zenavif-serialize)
[![license](https://img.shields.io/crates/l/zenavif-serialize.svg)](LICENSE)

AVIF container serializer (muxer) in pure Rust. Creates MPEG/HEIF/MIAF/ISO-BMFF boxes for still images, animations, and grid layouts. Only dependency is `arrayvec`.

Together with [zenrav1e](https://lib.rs/zenrav1e), it enables pure-Rust AVIF encoding.

## Fork of avif-serialize

Forked from [avif-serialize](https://lib.rs/avif-serialize) (v0.8.x) by Kornel Lesinski. Fully synced with upstream — no upstream commits are missing.

Changes from upstream (+2,400 lines, -54 lines):

- **Animation** — `AnimatedImage` builder with per-frame durations, keyframe control, alpha track (`animated.rs`, 795 lines)
- **Grid/tiled images** — `GridImage` builder for tile-based encoding up to 256x256 (`grid.rs`, 695 lines)
- **HDR metadata** — content light level (clli) and mastering display color volume (mdcv) boxes
- **Transforms** — rotation (irot), mirror (imir), clean aperture crop (clap), pixel aspect ratio (pasp)
- **Metadata** — ICC profile, EXIF, and XMP embedding as separate items with item references
- **Builder API** — `Aviffy` builder with `#[non_exhaustive]` types for forward compatibility
- **Edition 2024** — migrated from edition 2021

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
zenavif-serialize = "0.9"
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

Produces files compatible with Chrome 85+, Firefox 92+, libavif, and other MIAF-conformant decoders.

## License

BSD-3-Clause. Original code copyright Cloudflare, Inc. Fork additions copyright Imazen LLC.
