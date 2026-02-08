# Streaming + Zero-Copy Implementation Handoff

**Date:** 2026-02-08
**Branch:** feat/extended-support
**Status:** Ready for implementation

## Context

zenavif (our AVIF decoder) needs:
1. **Streaming animation** - Extract frames on-demand (not all at once)
2. **Zero-copy** - Return slices instead of owned data
3. **Low memory** - Don't duplicate frame data

Currently `read_avif()` loads ALL 95 frames immediately (~1MB for 500KB file).

## ✅ Completed (3 commits)

### Commit 918ef6c: Grid Documentation
- Enhanced API docs for grid support

### Commit 1a03931: Animation Support
- Full .avifs parsing (moov/trak/stbl)
- Frame extraction with timing
- **Problem:** Eager loading (2× memory)
- All tests passing (9 tests)

### Commit 72c85d7: Safe Rust + Memory Docs
- `#![deny(unsafe_code)]` in core parser
- C FFI allowed only in c_api.rs
- Documented 2× memory usage for animations

## 🎯 Implementation Goals

### 1. Streaming API (Priority 1)
**Goal:** Extract frames on-demand, not all at once
**Benefit:** 50% memory reduction
**Complexity:** Medium (~400 lines)

### 2. Zero-Copy API (Priority 2)
**Goal:** Return slices into buffered mdat, not owned Vec
**Benefit:** Further memory reduction, faster parsing
**Complexity:** High (~500 lines, breaking API change)

## Architecture Design

### Current (Eager Loading)
```rust
pub fn read_avif<R: Read>(r: &mut R) -> Result<AvifData> {
    // Parse file
    // Load mdat boxes
    // Extract ALL frames immediately ← PROBLEM
    // Return owned data (TryVec<u8>)
}

pub struct AvifData {
    primary_item: TryVec<u8>,           // Owned
    animation: Option<AnimationConfig>, // All frames owned
    // ...
}

pub struct AnimationConfig {
    frames: TryVec<AnimationFrame>,     // ALL frames in memory
}
```

**Memory for 95-frame animation:**
- mdat boxes: ~500KB
- Extracted frames: ~500KB (duplicated!)
- **Total: ~1MB** (2× file size)

### Proposed (Streaming)
```rust
pub struct AvifParser {
    mdats: TryVec<MediaDataBox>,     // Buffered once
    idat: Option<TryVec<u8>>,

    // Metadata only (no extracted data)
    primary_item_extents: TryVec<ExtentRange>,
    alpha_item_extents: Option<TryVec<ExtentRange>>,
    grid_tile_extents: TryVec<TryVec<ExtentRange>>,
    animation_data: Option<AnimationParserData>,

    premultiplied_alpha: bool,
}

struct AnimationParserData {
    media_timescale: u32,
    sample_table: SampleTable,  // Metadata only
    loop_count: u32,
}

impl AvifParser {
    pub fn from_reader<R: Read>(r: &mut R) -> Result<Self> {
        // Parse file structure
        // Load mdat boxes
        // Store item LOCATIONS, not data
        // Store sample tables for animation
        // Return immediately (no frame extraction)
    }

    // Extract on-demand (streaming!)
    pub fn primary_item(&self) -> Result<TryVec<u8>> {
        self.extract_item(&self.primary_item_extents)
    }

    pub fn animation_frame(&self, index: usize) -> Result<AnimationFrame> {
        // Calculate frame offset from sample_table
        // Extract from mdat on-demand
        // Return single frame
    }

    pub fn animation_info(&self) -> Option<AnimationInfo> {
        // Return frame count without loading frames
    }
}
```

**Memory for 95-frame animation:**
- mdat boxes: ~500KB
- Sample table metadata: ~5KB
- **Total: ~505KB** (1× file size!)

