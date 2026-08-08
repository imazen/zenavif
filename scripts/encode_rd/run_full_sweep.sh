#!/usr/bin/env bash
# Full matched-wall-clock encode RD sweep — the sized, launchable grid.
#
# WHY THE LADDER SET DIFFERS PER SIZE, AND PER ARM
# ------------------------------------------------
# The comparison is at equal measured TIME, so the rungs worth measuring are the
# ones that populate the four-way time OVERLAP — and that window moves with
# image size. Measured on the sizing probe (benchmarks/encode_rd_probe_ladder_*):
#
#     size 256  : four-way overlap  11.8 .. 136 ms
#     size 1024 : four-way overlap   103 .. 1252 ms
#
# It also moves per arm, because the ladders are not aligned: at 1024, zenrav1e
# CANNOT go faster than 103 ms (its s10) while svtc reaches 7.8 ms, and svtc
# cannot go slower than 1252 ms (its p0) while zenrav1e reaches 37.9 s. One
# shared --ladder list cannot express that, hence --ladder-map.
#
# Two ladder facts from the probe drive the exclusions below, both verified at
# two sizes with <1.1% timing spread:
#
#   * zenrav1e s4 and s5 are STRICTLY DOMINATED by s3 — slower AND larger
#     (1024: s3 6796 ms / 61009 B vs s4 10637 ms / 62000 B). A dominated rung is
#     never the right choice, so measuring it at full rate density buys nothing.
#     They are kept only in the probe, which is committed as the evidence.
#   * svtc / svtrs presets 10-13 are byte-identical to preset 9 and take the
#     same time. The SVT still-picture ladder has 10 distinct rungs, not 14.
#
# Rungs beyond each end of the overlap are kept where affordable so the frontier
# interpolation has support rather than ending exactly at the comparison edge.
#
# Everything writes continuously to a progress file; nothing waits for the end.

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${OUT:-$HOME/tmp/encrd2}"
export RD_TOOL="${RD_TOOL:-$REPO/target/release/examples/rd_tool}"
mkdir -p "$OUT"
cd "$REPO/scripts/encode_rd" || exit 1

PHOTO="clic2025/training/4cd6910a0b7b39365fda5df87618d091.png,clic2025/training/7e499613c3e376ea93afb3649719abeb.png,clic2025/training/ddcd24d99f48eaa369207882a6f37831.png,clic2025/training/14ab4af28901fbeb1356b06d2d08ae06.png,clic2025/training/1e2f9d41529197f10d32bfa68a1e0bcc.png"
SCREEN="gb82-sc/codec_wiki.png,gb82-sc/gui.png,gb82-sc/terminal.png,qoi-benchmark/screenshot_web/en.wikipedia.org.png"
LINEART="gb82-sc/graph.png,CID22/CID22-512/training/Boxplot.png,CID22/CID22-512/training/newplot.png"
ALL="$PHOTO,$SCREEN,$LINEART"

# half the image set per 1024 invocation, so one interrupted process costs ~1.5 h
# rather than ~3.2 h. Arms stay interleaved WITHIN each invocation, which is what
# the arm-vs-arm comparison depends on.
HALF_A="clic2025/training/4cd6910a0b7b39365fda5df87618d091.png,clic2025/training/7e499613c3e376ea93afb3649719abeb.png,clic2025/training/ddcd24d99f48eaa369207882a6f37831.png,gb82-sc/codec_wiki.png,gb82-sc/gui.png,gb82-sc/graph.png"
HALF_B="clic2025/training/14ab4af28901fbeb1356b06d2d08ae06.png,clic2025/training/1e2f9d41529197f10d32bfa68a1e0bcc.png,gb82-sc/terminal.png,qoi-benchmark/screenshot_web/en.wikipedia.org.png,CID22/CID22-512/training/Boxplot.png,CID22/CID22-512/training/newplot.png"

# --- ladder maps, one per size tier -----------------------------------------
# 64 px: everything is milliseconds, so take the whole useful ladder.
LM64='zenrav1e:10,9,8,7,6,3,2,1,0;aom:9,8,7,6,5,4,3,2,1,0;svtc:9,8,7,6,5,4,3,2,1,0;svtrs:9,8,7,6,5,4,3,2,1,0'
# 256 px: overlap 11.8..136 ms. zenrav1e keeps s3 (529 ms) to reach past svtc p0.
LM256='zenrav1e:10,9,8,7,6,3;aom:9,8,7,6,5,4,3,2,0;svtc:9,8,7,6,5,4,3,2,0;svtrs:9,8,7,6,5,4,3,2,0'
# 1024 px: overlap 103..1252 ms. zenrav1e s3 costs 6.8 s/encode = 2.1 h/image at
# full rate density, so the zenrav1e frontier stops at s6 (1123 ms) here and the
# analysis will correctly refuse to compare it above that.
LM1024='zenrav1e:10,9,8,7,6;aom:9,8,6,5,4,2;svtc:9,8,7,6,5,4,3,2,0;svtrs:9,8,7,6,5,4,3'

run () {   # run <tag> <images> <sizes> <laddermap> [extra...]
  local tag="$1" imgs="$2" sizes="$3" lm="$4"; shift 4
  local t0; t0=$(date +%s)
  echo "=== $(date -u +%H:%M:%SZ) START $tag sizes=$sizes" | tee -a "$OUT/sweep.log"
  python3 run_grid.py --images "$imgs" --sizes "$sizes" \
      --arms aom,svtc,zenrav1e,svtrs --ladder-map "$lm" \
      --rate-stride 1 --reps 5 --verify-yuv \
      --workdir "$OUT/work" --artifacts "$OUT/artifacts" \
      --progress "$OUT/prog_$tag.tsv" --out "$OUT/cells_$tag.tsv" \
      "$@" >>"$OUT/sweep.log" 2>&1
  echo "=== $(date -u +%H:%M:%SZ) END   $tag rc=$? in $(( $(date +%s) - t0 ))s" | tee -a "$OUT/sweep.log"
}

run t64    "$ALL"    64   "$LM64"
run t256   "$ALL"    256  "$LM256"
run t1024a "$HALF_A" 1024 "$LM1024"
run t1024b "$HALF_B" 1024 "$LM1024"

# --- the size-scaling extension ---------------------------------------------
# The RD grid above stops at 1024 because zenrav1e's mid ladder costs seconds per
# encode at 2048. But `t = alpha + beta*px` needs the widest size span available,
# and a ms/MP figure quoted from one size is wrong at every other size (the probe
# saw a 76x swing for one arm at one rung). So the fit gets its own reduced grid:
# few rates, three rungs, five sizes. No local photo or line-art source exceeds
# 2048 px, so the 4096 tier is screen content only.
LMFIT='zenrav1e:9,8,6;aom:9,8,6;svtc:9,8,6;svtrs:9,8,6'
run fit_photo  "clic2025/training/ddcd24d99f48eaa369207882a6f37831.png,clic2025/training/1e2f9d41529197f10d32bfa68a1e0bcc.png" \
               64,256,1024,2048 "$LMFIT" --rate-stride 6
run fit_screen "qoi-benchmark/screenshot_web/creativecommons.org.png,qoi-benchmark/screenshot_web/reddit.com.png" \
               64,256,1024,2048,4096 "$LMFIT" --rate-stride 6

echo "=== $(date -u +%H:%M:%SZ) ALL DONE" | tee -a "$OUT/sweep.log"
