#!/usr/bin/env bash
# P1PART chain (2026-07-04, FAST_TIER_PARITY_PLAN Phase P1 lever 1):
#   Replace partition-search amputation with pruning at s4-s8. The ravif
#   table kills rect partitions at s4+ (non_square thr -> 8x8) and caps
#   blocks at 16; these arms keep HORZ/VERT (+16-parent 4-ways) LIVE and
#   control cost with the zenrav1e topdown_prune knob (725f5f71):
#   NONE-first walk + none_breakout / rect|4way margins / homogeneity gate.
#   Driven via the ravif--p1part DEV-ONLY env passthroughs
#   (ZENRAVIF_RECT_THR / PART_MIN / PART_MAX / PRUNE_BK / PRUNE_RECTM /
#   PRUNE_4WM / PRUNE_VARG), zenrav1e--p1part path dep.
#
# All RD cells: train26, tune-ss2 + palette auto, BUTTER on, PALCONF=1
# (rect/4-way liveness at fast tiers exercises never-shipped coded paths).
# s6/s8 arms ride the P0 landed baseline (tx-size RDO depth-1); s4 rides the
# stock table. Coarse 6-q fitting grid; landing verdicts full-grid confirm.
#
# Run ON THE BOX under nohup:
#   OUTDIR=/home/lilith/sweep_out/p1part_20260704 PHASES="rd" \
#     nohup bash chain_p1part.sh > $OUTDIR/chain.log 2>&1 &
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUTDIR="${OUTDIR:?set OUTDIR (per-run output dir)}"
mkdir -p "$OUTDIR"

export CAVIF="${CAVIF:-/home/lilith/work/zen/ravif--p1part/target/release/cavif}"
export SAVE_PNG="${SAVE_PNG:-/home/lilith/work/zen/zenavif/target/release/examples/save_png}"
export SCORER="${SCORER:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}"
export AOMDEC="${AOMDEC:-/home/lilith/work/aom/build_slow/aomdec}"
export BUTTER="${BUTTER:-/home/lilith/work/butteraugli/target/release/butteraugli}"
export EXTRACT_AV1="${EXTRACT_AV1:-/home/lilith/work/zen/zenavif/target/release/examples/extract_av1}"
export IVF_RAW="${IVF_RAW:-/home/lilith/work/zen/zenavif/target/release/examples/ivf_raw}"
PHASES="${PHASES:-rd}"

SAMPLE_T26="$HERE/sample_images_train26.tsv"  # 24 images
SAMPLE_TIM="$HERE/sample_timing4.tsv"         # 4 images
QCOARSE="30 50 60 75 85 95"

say() { echo "[p1part $(date -u +%H:%M:%SZ)] $*"; }

run_one() { # <out.tsv> <expected_rows> [ENV=val ...]
  local out="$1" want="$2"; shift 2
  if [ -s "$out" ] && [ "$(($(wc -l < "$out") - 1))" -ge "$want" ]; then
    say "SKIP (complete $(($(wc -l < "$out") - 1))/$want): $(basename "$out")"
    return 0
  fi
  local t0=$(date +%s)
  say "RUN $(basename "$out") [$*]"
  if env "$@" OUT="$out" bash "$HERE/run_gap.sh" > "$out.log" 2>&1; then
    local rows=$(($(wc -l < "$out") - 1))
    say "DONE $(basename "$out") rows=$rows/$want in $(( $(date +%s) - t0 ))s"
    if [ "$rows" -lt "$want" ]; then
      say "WARNING: INCOMPLETE $(basename "$out") ($rows/$want) -- see $out.log"
      grep -h "CELLFAIL\|CONFFAIL\|ENCFAIL\|DECFAIL" "$out.log" | tail -5 || true
    fi
  else
    say "FAILED run $(basename "$out") -- see $out.log"; tail -5 "$out.log"
  fi
}

# Common: tune-ss2 + palette (shipped-best), PALCONF on, no aom side, coarse grid.
common=(AOMENC= PALCONF=1 AOMDEC="$AOMDEC" EXTRACT_AV1="$EXTRACT_AV1" IVF_RAW="$IVF_RAW"
        BUTTER="$BUTTER" ZENRAVIF_TUNE=ssimulacra2 ZENRAVIF_PALETTE=auto
        SAMPLE="$SAMPLE_T26" JOBS=24 QGRID_ZR="$QCOARSE")