### Future (Zero-Copy)
```rust
impl AvifParser {
    // Return slice into mdat buffer (zero-copy!)
    pub fn animation_frame_slice(&self, index: usize) -> Result<&[u8]> {
        let (offset, size) = self.calculate_sample_location(index)?;
        let range = ExtentRange::WithLength(Range { start: offset, end: offset + size });

        for mdat in &self.mdats {
            if mdat.contains_extent(&range) {
                return mdat.extent_slice(&range); // Zero-copy!
            }
        }
        Err(Error::InvalidData("frame not found"))
    }
}

// Add to MediaDataBox
impl MediaDataBox {
    fn extent_slice(&self, extent: &ExtentRange) -> Result<&[u8]> {
        // Return slice into self.data (no copy!)
    }
}
```

## Implementation Plan

### Step 1: Add AvifParser Structure (50 lines)

**File:** `src/lib.rs` after line 495 (before `AvifInternalMeta`)

```rust
/// Streaming AVIF parser for on-demand frame extraction (low memory usage)
///
/// Unlike [`AvifData`] which eagerly loads all animation frames,
/// `AvifParser` extracts frames on-demand, using ~50% less memory.
///
/// # Memory Usage
///
/// - **Animated images**: ~50% less memory (mdat only, no pre-extracted frames)
///
/// # Example
///
/// ```no_run
/// use avif_parse::AvifParser;
/// use std::fs::File;
///
/// let mut file = File::open("animation.avifs")?;
/// let parser = AvifParser::from_reader(&mut file)?;
///
/// if let Some(info) = parser.animation_info() {
///     for i in 0..info.frame_count {
///         let frame = parser.animation_frame(i)?; // On-demand!
///     }
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct AvifParser {
    mdats: TryVec<MediaDataBox>,
    idat: Option<TryVec<u8>>,
    primary_item_extents: TryVec<ExtentRange>,
    alpha_item_extents: Option<TryVec<ExtentRange>>,
    grid_config: Option<GridConfig>,
    grid_tile_extents: TryVec<TryVec<ExtentRange>>,
    animation_data: Option<AnimationParserData>,
    premultiplied_alpha: bool,
}

struct AnimationParserData {
    media_timescale: u32,
    sample_table: SampleTable,
    loop_count: u32,
}

/// Animation metadata from [`AvifParser`]
#[derive(Debug, Clone, Copy)]
pub struct AnimationInfo {
    pub frame_count: usize,
    pub loop_count: u32,
}
```

### Step 2: Implement from_reader (150 lines)

**Key points:**
- Parse file structure (reuse existing code from `read_avif`)
- Load mdat boxes
- Store item LOCATIONS (extents), not data
- Handle grid tiles with dimgIdx sorting

**Critical code patterns from existing read_avif:**

```rust
// Get item extents (line 1406-1425 in read_avif)
fn get_item_extents(meta: &AvifInternalMeta, item_id: u32) -> Result<TryVec<ExtentRange>> {
    let item = meta.iloc_items.iter()
        .find(|item| item.item_id == item_id)
        .ok_or(Error::InvalidData("item not found"))?;

    // Manual clone since TryVec doesn't impl Clone
    let mut extents = TryVec::new();
    for extent in &item.extents {
        extents.push(extent.clone())?;
    }
    Ok(extents)
}

// Find alpha item (line 1415-1424)
let alpha_item_id = meta.item_references.iter()
    .filter(|iref| {
        iref.to_item_id == meta.primary_item_id
            && iref.from_item_id != meta.primary_item_id
            && iref.item_type == b"auxl"
    })
    .map(|iref| iref.from_item_id)
    .find(|&item_id| {
        meta.properties.iter().any(|prop| {
            prop.item_id == item_id
                && matches!(&prop.property, ItemProperty::AuxiliaryType(_))
        })
    });

// Get grid tiles with dimgIdx sorting (line 1444-1461)
let mut tiles_with_index: TryVec<(u32, u16)> = TryVec::new();
for iref in meta.item_references.iter() {
    if iref.from_item_id == meta.primary_item_id && iref.item_type == b"dimg" {
        tiles_with_index.push((iref.to_item_id, iref.reference_index))?;
    }
}
tiles_with_index.sort_by_key(|&(_, idx)| idx);

