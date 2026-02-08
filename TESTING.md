# Testing Strategy

## Current Test Coverage

### 1. Integration Tests ✅ (96.4%)

**Location:** `tests/integration_corpus.rs`

**What it tests:**
- Decoding succeeds without errors
- Correct image dimensions
- Correct pixel format (RGB8/RGBA8/RGB16/etc.)

**What it does NOT test:**
- ❌ Pixel accuracy (values could be wrong!)
- ❌ Color space correctness
- ❌ Alpha channel values
- ❌ Bit depth accuracy

**Result:** 53/55 files (96.4%)

**Coverage:**
```
✅ Single-frame AVIF     100%
✅ HDR/gainmap           100%
✅ Grid/tiles            100%
✅ Animated (1st frame)  100%
✅ idat construction     50% (2/4)
✅ Extended formats      100%
```

### 2. Pixel Verification ⚠️ (Not Implemented)

**Location:** `tests/pixel_verification.rs`

**Current Status:** Regression testing only
- References generated FROM zenavif
- Compares current output vs previous output
- Does NOT verify correctness against libavif

**Why this matters:**
Our decoder could be producing incorrect pixels and we wouldn't know!

---

## What We Need: True Pixel Verification

### Approach 1: Compare Against libavif (Recommended)

**Steps:**

1. **Install libavif tools:**
```bash
# Ubuntu/Debian
sudo apt-get install libavif-bin

# macOS
brew install libavif

# Build from source
git clone https://github.com/AOMediaCodec/libavif.git
cd libavif && mkdir build && cd build
cmake .. && make && sudo make install
```

2. **Generate reference images:**
```bash
# Create reference directory
mkdir -p tests/references/libavif

# For each test file, generate reference PNG
for f in tests/vectors/libavif/*.avif; do
    basename=$(basename "$f" .avif)
    avifdec "$f" "tests/references/libavif/${basename}.png"
done
```

3. **Run pixel comparison test:**
```bash
cargo test --features managed --test pixel_verification -- --ignored verify_against_libavif
```

### Approach 2: Use Existing Test Images with References

Some AVIF test suites include reference images:

- **Netflix AVIF Test Suite:** https://github.com/Netflix/avif-test
- **AOM Test Vectors:** https://people.xiph.org/~negge/avif-test/

These have:
- Source images (PNG/YUV)
- Encoded AVIF files
- Reference decoded outputs

### Approach 3: Perceptual Metrics

Instead of exact pixel comparison, use perceptual metrics:

```bash
# Using ImageMagick
compare -metric RMSE zenavif-output.png libavif-output.png diff.png

# Using SSIM (Structural Similarity)
compare -metric SSIM zenavif-output.png libavif-output.png diff.png

# Acceptable thresholds:
# - RMSE < 1.0 (very close)
# - SSIM > 0.99 (nearly identical)
```

---

## Implementation Plan

### Phase 1: Add libavif Reference Generation ✅

**Script:** `scripts/generate-libavif-references.sh`

```bash
#!/bin/bash
set -e

# Check if avifdec is available
if ! command -v avifdec &> /dev/null; then
    echo "Error: avifdec not found. Install libavif-bin."
    exit 1
fi

REF_DIR="tests/references/libavif"
mkdir -p "$REF_DIR"

# Generate references for all test vectors
for avif_file in tests/vectors/libavif/*.avif; do
    [ -f "$avif_file" ] || continue
    
    basename=$(basename "$avif_file" .avif)
    ref_file="$REF_DIR/${basename}.png"
    
    if [ ! -f "$ref_file" ]; then
        echo "Generating reference: $basename"
        avifdec "$avif_file" "$ref_file" || echo "  Failed: $basename"
    fi
done

echo "✓ Reference generation complete"
echo "  References: $REF_DIR"
echo "  Count: $(ls -1 $REF_DIR/*.png 2>/dev/null | wc -l)"
```

### Phase 2: Update Pixel Verification Test

**Add to:** `tests/pixel_verification.rs`

