# avif-parse Fork - Extended Support

This fork adds extended AVIF support to [kornelski/avif-parse](https://github.com/kornelski/avif-parse) v1.4.0.

## Features Added

### ✅ Phase 1 Complete (70.9% test coverage)

1. **Size=0 Box Support** - ISOBMFF spec compliance
   - Handles boxes with size=0 ("extends to EOF")
   - Required for HDR AVIF files with HDR metadata
   - Fixes 10 HDR test files

2. **Lenient Parsing Mode** - Backwards compatible
   - `ParseOptions` struct for configuration
   - `read_avif_with_options()` API
   - Skips non-critical validation errors
   - Fixes 1 extended format file

## API

### Standard Usage (Strict Mode)
```rust
use avif_parse::read_avif;
use std::fs::File;

let mut f = File::open("image.avif")?;
let avif_data = read_avif(&mut f)?;
```

### Lenient Parsing
```rust
use avif_parse::{read_avif_with_options, ParseOptions};
use std::fs::File;

let mut f = File::open("image.avif")?;
let options = ParseOptions { lenient: true };
let avif_data = read_avif_with_options(&mut f, &options)?;
```

## Testing

```bash
# All avif-parse tests
cargo test

# Integration with zenavif
cd ../zenavif
cargo test --release --test integration_corpus -- --ignored
```

## Changes vs Upstream

**Additions:**
- `ParseOptions` struct
- `read_avif_with_options()` function
- Size=0 box handling (u64::MAX sentinel)
- Extended pixi box support in lenient mode

**Modifications:**
- `read_box_header` - accepts size=0
- `read_into_try_vec` - handles u64::MAX limit
- `check_parser_state` - checks original box size
- `read_fullbox_version_no_flags` - respects lenient flag
- `read_pixi` - skips extra bytes in lenient mode

**Backwards Compatibility:**
- ✅ All existing APIs unchanged
- ✅ Default behavior is strict (no changes for existing users)
- ✅ All upstream tests still pass

## Branch: feat/extended-support

**Commits:**
1. c11e216 - feat: support size=0 boxes (extends to EOF)
2. aaf22e7 - feat: add ParseOptions with lenient mode
3. d722a5a - fix: detect size=0 boxes after offset subtraction
4. 411d227 - fix: allow size=0 boxes in parser state check
5. c72285d - fix: check original box size for size=0 detection
6. ee7acb9 - fix: skip extra bytes in pixi box in lenient mode
7. 0338069 - docs: add comprehensive session handoff document
8. ce48cb8 - docs: Phase 1 completion summary - 70.9% achieved!

## Documentation

- `PHASE1_COMPLETE.md` - Final results and metrics
- `SESSION_HANDOFF_2026-02-07.md` - Technical implementation details
- `IMPLEMENTATION_PLAN.md` - Future phases roadmap

## Future Work (Optional)

### Phase 2: Grid Support (+7 files → 87%)
- Parse grid configuration from iref/iprp boxes
- Extract and return tile data
- 1-2 weeks estimated

### Phase 3: idat Construction (+4 files → 94%)
- Support iloc construction_method = 1
- Read from idat box instead of file offset
- 2-3 days estimated

### Phase 4: Animated AVIF (+5 files → 100%)
- Accept avis brand
- Parse track/media boxes
- 1 week estimated (optional - maintainer doesn't want this)

## License

MPL-2.0 (same as upstream)

## Credits

Original: https://github.com/kornelski/avif-parse  
Fork: Extended support for zenavif project  
Authors: See Cargo.toml
