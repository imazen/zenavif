#!/usr/bin/env bash
# One SVT-AV1 reference cell for the G2 ladder (GOAL_PARETO_FRONT): encode a
# still (--avif 1, single frame) at (preset, crf, tune), decode, score through
# the OWNED color path (png_to_y4m.py forward / yuv_to_png.py inverse — the
# same BT.601-full matrix the zenrav1e trace corpus uses), and emit one TSV row:
#   image family encoder fmt preset crf tune bytes ssim2 mse enc_ms
#
# Usage: svt_cell.sh IMG.png FAMILY PRESET CRF TUNE TMPDIR
# Env: SVTENC (default ~/work/zen/svtav1-v4.1.0), AOMDEC, SCORER.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
IMG="$1"; FAM="$2"; PRESET="$3"; CRF="$4"; TUNE="$5"; TMP="$6"
SVTENC="${SVTENC:-/home/lilith/work/zen/svtav1-v4.1.0/Bin/Release/SvtAv1EncApp}"
AOMDEC="${AOMDEC:-/home/lilith/work/aom/build_butteraugli/aomdec}"
SCORER="${SCORER:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}"
# Optional butteraugli CLI (3-norm + max for the objective.py veto columns).
BUTTER="${BUTTER:-/home/lilith/work/butteraugli/target/release/butteraugli}"

base=$(basename "$IMG" .png)
y4m="$TMP/${base}.y4m"
[ -f "$y4m" ] || python3 "$HERE/png_to_y4m.py" "$IMG" "$y4m" || exit 3
# even-cropped source png for scoring (same crop as the forward)
srcpng="$TMP/${base}.src.png"
if [ ! -f "$srcpng" ]; then
  read -r pw ph < <(identify -format "%w %h" "$IMG")
  ew=$((pw & ~1)); eh=$((ph & ~1))
  convert "$IMG" -crop "${ew}x${eh}+0+0" +repage "$srcpng"
fi
read -r ew eh < <(identify -format "%w %h" "$srcpng")

ivf="$TMP/${base}_p${PRESET}_c${CRF}_t${TUNE}.ivf"
t0=$(date +%s%N)
# SOLO WALL by default (GOAL_PARETO scoring convention): SVT defaults to all
# logical processors, which measured 3.4x faster walls than --lp 1 on p2 —
# not comparable to the single-threaded cavif cells. LP env overrides.
LP="${LP:---lp 1}"
"$SVTENC" -i "$y4m" -b "$ivf" --avif 1 -n 1 --preset "$PRESET" \
  --crf "$CRF" --tune "$TUNE" $LP --progress 0 >/dev/null 2>&1 || exit 4
enc_ms=$(( ($(date +%s%N) - t0) / 1000000 ))
bytes=$(stat -c%s "$ivf")

yuv="$TMP/${base}_p${PRESET}_c${CRF}_t${TUNE}.yuv"
dpng="$TMP/${base}_p${PRESET}_c${CRF}_t${TUNE}.png"
"$AOMDEC" --rawvideo -o "$yuv" "$ivf" >/dev/null 2>&1 || exit 5
mse=$(python3 "$HERE/yuv_to_png.py" "$yuv" "$ew" "$eh" "$dpng" "$srcpng" 2>/dev/null \
      | grep -oE 'mse [0-9.]+' | cut -d' ' -f2)
ssim2=$("$SCORER" image "$srcpng" "$dpng" 2>/dev/null | grep -oE '[-0-9.]+' | head -1)
ba3=NA; bamax=NA
if [ -x "$BUTTER" ]; then
  bj=$("$BUTTER" --json "$srcpng" "$dpng" 2>/dev/null)
  ba3=$(echo "$bj" | python3 -c 'import json,sys
d=json.load(sys.stdin); print(d.get("pnorm_3", d.get("3-norm","NA")))' 2>/dev/null)
  bamax=$(echo "$bj" | python3 -c 'import json,sys
d=json.load(sys.stdin); print(d.get("score", d.get("max","NA")))' 2>/dev/null)
fi
rm -f "$yuv" "$dpng" "$ivf"
# run_gap schema (objective.py-ready): image w h family encoder fmt q bytes
# bpp ssim2 enc_ms butteraugli_3n butteraugli_max — q carries "p<preset>c<crf>t<tune>".
px=$((ew * eh))
bpp=$(python3 -c "print(f'{$bytes*8/$px:.5f}')")
echo -e "$base\t$ew\t$eh\t$FAM\tsvt-av1-v4.1.0\tp${PRESET}t${TUNE}\t$CRF\t$bytes\t$bpp\t${ssim2:-NA}\t$enc_ms\t${ba3:-NA}\t${bamax:-NA}"
