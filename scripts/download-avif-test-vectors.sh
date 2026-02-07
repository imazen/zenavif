#!/bin/bash
set -e

VECTORS_DIR="tests/vectors"
mkdir -p "$VECTORS_DIR"

echo "Downloading AVIF test vectors..."

# 1. libavif (most comprehensive)
echo "1/3 Downloading libavif tests..."
if [ ! -d "$VECTORS_DIR/libavif" ]; then
    mkdir -p "$VECTORS_DIR/libavif"
    git clone --depth=1 https://github.com/AOMediaCodec/libavif.git /tmp/libavif-tests
    find /tmp/libavif-tests/tests -name "*.avif" -exec cp {} "$VECTORS_DIR/libavif/" \; 2>/dev/null || true
    rm -rf /tmp/libavif-tests
fi

# 2. cavif-rs
echo "2/3 Downloading cavif-rs tests..."
if [ ! -d "$VECTORS_DIR/cavif" ]; then
    mkdir -p "$VECTORS_DIR/cavif"
    git clone --depth=1 https://github.com/kornelski/cavif-rs.git /tmp/cavif
    find /tmp/cavif -name "*.avif" -exec cp {} "$VECTORS_DIR/cavif/" \; 2>/dev/null || true
    rm -rf /tmp/cavif
fi

# 3. Create simple test images using avif-parse test data
echo "3/3 Copying avif-parse test vectors..."
if [ ! -d "$VECTORS_DIR/avif-parse" ]; then
    mkdir -p "$VECTORS_DIR/avif-parse"
    # Copy any AVIF files from avif-parse crate's test data
    AVIF_PARSE_DIR=$(find ~/.cargo/registry/src -name "avif-parse-*" -type d | head -1)
    if [ -n "$AVIF_PARSE_DIR" ]; then
        find "$AVIF_PARSE_DIR" -name "*.avif" -exec cp {} "$VECTORS_DIR/avif-parse/" \; 2>/dev/null || true
    fi
fi

echo "Done! Test vectors in $VECTORS_DIR"
find "$VECTORS_DIR" -name "*.avif" | wc -l | xargs echo "Total AVIF files:"