// Premultiplied alpha check (line 1428-1435)
let premultiplied_alpha = alpha_item_id.map_or(false, |alpha_item_id| {
    meta.item_references.iter().any(|iref| {
        iref.from_item_id == meta.primary_item_id
            && iref.to_item_id == alpha_item_id
            && iref.item_type == b"prem"
    })
});
```

### Step 3: Implement Data Extraction Methods (100 lines)

```rust
impl AvifParser {
    pub fn primary_item(&self) -> Result<TryVec<u8>> {
        self.extract_item(&self.primary_item_extents)
    }

    pub fn alpha_item(&self) -> Option<Result<TryVec<u8>>> {
        self.alpha_item_extents.as_ref()
            .map(|extents| self.extract_item(extents))
    }

    pub fn grid_tile(&self, index: usize) -> Result<TryVec<u8>> {
        let extents = self.grid_tile_extents.get(index)
            .ok_or(Error::InvalidData("tile index out of bounds"))?;
        self.extract_item(extents)
    }

    fn extract_item(&self, extents: &[ExtentRange]) -> Result<TryVec<u8>> {
        let mut data = TryVec::new();
        for extent in extents {
            // Try idat first
            if let Some(idat_data) = &self.idat {
                if extent.start() == 0 {
                    match extent {
                        ExtentRange::WithLength(range) => {
                            let len = (range.end - range.start) as usize;
                            data.extend_from_slice(&idat_data[..len])?;
                            continue;
                        }
                        ExtentRange::ToEnd(_) => {
                            data.extend_from_slice(idat_data)?;
                            continue;
                        }
                    }
                }
            }

            // Try mdat boxes
            let mut found = false;
            for mdat in &self.mdats {
                if mdat.contains_extent(extent) {
                    mdat.read_extent(extent, &mut data)?;
                    found = true;
                    break;
                }
            }

            if !found {
                return Err(Error::InvalidData("extent not found"));
            }
        }
        Ok(data)
    }
}
```

### Step 4: Implement Animation Streaming (100 lines)

```rust
impl AvifParser {
    pub fn animation_info(&self) -> Option<AnimationInfo> {
        self.animation_data.as_ref().map(|data| AnimationInfo {
            frame_count: data.sample_table.sample_sizes.len(),
            loop_count: data.loop_count,
        })
    }

    pub fn animation_frame(&self, index: usize) -> Result<AnimationFrame> {
        let data = self.animation_data.as_ref()
            .ok_or(Error::InvalidData("not animated"))?;

        if index >= data.sample_table.sample_sizes.len() {
            return Err(Error::InvalidData("frame index out of bounds"));
        }

        // Calculate duration
        let duration_ms = self.calculate_frame_duration(
            &data.sample_table,
            data.media_timescale,
            index
        )?;

        // Calculate offset
        let (offset, size) = self.calculate_sample_location(
            &data.sample_table,
            index
        )?;

        // Extract from mdat
        let range = ExtentRange::WithLength(Range {
            start: offset,
            end: offset + size as u64,
        });

        let mut frame_data = TryVec::new();
        for mdat in &self.mdats {
            if mdat.contains_extent(&range) {
                mdat.read_extent(&range, &mut frame_data)?;
                return Ok(AnimationFrame {
                    data: frame_data,
                    duration_ms,
                });
            }
        }

        Err(Error::InvalidData("frame not found in mdat"))
    }

    fn calculate_frame_duration(&self, st: &SampleTable, timescale: u32, index: usize) -> Result<u32> {
        let mut current_sample = 0;
        for entry in &st.time_to_sample {
            if current_sample + entry.sample_count as usize > index {
                let duration_ms = if timescale > 0 {
                    ((entry.sample_delta as u64) * 1000) / (timescale as u64)
                } else {
                    0
                };
                return Ok(duration_ms as u32);
            }
            current_sample += entry.sample_count as usize;
        }
        Ok(0)
    }

