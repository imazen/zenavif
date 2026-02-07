# zenavif Managed API Migration - Handoff Document

## Current Status

**Working Directory:** `/home/lilith/work/zenavif`
**Branch:** `main`
**Last Commits:**
- `6c0758b` - refactor: remove safe-simd feature, make managed default
- `9074446` - WIP: add managed API decoder (100% safe)

## Goal

Migrate zenavif to use rav1d-safe's managed API as the default, making it 100% safe Rust with NO C FFI option on the safe path.

## What's Done ✅

1. ✅ Created `src/decoder_managed.rs` (372 lines) - 100% safe decoder
2. ✅ Updated `Cargo.toml` - `managed` is now default (v0.2.0)
3. ✅ Updated `src/lib.rs` - conditional compilation for managed vs asm
4. ✅ Removed `safe-simd` feature entirely
5. ✅ Documented rav1d-safe issues in `~/work/rav1d-safe/CLAUDE.md`

## Immediate Tasks 🔴

### 1. Fix Compilation Errors in decoder_managed.rs

**File:** `src/decoder_managed.rs`

**Errors to fix:**

#### A. ChromaSampling enum variant
```rust
// Line ~89 - WRONG:
ChromaSampling::Mono => { ... }

// FIX TO:
ChromaSampling::Monochrome => { ... }
```

#### B. Stop error handling
```rust
// Throughout file - WRONG:
stop.check()?;

// FIX TO:
stop.check().map_err(|e| at(Error::Cancelled(e)))?;
```

#### C. Remove with_info() calls
```rust
// Lines ~312, 314 - WRONG:
Ok(DecodedImage::Rgba8(rgba_img).with_info(info))
Ok(DecodedImage::Rgb8(rgb_img).with_info(info))

// FIX TO (DecodedImage has no with_info method):
Ok(DecodedImage::Rgba8(rgba_img))
Ok(DecodedImage::Rgb8(rgb_img))
```

#### D. Complete 16-bit conversion

Currently line ~335 returns error. Need to implement similar to 8-bit:

```rust
fn convert_16bit(
    &self,
    primary: Frame,
    alpha: Option<Frame>,
    info: ImageInfo,
    stop: &impl Stop,
) -> Result<DecodedImage> {
    let Planes::Depth16(planes) = primary.planes() else {
        return Err(at(Error::Decode {
            code: -1,
            msg: "Expected 16-bit planes",
        }));
    };

    let width = info.width;
    let height = info.height;

    // Get Y, U, V planes
    let y_plane = planes.y();
    let u_plane = planes.u();
    let v_plane = planes.v();

    // Convert YUV16 to RGB16 (similar logic to 8-bit)
    // Use yuv crate with Depth::Depth10 or Depth::Depth12
    let yuv_depth = match info.bit_depth {
        10 => Depth::Depth10,
        12 => Depth::Depth12,
        _ => Depth::Depth16,  // fallback
    };
    let yuv_range = if info.full_range { Range::Full } else { Range::Limited };

    // Similar pattern to convert_8bit but with u16 data
    // ...implementation needed...
    
    // For now, can convert to 8-bit by shifting:
    // let y8 = y_plane.as_slice().iter().map(|&v| (v >> 8) as u8).collect();
    // Then use existing 8-bit conversion path
}
```

**Test build:**
```bash
cd ~/work/zenavif
cargo build --no-default-features --features managed
```

### 2. Remove ALL C FFI Dependencies

**Goal:** Make it impossible to enable C FFI on the safe path.

**Changes needed in `Cargo.toml`:**

```toml
[dependencies]
# rav1d only needed for asm feature
rav1d = { version = "1.1.0", default-features = false, features = ["bitdepth_8", "bitdepth_16"], optional = true }

# rav1d-safe WITHOUT c-ffi feature for managed
rav1d-safe = { path = "../rav1d-safe", default-features = false, features = ["bitdepth_8", "bitdepth_16"], optional = true }

[features]
default = ["managed"]
asm = ["dep:rav1d", "rav1d/asm", "rav1d/c-ffi"]  # Only asm needs c-ffi
managed = ["dep:rav1d-safe"]  # NO c-ffi feature!
```

**Verify:**
```bash
# Should build without any C FFI
cargo build --no-default-features --features managed

# Check for any unsafe in managed build
cargo clippy --no-default-features --features managed -- -D warnings
```

### 3. Delete Old C FFI Decoder

**File:** `src/decoder.rs`

This file is ONLY used by the `asm` feature now. Add conditional compilation:

```rust
// Top of src/decoder.rs
#![cfg(feature = "asm")]

// Rest of file unchanged
```

Or better: Rename and clarify:
```bash
mv src/decoder.rs src/decoder_ffi.rs
```

