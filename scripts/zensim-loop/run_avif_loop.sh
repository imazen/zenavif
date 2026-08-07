#!/bin/bash
# AVIF zensim target-hitting loop runner (2026-08-07) — protocol + gates:
#   benchmarks/zensim_avif_loop_2026-08-07.md   (registered before runs)
# Phases: smoke matrix    (usage: run_avif_loop.sh <phase> [bake])
#
# The corpus recipe is the beats-butter 9-ref set VERBATIM
# (jxl-encoder scripts/zensim-loop-eff/run_beatbutter.sh): coherence refs
# city/dog/girl 576², CID22-512 validation 1025469/1418519/1189261, and
# 576² crops of the gb82-sc screen images at +512+256.
#
# Build first (from repo root; own target dir, nice'd):
#   nice -n19 ionice -c3 cargo build --release -p zenavif \
#     --features encode-imazen,two-pass-butteraugli --example zensim_cq_rd
#
# The MATRIX phase is registered but does NOT run this session — it runs
# when the wave-12 candidate bake lands (campaign appendix AC), with the
# shipped C bake (W10L9_s4003_packed) as the control arm. The h3-mag arms
# additionally require zenravif FRAME_HINTS_LIVE == true (the zenrav1e
# dep bump); until then the harness refuses them loudly.
set -u
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN=${ZCQ_BIN:-$REPO/target/release/examples/zensim_cq_rd}
OUT=${AVL_OUT:-$HOME/tmp/avifloop}
CAND=${2:-${CAND_BAKE:-/mnt/v/output/zensim/bakes/sota944/bakes/W10L9_s4003_packed.bin}}
CTRL=${CTRL_BAKE:-/mnt/v/output/zensim/bakes/sota944/bakes/W10L9_s4003_packed.bin}
GB82=${GB82_DIR:-$HOME/work/codec-corpus/gb82-sc}
CID=${CID_DIR:-$HOME/work/codec-corpus/CID22/CID22-512/validation}
COH=${COH_DIR:-/mnt/v/output/zensim/diffmap-coherence-2026-07-18}
RUN="nice -n19 ionice -c3"
mkdir -p "$OUT/fixtures"
LOG=$OUT/run_avif_loop.log
say() { echo "[$(date -u +%FT%TZ)] $*" | tee -a "$LOG"; }

for sc in codec_wiki gui imessage; do
  f=$OUT/fixtures/sc_${sc}.png
  [ -f "$f" ] || convert "$GB82/${sc}.png" -crop 576x576+512+256 +repage "$f"
done
CORPUS=$OUT/corpus9.tsv
{
  printf '%s\tcity\tphoto\n'        "$COH/city.png"
  printf '%s\tdog\tphoto\n'         "$COH/dog.png"
  printf '%s\tgirl\tphoto\n'        "$COH/girl.png"
  printf '%s\tcid1025469\tphoto\n'  "$CID/1025469.png"
  printf '%s\tcid1418519\tphoto\n'  "$CID/1418519.png"
  printf '%s\tcid1189261\tphoto\n'  "$CID/1189261.png"
  printf '%s\tsc_wiki\tnonphoto\n'    "$OUT/fixtures/sc_codec_wiki.png"
  printf '%s\tsc_gui\tnonphoto\n'     "$OUT/fixtures/sc_gui.png"
  printf '%s\tsc_imessage\tnonphoto\n' "$OUT/fixtures/sc_imessage.png"
} > "$CORPUS"

phase=${1:-smoke}
run_cells() { # run_cells <outdir> <label> <arms> <iters> <bake> <corpus>
  local od=$1 lbl=$2 arms=$3 it=$4 bake=$5 corpus=$6
  mkdir -p "$od"
  say "run_cells $lbl arms=$arms iters=$it bake=$bake"
  AVIF_ZENSIM_EMIT_BEST=1 $RUN "$BIN" --corpus-file "$corpus" \
    --zensim-targets 70,80,88 --arms "$arms" --bake "$bake" --iters "$it" \
    --label "$lbl" --out-dir "$od" >> "$LOG" 2>&1
}

if [ "$phase" = smoke ]; then
  # G-AV2: ONE cell (city t80 k3), both inner arms + the outer comparator.
  # h3-mag is EXPECTED to refuse loudly while FRAME_HINTS_LIVE == false.
  D=$OUT/smoke; mkdir -p "$D"
  CITY=$D/corpus_city.tsv
  head -1 "$CORPUS" > "$CITY"
  AVIF_ZENSIM_EMIT_BEST=1 $RUN "$BIN" --corpus-file "$CITY" --zensim-targets 80 \
    --arms baseline --iters 3 --label smoke_base_k3 --out-dir "$D" >> "$LOG" 2>&1
  AVIF_ZENSIM_EMIT_BEST=1 $RUN "$BIN" --corpus-file "$CITY" --zensim-targets 80 \
    --arms outer --iters 3 --label smoke_outer_j3 --out-dir "$D" >> "$LOG" 2>&1
  if AVIF_ZENSIM_EMIT_BEST=1 $RUN "$BIN" --corpus-file "$CITY" --zensim-targets 80 \
    --arms h3-mag --iters 3 --label smoke_h3_k3 --out-dir "$D" >> "$LOG" 2>&1; then
    say "smoke: h3-mag ran (FRAME_HINTS_LIVE build)"
  else
    say "smoke: h3-mag refused (expected while FRAME_HINTS_LIVE == false)"
  fi
  say "smoke done -> $D"
fi

if [ "$phase" = matrix ]; then
  # G-AV3 (runs when the wave-12 candidate lands; C = control):
  # arms {baseline, h3-mag} × k{2,3} for candidate AND control, plus the
  # outer comparator at j{2,3}. Stats owner: the jxl series'
  # analyze_23shot.cells_stats over the target_ab TSVs.
  D=$OUT/matrix; mkdir -p "$D"
  for k in 2 3; do
    run_cells "$D" "cand_base_k${k}" baseline "$k" "$CAND" "$CORPUS"
    run_cells "$D" "cand_h3_k${k}"   h3-mag   "$k" "$CAND" "$CORPUS"
    run_cells "$D" "ctrl_base_k${k}" baseline "$k" "$CTRL" "$CORPUS"
    run_cells "$D" "ctrl_h3_k${k}"   h3-mag   "$k" "$CTRL" "$CORPUS"
    run_cells "$D" "outer_j${k}"     outer    "$k" "$CTRL" "$CORPUS"
  done
  say "matrix done -> $D (collect via analyze_23shot.cells_stats)"
fi

say "phase '$phase' complete"
touch "$OUT/PHASE_${phase}.done"