    fn calculate_sample_location(&self, st: &SampleTable, index: usize) -> Result<(u64, u32)> {
        let sample_size = *st.sample_sizes.get(index)
            .ok_or(Error::InvalidData("sample index out of bounds"))?;

        // Build sample-to-chunk mapping (reuse from extract_animation_frames)
        let mut current_sample = 0;
        for (chunk_map_idx, entry) in st.sample_to_chunk.iter().enumerate() {
            let next_first_chunk = st.sample_to_chunk
                .get(chunk_map_idx + 1)
                .map(|e| e.first_chunk)
                .unwrap_or(u32::MAX);

            for chunk_idx in entry.first_chunk..next_first_chunk {
                if chunk_idx == 0 || (chunk_idx as usize) > st.chunk_offsets.len() {
                    break;
                }

                let chunk_offset = st.chunk_offsets[(chunk_idx - 1) as usize];

                for sample_in_chunk in 0..entry.samples_per_chunk {
                    if current_sample == index {
                        // Calculate offset within chunk
                        let mut offset_in_chunk = 0u64;
                        for s in 0..sample_in_chunk {
                            let prev_idx = current_sample.saturating_sub((sample_in_chunk - s) as usize);
                            if let Some(&prev_size) = st.sample_sizes.get(prev_idx) {
                                offset_in_chunk += prev_size as u64;
                            }
                        }

                        return Ok((chunk_offset + offset_in_chunk, sample_size));
                    }
                    current_sample += 1;
                }
            }
        }

        Err(Error::InvalidData("sample not found"))
    }
}
```

### Step 5: Add Convenience Methods (50 lines)

```rust
impl AvifParser {
    pub fn grid_config(&self) -> Option<&GridConfig> {
        self.grid_config.as_ref()
    }

    pub fn grid_tile_count(&self) -> usize {
        self.grid_tile_extents.len()
    }

    pub fn grid_tiles(&self) -> Result<TryVec<TryVec<u8>>> {
        let mut tiles = TryVec::new();
        for i in 0..self.grid_tile_count() {
            tiles.push(self.grid_tile(i)?)?;
        }
        Ok(tiles)
    }

    pub fn premultiplied_alpha(&self) -> bool {
        self.premultiplied_alpha
    }

