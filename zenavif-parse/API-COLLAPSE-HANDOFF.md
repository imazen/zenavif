# API Collapse Handoff: AvifParser v2 Rewrite

**Branch:** `main`
**Date:** 2026-02-08
**Goal:** Replace the current 3-layer AvifParser API with a single Cow-based interface backed by a zero-copy `AvifParser<'data>`.

## The Problem

AvifParser currently has 3 ways to get the same data:

| Method | Returns | When to use |
|--------|---------|-------------|
| `primary_item()` | `TryVec<u8>` (copy) | Never — always allocates |
| `primary_item_slice()` | `&[u8]` | Only single-extent items |
| `primary_data()` | `Cow<[u8]>` | Always works |

Same pattern repeated for alpha, tiles, and frames = **15 public methods** doing the same thing three ways. Plus `can_zero_copy_*` helpers and two iterator types (`FrameIterator`, `FrameDataIterator`).

The `_slice` methods fail on multi-extent items. The copying methods always allocate. The Cow methods try slice first, fall back to copy. Cow is the one true way — the other two layers are redundant.

## Target Public API

```rust
pub struct AvifParser<'data> { /* ... */ }

impl<'data> AvifParser<'data> {
    // --- Constructors ---
    pub fn from_bytes(data: &'data [u8]) -> Result<Self>
    pub fn from_bytes_with_config(data: &'data [u8], config: &DecodeConfig, stop: &dyn Stop) -> Result<Self>
    pub fn from_owned(data: Vec<u8>) -> Result<AvifParser<'static>>
    pub fn from_owned_with_config(data: Vec<u8>, config: &DecodeConfig, stop: &dyn Stop) -> Result<AvifParser<'static>>
    pub fn from_reader<R: Read>(reader: &mut R) -> Result<AvifParser<'static>>
    pub fn from_reader_with_config<R: Read>(reader: &mut R, config: &DecodeConfig, stop: &dyn Stop) -> Result<AvifParser<'static>>

    // --- Data access (one way each) ---
    pub fn primary_data(&self) -> Result<Cow<'_, [u8]>>
    pub fn alpha_data(&self) -> Option<Result<Cow<'_, [u8]>>>
    pub fn tile_data(&self, index: usize) -> Result<Cow<'_, [u8]>>
    pub fn frame(&self, index: usize) -> Result<FrameRef<'_>>
    pub fn frames(&self) -> FrameIterator<'_>

    // --- Metadata (no data access) ---
    pub fn animation_info(&self) -> Option<AnimationInfo>
    pub fn grid_config(&self) -> Option<&GridConfig>
    pub fn grid_tile_count(&self) -> usize
    pub fn premultiplied_alpha(&self) -> bool
    pub fn primary_metadata(&self) -> Result<AV1Metadata>
    pub fn alpha_metadata(&self) -> Option<Result<AV1Metadata>>

    // --- Conversion ---
    pub fn to_avif_data(&self) -> Result<AvifData>
}

pub struct FrameRef<'a> {
    pub data: Cow<'a, [u8]>,
    pub duration_ms: u32,
}
```

**17 methods** (down from 24). One way to get each piece of data.

## What Gets Deleted

### AvifParser methods (15 deleted)
- `primary_item()` — replaced by `primary_data()`
- `alpha_item()` — replaced by `alpha_data()`
- `grid_tile(i)` — replaced by `tile_data(i)`
- `grid_tiles()` — use `tile_data(i)` in a loop
- `animation_frame(i)` — replaced by `frame(i)`
- `primary_item_slice()` — subsumed by Cow (auto-borrows)
- `alpha_item_slice()` — subsumed
- `grid_tile_slice(i)` — subsumed
- `animation_frame_slice(i)` — subsumed
- `frame_data(i)` — renamed to `frame(i)` returning `FrameRef`
- `frame_data_iter()` — merged into `frames()`
- `can_zero_copy_frame(i)` — not needed
- `can_zero_copy_primary()` — not needed
- `extract_item()` (internal) — replaced by `resolve_item()`
- `extract_item_slice()` (internal) — replaced by `resolve_item()`

### Types (3 deleted)
- `FrameDataIterator` — merged into `FrameIterator`
- `AnimationFrame` — replaced by `FrameRef` (Cow instead of TryVec)
- `ParseOptions` — subsumed by `DecodeConfig.lenient` (keep for `read_avif_with_options` signature only)