```rust
#[test]
#[ignore]
fn verify_against_libavif() {
    let reference_dir = Path::new("tests/references/libavif");
    
    if !reference_dir.exists() {
        eprintln!("⚠️  No libavif references found!");
        eprintln!("Run: bash scripts/generate-libavif-references.sh");
        return;
    }
    
    let vectors = find_test_vectors();
    let config = DecoderConfig::new().threads(1);
    
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    
    for avif_path in vectors {
        let basename = avif_path.file_stem().unwrap().to_str().unwrap();
        let ref_path = reference_dir.join(format!("{}.png", basename));
        
        if !ref_path.exists() {
            skipped += 1;
            continue;
        }
        
        eprint!("  {:50} ", basename);
        
        let data = fs::read(&avif_path).unwrap();
        match decode_with(&data, &config, &Unstoppable) {
            Ok(image) => {
                match compare_against_reference(&image, &ref_path, 1) {
                    Ok(true) => {
                        eprintln!("✓ Matches libavif");
                        passed += 1;
                    },
                    Ok(false) => {
                        eprintln!("✗ Pixel mismatch");
                        failed += 1;
                    },
                    Err(e) => {
                        eprintln!("✗ Compare error: {}", e);
                        failed += 1;
                    }
                }
            },
            Err(e) => {
                eprintln!("✗ Decode failed: {}", e);
                failed += 1;
            }
        }
    }
    
    eprintln!("\n📊 Pixel Accuracy vs libavif:");
    eprintln!("  Matches:  {}", passed);
    eprintln!("  Mismatch: {}", failed);
    eprintln!("  Skipped:  {}", skipped);
    
    assert_eq!(failed, 0, "Pixel verification failed");
}
```

### Phase 3: CI Integration

**Add to:** `.github/workflows/test.yml`

```yaml
  pixel-verification:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      
      - name: Install libavif
        run: |
          sudo apt-get update
          sudo apt-get install -y libavif-bin
      
      - name: Generate libavif references
        run: bash scripts/generate-libavif-references.sh
      
      - name: Run pixel verification
        run: cargo test --release --features managed --test pixel_verification -- --ignored verify_against_libavif
```

---

## Expected Results

Once implemented, we should see:

**Best case:** 100% pixel-perfect match
```
✓ All 53 decodeable files match libavif exactly
```

**Realistic case:** Some rounding differences
```
✓ 50 files match exactly (94%)
⚠️  3 files have minor differences (< 1 RMSE)
✗ 0 files have significant differences
```

**If we find issues:**
- Investigate color space conversion
- Check YUV → RGB formula
- Verify chroma upsampling
- Check alpha premultiplication

---

## Manual Verification (Quick Check)

For a quick sanity check without automation:

```bash
# Pick a test file
TEST_FILE="tests/vectors/libavif/colors-profile2-420-8-094.avif"

# Decode with libavif
avifdec "$TEST_FILE" /tmp/libavif-output.png

# Decode with zenavif (need to create example first)
cargo run --example decode_to_png "$TEST_FILE" /tmp/zenavif-output.png

# Visual comparison
feh /tmp/libavif-output.png /tmp/zenavif-output.png

# Metric comparison
compare -metric RMSE /tmp/libavif-output.png /tmp/zenavif-output.png /tmp/diff.png
# RMSE should be < 1.0 for a good match

# View differences (amplified)
display /tmp/diff.png
```

---

## Current Status

**What we know:**
- ✅ 96.4% of files decode without errors
- ✅ Dimensions are correct
- ✅ Formats are correct
- ❓ Pixel values are unknown (not verified)

**What we need:**
- ❌ libavif reference images
- ❌ Pixel comparison test
- ❌ CI automation

**Priority:** High - this is essential for production use!

---

## References

- libavif: https://github.com/AOMediaCodec/libavif
- AV1 Codec Test Vectors: https://people.xiph.org/~negge/
- AVIF Spec: https://aomediacodec.github.io/av1-avif/
- YUV→RGB Conversion: ITU-R BT.709, BT.2020