    /// Convert to AvifData (loads all frames - high memory!)
    pub fn to_avif_data(&self) -> Result<AvifData> {
        let primary_item = self.primary_item()?;
        let alpha_item = self.alpha_item().transpose()?;
        let grid_tiles = self.grid_tiles()?;

        let animation = if let Some(info) = self.animation_info() {
            let mut frames = TryVec::new();
            for i in 0..info.frame_count {
                frames.push(self.animation_frame(i)?)?;
            }
            Some(AnimationConfig {
                loop_count: info.loop_count,
                frames,
            })
        } else {
            None
        };

        Ok(AvifData {
            primary_item,
            alpha_item,
            premultiplied_alpha: self.premultiplied_alpha,
            grid_config: self.grid_config.clone(),
            grid_tiles,
            animation,
        })
    }
}
```

### Step 6: Add Grid Helper Functions (50 lines)

```rust
impl AvifParser {
    fn calculate_grid_config(meta: &AvifInternalMeta, tile_count: usize) -> Result<GridConfig> {
        // Try explicit grid property first
        for prop in &meta.properties {
            if let ItemProperty::ImageGrid(grid) = &prop.property {
                return Ok(grid.clone());
            }
        }

        // Fall back to ispe calculation (use existing code from read_avif line 1466-1507)
        let grid_ispe = meta.properties.iter()
            .find(|p| matches!(&p.property, ItemProperty::ImageSpatialExtents(_)))
            .and_then(|p| if let ItemProperty::ImageSpatialExtents(ispe) = &p.property {
                Some(ispe)
            } else {
                None
            })
            .ok_or(Error::InvalidData("no ispe for grid"))?;

        // Infer N×1 vertical grid
        let columns = 1u8;
        let rows = tile_count.min(255) as u8;

        Ok(GridConfig {
            rows,
            columns,
            output_width: grid_ispe.width,
            output_height: grid_ispe.height,
        })
    }
}
```

## Testing Strategy

### Test 1: Basic Streaming
```rust
#[test]
fn streaming_parser_basic() {
    let input = &mut File::open("link-u-samples/star-8bpc.avifs").unwrap();
    let parser = AvifParser::from_reader(input).unwrap();

    // Should parse without loading frames
    assert!(parser.animation_info().is_some());

    let info = parser.animation_info().unwrap();
    assert_eq!(info.frame_count, 5);

    // Extract single frame
    let frame = parser.animation_frame(0).unwrap();
    assert!(!frame.data.is_empty());
    assert_eq!(frame.duration_ms, 100);
}
```

### Test 2: Memory Comparison
```rust
#[test]
fn streaming_vs_eager_memory() {
    use std::mem::size_of_val;

    // Eager loading
    let avif_data = read_avif(&mut File::open("animation.avifs").unwrap()).unwrap();
    let eager_size = size_of_val(&avif_data)
        + avif_data.animation.as_ref().unwrap().frames.len() * 5000; // ~5KB per frame

    // Streaming
    let parser = AvifParser::from_reader(&mut File::open("animation.avifs").unwrap()).unwrap();
    let streaming_size = size_of_val(&parser);

    println!("Eager: {}KB, Streaming: {}KB", eager_size / 1024, streaming_size / 1024);
    assert!(streaming_size < eager_size / 2); // Should be <50% memory
}
```

### Test 3: Frame Extraction Correctness
```rust
#[test]
fn streaming_matches_eager() {
    // Parse with both methods
    let avif_data = read_avif(&mut File::open("animation.avifs").unwrap()).unwrap();
    let parser = AvifParser::from_reader(&mut File::open("animation.avifs").unwrap()).unwrap();

    let info = parser.animation_info().unwrap();
    assert_eq!(info.frame_count, avif_data.animation.as_ref().unwrap().frames.len());

    // Compare first 5 frames
    for i in 0..5 {
        let eager_frame = &avif_data.animation.as_ref().unwrap().frames[i];
        let streaming_frame = parser.animation_frame(i).unwrap();

        assert_eq!(eager_frame.data.len(), streaming_frame.data.len());
        assert_eq!(eager_frame.data.as_slice(), streaming_frame.data.as_slice());
        assert_eq!(eager_frame.duration_ms, streaming_frame.duration_ms);
    }
}
```

### Test 4: Grid Compatibility
```rust
#[test]
fn streaming_parser_grid() {
    let parser = AvifParser::from_reader(
        &mut File::open("av1-avif/testFiles/Microsoft/Summer_in_Tomsk_720p_5x4_grid.avif").unwrap()
    ).unwrap();

    let grid = parser.grid_config().unwrap();
    assert_eq!(grid.rows, 4);
    assert_eq!(grid.columns, 5);

    assert_eq!(parser.grid_tile_count(), 20);

    // Extract first tile
    let tile = parser.grid_tile(0).unwrap();
    assert!(!tile.is_empty());
}
```

### Test 5: Integration with read_avif
```rust
#[test]
fn parser_to_avif_data_conversion() {
    let parser = AvifParser::from_reader(&mut File::open("image.avifs").unwrap()).unwrap();
    let avif_data = parser.to_avif_data().unwrap();

    // Should produce identical result to direct read_avif
    let direct = read_avif(&mut File::open("image.avifs").unwrap()).unwrap();

    assert_eq!(avif_data.primary_item.len(), direct.primary_item.len());
    assert_eq!(avif_data.primary_item.as_slice(), direct.primary_item.as_slice());
}
```

## Zero-Copy Extension (Phase 2)

After streaming works, add zero-copy slice methods:

### Step 1: Add extent_slice to MediaDataBox
```rust
impl MediaDataBox {
    /// Zero-copy access to extent data
    fn extent_slice(&self, extent: &ExtentRange) -> Result<&[u8]> {
        let start_offset = extent.start()
            .checked_sub(self.offset)
            .ok_or(Error::InvalidData("mdat doesn't contain extent"))?;

        let slice = match extent {
            ExtentRange::WithLength(range) => {
                let range_len = range.end.checked_sub(range.start)
                    .ok_or(Error::InvalidData("invalid range"))?;
                let end = start_offset.checked_add(range_len)
                    .ok_or(Error::InvalidData("extent overflow"))?;
                self.data.get(start_offset as usize..end as usize)
            }
            ExtentRange::ToEnd(_) => {
                self.data.get(start_offset as usize..)
            }
        };

        slice.ok_or(Error::InvalidData("extent out of bounds"))
    }
}
```

### Step 2: Add zero-copy methods to AvifParser
```rust
impl AvifParser {
    /// Zero-copy access to animation frame (returns slice into mdat)
    pub fn animation_frame_slice(&self, index: usize) -> Result<(&[u8], u32)> {
        let data = self.animation_data.as_ref()
            .ok_or(Error::InvalidData("not animated"))?;

        let duration_ms = self.calculate_frame_duration(
            &data.sample_table,
            data.media_timescale,
            index
        )?;

        let (offset, size) = self.calculate_sample_location(&data.sample_table, index)?;

        let range = ExtentRange::WithLength(Range {
            start: offset,
            end: offset + size as u64,
        });

        for mdat in &self.mdats {
            if mdat.contains_extent(&range) {
                let slice = mdat.extent_slice(&range)?;
                return Ok((slice, duration_ms));
            }
        }

        Err(Error::InvalidData("frame not found"))
    }

