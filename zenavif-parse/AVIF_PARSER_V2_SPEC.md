# AvifParser<'data> v2 Spec

**Date:** 2026-02-08
**Branch:** feat/extended-support
**Status:** Design approved, ready for implementation

## Problem

Current AvifParser copies all mdat data during parsing, even when input is
already `&[u8]`. For a 500KB animated AVIF, this means 500KB+ of unnecessary
allocation. The API surface has 12+ methods (copy, slice, Cow variants) for
what should be one operation.

## Design

### Core Type

```rust
pub struct AvifParser<'data> {
    /// Entire AVIF file. Cow::Borrowed for from_bytes, Cow::Owned for from_owned.
    raw: Cow<'data, [u8]>,

    /// Mdat box boundaries within raw (for bounds validation only)
    mdat_bounds: TryVec<MdatBounds>,

    /// Idat data (copied, lives inside meta box, typically small or absent)
    idat: Option<TryVec<u8>>,

    /// Primary image item location
    primary: ItemExtents,

    /// Alpha channel item location (if present)
    alpha: Option<ItemExtents>,

    /// Grid layout config
    grid_config: Option<GridConfig>,

    /// Grid tile locations (sorted by dimgIdx)
    tiles: TryVec<ItemExtents>,

    /// Animation metadata (sample table, timescale, loop count)
    animation: Option<AnimationParserData>,

    /// Premultiplied alpha flag
    premultiplied_alpha: bool,
}
```

### Supporting Types

```rust
/// Location of an mdat box within the file (no data, just bounds)
struct MdatBounds {
    offset: u64,
    length: u64,
}

/// Where an item's data lives (extents + construction method)
struct ItemExtents {
    construction_method: ConstructionMethod,
    extents: TryVec<ExtentRange>,
}

/// Returned from frame() and frames()
pub struct FrameRef<'a> {
    pub data: Cow<'a, [u8]>,
    pub duration_ms: u32,
}

/// Unchanged from current
struct AnimationParserData {
    media_timescale: u32,
    sample_table: SampleTable,
    loop_count: u32,
}
```

### Constructors

```rust
impl<'data> AvifParser<'data> {
    /// Parse from borrowed bytes. Zero-copy: stores Cow::Borrowed.
    /// Unlimited resource limits.
    pub fn from_bytes(data: &'data [u8]) -> Result<Self>

    /// Parse from borrowed bytes with resource limits and cancellation.
    pub fn from_bytes_with_config(
        data: &'data [u8],
        config: &DecodeConfig,
        stop: &dyn Stop,
    ) -> Result<Self>

    /// Parse from owned bytes. Stores Cow::Owned.
    /// Unlimited resource limits.
    pub fn from_owned(data: Vec<u8>) -> Result<AvifParser<'static>>

    /// Parse from owned bytes with resource limits and cancellation.
    pub fn from_owned_with_config(
        data: Vec<u8>,
        config: &DecodeConfig,
        stop: &dyn Stop,
    ) -> Result<AvifParser<'static>>
}

/// std-only convenience
#[cfg(feature = "std")]
impl AvifParser<'static> {
    /// Read all bytes then parse. Unlimited resource limits.
    pub fn from_reader<R: Read>(reader: &mut R) -> Result<Self>

    /// Read all bytes then parse with resource limits and cancellation.
    pub fn from_reader_with_config<R: Read>(
        reader: &mut R,
        config: &DecodeConfig,
        stop: &dyn Stop,
    ) -> Result<Self>
}
```

### Public API (complete, no duplicates)

