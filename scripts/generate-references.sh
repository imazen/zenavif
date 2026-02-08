#!/bin/bash
set -e

VECTORS_DIR="/vectors"
REFERENCES_DIR="/references"

echo "=== AVIF Reference Generation using libavif ==="
echo "Vectors directory: $VECTORS_DIR"
echo "References directory: $REFERENCES_DIR"
echo ""

# Check if avifdec is available
if ! command -v avifdec &> /dev/null; then
    echo "ERROR: avifdec not found. Install libavif-bin or build libavif."
    exit 1
fi

# Show avifdec version
echo "Using: $(avifdec --version)"
echo ""

# Create references directory if it doesn't exist
mkdir -p "$REFERENCES_DIR"

# Build array of AVIF files
mapfile -t avif_files < <(find "$VECTORS_DIR" -type f -name "*.avif" | sort)

total_files=${#avif_files[@]}
echo "Found $total_files AVIF files to process"
echo ""

# Track statistics
decoded=0
failed=0
skipped=0

# Process all AVIF files
for avif_file in "${avif_files[@]}"; do
    # Get basename without extension
    basename=$(basename "$avif_file" .avif)
    ref_file="$REFERENCES_DIR/${basename}.png"

    # Skip if reference already exists
    if [ -f "$ref_file" ]; then
        echo "  SKIP: $basename (reference exists)"
        skipped=$((skipped + 1))
        continue
    fi

    # Decode with avifdec
    echo "  DECODE: $basename"
    if avifdec "$avif_file" "$ref_file" >/dev/null 2>&1; then
        echo "    SUCCESS: ${basename}.png"
        decoded=$((decoded + 1))
    else
        echo "    FAILED: $basename"
        failed=$((failed + 1))
        # Remove partial file
        rm -f "$ref_file"
    fi
done

echo ""
echo "=== Generation Complete ==="
echo "  Decoded: $decoded"
echo "  Failed:  $failed"
echo "  Skipped: $skipped (already exist)"
echo "  Total:   $total_files"
echo ""
echo "References saved to: $REFERENCES_DIR"
