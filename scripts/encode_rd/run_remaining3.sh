#!/usr/bin/env bash
# The remainder of the sweep, REORDERED so the decision-critical tier lands first.
#
# WHY THE ORDER CHANGED. run_full_sweep.sh runs cheapest-tier-first (64, 256,
# 1024a, 1024b) which is the right order for validating an instrument — you find
# out the grid works before spending the expensive hours. But once 64 has landed
# clean, cheapest-first is exactly backwards for RISK: it puts the tier the
# decision actually rests on last, so an interrupted run loses precisely the
# data that mattered.
#
# 1024 is that tier. It is where the wall clock is genuinely an encoder
# measurement (0-3% overhead on the slow rungs, vs 46-90% at 64 px — see
# overhead_share.py) and where all four arms have a wide, well-populated time
# overlap (103-1252 ms). So it goes first here, and 256 — already fully
# specified, 12 images, cheap — follows.
#
# Cost of the reorder: the ~6 minutes of 256 px encoding discarded when this
# replaced the in-flight tier. Paid once, deliberately.
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${OUT:-$HOME/tmp/encrd2}"
export RD_TOOL="${RD_TOOL:-$REPO/target/release/examples/rd_tool}"
cd "$REPO/scripts/encode_rd" || exit 1

ALL="clic2025/training/4cd6910a0b7b39365fda5df87618d091.png,clic2025/training/7e499613c3e376ea93afb3649719abeb.png,clic2025/training/ddcd24d99f48eaa369207882a6f37831.png,clic2025/training/14ab4af28901fbeb1356b06d2d08ae06.png,clic2025/training/1e2f9d41529197f10d32bfa68a1e0bcc.png,gb82-sc/codec_wiki.png,gb82-sc/gui.png,gb82-sc/terminal.png,qoi-benchmark/screenshot_web/en.wikipedia.org.png,gb82-sc/graph.png,CID22/CID22-512/training/Boxplot.png,CID22/CID22-512/training/newplot.png"
HALF_A="clic2025/training/4cd6910a0b7b39365fda5df87618d091.png,clic2025/training/ddcd24d99f48eaa369207882a6f37831.png,gb82-sc/codec_wiki.png,gb82-sc/graph.png"

LM256='zenrav1e:10,9,8,7,6,3;aom:9,8,7,6,5,4,3,2,0;svtc:9,8,7,6,5,4,3,2,0;svtrs:9,8,7,6,5,4,3,2,0'
LM1024='zenrav1e:10,9,8,7,6;aom:9,8,6,5,4,2;svtc:9,8,7,6,5,4,3,2,0;svtrs:9,8,7,6,5,4,3'
LMFIT='zenrav1e:9,8,6;aom:9,8,6;svtc:9,8,6;svtrs:9,8,6'

run () {
  local tag="$1" imgs="$2" sizes="$3" lm="$4"; shift 4
  [ -s "$OUT/cells_$tag.tsv" ] && { echo "=== skip $tag (already landed)"; return; }
  local t0; t0=$(date +%s)
  echo "=== $(date -u +%H:%M:%SZ) START $tag sizes=$sizes" | tee -a "$OUT/sweep.log"
  python3 run_grid.py --images "$imgs" --sizes "$sizes" \
      --arms aom,svtc,zenrav1e,svtrs --ladder-map "$lm" \
      --reps 5 --verify-yuv \
      --workdir "$OUT/work" --artifacts "$OUT/artifacts" \
      --progress "$OUT/prog_$tag.tsv" --out "$OUT/cells_$tag.tsv" \
      "$@" >>"$OUT/sweep.log" 2>&1
  echo "=== $(date -u +%H:%M:%SZ) END   $tag in $(( $(date +%s) - t0 ))s" | tee -a "$OUT/sweep.log"
}

# Order is by value per minute, not by size. The fit runs are the ONLY coverage
# of the 2048 and 4096 tiers the sweep discipline asks for and cost ~25 min
# between them; t256 is breadth at a size already covered by t64 and costs ~85.
run t1024a "$HALF_A" 1024 "$LM1024" --rate-stride 1
run fit_screen "qoi-benchmark/screenshot_web/creativecommons.org.png,qoi-benchmark/screenshot_web/reddit.com.png" \
    64,256,1024,2048,4096 "$LMFIT" --rate-stride 6
run fit_photo "clic2025/training/ddcd24d99f48eaa369207882a6f37831.png,clic2025/training/1e2f9d41529197f10d32bfa68a1e0bcc.png" \
    64,256,1024,2048 "$LMFIT" --rate-stride 6
run t256   "$ALL"    256  "$LM256"  --rate-stride 1
bash "$REPO/scripts/encode_rd/run_topup_1024.sh"
bash "$REPO/scripts/encode_rd/merge_and_analyze.sh" 2>&1 | tee -a "$OUT/sweep.log"
echo "=== $(date -u +%H:%M:%SZ) ALL DONE" | tee -a "$OUT/sweep.log"
