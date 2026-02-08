# YUV Conversion Analysis - FINAL

## TL;DR: Create zenyuv? **NO, but keep custom implementations**

The yuv crate's bilinear functions exist but use **13-bit fixed-point** precision, which is insufficient for pixel-perfect AVIF decoding. zenavif needs **f32 floating-point** with bilinear upsampling.

## Test Results

### Before (custom f32 bilinear):
- kodim03_yuv420_8bpc: <1% pixel errors  
- kodim23_yuv420_8bpc: <1% pixel errors
- Overall: 99%+ accuracy vs libavif

### After switching to yuv crate bilinear (13-bit fixed-point):
- kodim03_yuv420_8bpc: **24.04% pixel errors** ❌
- kodim23_yuv420_8bpc: **42.63% pixel errors** ❌  
- Overall: **17 files with mismatches** ❌

### After reverting to custom implementation:
- Back to <1% pixel errors ✅

## The Precision Gap

| Implementation | Precision | Bilinear | Accuracy |
|----------------|-----------|----------|----------|
| **yuv crate default** | 14/15-bit fixed | No | ~1 level error |
| **yuv crate bilinear** | **13-bit fixed** | Yes | **20-40% pixel errors** |
| **zenavif custom** | **f32 float** | Yes | **<1% pixel errors** |

**Key insight:** The yuv crate's bilinear functions sacrifice precision for speed. 13-bit fixed-point + bilinear upsampling gives WORSE results than 15-bit fixed-point without bilinear.

## Why yuv Crate Isn't Sufficient

1. **No f32 conversion mode** - Only 13/14/15-bit fixed-point
2. **Bilinear = lower precision** - Uses 13-bit (worse than non-bilinear 15-bit Professional mode)
3. **Can't mix bilinear + Professional** - API doesn't support it

From yuv-0.8.9/src/yuv_to_rgba_bilinear.rs:
```rust
pub fn yuv420_to_rgb_bilinear(
    planar_image: &YuvPlanarImage<u8>,
    rgb: &mut [u8],
    rgb_stride: u32,
    range: YuvRange,
    matrix: YuvStandardMatrix,
    // NO YuvConversionMode parameter!
) -> Result<(), YuvError>
```

The bilinear functions hardcode 13-bit precision and can't use Professional (15-bit) mode.

## Recommendation: Keep Custom Implementations

### zenavif ✅ 
**Keep both custom implementations:**
- `yuv_convert.rs` - f32 bilinear (pixel-perfect for verification)
- `yuv_convert_fast.rs` - Q13 integer (faster, still accurate enough)

**Why:** Needs <1% pixel error for libavif reference verification. yuv crate's 13-bit bilinear gives 20-40% errors.

### zenjpeg ✅
**Keep using yuv crate Professional mode:**
```rust
rgb_to_yuv420(..., YuvConversionMode::Professional)
```

**Why:** 15-bit precision is perfect for JPEG encoding. No bilinear needed (encoding, not decoding). 10-150× faster than scalar.

### zenwebp ✅  
**Keep custom libwebp port:**

**Why:** Must match libwebp exactly for parity tests. Different precision (14-bit vs yuv's 15-bit).

## Should We Create zenyuv?

### Arguments FOR:
1. Eliminate duplication (~700 lines across zenavif + zenwebp)
2. Offer f32 + bilinear option that yuv crate lacks
3. Unified API across our codecs
4. no_std support
5. Token-based SIMD with archmage

### Arguments AGAINST:
1. ✅ **zenjpeg already satisfied with yuv crate**
2. ✅ **zenwebp needs libwebp parity anyway**
3. ⚠️ **Only zenavif needs f32 bilinear** 
4. ❌ yuv crate has extensive SIMD (AVX-512, AVX2, SSE, NEON, WASM)
5. ❌ yuv crate is actively maintained  
6. ❌ Maintenance burden for marginal benefit

## Verdict: **NO, don't create zenyuv**

**Reasoning:**
- Only 1 project (zenavif) needs f32 + bilinear
- zenwebp needs custom code anyway (libwebp parity)
- zenjpeg is happy with yuv crate
- ~700 lines of custom code for ONE project isn't worth a whole new crate
- yuv crate will likely never add f32 mode (conflicts with their speed-focused design)

## What We Learned

1. **yuv crate DOES have bilinear upsampling** - but at lower precision (13-bit)
2. **Bilinear + high precision can't be mixed** in yuv crate
3. **f32 arithmetic is necessary** for pixel-perfect AVIF verification
4. **Different codecs have different accuracy needs:**
   - AVIF decode: Need <1% error (f32 + bilinear)
   - JPEG encode: 0.5 level error OK (15-bit fixed, no bilinear)
   - WebP: Must match libwebp exactly (14-bit fixed)

## Action: Document and Move On

- ✅ Keep zenavif custom YUV implementations
- ✅ Keep zenjpeg using yuv crate  
- ✅ Keep zenwebp custom libwebp port
- ✅ Document why in each project's CLAUDE.md
- ✅ This analysis saved to zenavif/YUV_ANALYSIS.md

No changes needed. Current setup is optimal for each project's requirements.
