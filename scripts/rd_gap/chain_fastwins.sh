#!/usr/bin/env bash
# FASTWINS P0 chain (2026-07-04, FAST_TIER_PARITY_PLAN Phase P0):
#   WIN-2  s4->s6 rdo_tx cliff decomposition -- size-RDO vs type-RDO vs depth cap vs
#          reduced signaling, at s6 (+ s8 shortlist), via the zenrav1e--fastwins knobs
#          (rdo_tx_size_override / rdo_tx_type_override / rdo_tx_size_depth) driven by
#          the ravif--fastwins DEV-ONLY env passthroughs.
#   WIN-1  cavif default-threading byte hazard -- tiles-vs-bytes curve at s6/s4 via
#          --threads N (tiles = min(threads, px/min_tile^2); pool size proven inert:
#          ZENRAVIF_TILES=4 == --threads 4 md5-identical).
#
# All RD cells: train26, tune-ss2 + palette auto (shipped-best config), BUTTER on,
# PALCONF=1 (aomdec must decode cleanly AND md5-agree with rav1d-safe -- coded behavior
# changes in every non-base arm). Coarse 6-q fitting grid per the two-stage rule;
# landing verdicts get a full-grid confirmation of the winner only.
#
# Run ON THE BOX under nohup:
#   OUTDIR=/home/lilith/sweep_out/fastwins_20260704 PHASES="rd" \
#     nohup bash chain_fastwins.sh > $OUTDIR/chain.log 2>&1 &
#
# Env: OUTDIR (required), CAVIF (default ravif--fastwins build), PHASES ("rd timing"),
#      TIMING_W2 / TIMING_W1_S6 / TIMING_W1_S4 (arm shortlists for the timing phase).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUTDIR="${OUTDIR:?set OUTDIR (per-run output dir)}"
mkdir -p "$OUTDIR"

export CAVIF="${CAVIF:-/home/lilith/work/zen/ravif--fastwins/target/release/cavif}"
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

say() { echo "[fastwins $(date -u +%H:%M:%SZ)] $*"; }

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

# WIN-2 tx-decomposition arm table: name -> extra envs (all --threads 1).
declare -A W2=(
  [base]=""
  [size1]="ZENRAVIF_TX_SIZE_RDO=1 ZENRAVIF_TX_SIZE_DEPTH=1"
  [size2]="ZENRAVIF_TX_SIZE_RDO=1"
  [type]="ZENRAVIF_TX_TYPE_RDO=1"
  [typred]="ZENRAVIF_TX_TYPE_RDO=1 ZENRAVIF_REDUCED_TX=1"
  [min]="ZENRAVIF_TX_SIZE_RDO=1 ZENRAVIF_TX_SIZE_DEPTH=1 ZENRAVIF_TX_TYPE_RDO=1 ZENRAVIF_REDUCED_TX=1"
  [full]="ZENRAVIF_TX_SIZE_RDO=1 ZENRAVIF_TX_TYPE_RDO=1"
  [red]="ZENRAVIF_REDUCED_TX=1"
)
W2_S6_ORDER="base size1 size2 type typred min full red"
W2_S8_ORDER="base size1 min red"

if [[ " $PHASES " == *" rd "* ]]; then
  say "=== PHASE rd: WIN-2 tx decomposition (s6 x 8 arms, s8 x 4 arms, t26 coarse) ==="
  for arm in $W2_S6_ORDER; do
    run_one "$OUTDIR/w2_s6_${arm}.tsv" 144 "${common[@]}" \
      ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" ${W2[$arm]}
  done
  for arm in $W2_S8_ORDER; do
    run_one "$OUTDIR/w2_s8_${arm}.tsv" 144 "${common[@]}" \
      ZENRAV1E_SPEED=8 CAVIF_EXTRA="--threads 1" ${W2[$arm]}
  done

  say "=== PHASE rd: WIN-1 tile-count byte curve (s6 threads {2,4,8,16,48}, s4 {4,8,48}) ==="
  # threads=1 baselines: w2_s6_base (identical cells) + w1_s4_thr1 below.
  for thr in 2 4 8 16 48; do
    run_one "$OUTDIR/w1_s6_thr${thr}.tsv" 144 "${common[@]}" \
      ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads $thr"
  done
  run_one "$OUTDIR/w1_s4_thr1.tsv" 144 "${common[@]}" ZENRAV1E_SPEED=4 CAVIF_EXTRA="--threads 1"
  for thr in 4 8 48; do
    run_one "$OUTDIR/w1_s4_thr${thr}.tsv" 144 "${common[@]}" \
      ZENRAV1E_SPEED=4 CAVIF_EXTRA="--threads $thr"
  done