    /// Zero-copy access to primary item
    pub fn primary_item_slice(&self) -> Result<&[u8]> {
        self.extract_item_slice(&self.primary_item_extents)
    }

    fn extract_item_slice(&self, extents: &[ExtentRange]) -> Result<&[u8]> {
        // Only works for single-extent items
        if extents.len() != 1 {
            return Err(Error::Unsupported("multi-extent zero-copy not supported"));
        }

        let extent = &extents[0];

        // Try idat
        if let Some(idat_data) = &self.idat {
            if extent.start() == 0 {
                match extent {
                    ExtentRange::WithLength(range) => {
                        let len = (range.end - range.start) as usize;
                        return Ok(&idat_data[..len]);
                    }
                    ExtentRange::ToEnd(_) => {
                        return Ok(idat_data.as_slice());
                    }
                }
            }
        }

        // Try mdat
        for mdat in &self.mdats {
            if mdat.contains_extent(extent) {
                return mdat.extent_slice(extent);
            }
        }

        Err(Error::InvalidData("item not found"))
    }
}
```

## Expected Outcomes

### Performance Metrics
| Metric | Current (Eager) | Streaming | Zero-Copy |
|--------|----------------|-----------|-----------|
| Memory (95 frames) | ~1MB | ~500KB | ~500KB |
| Parse time | 50ms | 10ms | 10ms |
| Time to 1st frame | 50ms | 10ms | 10ms |
| Frame extraction | 0ms (cached) | 1ms | 0.1ms |

### zenavif Integration
```rust
// Current (not possible - zenavif doesn't support animation)
let avif = read_avif(&mut file)?;
// Error: no animation support

// After streaming
let parser = AvifParser::from_reader(&mut file)?;
if let Some(info) = parser.animation_info() {
    for i in 0..info.frame_count {
        let frame = parser.animation_frame(i)?;
        let decoded = rav1d.decode(&frame.data)?;
        display(decoded);
    }
}
```

## Common Pitfalls to Avoid

### 1. TryVec doesn't implement Clone
```rust
// ❌ Wrong
let extents = item.extents.clone();