### Free functions (0 deleted, 1 could go later)
- `read_avif_with_options` — keep for now (existing callers), could deprecate later since `read_avif_with_config` subsumes it

## Structural Change: Zero-Copy Storage

Current `AvifParser` copies all mdat data into `TryVec<MediaDataBox>`.
New `AvifParser<'data>` stores `Cow<'data, [u8]>` (the whole file) and records mdat offsets only.

```rust
pub struct AvifParser<'data> {
    raw: Cow<'data, [u8]>,          // Whole file: Borrowed for from_bytes, Owned for from_owned/from_reader
    mdat_bounds: TryVec<MdatBounds>,// Just offset+length, no data
    idat: Option<TryVec<u8>>,       // Small, lives in meta box
    primary: ItemExtents,            // Where primary item data lives
    alpha: Option<ItemExtents>,
    grid_config: Option<GridConfig>,
    tiles: TryVec<ItemExtents>,
    animation: Option<AnimationParserData>,
    premultiplied_alpha: bool,
}

struct MdatBounds { offset: u64, length: u64 }
struct ItemExtents { construction_method: ConstructionMethod, extents: TryVec<ExtentRange> }
```

All data access goes through one internal method:

```rust
fn resolve_item(&self, item: &ItemExtents) -> Result<Cow<'_, [u8]>> {
    // 1 extent in mdat → Cow::Borrowed(&self.raw[start..end])
    // N extents → Cow::Owned(concatenated)
    // idat → Cow::Borrowed or Cow::Owned depending on extent count
}
```

## Implementation Steps

1. Add `MdatBounds`, `ItemExtents`, `FrameRef` types
2. Change `AvifParser` to `AvifParser<'data>` with `raw: Cow<'data, [u8]>` and `mdat_bounds`
3. Implement `parse_raw(data: &[u8], config: &DecodeConfig, stop: &dyn Stop) -> Result<ParsedStructure>` — reuse existing box parsing, but skip mdat content (record bounds only)
4. Implement `from_bytes` / `from_owned` / `from_reader` + `_with_config` variants
5. Implement `resolve_item()`, `resolve_file_extents()`, `resolve_idat_extents()` — the single extraction path
6. Implement public API: `primary_data`, `alpha_data`, `tile_data`, `frame`, `frames`, metadata methods, `to_avif_data`
7. Delete all Layer 1 (copying) and Layer 2 (slice) methods, `FrameDataIterator`, `can_zero_copy_*`
8. Update `FrameIterator` to yield `Result<FrameRef<'a>>`
9. Update tests — replace all `primary_item()` calls with `primary_data()`, etc.
10. Run `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`

## Key Decisions Already Made

- **Cow, not slices**: Cow handles both single-extent (borrow) and multi-extent (own) transparently
- **`&dyn Stop` not `impl Stop`**: Avoids monomorphizing the entire parser per Stop type
- **`AvifParser` does NOT store config or stop**: They're parse-time only. Data access is pure slicing.
- **`from_reader` reads everything then delegates to `from_owned`**: No streaming I/O during data access
- **`AvifData` stays as-is**: It's the eager, fully-materialized format. `to_avif_data()` bridges.
- **`read_avif` / `read_avif_with_options` / `read_avif_with_config` stay**: They return `AvifData` directly, separate code path from `AvifParser`

## Files to Modify

- `src/lib.rs` — all the changes above
- `tests/public.rs` — update method calls, add from_bytes/from_owned tests
- `AVIF_PARSER_V2_SPEC.md` — mark as implemented, remove TODOs

## Test Coverage Needed

- `from_bytes` on all 3 corpuses (zero-copy path)
- `from_owned` on all 3 corpuses
- `from_reader` still works (delegates to from_owned internally)
- `from_bytes_with_config` with limits that reject
- Single-extent item returns `Cow::Borrowed` (verify with `matches!(data, Cow::Borrowed(_))`)
- Multi-extent item returns `Cow::Owned` (use `kodim-extents.avif`)
- `FrameRef` has correct duration_ms
- `frames()` iterator yields correct count and data
- `to_avif_data()` matches `read_avif()` output

## What NOT to Change

- `AvifData` struct and its methods — stable public type
- `read_avif()` / `read_avif_with_options()` / `read_avif_with_config()` — separate eager path
- `DecodeConfig`, `ResourceTracker`, `Stop` integration — just done, working
- `c_api.rs` — uses `read_avif()`, unaffected
- Internal box parsing functions (`read_avif_meta`, `read_ftyp`, etc.) — reused by `parse_raw`
