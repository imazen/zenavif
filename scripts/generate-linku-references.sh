#!/bin/bash
set -e

VECTORS_DIR="${VECTORS_DIR:-/vectors}"
REFERENCES_DIR="${REFERENCES_DIR:-/references}"

echo "=== link-u AVIF Reference Generation using libavif ==="
echo "Vectors: $VECTORS_DIR"
echo "References: $REFERENCES_DIR"

if ! command -v avifdec &> /dev/null; then
    echo "ERROR: avifdec not found"
    exit 1
fi

echo "Using: $(avifdec --version 2>&1 || true)"
echo ""

mkdir -p "$REFERENCES_DIR"

decoded=0
failed=0
skipped=0
failed_files=""

for avif_file in "$VECTORS_DIR"/*.avif "$VECTORS_DIR"/*.avifs; do
    [ -f "$avif_file" ] || continue

    basename=$(basename "$avif_file")
    # Strip both .avif and .avifs extensions
    stem="${basename%.avifs}"
    stem="${stem%.avif}"
    ref_file="$REFERENCES_DIR/${stem}.png"

    if [ -f "$ref_file" ]; then
        skipped=$((skipped + 1))
        continue
    fi

    # avifdec writes 16-bit PNGs for >8bpc input by default
    if avifdec --jobs 1 "$avif_file" "$ref_file" >/dev/null 2>&1; then
        echo "  OK: $basename"
        decoded=$((decoded + 1))
    else
        echo "  FAIL: $basename"
        failed=$((failed + 1))
        failed_files="$failed_files $basename"
        rm -f "$ref_file"
    fi
done

echo ""
echo "=== Done ==="
echo "  Decoded: $decoded"
echo "  Failed:  $failed"
echo "  Skipped: $skipped (already exist)"
if [ -n "$failed_files" ]; then
    echo "  Failed files:$failed_files"
fi
