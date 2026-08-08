#!/usr/bin/env bash
# Stop the main sweep at a tier boundary and still finish the deliverable.
#
# The RD grid is ordered cheapest-tier-first, so an early stop costs whole
# IMAGES at the largest size rather than a whole size or a whole quality range —
# which is the axis the sweep discipline says to give up first. This makes that
# choice explicit and recorded instead of a silent truncation.
#
# It kills only the driver loop, waits for the in-flight run_grid.py to write
# its TSV (so the tier in progress is not lost), then runs the cheap pieces that
# still matter — the 2048/4096 size-model runs and the 1024 frontier top-up —
# and the full reduction.
#
#     bash cut_and_finish.sh [--skip-topup] [--skip-fit]
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${OUT:-$HOME/tmp/encrd2}"
export OUT RD_TOOL="${RD_TOOL:-$REPO/target/release/examples/rd_tool}"
SKIP_TOPUP=0; SKIP_FIT=0
for a in "$@"; do
  [ "$a" = "--skip-topup" ] && SKIP_TOPUP=1
  [ "$a" = "--skip-fit" ] && SKIP_FIT=1
done

echo "=== $(date -u +%H:%M:%SZ) CUT: stopping the driver loop" | tee -a "$OUT/sweep.log"
pkill -f run_full_sweep.sh 2>/dev/null
# Let the in-flight run_grid.py finish and write its tier; killing it would
# throw away every encode it has already paid for.
if pgrep -f 'run_grid.py' >/dev/null 2>&1; then
  echo "    waiting for the in-flight tier to write its TSV (not killing it)" \
    | tee -a "$OUT/sweep.log"
  while pgrep -f 'run_grid.py' >/dev/null 2>&1; do sleep 20; done
fi
echo "=== $(date -u +%H:%M:%SZ) CUT: driver stopped; tiers present:" | tee -a "$OUT/sweep.log"
find "$OUT" -maxdepth 1 -name 'cells_*.tsv' ! -name '*floor*' -exec basename {} \; \
  | sort | tee -a "$OUT/sweep.log"

cd "$REPO/scripts/encode_rd" || exit 1
LMFIT='zenrav1e:9,8,6;aom:9,8,6;svtc:9,8,6;svtrs:9,8,6'
runfit () {
  local tag="$1" imgs="$2" sizes="$3"
  echo "=== $(date -u +%H:%M:%SZ) START $tag" | tee -a "$OUT/sweep.log"
  python3 run_grid.py --images "$imgs" --sizes "$sizes" \
    --arms aom,svtc,zenrav1e,svtrs --ladder-map "$LMFIT" \
    --rate-stride 6 --reps 5 --verify-yuv \
    --workdir "$OUT/work" --artifacts "$OUT/artifacts" \
    --progress "$OUT/prog_$tag.tsv" --out "$OUT/cells_$tag.tsv" >>"$OUT/sweep.log" 2>&1
  echo "=== $(date -u +%H:%M:%SZ) END   $tag" | tee -a "$OUT/sweep.log"
}
if [ $SKIP_FIT -eq 0 ]; then
  [ -s "$OUT/cells_fit_photo.tsv" ] || runfit fit_photo \
    "clic2025/training/ddcd24d99f48eaa369207882a6f37831.png,clic2025/training/1e2f9d41529197f10d32bfa68a1e0bcc.png" \
    64,256,1024,2048
  [ -s "$OUT/cells_fit_screen.tsv" ] || runfit fit_screen \
    "qoi-benchmark/screenshot_web/creativecommons.org.png,qoi-benchmark/screenshot_web/reddit.com.png" \
    64,256,1024,2048,4096
fi
[ $SKIP_TOPUP -eq 0 ] && bash "$REPO/scripts/encode_rd/run_topup_1024.sh"
bash "$REPO/scripts/encode_rd/merge_and_analyze.sh" 2>&1 | tee -a "$OUT/sweep.log"
echo "=== $(date -u +%H:%M:%SZ) PIPELINE COMPLETE (CUT)" | tee -a "$OUT/sweep.log"
