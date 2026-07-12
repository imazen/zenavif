#!/usr/bin/env bash
# Multi-scale sbmap passes from the persisted IVFs (decode-only): per cell,
# butteraugli_sbmap at SBSIZE px -> sbmap<SBSIZE>_*.tsv beside the traces.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
D="${1:?trace dir}"; SBSIZE="${2:?sb size}"
CORPUS="${CORPUS:-/mnt/v/output/rd-gap-train26-2026-07-02}"
AOMDEC="${AOMDEC:-/home/lilith/work/aom/build_butteraugli/aomdec}"
SBMAP="${SBMAP:-/home/lilith/work/zen/zenavif/target/release/examples/butteraugli_sbmap}"
TMP=$(mktemp -d /tmp/sbmaps.XXXX); trap 'rm -rf "$TMP"' EXIT
n=0
tail -n +2 "$D/manifest.tsv" | while IFS=$'\t' read -r image fam speed q trace rows bytes ssim2 mse; do
  out="$D/sbmap${SBSIZE}_${image}_s${speed}_q${q}.tsv"
  [ -s "$out" ] && continue
  src="$CORPUS/${image}.png"; ivf="$D/ivf/${image}_q${q}.ivf"
  [ -f "$src" ] && [ -f "$ivf" ] || continue
  read -r w h < <(identify -format "%w %h" "$src"); w=$((w & ~1)); h=$((h & ~1))
  convert "$src" -crop "${w}x${h}+0+0" +repage "$TMP/s.png"
  "$AOMDEC" --rawvideo -o "$TMP/d.yuv" "$ivf" >/dev/null 2>&1 || continue
  python3 "$HERE/yuv_to_png.py" "$TMP/d.yuv" "$w" "$h" "$TMP/d.png" || continue
  "$SBMAP" "$TMP/s.png" "$TMP/d.png" "$SBSIZE" "$out" 2>/dev/null || echo "fail $image q$q" >&2
  n=$((n+1)); [ $((n % 30)) -eq 0 ] && echo "[sbmaps$SBSIZE] $n done"
done
ls "$D"/sbmap${SBSIZE}_*.tsv 2>/dev/null | wc -l | sed "s/^/[sbmaps$SBSIZE] total: /"