```rust
impl<'data> AvifParser<'data> {
    // --- Data access (unified, Cow-based) ---

    /// Primary image AV1 payload.
    /// Cow::Borrowed for single-extent, Cow::Owned for multi-extent.
    pub fn primary_data(&self) -> Result<Cow<'_, [u8]>>

    /// Alpha channel AV1 payload, if present.
    pub fn alpha_data(&self) -> Option<Result<Cow<'_, [u8]>>>

    /// Grid tile AV1 payload by index.
    pub fn tile_data(&self, index: usize) -> Result<Cow<'_, [u8]>>

    /// Single animation frame by index.
    pub fn frame(&self, index: usize) -> Result<FrameRef<'_>>

    /// Iterator over all animation frames (lazy, on-demand).
    pub fn frames(&self) -> FrameIterator<'_>

    // --- Metadata (no data access) ---

    /// Animation info (frame count, loop count). None if not animated.
    pub fn animation_info(&self) -> Option<AnimationInfo>

    /// Grid config. None if not a grid image.
    pub fn grid_config(&self) -> Option<&GridConfig>

    /// Number of grid tiles (0 if not grid).
    pub fn grid_tile_count(&self) -> usize

    /// Whether alpha uses premultiplied mode.
    pub fn premultiplied_alpha(&self) -> bool

    /// Parse AV1 OBU header from primary item.
    pub fn primary_metadata(&self) -> Result<AV1Metadata>

    /// Parse AV1 OBU header from alpha item.
    pub fn alpha_metadata(&self) -> Option<Result<AV1Metadata>>

    // --- Conversion ---

    /// Eagerly load everything into AvifData (backwards compat).
    pub fn to_avif_data(&self) -> Result<AvifData>
}
```

### Iterator

```rust
pub struct FrameIterator<'a> {
    parser: &'a AvifParser<'a>,
    index: usize,
    count: usize,
}

impl<'a> Iterator for FrameIterator<'a> {
    type Item = Result<FrameRef<'a>>;
    // ...
}

impl ExactSizeIterator for FrameIterator<'_> {}
```

### Deleted API

Remove all of these from current AvifParser (never published, feature branch only):

- `primary_item()` → replaced by `primary_data()`
- `alpha_item()` → replaced by `alpha_data()`
- `grid_tile()` → replaced by `tile_data()`
- `grid_tiles()` → use iterator or tile_data(i)
- `animation_frame()` → replaced by `frame()`
- `primary_item_slice()` → subsumed (Cow auto-borrows)
- `alpha_item_slice()` → subsumed
- `grid_tile_slice()` → subsumed
- `animation_frame_slice()` → subsumed
- `frame_data()` → replaced by `frame()`
- `frame_data_iter()` → replaced by `frames()`
- `can_zero_copy_frame()` → not needed (Cow handles it)
- `can_zero_copy_primary()` → not needed
- `extract_item()` → internal, replaced
- `extract_item_slice()` → internal, replaced

## Internal Implementation

### Parse Flow

Both `from_bytes` and `from_owned` call the same internal function:

```rust
/// Parse structure from raw bytes. Returns metadata only, no data copying.
fn parse_raw(data: &[u8]) -> Result<ParsedStructure>

struct ParsedStructure {
    mdat_bounds: TryVec<MdatBounds>,
    idat: Option<TryVec<u8>>,
    primary: ItemExtents,
    alpha: Option<ItemExtents>,
    grid_config: Option<GridConfig>,
    tiles: TryVec<ItemExtents>,
    animation: Option<AnimationParserData>,
    premultiplied_alpha: bool,
}
```

`parse_raw` works by:

1. Wrapping `&data[..]` as `&mut &[u8]` (implements `Read` in std)
2. Using existing OffsetReader + BoxIter infrastructure
3. Parsing ftyp, meta, moov boxes normally (these are small)
4. For mdat boxes: **skip content, record offset+length only**
5. Building ItemExtents from iloc items (preserving construction_method!)
6. Calculating grid config from metadata

Key change in parse loop:

```rust
// OLD (copies all mdat data):
BoxType::MediaDataBox => {
    let offset = b.offset();
    let data = b.read_into_try_vec()?;
    mdats.push(MediaDataBox { offset, data })?;
}

// NEW (records bounds, skips data):
BoxType::MediaDataBox => {
    let offset = b.offset();
    let length = b.bytes_left() as u64;
    mdat_bounds.push(MdatBounds { offset, length })?;
    skip_box_content(&mut b)?;
}
```

### Data Resolution

Core extraction function:

