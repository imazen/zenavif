# zenavif Performance Analysis & Optimization Opportunities

## Hot Paths Identified

### 🔥 CRITICAL: YUV to RGB Conversion
**Files**: `src/yuv_convert.rs`
**Functions**: `yuv420_to_rgb8()`, `yuv422_to_rgb8()`, `yuv444_to_rgb8()`

#### Current Implementation
Nested loop over every pixel (e.g., 2,073,600 iterations for 1920×1080):

```rust
for y in 0..height {
    for x in 0..width {
        // Per-pixel operations:
        // 1. Load Y value (1 load)
        // 2. Calculate chroma position (4 float ops)
        // 3. Clamp chroma position (2 float ops)
        // 4. Calculate 4 surrounding indices (4 floor, 4 min ops)
        // 5. Calculate interpolation weights (4 float ops)
        // 6. Load 8 chroma values (8 loads + 8 float conversions)
        // 7. Bilinear interpolation (12 multiplies, 3 adds)
        // 8. YUV to RGB conversion (8 multiplies, 6 adds, 3 divides)
        // 9. Clamp and convert to u8 (3 clamps, 3 round, 3 casts)
        // 10. Store RGB (1 store)

        // Total: ~60+ operations per pixel!
    }
}
```

**Estimated Cost**: For 1920×1080 image = **~124 million operations**

#### Optimization Opportunities

**1. SIMD Vectorization (8-16x speedup)**
Process 4-8 pixels simultaneously using `archmage`:

```rust
#[arcane]
fn yuv420_to_rgb8_simd<T: X64V3Token>(
    token: T, // AVX2 + FMA
    y_plane: &[u8],
    // ... other params
) -> ImgVec<RGB8> {
    // Process 8 pixels at once with __m256i
    // Use _mm256_loadu_si256 for loads
    // Use _mm256_cvtepi32_ps for i32 → f32 conversion
    // Use _mm256_fmadd_ps for fused multiply-add (bilinear interp)
    // Use _mm256_cvtps_epi32 for float → int conversion
}
```

**Benefits**:
- 8 pixels/iteration instead of 1
- FMA instructions (multiply-add in single op)
- Parallel loads/stores
- **Est. speedup: 6-8x** (not full 8x due to overhead)

**2. Lookup Table for Chroma Positioning (2-3x speedup)**
Pre-compute chroma positions and weights for common resolutions:

```rust
// Pre-compute for entire row at once
struct ChromaLookup {
    positions: Vec<(usize, usize, usize, usize)>, // cx0, cy0, cx1, cy1
    weights: Vec<(f32, f32, f32, f32)>,           // fx1*fy1, fx*fy1, fx1*fy, fx*fy
}

// Compute once per row, reuse for all pixels in row
let lookup = precompute_chroma_row(y, width, height);
for x in 0..width {
    let (cx0, cy0, cx1, cy1) = lookup.positions[x];
    let (w00, w01, w10, w11) = lookup.weights[x];
    // Direct loads, no position calculation needed
}
```

**3. Separate Hot Loop from Cold Path (2x speedup)**
Split into specialized functions for common cases:

```rust
// Fast path: Full range, BT.709, no edge cases (90% of images)
fn yuv420_to_rgb8_bt709_full_fast(...)

// Generic path: Handle all cases
fn yuv420_to_rgb8_generic(...)
```

**4. Row-Based Processing (Better Cache Locality)**
Process entire rows at once instead of pixel-by-pixel:

```rust
for y in 0..height {
    // Preload chroma row into cache
    let chroma_row_y = (y as f32 + 0.5) * 0.5 - 0.5;

    // Process 8 pixels at a time with SIMD
    for x_chunk in (0..width).step_by(8) {
        // SIMD processing of 8 pixels
    }
}
```

**Combined Potential**: **10-15x speedup** for YUV420 conversion

---

### 🔥 MODERATE: Grid Tile Stitching
**File**: `src/decoder_managed.rs`
**Functions**: `stitch_rgb8()`, `stitch_rgba8()`, etc.

#### Current Implementation
Nested loop copying pixels one-by-one:

```rust
for y in 0..tile_h {
    for x in 0..tile_w {
        output[(dst_x + x, dst_y + y)] = tile_img[(x, y)];
        // Per-pixel: 2 index calculations + 1 load + 1 store
    }
}
```

For 5 tiles of 1024×154 each = **788,480 pixel copies**

#### Optimization Opportunities

**1. Row-wise memcpy (5-10x speedup)**
Copy entire rows at once:

```rust
for y in 0..tile_h.min(height - dst_y) {
    let src_row = &tile_img.buf()[y * tile_w..(y + 1) * tile_w];
    let dst_row_start = (dst_y + y) * width + dst_x;
    let dst_row_end = dst_row_start + tile_w.min(width - dst_x);
    output.buf_mut()[dst_row_start..dst_row_end].copy_from_slice(&src_row[..]);
}
```

