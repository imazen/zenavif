# Context Handoff: zenavif SIMD Investigation

## Summary

We created `zenavif`, a pure Rust AVIF decoder wrapping rav1d and avif-parse. Then investigated whether rav1d's 160k lines of x86 assembly could be replaced with safe Rust intrinsics using archmage.

**Answer: YES, it's feasible.** Successfully prototyped the AVG function.

## Current State

### Commits
```
1b0ec3c Add SIMD prototype: AVG operation using archmage
db6fec0 Add decode-test recipe to justfile
9990615 Fix rav1d decode loop and add decode_avif example
58376c7 Initial zenavif crate skeleton
```

### Working Features
- AVIF decoding via rav1d + avif-parse
- 8/10/12-bit depth support
- Alpha channel with premultiplied handling
- Example: `cargo run --release --example decode_avif -- input.avif output.png`
- SIMD prototype: `src/simd/avg.rs` - AVG operation ported from asm to safe Rust

### Test Output
Files decoded to `/mnt/v/output/zenavif/test/`:
- test.png (640x480 RGBA)
- circle.png, colors_sdr_srgb.png, etc.

## Key Technical Findings

### rav1d Analysis
- **50k lines Rust** + **160k lines x86 asm** + ARM asm
- The asm provides performance, Rust provides structure
- Unsafe in Rust layer is mostly: C FFI, buffer management (DisjointMut), not SIMD
- Funded by ISRG/Prossimo, actively maintained

### avif-parse Analysis
- **2k lines pure Rust**, essentially safe (2 unsafe fns are C FFI exports)
- Limited feature support: no animated AVIF, no HDR gainmap boxes
- Maintained by Kornel Lesiński (Mozilla contributor)

### SIMD Prototype Success
Ported dav1d's AVG function from assembly to safe Rust:

```rust
// src/simd/avg.rs
#[arcane]
pub fn avg_8bpc_avx2(_token: Desktop64, dst: &mut [u8], ...) {
    let t1 = safe_unaligned_simd::x86_64::_mm256_loadu_si256(tmp1_arr);
    let sum = _mm256_add_epi16(t1, t2);
    let avg = _mm256_mulhrs_epi16(sum, round);
    // ...
}
```

Key patterns:
- `#[arcane]` macro enables target features, makes intrinsics safe
- `Desktop64::summon()` checks CPU at runtime, returns token if AVX2+FMA available
- Brute-force testing against scalar caught a signed-arithmetic bug

## Dependencies

```toml
[dependencies]
rav1d = { version = "1.1.0", default-features = false, features = ["bitdepth_8", "bitdepth_16"] }
avif-parse = "1.4.0"
archmage = { version = "0.4.0", features = ["macros"] }
safe_unaligned_simd = "0.2.4"
# ... plus yuv, imgref, rgb with patches
```

## Proposed Fork Strategy

### 1. Fork avif-parse → "avif-box" (Tractable)
- Add HDR gainmap box support
- Add animated AVIF support (or explicit rejection)
- ~2k lines, clear scope

### 2. Fork rav1d → "rav1d-safe" (Large but feasible)
- Replace 160k asm with ~40-60k Rust using archmage
- Incremental: port one function at a time, benchmark
- Keep asm feature flag for comparison
- Pattern proven with AVG prototype

## Files to Read

- `/home/lilith/work/zenavif/src/simd/avg.rs` - Working SIMD prototype
- `/home/lilith/work/zenavif/src/decoder.rs` - rav1d FFI wrapper
- `/home/lilith/work/downloaded-crates/rav1d-1.1.0/src/x86/mc_avx2.asm` - Example asm to port
- `/home/lilith/work/downloaded-crates/archmage-0.4.0/CLAUDE.md` - archmage usage guide

## Commands

```bash
cd ~/work/zenavif
just check      # cargo check
just test       # cargo test
just clippy     # cargo clippy -D warnings
cargo test simd --release  # Run SIMD tests specifically
cargo run --release --example decode_avif -- input.avif output.png
```

## Open Questions

1. **Performance**: Need to benchmark archmage intrinsics vs hand-tuned asm
2. **ARM support**: archmage supports NEON, but rav1d's ARM asm is also extensive
3. **Upstream interest**: Would ISRG/Prossimo accept PRs converting asm to safe Rust?

## User's Intent

User wants to explore replacing unsafe assembly with safe Rust intrinsics via archmage for both safety and maintainability. The prototype proves it's technically feasible. Next step is deciding whether to fork rav1d for a larger porting effort.