```rust
impl<'data> AvifParser<'data> {
    /// Resolve item extents into data. Returns Cow::Borrowed when
    /// item is single-extent in mdat. Returns Cow::Owned when
    /// multi-extent (must concatenate) or idat construction.
    fn resolve_item(&self, item: &ItemExtents) -> Result<Cow<'_, [u8]>> {
        match item.construction_method {
            ConstructionMethod::File => self.resolve_file_extents(&item.extents),
            ConstructionMethod::Idat => self.resolve_idat_extents(&item.extents),
            ConstructionMethod::Item => {
                Err(Error::Unsupported("construction_method Item"))
            }
        }
    }

    fn resolve_file_extents(&self, extents: &[ExtentRange]) -> Result<Cow<'_, [u8]>> {
        if extents.len() == 1 {
            // Single extent: zero-copy slice into raw
            let (start, end) = self.extent_byte_range(&extents[0])?;
            self.validate_in_mdat(start, end)?;
            Ok(Cow::Borrowed(&self.raw[start..end]))
        } else {
            // Multi-extent: must concatenate
            let mut buf = Vec::new();
            for extent in extents {
                let (start, end) = self.extent_byte_range(extent)?;
                self.validate_in_mdat(start, end)?;
                buf.extend_from_slice(&self.raw[start..end]);
            }
            Ok(Cow::Owned(buf))
        }
    }

    fn resolve_idat_extents(&self, extents: &[ExtentRange]) -> Result<Cow<'_, [u8]>> {
        let idat = self.idat.as_ref()
            .ok_or(Error::InvalidData("idat missing"))?;
        if extents.len() == 1 {
            let (start, end) = self.extent_byte_range(&extents[0])?;
            Ok(Cow::Borrowed(&idat[start..end]))
        } else {
            let mut buf = Vec::new();
            for extent in extents {
                let (start, end) = self.extent_byte_range(extent)?;
                buf.extend_from_slice(&idat[start..end]);
            }
            Ok(Cow::Owned(buf))
        }
    }

    /// Convert ExtentRange to (start, end) byte indices
    fn extent_byte_range(&self, extent: &ExtentRange) -> Result<(usize, usize)> {
        match extent {
            ExtentRange::WithLength(range) => {
                let start = usize::try_from(range.start)
                    .map_err(|_| Error::InvalidData("extent start overflow"))?;
                let end = usize::try_from(range.end)
                    .map_err(|_| Error::InvalidData("extent end overflow"))?;
                Ok((start, end))
            }
            ExtentRange::ToEnd(range) => {
                let start = usize::try_from(range.start)
                    .map_err(|_| Error::InvalidData("extent start overflow"))?;
                Ok((start, self.raw.len()))
            }
        }
    }

    /// Validate that byte range falls within a known mdat box
    fn validate_in_mdat(&self, start: usize, end: usize) -> Result<()> {
        let start64 = start as u64;
        let end64 = end as u64;
        for mdat in &self.mdat_bounds {
            if start64 >= mdat.offset
                && end64 <= mdat.offset + mdat.length
            {
                return Ok(());
            }
        }
        Err(Error::InvalidData("extent outside mdat bounds"))
    }
}
```

### Animation Frame Resolution

```rust
fn frame(&self, index: usize) -> Result<FrameRef<'_>> {
    let anim = self.animation.as_ref()
        .ok_or(Error::InvalidData("not animated"))?;

    let duration_ms = self.calculate_frame_duration(
        &anim.sample_table, anim.media_timescale, index
    )?;
    let (offset, size) = self.calculate_sample_location(
        &anim.sample_table, index
    )?;

    // Frame data is always in mdat (File construction)
    let start = offset as usize;
    let end = start + size as usize;
    self.validate_in_mdat(start, end)?;

    Ok(FrameRef {
        data: Cow::Borrowed(&self.raw[start..end]),
        duration_ms,
    })
}
```

Note: animation frames are always single-extent (one contiguous sample),
so they ALWAYS return Cow::Borrowed. No copying ever needed for frames.

### Building ItemExtents (fix current bug)

Current code loses construction_method when extracting extents.
Fix by storing it:

```rust
fn get_item_info(meta: &AvifInternalMeta, item_id: u32) -> Result<ItemExtents> {
    let item = meta.iloc_items.iter()
        .find(|item| item.item_id == item_id)
        .ok_or(Error::InvalidData("item not found in iloc"))?;

    let mut extents = TryVec::new();
    for extent in &item.extents {
        extents.push(extent.extent_range.clone())?;
    }

    Ok(ItemExtents {
        construction_method: item.construction_method,
        extents,
    })
}
```

## Cargo.toml Changes

```toml
[features]
default = ["std"]
std = ["fallible_collections/std"]  # Enable from_reader
```

Note: from_bytes requires std too (internal parser uses Read on &[u8]).
True no_std would require rewriting the internal parser to work on
slices directly. That's a separate project (~2000 LOC rewrite of
OffsetReader, BoxIter, and all read_* functions). Not in scope.