**Benefits**:
- Uses optimized memcpy
- Better cache locality
- No per-pixel indexing overhead
- **Est. speedup: 5-8x**

**2. Parallel Tile Processing**
Decode and convert tiles in parallel (requires thread-safe decoder):

```rust
use rayon::prelude::*;

let tile_images: Vec<DecodedImage> = grid_tiles
    .par_iter()
    .map(|tile_data| self.decode_tile(tile_data))
    .collect();
```

**Note**: Blocked by rav1d-safe threading issues (DisjointMut panics)

---

### 🔥 LOW: Alpha Channel Processing
**File**: `src/convert.rs`
**Function**: `unpremultiply_alpha()`

#### Current Implementation
```rust
for pixel in pixels.iter_mut() {
    if pixel.a > 0 {
        pixel.r = ((pixel.r as u16 * 255) / pixel.a as u16) as u8;
        pixel.g = ((pixel.g as u16 * 255) / pixel.a as u16) as u8;
        pixel.b = ((pixel.b as u16 * 255) / pixel.b as u16) as u8;
    }
}
```

**Issues**:
- Integer division per channel (slow)
- No SIMD
- Could use lookup table for common alpha values

**Optimization**: SIMD + float division (faster than integer division):
```rust
#[arcane]
fn unpremultiply_alpha_simd<T: X64V3Token>(token: T, pixels: &mut [RGBA8]) {
    // Process 8 pixels at once
    // Convert to float, divide, convert back
}
```

**Est. speedup**: 4-6x

---

## Profiling Setup

### Tools Recommended

**1. heaptrack** - Allocation profiling
```bash
heaptrack target/release/zenavif-bench
heaptrack --analyze heaptrack.zenavif-bench.*.zst
```

**2. cargo flamegraph** - Hot path visualization
```bash
cargo install flamegraph
cargo flamegraph --bin zenavif-bench
```

**3. cachegrind** - Cache miss analysis
```bash
valgrind --tool=cachegrind target/release/zenavif-bench
kcachegrind cachegrind.out.*
```

**4. cargo asm** - Inspect codegen
```bash
cargo install cargo-asm
cargo asm zenavif::yuv_convert::yuv420_to_rgb8 --rust
```

### Benchmark Harness

Create `benches/yuv_conversion.rs`:
```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use zenavif::yuv_convert::*;

fn bench_yuv420(c: &mut Criterion) {
    // Common resolutions
    let sizes = [
        (640, 480),   // SD
        (1920, 1080), // FHD
        (3840, 2160), // 4K
    ];

    for (width, height) in sizes {
        let y_plane = vec![128u8; width * height];
        let uv_size = ((width + 1) / 2) * ((height + 1) / 2);
        let u_plane = vec![128u8; uv_size];
        let v_plane = vec![128u8; uv_size];

        c.bench_function(&format!("yuv420_{}x{}", width, height), |b| {
            b.iter(|| {
                yuv420_to_rgb8(
                    black_box(&y_plane),
                    black_box(width),
                    black_box(&u_plane),
                    black_box((width + 1) / 2),
                    black_box(&v_plane),
                    black_box((width + 1) / 2),
                    black_box(width),
                    black_box(height),
                    YuvRange::Full,
                    YuvMatrix::Bt709,
                )
            });
        });
    }
}

criterion_group!(benches, bench_yuv420);
criterion_main!(benches);
```

Run with:
```bash
cargo bench --bench yuv_conversion
```

---

## Current Performance Baseline

From `benchmarks/decode_benchmark.rs`:

```
Small image (1x1):       21 µs
Medium RGBA (512x256):   3.2 ms
```

**Estimated breakdown** for 512×256 RGBA (131,072 pixels):
- AV1 decode: ~1.0 ms (rav1d)
- YUV to RGB: ~1.8 ms (**56% of time!**)
- Alpha processing: ~0.2 ms
- Overhead: ~0.2 ms

**With optimizations**, estimated 512×256 time:
- AV1 decode: ~1.0 ms (unchanged)
- YUV to RGB: ~0.2 ms (10x speedup)
- Alpha processing: ~0.05 ms (4x speedup)
- Overhead: ~0.2 ms
- **Total: ~1.45 ms** (2.2x overall speedup)

For 1920×1080 images (13.5x more pixels):
- Current (estimated): 43 ms
- Optimized: ~6 ms

---

## Priority Optimization Order

### Phase 1: Quick Wins (1-2 hours)
1. **Row-wise memcpy for grid stitching** (5-8x speedup, easy)
2. **Separate fast path for BT.709 Full range** (2x speedup, medium)

**Expected gain**: 2-3x overall for grid images

