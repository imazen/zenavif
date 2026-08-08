#!/usr/bin/env bash
# Frontier top-up at 1024 px: the slow rungs the main grid could not afford at
# full rate density, measured at reduced rate density instead of not at all.
#
# WHY. The main 1024 ladder was chosen from the sizing probe's four-way time
# overlap, which is the right criterion for the CENTRE of the comparison but
# truncates each arm's frontier at the top. Re-reading the probe's (time,bytes)
# Pareto frontier at 1024 shows what the truncation costs:
#
#     zenrav1e frontier rungs 10, 9, 8, 3   (main grid has 10,9,8,7,6 — misses s3)
#     aom      frontier rungs 9,8,7,5,4,3   (main grid misses 7 and 3)
#     svtc     frontier rungs 10,6,5,4,2,0  (main grid has all of them)
#     svtrs    frontier rungs 10,6,5,4,2,0  (main grid misses 2 and 0)
#
# Those are measured at ONE rate (each arm's knob = 31), so the dominance is
# rate-specific and must not be over-generalised — the full-grid analysis
# re-derives the frontier at each quality target. But it is enough to show the
# main grid ends zenrav1e's frontier at ~0.5 s while the arm can usefully spend
# 6.8 s, and a matched-time comparison that stops there cannot answer "is the
# extra spend worth it".
#
# Cost is why these are separate: zenrav1e s3 is 6.8 s and svtrs p0 is 7.9 s per
# encode at 1024, i.e. ~2 h per image each at the main grid's 20 rate points.
# At rate-stride 4 (6 points) they fit in ~35 minutes total. Fewer rate points
# per rung is the right thing to give up here — the analysis interpolates on
# achieved quality, so a rung needs to SPAN the range, not sample it densely.

set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${OUT:-$HOME/tmp/encrd2}"
export RD_TOOL="${RD_TOOL:-$REPO/target/release/examples/rd_tool}"
cd "$REPO/scripts/encode_rd" || exit 1

HALF_A="clic2025/training/4cd6910a0b7b39365fda5df87618d091.png,clic2025/training/7e499613c3e376ea93afb3649719abeb.png,clic2025/training/ddcd24d99f48eaa369207882a6f37831.png,gb82-sc/codec_wiki.png,gb82-sc/gui.png,gb82-sc/graph.png"
HALF_B="clic2025/training/14ab4af28901fbeb1356b06d2d08ae06.png,clic2025/training/1e2f9d41529197f10d32bfa68a1e0bcc.png,gb82-sc/terminal.png,qoi-benchmark/screenshot_web/en.wikipedia.org.png,CID22/CID22-512/training/Boxplot.png,CID22/CID22-512/training/newplot.png"

run () {
  local tag="$1" imgs="$2" lm="$3"; shift 3
  local t0; t0=$(date +%s)
  echo "=== $(date -u +%H:%M:%SZ) START $tag" | tee -a "$OUT/sweep.log"
  python3 run_grid.py --images "$imgs" --sizes 1024 \
      --arms aom,svtc,zenrav1e,svtrs --ladder-map "$lm" \
      --rate-stride 4 --reps 5 --verify-yuv \
      --workdir "$OUT/work" --artifacts "$OUT/artifacts" \
      --progress "$OUT/prog_$tag.tsv" --out "$OUT/cells_$tag.tsv" \
      "$@" >>"$OUT/sweep.log" 2>&1
  echo "=== $(date -u +%H:%M:%SZ) END   $tag rc=$? in $(( $(date +%s) - t0 ))s" | tee -a "$OUT/sweep.log"
}

# aom cpu7 and svtc p1 are cheap fill-ins for gaps in the main grid; the
# expensive ones (zenrav1e s3, svtrs p2/p0, aom cpu3) are what this run is for.
LM='zenrav1e:3;aom:7,3;svtc:1;svtrs:2,0'
run tu1024a "$HALF_A" "$LM"
run tu1024b "$HALF_B" "$LM"
echo "=== $(date -u +%H:%M:%SZ) TOPUP DONE" | tee -a "$OUT/sweep.log"
