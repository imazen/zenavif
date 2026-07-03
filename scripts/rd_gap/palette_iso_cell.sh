#!/usr/bin/env bash
# One isolated-config palette A/B cell: rav1e CLI on a color-exact 420 y4m,
# aomdec decode, rav1d-safe byte-agreement, color-exact inverse, ssim2 +
# butteraugli scoring. Reconstructs the palette-ab-final2 pipeline
# (benchmarks/palette_ab_train26_2026-07-03.pointer.md) as a committed script;
# the isolated config is `--still-picture --threads 1 --lrf false
# --filter-intra false` per zenrav1e#32/#33 isolation.
#
# Args:  IMG Y4M SPEED Q ARM TMP     (Y4M pre-converted by the driver)
# Env:   RAV1E AOMDEC IVF_RAW SCORER COLOR  required; BUTTER optional
# Emits: image  speed  q  arm  bytes  enc_ms  ssim2  butter_max  butter_p3  md5_agree
#   (the palette-ab-final2 results.tsv column convention; image = basename
#    without .png so label-store ingestion stays uniform)
#
# Conformance (zero-corruption bar): aomdec must decode every cell; palette-
# armed cells (arm != off) must additionally byte-agree raw-I420 between
# aomdec and rav1d-safe (ivf_raw example). Any failure fails the cell loudly.
set -uo pipefail
RAV1E="${RAV1E:?set RAV1E to the zenrav1e rav1e CLI}"
AOMDEC="${AOMDEC:?set AOMDEC}"
IVF_RAW="${IVF_RAW:?set IVF_RAW to the zenavif ivf_raw example}"
SCORER="${SCORER:?set SCORER to fast-ssim2-cli}"
COLOR="${COLOR:?set COLOR to scripts/rd_gap/color.py}"

IMG="$1"; Y4M="$2"; SPEED="$3"; Q="$4"; ARM="$5"; TMP="$6"
base=$(basename "$IMG" .png)
cell="$TMP/${base}.s${SPEED}.q${Q}.${ARM}"
ivf="$cell.ivf"

source "$(dirname "${BASH_SOURCE[0]}")/cell_cache.sh"
rd_cache_row_key "$RAV1E" "$IMG" "pal_iso" "s=$SPEED" "q=$Q" "arm=$ARM" "extra=${EXTRA_RAV1E:-}" "butter=${BUTTER:+on}"
if row=$(rd_cache_row_get); then printf '%s\n' "$row"; exit 0; fi

t0=$(date +%s.%N)
# EXTRA_RAV1E: additive passthrough for extra encoder flags (e.g.
# --intrabc); default empty keeps historical byte behavior. It is part of
# the cell cache key via the arm/key line below when callers encode it in
# ARM naming; standalone use MUST use a fresh cache dir or RD_CACHE=off.
"$RAV1E" "$Y4M" --still-picture --threads 1 --lrf false --filter-intra false \
  -s "$SPEED" --quantizer "$Q" --palette "$ARM" ${EXTRA_RAV1E:-} -o "$ivf" -y > "$cell.enc.log" 2>&1
rc=$?; t1=$(date +%s.%N)
enc_ms=$(python3 -c "print(f'{($t1-$t0)*1000:.1f}')")
{ [ $rc -ne 0 ] || [ ! -s "$ivf" ]; } && { echo "ENCFAIL $base s$SPEED q$Q $ARM rc=$rc"; exit 1; }
bytes=$(stat -c%s "$ivf")

# Reference-decoder decode (conformance + the scoring pixels).
"$AOMDEC" -o "$cell.dec.y4m" "$ivf" > /dev/null 2>&1 \
  || { echo "AOMDECFAIL $base s$SPEED q$Q $ARM"; exit 1; }

# Byte agreement aomdec vs rav1d-safe on palette-armed cells.
md5_agree="na"
if [ "$ARM" != "off" ]; then
  "$AOMDEC" --rawvideo -o "$cell.aom.raw" "$ivf" > /dev/null 2>&1 \
    || { echo "AOMRAWFAIL $base s$SPEED q$Q $ARM"; exit 1; }
  "$IVF_RAW" "$ivf" "$cell.rav1d.raw" > /dev/null 2>&1 \
    || { echo "RAV1DFAIL $base s$SPEED q$Q $ARM"; exit 1; }
  a=$(md5sum < "$cell.aom.raw" | cut -d' ' -f1)
  b=$(md5sum < "$cell.rav1d.raw" | cut -d' ' -f1)
  rm -f "$cell.aom.raw" "$cell.rav1d.raw"
  if [ "$a" = "$b" ]; then md5_agree="yes"; else
    echo "MD5DISAGREE $base s$SPEED q$Q $ARM aom=$a rav1d=$b"; exit 1
  fi
fi

# Color-exact inverse back to RGB, then score against the source PNG.
python3 "$COLOR" from_y4m "$cell.dec.y4m" 420 "$IMG" "$cell.dec.png" > /dev/null 2>&1 \
  || { echo "FROMY4MFAIL $base s$SPEED q$Q $ARM"; exit 1; }
ss=$("$SCORER" image "$IMG" "$cell.dec.png" 2>/dev/null | grep -oE '[0-9.]+' | head -1)
[ -z "$ss" ] && { echo "SCOREFAIL $base s$SPEED q$Q $ARM"; exit 1; }

b3="NA"; bmax="NA"
if [ -n "${BUTTER:-}" ]; then
  bout=$("$BUTTER" --json "$IMG" "$cell.dec.png" 2>/dev/null | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
    print("%.6f %.6f" % (d["pnorm_3"], d["score"]))
except Exception:
    print("NA NA")')
  b3="${bout%% *}"; bmax="${bout##* }"
  [ -z "$b3" ] && b3="NA"; [ -z "$bmax" ] && bmax="NA"
fi

# Persist the encoded stream (content-addressed by cell name) per the
# always-persist-encodes rule; IVF_KEEP_DIR is set by the driver.
if [ -n "${IVF_KEEP_DIR:-}" ]; then
  mkdir -p "$IVF_KEEP_DIR"
  cp -f "$ivf" "$IVF_KEEP_DIR/${base}-s${SPEED}-q${Q}-${ARM}.ivf"
fi
rm -f "$ivf" "$cell.dec.y4m" "$cell.dec.png"

row=$(printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s' \
  "$base" "$SPEED" "$Q" "$ARM" "$bytes" "$enc_ms" "$ss" "$bmax" "$b3" "$md5_agree")
rd_cache_row_put "$row"
printf '%s\n' "$row"