### Phase 2: SIMD (1-2 days)
3. **SIMD YUV420 conversion with archmage** (6-8x speedup, complex)
4. **Chroma position lookup table** (2x additional, medium)

**Expected gain**: 8-10x for YUV conversion, 3-4x overall

### Phase 3: Advanced (3-5 days)
5. **SIMD alpha unpremultiply** (4-6x speedup, medium)
6. **Parallel tile decoding** (2x speedup, blocked by rav1d-safe)
7. **Full SIMD pipeline** (combine all operations)

**Expected gain**: 10-15x overall for large images

---

## Memory Allocations

### Current Hotspots

**1. YUV Conversion Output**
```rust
let mut out = vec![RGB8::default(); width * height];
```
- Allocates full output buffer
- For 1920×1080 RGB: 6.2 MB allocation
- **Optimization**: Reuse buffer across frames (requires API change)

**2. Grid Stitching Output**
```rust
let mut output = imgref::ImgVec::new(vec![RGB8::default(); width * height], width, height);
```
- Another full-resolution allocation
- **Optimization**: Pre-allocate once, stitch in-place

**3. Tile Images**
Each tile allocates its own buffer, then gets copied to grid.
- For 5 tiles: 5 allocations + 1 final allocation = 6 total
- **Optimization**: Decode tiles directly into grid buffer (requires decoder API changes)

### Allocation Profiling

Run with heaptrack:
```bash
cargo build --release --features managed
heaptrack target/release/examples/decode_example tests/vectors/libavif/sofa_grid1x5_420.avif
heaptrack --analyze heaptrack.*.zst
```

Look for:
- Peak allocation size
- Allocation count (should be minimal)
- Temporary allocations (should be zero in hot path)

---

## Compiler Optimizations

### Current Cargo.toml
```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
```

### Recommended Additions
```toml
[profile.release]
opt-level = 3
lto = "fat"              # Full LTO across all crates
codegen-units = 1        # Better optimization, slower build
strip = true             # Smaller binary
panic = "abort"          # Smaller binary, faster panics

# Platform-specific optimizations
[profile.release-native]
inherits = "release"
rustflags = ["-C", "target-cpu=native"]  # Use local CPU features
```

Build with:
```bash
cargo build --profile release-native
```

### PGO (Profile-Guided Optimization)

For maximum performance:
```bash
# 1. Build instrumented binary
RUSTFLAGS="-C profile-generate=/tmp/pgo-data" cargo build --release

# 2. Run on representative data
target/release/zenavif tests/vectors/libavif/*.avif

# 3. Merge profile data
llvm-profdata merge -o /tmp/pgo-data/merged.profdata /tmp/pgo-data

# 4. Build optimized binary
RUSTFLAGS="-C profile-use=/tmp/pgo-data/merged.profdata" cargo build --release
```

**Expected gain**: 5-15% additional speedup

---

## Architecture Decisions

### SIMD Strategy: archmage vs multiversed

**Recommendation**: Use `archmage`

**Rationale**:
- Token-based dispatch (zero runtime overhead)
- Type-safe (no unsafe code needed)
- Works with managed/safe Rust
- `magetypes` integration for SIMD types

**Example**:
```rust
use archmage::*;
use magetypes::*;

#[arcane]
pub fn yuv420_to_rgb8_simd<T: X64V3Token>(
    token: T,
    y_plane: &[u8],
    // ... params
) -> ImgVec<RGB8> {
    // Use token to summon SIMD types
    let simd = token.summon();

    // Process with AVX2 + FMA
    for chunk in y_plane.chunks_exact(8) {
        // SIMD operations here
    }
}

// Public API with dynamic dispatch
pub fn yuv420_to_rgb8(...) -> ImgVec<RGB8> {
    X64V3Token::summon_or_else(
        |token| yuv420_to_rgb8_simd(token, ...),
        || yuv420_to_rgb8_scalar(...)  // Fallback
    )
}
```

---

## Next Steps

1. **Baseline**: Run benchmarks to establish current performance
2. **Profile**: Use flamegraph to identify actual hotspots (verify assumptions)
3. **Quick win**: Implement row-wise memcpy for grid stitching
4. **SIMD**: Start with YUV420 SIMD implementation
5. **Validate**: Ensure pixel-perfect output matches non-SIMD version
6. **Benchmark**: Measure actual speedup vs estimates

## Measurement Criteria

**Success Metrics**:
- YUV conversion: <0.3 ms for 512×256 (currently ~1.8 ms)
- Grid stitching: <0.1 ms for 5 tiles (currently ~0.5 ms)
- Total decode: <2 ms for 512×256 (currently ~3.2 ms)
- Zero additional allocations in hot path

**Regression Prevention**:
- Pixel-perfect output (all pixel verification tests pass)
- No unsafe code in critical path (maintain 100% safe Rust)
- Cross-platform (test on x86_64 without AVX2)