fi

# ---- phase: confirm (stage 2: FULL 12-q grid for the landing configs) -------
QFULL="30 40 50 55 60 65 70 75 80 85 90 95"
if [[ " $PHASES " == *" confirm "* ]]; then
  say "=== PHASE confirm: full-grid landing arms (s6/s8 x {base,size1}) ==="
  for sp in 6 8; do
    for arm in base size1; do
      run_one "$OUTDIR/confirm_s${sp}_${arm}.tsv" 288 "${common[@]}" QGRID_ZR="$QFULL"         ZENRAV1E_SPEED=$sp CAVIF_EXTRA="--threads 1" ${W2[$arm]}
    done
  done
fi

# ---- phase: timing (solo, RD_CACHE=off, no PALCONF, JOBS=1) -----------------
# Shortlists set post-analysis; defaults cover the likely candidates.
TIMING_W2="${TIMING_W2:-base size1 min}"
TIMING_W1_S6="${TIMING_W1_S6:-1 2 4 8 16 48}"
TIMING_W1_S4="${TIMING_W1_S4:-1 8 48}"
tcommon=(AOMENC= BUTTER="$BUTTER" ZENRAVIF_TUNE=ssimulacra2 ZENRAVIF_PALETTE=auto
         SAMPLE="$SAMPLE_TIM" JOBS=1 RD_CACHE=off QGRID_ZR="40 65 85")
if [[ " $PHASES " == *" timing "* ]]; then
  say "=== PHASE timing: solo wall pass (4 img x 3 q, JOBS=1, cache off) ==="
  for arm in $TIMING_W2; do
    run_one "$OUTDIR/timing_w2_s6_${arm}.tsv" 12 "${tcommon[@]}" \
      ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" ${W2[$arm]}
    run_one "$OUTDIR/timing_w2_s8_${arm}.tsv" 12 "${tcommon[@]}" \
      ZENRAV1E_SPEED=8 CAVIF_EXTRA="--threads 1" ${W2[$arm]}
  done
  # Old-default tile ladder (ZENRAVIF_TILE_POLICY=off = pre-policy auto formula).
  for thr in $TIMING_W1_S6; do
    run_one "$OUTDIR/timing_w1_s6_thr${thr}.tsv" 12 "${tcommon[@]}" \
      ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads $thr" ZENRAVIF_TILE_POLICY=off
  done
  for thr in $TIMING_W1_S4; do
    run_one "$OUTDIR/timing_w1_s4_thr${thr}.tsv" 12 "${tcommon[@]}" \
      ZENRAV1E_SPEED=4 CAVIF_EXTRA="--threads $thr" ZENRAVIF_TILE_POLICY=off
  done
  # Before/after default pareto pair: cavif invoked with NO --threads flag
  # (the real default UX; pool = host cores) under old vs new tile policy.
  for sp in 6 4; do
    run_one "$OUTDIR/timing_default_old_s${sp}.tsv" 12 "${tcommon[@]}" \
      ZENRAV1E_SPEED=$sp CAVIF_EXTRA="" ZENRAVIF_TILE_POLICY=off
    run_one "$OUTDIR/timing_default_new_s${sp}.tsv" 12 "${tcommon[@]}" \
      ZENRAV1E_SPEED=$sp CAVIF_EXTRA=""
  done
fi

say "CHAIN COMPLETE. TSVs in $OUTDIR:"
ls -la "$OUTDIR"/*.tsv 2>/dev/null | awk '{print "  " $NF " (" $5 "b)"}'
say "failure scan:"
grep -l "CELLFAIL\|CONFFAIL\|ENCFAIL\|DECFAIL" "$OUTDIR"/*.log 2>/dev/null || echo "  no cell failures in any run log"