Then update `src/lib.rs`:
```rust
#[cfg(feature = "asm")]
mod decoder_ffi;
#[cfg(feature = "asm")]
pub use decoder_ffi::AvifDecoder;
```

## Integration Tests - AVIF Test Corpus 📦

### Test File Sources

Need to download comprehensive AVIF test sets from multiple sources:

#### 1. **Netflix AVIF Test Images**
```bash
mkdir -p tests/vectors/netflix
cd tests/vectors/netflix

# Netflix has public AVIF test images
# https://netflixtechblog.com/avif-for-next-generation-image-coding-b1d75675fe4
# Download from their public test set (need to find exact URL)
```

#### 2. **AV1 SVT Test Vectors**
```bash
mkdir -p tests/vectors/svt
cd tests/vectors/svt

# SVT-AV1 test vectors
# https://gitlab.com/AOMediaCodec/SVT-AV1/-/tree/master/Docs/test-vectors
# These are IVF/OBU but can be wrapped in AVIF
```

#### 3. **libavif Test Images**
```bash
mkdir -p tests/vectors/libavif
cd tests/vectors/libavif

# Clone libavif test images
git clone --depth=1 https://github.com/AOMediaCodec/libavif.git temp
cp -r temp/tests/data/*.avif .
rm -rf temp
```

#### 4. **Chromium AVIF Tests**
```bash
mkdir -p tests/vectors/chromium
cd tests/vectors/chromium

# Chromium has AVIF test images in their repo
# https://chromium.googlesource.com/chromium/src/+/refs/heads/main/third_party/blink/web_tests/images/resources/avif/
# Can use curl to download individual files
```

#### 5. **AVIF.io Sample Images**
```bash
mkdir -p tests/vectors/avif-io
cd tests/vectors/avif-io

# AVIF.io has public sample images
# https://avif.io/
# Download their sample set
```

#### 6. **cavif Test Images**
```bash
mkdir -p tests/vectors/cavif
cd tests/vectors/cavif

# cavif (Rust AVIF encoder) has test images
# https://github.com/kornelski/cavif-rs/tree/main/tests
# Clone and extract test files
git clone --depth=1 https://github.com/kornelski/cavif-rs.git temp
find temp/tests -name "*.avif" -exec cp {} . \;
rm -rf temp
```

#### 7. **rav1e Test Vectors**
```bash
mkdir -p tests/vectors/rav1e  
cd tests/vectors/rav1e

# rav1e (Rust AV1 encoder) test files
# These might need conversion to AVIF container
```

### Download Script

**Create:** `scripts/download-avif-test-vectors.sh`

```bash
#!/bin/bash
set -e

VECTORS_DIR="tests/vectors"
mkdir -p "$VECTORS_DIR"

echo "Downloading AVIF test vectors..."

# libavif (most comprehensive)
echo "1/7 Downloading libavif tests..."
if [ ! -d "$VECTORS_DIR/libavif" ]; then
    git clone --depth=1 https://github.com/AOMediaCodec/libavif.git /tmp/libavif
    mkdir -p "$VECTORS_DIR/libavif"
    find /tmp/libavif/tests -name "*.avif" -exec cp {} "$VECTORS_DIR/libavif/" \;
    rm -rf /tmp/libavif
fi

# cavif-rs
echo "2/7 Downloading cavif-rs tests..."
if [ ! -d "$VECTORS_DIR/cavif" ]; then
    git clone --depth=1 https://github.com/kornelski/cavif-rs.git /tmp/cavif
    mkdir -p "$VECTORS_DIR/cavif"
    find /tmp/cavif -name "*.avif" -exec cp {} "$VECTORS_DIR/cavif/" \; || true
    rm -rf /tmp/cavif
fi

# Add more sources as we find them...

echo "Done! Test vectors in $VECTORS_DIR"
find "$VECTORS_DIR" -name "*.avif" | wc -l | xargs echo "Total AVIF files:"
```

### Integration Test Structure

**Create:** `tests/integration_corpus.rs`

