#!/usr/bin/env bash
# DFIT5 driver: per-committed-block metric/feature maps for every corpus cell,
# decoding from the PERSISTED IVFs (no re-encoding). Emits blockmap_*.tsv
# beside the traces.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
D="${1:?trace dir}"
CORPUS="${CORPUS:-/mnt/v/output/rd-gap-train26-2026-07-02}"
AOMDEC="${AOMDEC:-/home/lilith/work/aom/build_butteraugli/aomdec}"
BLOCKMAP="${BLOCKMAP:-/home/lilith/work/zen/zenavif/target/release/examples/butteraugli_blockmap}"
TMP=$(mktemp -d /tmp/blockmaps.XXXX)
trap 'rm -rf "$TMP"' EXIT
n=0
tail -n +2 "$D/manifest.tsv" | while IFS=$'\t' read -r image fam speed q trace rows bytes ssim2 mse; do
  out="$D/blockmap_${image}_s${speed}_q${q}.tsv"
  [ -s "$out" ] && continue
  src="$CORPUS/${image}.png"
  ivf="$D/ivf/${image}_q${q}.ivf"
  [ -f "$src" ] && [ -f "$ivf" ] || { echo "missing inputs for $image q$q" >&2; continue; }
  read -r w h < <(identify -format "%w %h" "$src")
  w=$((w & ~1)); h=$((h & ~1))
  yuv="$TMP/d.yuv"; dec="$TMP/d.png"; srce="$TMP/s.png"
  convert "$src" -crop "${w}x${h}+0+0" +repage "$srce"
  "$AOMDEC" --rawvideo -o "$yuv" "$ivf" >/dev/null 2>&1 || { echo "decode fail $image q$q" >&2; continue; }
  python3 "$HERE/yuv_to_png.py" "$yuv" "$w" "$h" "$dec" || { echo "inverse fail $image q$q" >&2; continue; }
  "$BLOCKMAP" "$srce" "$dec" "$D/$trace" "$out" 2>/dev/null || { echo "blockmap fail $image q$q" >&2; continue; }
  n=$((n+1)); [ $((n % 20)) -eq 0 ] && echo "[blockmaps] $n done"
done
ls "$D"/blockmap_*.tsv | wc -l | sed 's/^/[blockmaps] total: /'
