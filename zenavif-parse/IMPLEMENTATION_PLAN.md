# avif-parse Extended Support Implementation Plan

## Phase 1: Quick Wins (Target: 2-3 hours)

### 1.1 Fix "unknown sized box" Error
**Current issue:** Parser rejects boxes with size=0
**ISOBMFF spec:** size=0 is valid for last box in file (extends to EOF)

**Implementation:**
```rust
// In read_box_header
0 => {
    // Box extends to end of file (valid for last box, usually mdat)
    BoxSize::UntilEof
}
```

**Files fixed:** ~10 HDR files

### 1.2 Add Lenient Parsing Mode
**Goal:** Skip unknown boxes instead of failing

**API:**
```rust
pub struct ParseOptions {
    pub lenient: bool,  // Skip unknown/unsupported boxes
}

pub fn read_avif_lenient<T: Read>(f: &mut T) -> Result<AvifData> {
    read_avif_with_options(f, ParseOptions { lenient: true })
}
```

**Files fixed:** Various edge cases

### 1.3 Relax Validation
**Issue:** `expected flags to be 0` - too strict

**Fix:** Make warnings instead of errors for non-critical validations

**Files fixed:** extended_pixi.avif (+1-3 files)

---

## Phase 2: Grid Support (Target: 1-2 weeks)

### 2.1 Parse Grid Configuration
**Boxes involved:**
- `iref` (item references) - find tiles
- `iprp`/`ipco` (properties) - grid dimensions

**New types:**
```rust
pub struct GridConfig {
    pub rows: u8,
    pub columns: u8,
    pub output_width: u32,
    pub output_height: u32,
}

pub enum ImageData {
    Single(TryVec<u8>),
    Grid {
        tiles: Vec<TryVec<u8>>,
        config: GridConfig,
    },
}
```

### 2.2 Extract Tiles
- Parse item references to find tile IDs
- Extract each tile's AV1 bitstream
- Return in correct order

**Files fixed:** 7 grid files

---

## Phase 3: Construction Methods (Target: 2-3 days)

### 3.1 Support idat Construction
**Current:** Only file offset (method 0)
**Add:** idat box (method 1)

**Implementation:**
- Parse `idat` box location
- Read from idat + extent offset
- Instead of file offset

**Files fixed:** 2 idat files

---

## Phase 4: Animated AVIF (Target: 1 week, OPTIONAL)

### 4.1 Accept avis Brand
### 4.2 Parse Track/Media Boxes
### 4.3 Extract Frame Sequence

**Files fixed:** 5 animated files

---

## Testing Strategy

### For Each Feature:
1. Write unit test with specific failing file
2. Verify fix works
3. Run full test suite
4. Ensure no regressions

### Integration with zenavif:
```bash
# Update zenavif to use local avif-parse
cd ~/work/zenavif
# In Cargo.toml:
# avif-parse = { path = "../avif-parse" }
cargo test --release --test integration_corpus -- --ignored
```

---

## Current Status

- [x] Repository cloned
- [x] Submodules initialized
- [x] Tests passing (6/6)
- [ ] Phase 1.1: Fix size=0 boxes
- [ ] Phase 1.2: Lenient parsing
- [ ] Phase 1.3: Relax validation
- [ ] Update zenavif to test
- [ ] Phase 2: Grid support
- [ ] Phase 3: idat support
- [ ] Phase 4: Animated (optional)
