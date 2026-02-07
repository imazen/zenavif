# API Feedback - zencodecs Integration

**Date:** 2026-02-06
**Context:** Implementing AVIF decode codec adapter in zencodecs (src/codecs/avif_dec.rs)

## Issues Encountered

### 1. No probe-only function
**Issue:** To get image metadata (width, height, has_alpha), had to fully decode the image
**Current approach:** `decode(data)` returns `DecodedImage` with `.width()`, `.height()`, `.has_alpha()` methods
**Request:** Add a `probe(data)` or `read_metadata(data)` function that parses the AVIF container and extracts metadata without decoding pixels
**Impact:** For use cases that only need metadata (e.g., image gallery thumbnails showing dimensions), full decode wastes CPU

### 2. Stop trait incompatibility
**Issue:** zenavif uses `enough::Stop` trait, but zencodecs has its own `Stop` trait
**Attempted:** Tried to pass `&dyn zencodecs::Stop` to `decode_with()`, got trait bound error
**Resolution:** Used simple `decode()` without stop token support
**Request:** Either:
- Re-export a generic stop trait that both can implement
- Accept a generic `impl Stop` where Stop is defined in zenavif
- Provide adapter/wrapper to convert between Stop traits

### 3. No animation metadata exposure
**Issue:** zenavif doesn't expose whether the AVIF has multiple frames or animation info
**Current:** `DecodedImage` only represents a single decoded frame
**Request:** Add metadata fields like:
```rust
pub struct ImageMetadata {
    pub has_animation: bool,
    pub frame_count: Option<usize>,
    pub loop_count: Option<u32>,
}
```

### 4. No ICC profile extraction
**Issue:** zenavif doesn't expose ICC profile from the AVIF container
**Request:** Add `icc_profile: Option<&[u8]>` to metadata or `DecodedImage`

### 5. rgb crate dependency required
**Issue:** `DecodedImage::Rgb8(ImgVec<Rgb<u8>>)` uses types from `rgb` crate
**Resolution:** Had to add `rgb` crate (0.8.52) as dependency to use `ComponentBytes::as_bytes()`
**Impact:** Not necessarily an issue, just an observation that integrators need the `rgb` crate
**Note:** Could provide `DecodedImage::into_bytes()` method to avoid needing the rgb crate import

## Current Implementation

```rust
// Probe (inefficient - full decode)
let image = zenavif::decode(data)?;
let width = image.width() as u32;
let height = image.height() as u32;
let has_alpha = image.has_alpha();

// Decode
let image = zenavif::decode(data)?;
match image {
    zenavif::DecodedImage::Rgb8(img_vec) => {
        let (vec, _, _) = img_vec.into_contiguous_buf();
        use rgb::ComponentBytes;
        let bytes = vec.as_slice().as_bytes().to_vec(); // Flat RGB bytes
    }
    zenavif::DecodedImage::Rgba8(img_vec) => {
        let (vec, _, _) = img_vec.into_contiguous_buf();
        use rgb::ComponentBytes;
        let bytes = vec.as_slice().as_bytes().to_vec(); // Flat RGBA bytes
    }
    _ => {} // Gray8, Gray16, Rgb16, Rgba16 not supported yet
}
```

## Recommendations

1. **Add probe function:**
   ```rust
   pub fn probe(data: &[u8]) -> Result<ImageMetadata> {
       // Parse AVIF container, extract metadata, don't decode pixels
   }
   ```

2. **Expose animation metadata:**
   - Add `has_animation`, `frame_count`, `loop_count` to metadata
   - Consider multi-frame decode API

3. **Expose ICC profile:**
   - Add `icc_profile: Option<&[u8]>` to metadata

4. **Add convenience methods:**
   ```rust
   impl DecodedImage {
       pub fn into_bytes(self) -> Vec<u8> {
           // Convert to flat byte array without requiring rgb crate import
       }
   }
   ```

5. **Stop trait compatibility:**
   - Document how to integrate with custom Stop traits
   - Or accept generic `impl Stop` with looser trait bounds

## What Worked Well

- **Simple decode API:** `decode(data)` is clean and easy to use
- **Safe by default:** No unsafe code in the decoder with `managed` feature
- **Good error messages:** Helpful error variants from avif-parse and rav1d
- **Type safety:** `DecodedImage` enum makes output format explicit
- **Performance:** Fast decode with rav1d
- **Feature flags:** `managed` vs `asm` features work well

## Overall Assessment

**Good foundation, needs metadata improvements.** The core decode functionality works well and the API is straightforward. The main gaps are:
1. Lack of probe-only function (wastes CPU for metadata-only queries)
2. Missing animation/ICC metadata exposure
3. Stop trait integration challenges

These are all solvable without breaking the existing API - just additions. The `rgb` crate dependency is fine (it's a standard in the Rust imaging ecosystem).