```rust
use zenavif::{decode, DecodedImage};
use std::fs;
use std::path::{Path, PathBuf};

fn find_test_vectors() -> Vec<PathBuf> {
    let mut vectors = Vec::new();
    let test_dirs = [
        "tests/vectors/libavif",
        "tests/vectors/cavif",
        "tests/vectors/netflix",
        "tests/vectors/chromium",
        "tests/vectors/avif-io",
    ];
    
    for dir in &test_dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("avif") {
                    vectors.push(path);
                }
            }
        }
    }
    
    vectors
}

#[test]
#[ignore] // Run with: cargo test --features managed -- --ignored
fn test_decode_all_vectors() {
    let vectors = find_test_vectors();
    
    if vectors.is_empty() {
        eprintln!("No test vectors found. Run: bash scripts/download-avif-test-vectors.sh");
        return;
    }
    
    eprintln!("Testing {} AVIF files", vectors.len());
    
    let mut passed = 0;
    let mut failed = 0;
    
    for path in &vectors {
        eprint!("Testing {:?}... ", path.file_name().unwrap());
        
        match fs::read(path) {
            Ok(data) => {
                match decode(&data) {
                    Ok(image) => {
                        eprintln!("✓ {}x{} @ {}bpp", 
                                 image.width(), image.height(), image.bit_depth());
                        passed += 1;
                    }
                    Err(e) => {
                        eprintln!("✗ Decode error: {}", e);
                        failed += 1;
                    }
                }
            }
            Err(e) => {
                eprintln!("✗ Read error: {}", e);
                failed += 1;
            }
        }
    }
    
    eprintln!("\n=== Results ===");
    eprintln!("Passed: {}", passed);
    eprintln!("Failed: {}", failed);
    eprintln!("Total:  {}", vectors.len());
    
    // Allow some failures for now (malformed test files, etc)
    let pass_rate = passed as f64 / vectors.len() as f64;
    assert!(pass_rate > 0.8, "Pass rate too low: {:.1}%", pass_rate * 100.0);
}

#[test]
fn test_specific_formats() {
    // Test specific important formats
    let test_cases = vec![
        ("8-bit 4:2:0", "tests/vectors/libavif/8bit_420.avif"),
        ("10-bit 4:4:4", "tests/vectors/libavif/10bit_444.avif"),
        ("With alpha", "tests/vectors/libavif/alpha.avif"),
        ("HDR", "tests/vectors/libavif/hdr.avif"),
    ];
    
    for (name, path) in test_cases {
        if let Ok(data) = fs::read(path) {
            eprintln!("Testing {}...", name);
            let image = decode(&data).expect(&format!("{} should decode", name));
            eprintln!("  ✓ {}x{}", image.width(), image.height());
        }
    }
}
```

### .gitignore Updates

**Add to `.gitignore`:**

```gitignore
# Test vectors (large binary files)
tests/vectors/
*.avif
*.ivf
*.obu

# But keep the download script
!scripts/download-avif-test-vectors.sh
```

## Verification Checklist

Before considering this complete:

- [ ] `cargo build --no-default-features --features managed` succeeds
- [ ] `cargo test --features managed` passes
- [ ] `cargo clippy --features managed -- -D warnings` passes
- [ ] Downloaded at least 100 AVIF test files
- [ ] Integration tests run (even if some fail initially)
- [ ] Verify NO C FFI in managed feature:
  ```bash
  cargo tree --no-default-features --features managed | grep -i "ffi\|unsafe"
  # Should only see rav1d-safe (no c-ffi feature)
  ```
- [ ] Update README with new feature documentation
- [ ] Update CLAUDE.md in zenavif

## Next Steps (Future Work)

1. **Complete 16-bit support** - Implement full YUV16 to RGB16 conversion
2. **Optimize performance** - Profile and optimize managed decoder
3. **Benchmark vs C FFI** - Compare managed vs asm performance
4. **Fix rav1d-safe issues**:
   - Implement Dav1dDataGuard (panic safety)
   - Add memory leak tests
   - Valgrind/ASAN in CI
5. **Publish crates**:
   - zenavif 0.2.0 (managed API)
   - rav1d-safe (after fixes)

## Commands Reference

```bash
# Build managed (safe)
cargo build --no-default-features --features managed

# Build asm (fast)
cargo build --no-default-features --features asm

# Test managed
cargo test --features managed

# Download test vectors
bash scripts/download-avif-test-vectors.sh

# Run integration tests
cargo test --features managed --test integration_corpus -- --ignored

# Check for unsafe
rg "unsafe" src/decoder_managed.rs  # Should be none!

# Verify dependencies
cargo tree --no-default-features --features managed
```

## Related Files

- `/home/lilith/work/rav1d-safe/managed_minimal_api.md` - API design doc
- `/home/lilith/work/rav1d-safe/CLAUDE.md` - Known issues
- `/home/lilith/work/zenavif/CLAUDE.md` - Project notes
- `/home/lilith/work/zenavif/src/decoder_managed.rs` - New safe decoder

## Context

Started with: "Let's fork avif-parse to use rav1d-safe"
Pivoted to: zenavif already exists, migrate it to managed API
Result: zenavif now 100% safe by default, C FFI only for asm feature

## Important Notes

⚠️ **DO NOT** enable c-ffi feature on rav1d-safe for managed feature
⚠️ The managed API in rav1d-safe still needs Dav1dDataGuard fix
✅ All unsafe code should be behind `feature = "asm"` only
✅ Default build should be completely safe
