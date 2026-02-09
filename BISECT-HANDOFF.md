# Bisect Handoff: rav1d-safe Pixel Correctness Regression

## The Problem

rav1d-safe produces **systematically wrong pixel values** for all decoded images.
The AV1 decoder runs without panics (3261/3261 files decode) but the pixel data
is garbage. This was discovered by comparing zenavif output against libavif
(avifdec v1.1.1) and Pillow (which also uses libavif/dav1d).

### Smoking Gun: white_1x1.avif

```
rav1d-safe Y plane:  Y[0,0] = 128  (mid-gray)
Pillow/libavif RGB:  (253, 253, 253)  (white)
```

A 1x1 white pixel decodes as gray. The Y plane value is 128 when it should be ~253.

### Corpus-Wide Evidence

Compared zenavif (rav1d-safe) against libavif references for 3261 real-world AVIF files:

- **0% exact pixel match** (zero files)
- **66% major mismatch** (max error = 255, nearly every pixel wrong)
- **34% no reference** (libavif also failed, or 16-bit skipped)
- Mean brightness ratio: ~1.8x (sometimes brighter, sometimes darker)
- Top-left pixels often grayscale (R=G=B) where libavif shows color

## When It Broke

The "34/51 test files pixel-perfect match" result from Feb 7 used `rav1d-safe-forbid`
(the old main branch). On Feb 8, `feat/fully-safe-intrinsics` was merged into main,
followed by aggressive PicBuf/alignment refactoring. The zenavif dep was then switched
from `../rav1d-safe-forbid` to `../rav1d-safe`.

### Suspect Commits in rav1d-safe (Feb 8-9)

```
a2f39ba  Feb 8 15:41  Merge branch 'main' into feat/fully-safe-intrinsics
8edc850  Feb 8 15:28  Merge branch 'feat/fully-safe-intrinsics'
2d07f61  Feb 8 23:30  refactor: replace custom align types with aligned/aligned-vec crates
b67f378  Feb 8 22:02  feat: achieve crate-level forbid(unsafe_code) for default build
f897769  Feb 9 01:27  refactor: replace StridedBuf with PicBuf that owns its Vec
0ce0957  Feb 9 02:10  refactor: eliminate raw pointers and manual Send/Sync from PicBuf
```

The most likely culprits are the alignment/PicBuf refactoring commits, which changed
how decoded pixel buffers are stored and accessed.

## How To Test (Quick Single-File Check)

### From zenavif (end-to-end)

```bash
cd ~/work/zenavif

# Decode white_1x1.avif and print pixel values
cargo run --release --example inspect_pixels -- tests/vectors/libavif/white_1x1.avif
# EXPECTED (correct): RGB8 1x1, (253,253,253) or similar near-white
# ACTUAL (broken):    RGB8 1x1, (128,128,128) mid-gray

# Decode and inspect raw Y/U/V planes from rav1d-safe
cargo run --release --example inspect_planes -- tests/vectors/libavif/white_1x1.avif
# EXPECTED: Y first value ≈ 253
# ACTUAL:   Y first value = 128
```

### From rav1d-safe directly

```bash
cd ~/work/rav1d-safe

# The safe_simd_crashes tests only check for panics (not pixel correctness):
cargo test --test safe_simd_crashes
# These will PASS even with wrong pixel values

# To check pixel correctness, use zenavif's examples (see above)
```

### Verify with Pillow (ground truth)

```python
from PIL import Image
import numpy as np
img = Image.open("tests/vectors/libavif/white_1x1.avif")
print(np.array(img))  # Should show [[[253, 253, 253]]]
```

## How To Test (Full Corpus Comparison)

### Prerequisites

1. **AVIF corpus**: `/mnt/v/datasets/scraping/avif/` (3261 files in google-native/ and unsplash/)
2. **libavif references**: `/mnt/v/output/zenavif/libavif-refs/` (3247 PNGs, flat by filename)
3. References were generated via Docker (libavif v1.1.1 + dav1d):
   ```bash
   cd ~/work/zenavif
   docker build -t libavif-ref -f Dockerfile.references .
   docker run --rm \
     -v /mnt/v/datasets/scraping/avif:/vectors:ro \
     -v /mnt/v/output/zenavif/libavif-refs:/references \
     libavif-ref
   ```

### Run Pixel Comparison

```bash
cd ~/work/zenavif
cargo run --release --example compare_libavif
```

This compares zenavif output (RGB8) against libavif reference PNGs for all 3261 files.
Reports: exact match, close (err≤2), minor (err≤10), major (err>10), dimension mismatch.
Full report written to `/mnt/v/output/zenavif/comparison-report.txt`.