For now: std is always required. The feature gate is for future
no_std work. The STRUCT and extraction methods are no_std-ready
(Cow is in alloc), but construction needs std.

## Existing API Compatibility

### read_avif() and AvifData - KEEP UNCHANGED

`read_avif()` and `AvifData` are the existing published API. Do not
modify them. They continue to work exactly as before - eagerly loading
all data. `AvifParser::to_avif_data()` bridges the two APIs.

MediaDataBox stays as-is for read_avif(). The new AvifParser doesn't
use it.

### AvifParser - REPLACE ENTIRELY

The current AvifParser (added today, unpublished, feature branch only)
gets replaced wholesale. No backwards compat concern.

## File Changes

### src/lib.rs

1. **Replace** AvifParser struct definition (add lifetime, new fields)
2. **Replace** entire impl AvifParser block with new implementation
3. **Add** MdatBounds, ItemExtents, FrameRef, FrameIterator types
4. **Delete** FrameDataIterator (merged into FrameIterator)
5. **Keep** AnimationParserData, AnimationInfo, GridConfig unchanged
6. **Keep** all internal parser functions unchanged (read_ftyp, etc.)
7. **Keep** MediaDataBox and its impl (used by read_avif)

Estimated diff: ~400 lines removed, ~300 lines added (net -100 LOC).

### tests/public.rs

1. **Replace** streaming_* tests with new API tests
2. **Replace** zero_copy_* tests (not needed, Cow handles it)
3. **Add** from_bytes vs from_owned equivalence test
4. **Keep** all AvifData/read_avif tests unchanged

### examples/test_streaming.rs

Rename to examples/streaming.rs. Update to use new API:

```rust
let data = std::fs::read("animation.avifs")?;
let parser = AvifParser::from_bytes(&data)?;

// Zero-copy frame iteration
for frame in parser.frames() {
    let frame = frame?;
    println!("{} bytes, {}ms", frame.data.len(), frame.duration_ms);
}
```

## Implementation Order

1. Add MdatBounds, ItemExtents, FrameRef types
2. Rewrite AvifParser struct with lifetime
3. Implement parse_raw (skip mdat, record bounds)
4. Implement from_bytes, from_owned, from_reader (and _with_config variants)
5. Implement resolve_item, resolve_file_extents, resolve_idat_extents
6. Implement primary_data, alpha_data, tile_data, frame, frames
7. Implement to_avif_data bridge
8. Update tests
9. Update examples
10. Delete old code (FrameDataIterator, duplicate methods)
11. Run cargo fmt, clippy, test
12. Commit

## Resource Limits & Cancellation

All constructors have `_with_config` variants accepting `&DecodeConfig` and `&dyn Stop`.
The plain constructors use `DecodeConfig::unlimited()` and `&Unstoppable`.

Error types include:
- `Error::ResourceLimitExceeded(&'static str)` — limit exceeded before allocation
- `Error::Stopped(enough::StopReason)` — cooperative cancellation via `Stop` trait

Re-exports: `pub use enough::{Stop, StopReason, Unstoppable};`

Checkpoints:
- `stop.check()?` in top-level box iteration loop
- `tracker.reserve(size)` before mdat reads, `tracker.release(size)` after
- `tracker.validate_grid_tiles()` after collecting tile refs
- `tracker.validate_animation_frames()` after moov parsing
- `tracker.validate_total_megapixels()` after grid dimensions parsed

## Commit Strategy

One commit: `refactor: rewrite AvifParser as zero-copy with lifetime`

This is a clean rewrite of unpublished code on a feature branch.
No need for incremental commits.

## Success Criteria

- [ ] from_bytes: zero allocation for data (only metadata allocs)
- [ ] from_owned: one allocation (the Vec), then zero-copy extraction
- [ ] Single-extent items: always Cow::Borrowed (verified by test)
- [ ] Multi-extent items: Cow::Owned (correct concatenation)
- [ ] Animation frames: always Cow::Borrowed
- [ ] All existing read_avif tests still pass
- [ ] Grid images work correctly (dimgIdx ordering preserved)
- [ ] Construction method preserved (idat vs file)
- [ ] validate_in_mdat prevents out-of-bounds access
- [ ] No unsafe code
- [ ] API surface: 10 public methods (down from 20+)
