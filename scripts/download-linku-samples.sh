#!/bin/bash
set -e

VECTORS_DIR="${1:-tests/vectors/link-u}"

if [ -d "$VECTORS_DIR" ] && [ "$(find "$VECTORS_DIR" -name '*.avif' 2>/dev/null | wc -l)" -gt 100 ]; then
    echo "link-u samples already present in $VECTORS_DIR ($(find "$VECTORS_DIR" -name '*.avif' | wc -l) files)"
    exit 0
fi

echo "Downloading link-u/avif-sample-images..."
mkdir -p "$VECTORS_DIR"

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

git clone --depth=1 https://github.com/link-u/avif-sample-images.git "$TMPDIR/link-u"

# Copy all .avif and .avifs (animated) files
find "$TMPDIR/link-u" -maxdepth 1 \( -name "*.avif" -o -name "*.avifs" \) -exec cp {} "$VECTORS_DIR/" \;

COUNT=$(find "$VECTORS_DIR" -name '*.avif' -o -name '*.avifs' | wc -l)
echo "Downloaded $COUNT AVIF files to $VECTORS_DIR"