# s6/s8 baseline = P0 landed config (tx-size RDO depth-1).
SIZE1="ZENRAVIF_TX_SIZE_RDO=1 ZENRAVIF_TX_SIZE_DEPTH=1"

# Arm tables: name -> extra envs (all --threads 1).
# Shapes: r16 = rects+16-parent 4-ways live; m32 = partition max 16 -> 32;
# r32 = rects also at 32. Prune: bk = none_breakout tau, rm/fm = rect/4-way
# rel-gap margins, vg = 4x4 log-var deviation gate (aom's 3.0 anchor).
declare -A ARMS=(
  [base]=""
  [r16]="ZENRAVIF_RECT_THR=16"
  [r16no4]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_4WM=0.0"
  [r16m32]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PART_MAX=32"
  [r32m32]="ZENRAVIF_RECT_THR=32 ZENRAVIF_PART_MAX=32"
  [r16_bk]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_BK=1.0"
  [r16_pr1]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_RECTM=0.25 ZENRAVIF_PRUNE_4WM=0.05 ZENRAVIF_PRUNE_VARG=3.0"
  [r16_pr2]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_BK=2.0 ZENRAVIF_PRUNE_RECTM=0.15 ZENRAVIF_PRUNE_4WM=0.05 ZENRAVIF_PRUNE_VARG=3.0"
  [r16m32_pr1]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PART_MAX=32 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_RECTM=0.25 ZENRAVIF_PRUNE_4WM=0.05 ZENRAVIF_PRUNE_VARG=3.0"
  [r16m32_pr2]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PART_MAX=32 ZENRAVIF_PRUNE_BK=2.0 ZENRAVIF_PRUNE_RECTM=0.15 ZENRAVIF_PRUNE_4WM=0.05 ZENRAVIF_PRUNE_VARG=3.0"
  # ---- wave 2 (zenrav1e 767c8ff5+): margin gates are ONE-SIDED NONE-dominance
  # tests (the wave-1 symmetric band forfeited 74% of the liveness win on
  # SPLIT-dominant content). _pr1/_pr2 arms above measured the OLD symmetric
  # semantics on 725f5f71; do not mix. base2 = byte-identity sentinel for the
  # rebuilt binary (must byte-match wave-1 base).
  [base2]=""
  [r16_pr3]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_RECTM=0.25 ZENRAVIF_PRUNE_4WM=0.05 ZENRAVIF_PRUNE_VARG=3.0"
  [r16_pr4]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_BK=2.0 ZENRAVIF_PRUNE_RECTM=0.10 ZENRAVIF_PRUNE_4WM=0.02 ZENRAVIF_PRUNE_VARG=3.0"
  [r16_vg3]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_VARG=3.0"
  [r16_vg2]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_VARG=2.0"
  [r16no4_pr3]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_4WM=0.0 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_RECTM=0.25 ZENRAVIF_PRUNE_VARG=3.0"
  [r16m32_pr3]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PART_MAX=32 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_RECTM=0.25 ZENRAVIF_PRUNE_4WM=0.05 ZENRAVIF_PRUNE_VARG=3.0"
  # ---- wave 3: margins are a measured dead end in BOTH semantics (sym kept
  # 26%, one-sided 46-48%, and pr4~=pr3 shows the lost rect wins sit exactly
  # where NONE dominates the split estimate — the gate premise is wrong on
  # our cost model). Composites of the gates that DO trade well: breakout
  # (free: 100% @ 2.57 vs 2.75) x homogeneity vargate (vg2 94% @ 2.38,
  # vg3 83% @ 2.19).
  [r16_bkvg2]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_VARG=2.0"
  [r16_bkvg3]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_VARG=3.0"
  [r16_bk4vg2]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_BK=4.0 ZENRAVIF_PRUNE_VARG=2.0"
  [r16no4_bkvg2]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_4WM=0.0 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_VARG=2.0"
  [r16m32_bkvg2]="ZENRAVIF_RECT_THR=16 ZENRAVIF_PART_MAX=32 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_VARG=2.0"
)
S6_ORDER="${S6_ORDER:-base r16 r16no4 r16m32 r32m32 r16_bk r16_pr1 r16_pr2 r16m32_pr1 r16m32_pr2}"
S8_ORDER="${S8_ORDER:-base r16 r16_pr1 r16_pr2}"
S4_ORDER="${S4_ORDER:-base r16 r16_pr1}"

