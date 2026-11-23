#!/bin/bash
set -e

VECTORS_DIR="tests/vectors"
mkdir -p "$VECTORS_DIR"

echo "Downloading AVIF test vectors..."

# 1. libavif (most comprehensive)
# Check the newer ICC gain-map and 12-bit negotiation fixtures as well:
# an older download can contain the basic gain map while missing these.
# The directory itself always exists because some tiny vectors are committed.
echo "1/3 Downloading libavif tests..."
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export TMPDIR="${TMPDIR:-$HOME/tmp}"
mkdir -p "$TMPDIR"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}" cargo run --locked --release --manifest-path "$ROOT/dev/prepare-test-vectors/Cargo.toml" -- "$ROOT/$VECTORS_DIR/libavif"

# 2. cavif-rs
echo "2/3 Downloading cavif-rs tests..."
if [ ! -d "$VECTORS_DIR/cavif" ]; then
    mkdir -p "$VECTORS_DIR/cavif"
    scratch=$(mktemp -d "$TMPDIR/zenavif-cavif.XXXXXX")
    git clone --depth=1 https://github.com/kornelski/cavif-rs.git "$scratch/cavif"
    find "$scratch/cavif" -name "*.avif" -exec cp -n {} "$VECTORS_DIR/cavif/" \;
    rm -rf "$scratch"
fi

# 3. Create simple test images using avif-parse test data
echo "3/3 Copying avif-parse test vectors..."
if [ ! -d "$VECTORS_DIR/avif-parse" ]; then
    mkdir -p "$VECTORS_DIR/avif-parse"
    # Use the workspace parser's own fixtures, including initialized submodules.
    find "$ROOT/zenavif-parse" -name "*.avif" -exec cp -n {} "$VECTORS_DIR/avif-parse/" \;
fi

echo "Done! Test vectors in $VECTORS_DIR"
find "$VECTORS_DIR" -name "*.avif" | wc -l | xargs echo "Total AVIF files:"