// ✅ Correct
let mut extents = TryVec::new();
for extent in &item.extents {
    extents.push(extent.clone())?;
}
```

### 2. SingleItemTypeReferenceBox structure
```rust
// Structure has changed - it's not a collection anymore
struct SingleItemTypeReferenceBox {
    item_type: FourCC,
    from_item_id: u32,      // Single ID, not Vec
    to_item_id: u32,        // Single ID, not Vec
    reference_index: u16,   // dimgIdx for sorting
}

// ❌ Wrong
r.from_item_id.contains(&primary_id)

// ✅ Correct
r.from_item_id == primary_id
```

### 3. Grid tile sorting by dimgIdx
```rust
// MUST sort tiles by reference_index, not item_id
let mut tiles_with_index: TryVec<(u32, u16)> = TryVec::new();
for iref in meta.item_references.iter() {
    if iref.from_item_id == primary_id && iref.item_type == b"dimg" {
        tiles_with_index.push((iref.to_item_id, iref.reference_index))?;
    }
}
tiles_with_index.sort_by_key(|&(_, idx)| idx); // CRITICAL!
```

### 4. Premultiplied alpha detection
```rust
// Check for "prem" reference, not auxiliary type property
let premultiplied_alpha = alpha_item_id.map_or(false, |alpha_id| {
    meta.item_references.iter().any(|iref| {
        iref.from_item_id == meta.primary_item_id
            && iref.to_item_id == alpha_id
            && iref.item_type == b"prem"  // Not auxl!
    })
});
```

### 5. MediaDataBox offset
```rust
// Use existing pattern from read_avif line 1375-1378
BoxType::MediaDataBox => {
    if b.bytes_left() > 0 {
        let offset = b.offset();  // Not bytes_left_at_cur_offset()
        let data = b.read_into_try_vec()?;
        mdats.push(MediaDataBox { offset, data })?;
    }
}
```

## Commit Strategy

1. **Commit 1:** Add AvifParser structure and AnimationInfo type
2. **Commit 2:** Implement from_reader parsing
3. **Commit 3:** Add item extraction methods (primary, alpha, grid)
4. **Commit 4:** Implement animation streaming (frame extraction)
5. **Commit 5:** Add tests
6. **Commit 6:** Add zero-copy extent_slice
7. **Commit 7:** Add zero-copy slice methods
8. **Commit 8:** Update examples and documentation

## Success Criteria

- [ ] All existing tests pass (9 tests)
- [ ] New streaming tests pass (5+ tests)
- [ ] Memory usage < 50% of eager loading for animations
- [ ] Frame extraction correctness matches eager loading
- [ ] Grid images work correctly
- [ ] Zero-copy slice methods work
- [ ] No unsafe code (except in c_api.rs)
- [ ] Documentation complete

## Files to Modify

- `src/lib.rs` - Add AvifParser (~400 lines)
- `tests/public.rs` - Add streaming tests (~100 lines)
- `examples/test_streaming.rs` - Already created, needs update
- `README.md` - Document streaming API
- `CHANGELOG.md` - Note new streaming feature

## Reference Code Locations

Key patterns are in current `read_avif()` function:
- Item extent extraction: line 1406-1425
- Alpha item detection: line 1415-1424
- Grid tile collection: line 1444-1461
- Grid config calculation: line 1466-1507
- Premultiplied alpha: line 1428-1435
- Frame extraction: line 1860-1950 (extract_animation_frames)

## Questions for Next Session

1. Should we deprecate eager `read_avif()` or keep both APIs?
2. Should zero-copy methods return `Result<&[u8]>` or `&[u8]` (panic on error)?
3. Should we add `Iterator` impl for frames?
4. Should we support multi-extent items in zero-copy mode?

## Estimated Timeline

- Streaming implementation: 2-3 hours
- Testing and debugging: 1 hour
- Zero-copy extension: 1-2 hours
- Documentation: 30 minutes
- **Total: 4-6 hours**

Good luck! 🚀