if [[ " $PHASES " == *" rd "* ]]; then
  say "=== PHASE rd: s6 shape+prune grid (${S6_ORDER}) ==="
  for arm in $S6_ORDER; do
    run_one "$OUTDIR/p1_s6_${arm}.tsv" 144 "${common[@]}" \
      ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 ${ARMS[$arm]}
  done
  say "=== PHASE rd: s8 shortlist (${S8_ORDER}) ==="
  for arm in $S8_ORDER; do
    run_one "$OUTDIR/p1_s8_${arm}.tsv" 144 "${common[@]}" \
      ZENRAV1E_SPEED=8 CAVIF_EXTRA="--threads 1" $SIZE1 ${ARMS[$arm]}
  done
  say "=== PHASE rd: s4 shortlist (${S4_ORDER}) ==="
  for arm in $S4_ORDER; do
    run_one "$OUTDIR/p1_s4_${arm}.tsv" 144 "${common[@]}" \
      ZENRAV1E_SPEED=4 CAVIF_EXTRA="--threads 1" ${ARMS[$arm]}
  done
fi

# ---- phase: confirm (stage 2: FULL 12-q grid for the landing configs) -------
QFULL="30 40 50 55 60 65 70 75 80 85 90 95"
CONFIRM_S6="${CONFIRM_S6:-base r16_pr1}"
CONFIRM_S8="${CONFIRM_S8:-base r16_pr1}"
CONFIRM_S4="${CONFIRM_S4:-base r16_pr1}"
if [[ " $PHASES " == *" confirm "* ]]; then
  say "=== PHASE confirm: full-grid landing arms ==="
  for arm in $CONFIRM_S6; do
    run_one "$OUTDIR/confirm_s6_${arm}.tsv" 288 "${common[@]}" QGRID_ZR="$QFULL" \
      ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 ${ARMS[$arm]}
  done
  for arm in $CONFIRM_S8; do
    run_one "$OUTDIR/confirm_s8_${arm}.tsv" 288 "${common[@]}" QGRID_ZR="$QFULL" \
      ZENRAV1E_SPEED=8 CAVIF_EXTRA="--threads 1" $SIZE1 ${ARMS[$arm]}
  done
  for arm in $CONFIRM_S4; do
    run_one "$OUTDIR/confirm_s4_${arm}.tsv" 288 "${common[@]}" QGRID_ZR="$QFULL" \
      ZENRAV1E_SPEED=4 CAVIF_EXTRA="--threads 1" ${ARMS[$arm]}
  done
fi

# ---- phase: timing (solo, RD_CACHE=off, no PALCONF, JOBS=1) -----------------
TIMING_S6="${TIMING_S6:-base r16 r16_pr1 r16_pr2 r16m32_pr1}"
TIMING_S8="${TIMING_S8:-base r16_pr1}"
TIMING_S4="${TIMING_S4:-base r16_pr1}"
tcommon=(AOMENC= BUTTER="$BUTTER" ZENRAVIF_TUNE=ssimulacra2 ZENRAVIF_PALETTE=auto
         SAMPLE="$SAMPLE_TIM" JOBS=1 RD_CACHE=off QGRID_ZR="40 65 85")
if [[ " $PHASES " == *" timing "* ]]; then
  say "=== PHASE timing: solo wall pass (4 img x 3 q, JOBS=1, cache off) ==="
  for arm in $TIMING_S6; do
    run_one "$OUTDIR/timing_s6_${arm}.tsv" 12 "${tcommon[@]}" \
      ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 ${ARMS[$arm]}
  done
  for arm in $TIMING_S8; do
    run_one "$OUTDIR/timing_s8_${arm}.tsv" 12 "${tcommon[@]}" \
      ZENRAV1E_SPEED=8 CAVIF_EXTRA="--threads 1" $SIZE1 ${ARMS[$arm]}
  done
  for arm in $TIMING_S4; do
    run_one "$OUTDIR/timing_s4_${arm}.tsv" 12 "${tcommon[@]}" \
      ZENRAV1E_SPEED=4 CAVIF_EXTRA="--threads 1" ${ARMS[$arm]}
  done
fi

say "CHAIN COMPLETE. TSVs in $OUTDIR:"
ls -la "$OUTDIR"/*.tsv 2>/dev/null | awk '{print "  " $NF " (" $5 "b)"}'
say "failure scan:"
grep -l "CELLFAIL\|CONFFAIL\|ENCFAIL\|DECFAIL" "$OUTDIR"/*.log 2>/dev/null || echo "  no cell failures in any run log"
