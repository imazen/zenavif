#!/usr/bin/env bash
# Differential check of the `media` AV1 sessions against FFmpeg (libdav1d,
# libaom), both directions:
#  1. zenrav1e (Av1VideoEncoder) encodes FFmpeg's testsrc2 (odd and even
#     sizes) in each pixel
#     format; libdav1d must decode the IVF to the source samples exactly
#     (lossless, quantizer 0) and to exactly what Av1VideoDecoder decodes
#     (lossy, quantizer 120, default speed: reordered hidden frames).
#  2. libaom encodes the same sources with alt-ref reordering; Av1VideoDecoder
#     must decode every frame bit-exactly as libdav1d does, with the same
#     frame count and presentation timestamps as ffprobe reports.
# Needs ffmpeg/ffprobe with libdav1d and libaom, and cargo.
# Usage: scripts/media-ffmpeg-oracle.sh [work-dir]
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
work=${1:-$here/target/media-ffmpeg-oracle}
mkdir -p "$work"
cd "$work"
ff() { ffmpeg -v error -y "$@"; }
(cd "$here" && cargo build -q --release --features media-encode --example media_ivf)
tool=$(cd "$here" && cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/release/examples/media_ivf
fail=0
check() { # name, a, b
  if cmp -s "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 ($2 vs $3)"; fail=1; fi
}
dav1d() { ff -c:v libdav1d -i "$1" -f rawvideo -pix_fmt "$2" "$3"; }
for spec in gray:64x48 yuv420p:33x17 yuv422p:40x30 yuv444p:24x18 \
            yuv420p10le:48x32 yuv444p10le:17x13 gray10le:20x20 \
            yuv420p12le:32x24 yuv422p12le:18x10 yuv444p12le:16x16; do
  fmt=${spec%%:*}; size=${spec#*:}; w=${size%x*}; h=${size#*x}
  src=$fmt.src.yuv
  # testsrc2 rounds odd sizes to the chroma grid; scaling keeps them exact.
  ff -f lavfi -i "testsrc2=size=96x64:rate=25" -vf "scale=$w:$h" -frames:v 12 \
    -pix_fmt "$fmt" -f rawvideo "$src"
  # 1. our encoder -> dav1d (and our decoder).
  "$tool" encode "$src" "$fmt" "$w" "$h" "$fmt.q0.ivf" 0 10
  dav1d "$fmt.q0.ivf" "$fmt" "$fmt.q0.dav1d.yuv"
  check "$fmt zenrav1e lossless -> dav1d = source" "$src" "$fmt.q0.dav1d.yuv"
  "$tool" encode "$src" "$fmt" "$w" "$h" "$fmt.q120.ivf" 120 6
  dav1d "$fmt.q120.ivf" "$fmt" "$fmt.q120.dav1d.yuv"
  "$tool" decode "$fmt.q120.ivf" "$fmt.q120.zen.yuv" > "$fmt.q120.zen.pts"
  check "$fmt zenrav1e lossy: rav1d-safe = dav1d" "$fmt.q120.dav1d.yuv" "$fmt.q120.zen.yuv"
  # 2. libaom -> our decoder.
  case $fmt in
    gray*) ;; # libaom takes no gray input
    *)
      ff -f rawvideo -pix_fmt "$fmt" -s "$size" -r 25 -i "$src" -c:v libaom-av1 -cpu-used 6 \
        -crf 40 -g 8 -lag-in-frames 10 -auto-alt-ref 1 "$fmt.aom.ivf"
      dav1d "$fmt.aom.ivf" "$fmt" "$fmt.aom.dav1d.yuv"
      "$tool" decode "$fmt.aom.ivf" "$fmt.aom.zen.yuv" > "$fmt.aom.zen.pts"
      check "$fmt libaom: rav1d-safe = dav1d" "$fmt.aom.dav1d.yuv" "$fmt.aom.zen.yuv"
      ffprobe -v error -select_streams v -show_entries frame=pts -of csv=p=0 "$fmt.aom.ivf" \
        > "$fmt.aom.ffprobe.pts"
      cut -d' ' -f1 "$fmt.aom.zen.pts" > "$fmt.aom.zen.ptsonly"
      check "$fmt libaom: frame pts = ffprobe" "$fmt.aom.ffprobe.pts" "$fmt.aom.zen.ptsonly"
      ;;
  esac
done
exit $fail