**When correct:** expect >90% exact match, remainder close/minor (rounding differences).
**Currently:** 0% exact, 66% major (max_err=255 on every compared file).

### Run Corpus Decode Test (Panic/Crash Check Only)

```bash
cd ~/work/zenavif
cargo run --release --example corpus_test
```

Decodes all 3261 files, copies failures to `/mnt/v/output/zenavif/parse-failures/`.
Currently: 3261/3261 pass (no panics). This does NOT check pixel correctness.

## Bisect Strategy

### Quick Bisect (rav1d-safe only)

The fastest check is decoding white_1x1.avif and checking the Y plane value.
You need zenavif's `inspect_planes` example to drive rav1d-safe.

```bash
cd ~/work/rav1d-safe

# Good: any commit before feat/fully-safe-intrinsics merge (before Feb 8 15:28)
# Bad:  HEAD (9e9d8e8)

git bisect start
git bisect bad HEAD
git bisect good <commit-before-merge>  # Find this from git log

# At each step, rebuild zenavif and check:
cd ~/work/zenavif
cargo run --release --example inspect_planes -- tests/vectors/libavif/white_1x1.avif 2>/dev/null | grep "Y first"
# Good = "Y first 10 values: [253, ..." (or near 253)
# Bad  = "Y first 10 values: [128, ..."

cd ~/work/rav1d-safe
git bisect good  # or git bisect bad
```

### Automated Bisect Script

Save as `~/work/bisect-check.sh`:

```bash
#!/bin/bash
set -e
cd ~/work/zenavif
OUTPUT=$(cargo run --release --example inspect_planes -- tests/vectors/libavif/white_1x1.avif 2>/dev/null)
Y_VAL=$(echo "$OUTPUT" | grep "Y first" | grep -oP '\[\K[0-9]+')
echo "Y[0] = $Y_VAL"
if [ "$Y_VAL" -gt 200 ]; then
    echo "GOOD (Y ≈ white)"
    exit 0  # good
else
    echo "BAD (Y = $Y_VAL, expected >200)"
    exit 1  # bad
fi
```

Then: `cd ~/work/rav1d-safe && git bisect run ~/work/bisect-check.sh`

### Finding the Good Commit

The last known-good state was the old `main` branch before the
`feat/fully-safe-intrinsics` merge. To find it:

```bash
cd ~/work/rav1d-safe
# Look for the merge commit
git log --oneline --all | grep -i "merge.*fully-safe"
# The parent of that merge on the main side is the last good commit
git log --oneline --first-parent 8edc850^ | head -5
```

## Separate Issue: yuv_convert_libyuv.rs Has Wrong Constants

Independent of the rav1d-safe regression, `src/yuv_convert_libyuv.rs` has broken
conversion constants:

- Uses limited-range Y gain (1.164) for full-range conversion
- BT.601 coefficients `ug` and `vg` are approximately doubled
- BT.709 `ub` coefficient is wrong (-128 instead of -135)
- `BT709_FULL` and `BT709_LIMITED` have identical constants
- `BT601_FULL` and `BT601_LIMITED` have identical constants

The decoder now bypasses this module entirely (uses `yuv_convert.rs` which has
correct floating-point math with SIMD via archmage). The libyuv module should be
either fixed or deleted.

## Files in This Investigation

### zenavif examples (new, uncommitted)

| File | Purpose |
|------|---------|
| `examples/corpus_test.rs` | Bulk decode 3261 files, collect failures |
| `examples/retry_failures.rs` | Re-test previously failed files |
| `examples/compare_libavif.rs` | Pixel comparison against libavif refs |
| `examples/inspect_pixels.rs` | Print decoded RGB values for one file |
| `examples/inspect_planes.rs` | Print raw Y/U/V plane data from rav1d-safe |
| `examples/inspect_metadata.rs` | Print AVIF container metadata (CICP, ICC) |
| `examples/save_png.rs` | Save zenavif decode result as PNG |

### zenavif code changes (uncommitted)

- `src/decoder_managed.rs`: Removed `yuv_convert_libyuv` dispatch, now uses
  `yuv_convert` directly (correct math, SIMD-accelerated)
- `tests/integration_corpus.rs`: Updated to expect 100% pass rate
- `CLAUDE.md`: Updated test results

### External data

- `/mnt/v/datasets/scraping/avif/` — 3261 AVIF corpus (google-native + unsplash)
- `/mnt/v/output/zenavif/libavif-refs/` — 3247 libavif reference PNGs
- `/mnt/v/output/zenavif/parse-failures/` — 45 previously-failing files (now fixed)
- `/mnt/v/output/zenavif/comparison-report.txt` — Full mismatch report
