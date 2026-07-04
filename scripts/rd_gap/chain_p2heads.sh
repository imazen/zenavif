#!/usr/bin/env bash
# P2HEADS chain (2026-07-04, FAST_TIER_PARITY_PLAN Phase P2):
#   1) intra-mode-budget response axis (head 3): top-7 keyframe intra RDO
#      (ComplexKeyframes + filter_intra=Some(false), the zenrav1e#5-safe
#      form) vs the stock forced-Simple top-3, at s6/s8 on the s6+size1
#      base and on the composed ship point (composition check).
#   2) composed fast-mode confirm (heads 1+2): per-image (tx, partition)
#      classes from the FROZEN threshold rules (emit_p2_composed_samples.py)
#      as per-class env sub-runs, 12-q full grid, train26 + the 14-origin
#      VAL-LSD corpus (honest held-out transfer).
#   3) solo timing for the composed mode vs plain s6 / global ship.
#
# Driven via the ravif--p2heads DEV-ONLY env passthroughs (p1part patch +
# ZENRAVIF_REDUCED_TX + ZENRAVIF_INTRA_MODES), zenrav1e--p2heads path dep
# (master e944ea71). All RD cells tune-ss2 + palette auto, BUTTER on,
# PALCONF=1. Cache: base/ship/confirm cells key-match the p1part run and
# ride the snapshot's sweep_cache.
#
# Run ON THE BOX under nohup:
#   OUTDIR=/home/lilith/sweep_out/p2heads_20260704 PHASES="intra composed val timing" \
#     nohup bash chain_p2heads.sh > $OUTDIR/chain.log 2>&1 &
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUTDIR="${OUTDIR:?set OUTDIR (per-run output dir)}"
mkdir -p "$OUTDIR"

export CAVIF="${CAVIF:-/home/lilith/work/zen/ravif--p2heads/target/release/cavif}"
export SAVE_PNG="${SAVE_PNG:-/home/lilith/work/zen/zenavif/target/release/examples/save_png}"
export SCORER="${SCORER:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}"
export AOMDEC="${AOMDEC:-/home/lilith/work/aom/build_slow/aomdec}"
export BUTTER="${BUTTER:-/home/lilith/work/butteraugli/target/release/butteraugli}"
export EXTRACT_AV1="${EXTRACT_AV1:-/home/lilith/work/zen/zenavif/target/release/examples/extract_av1}"
export IVF_RAW="${IVF_RAW:-/home/lilith/work/zen/zenavif/target/release/examples/ivf_raw}"
PHASES="${PHASES:-intra composed val timing}"

SAMPLE_T26="$HERE/sample_images_train26.tsv"  # 24 images
SAMPLE_TIM="$HERE/sample_timing4.tsv"         # 4 images
QCOARSE="30 50 60 75 85 95"
QFULL="30 40 50 55 60 65 70 75 80 85 90 95"

say() { echo "[p2heads $(date -u +%H:%M:%SZ)] $*"; }

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

common=(AOMENC= PALCONF=1 AOMDEC="$AOMDEC" EXTRACT_AV1="$EXTRACT_AV1" IVF_RAW="$IVF_RAW"
        BUTTER="$BUTTER" ZENRAVIF_TUNE=ssimulacra2 ZENRAVIF_PALETTE=auto
        JOBS=24)

# TX classes (fastwins env conventions)
SIZE1="ZENRAVIF_TX_SIZE_RDO=1 ZENRAVIF_TX_SIZE_DEPTH=1"
MIN="ZENRAVIF_TX_SIZE_RDO=1 ZENRAVIF_TX_SIZE_DEPTH=1 ZENRAVIF_TX_TYPE_RDO=1 ZENRAVIF_REDUCED_TX=1"
# Partition rungs (p1part env conventions)
SHIP="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_4WM=0.0 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_VARG=2.0"
M32="ZENRAVIF_RECT_THR=16 ZENRAVIF_PART_MAX=32 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_VARG=2.0"
I7="ZENRAVIF_INTRA_MODES=7"

# Composed class name -> tx env + partition env
cls_env() { # <class>
  case "$1" in
    none_ship)  echo "$SHIP" ;;
    none_m32)   echo "$M32" ;;
    size1_ship) echo "$SIZE1 $SHIP" ;;
    size1_m32)  echo "$SIZE1 $M32" ;;
    min_ship)   echo "$MIN $SHIP" ;;
    min_m32)    echo "$MIN $M32" ;;
    *) echo "BAD CLASS $1" >&2; exit 1 ;;
  esac
}
CLASSES="none_ship none_m32 size1_ship size1_m32 min_ship min_m32"

cls_rows() { # <sample.tsv> <qcount>
  echo $(( ($(wc -l < "$1") - 1) * $2 ))
}

if [[ " $PHASES " == *" intra "* ]]; then
  say "=== PHASE intra: head-3 response axis (s6/s8, coarse, t26) ==="
  run_one "$OUTDIR/p2_s6_base.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1
  run_one "$OUTDIR/p2_s6_intra7.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $I7
  run_one "$OUTDIR/p2_s6_ship.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $SHIP
  run_one "$OUTDIR/p2_s6_intra7ship.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $SHIP $I7
  run_one "$OUTDIR/p2_s8_base.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=8 CAVIF_EXTRA="--threads 1" $SIZE1
  run_one "$OUTDIR/p2_s8_intra7.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=8 CAVIF_EXTRA="--threads 1" $SIZE1 $I7
fi

if [[ " $PHASES " == *" composed "* ]]; then
  say "=== PHASE composed: per-class 12q confirm (t26) + global refs ==="
  run_one "$OUTDIR/p2_conf_s6_base.tsv" 288 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QFULL" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1
  run_one "$OUTDIR/p2_conf_s6_ship.tsv" 288 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QFULL" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $SHIP
  for cls in $CLASSES; do
    smp="$HERE/sample_p2c_${cls}.tsv"
    [ -f "$smp" ] || { say "no $smp — skipping class $cls"; continue; }
    run_one "$OUTDIR/p2c_${cls}.tsv" "$(cls_rows "$smp" 12)" "${common[@]}" \
      SAMPLE="$smp" QGRID_ZR="$QFULL" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" \
      $(cls_env "$cls")
  done
fi

if [[ " $PHASES " == *" val "* ]]; then
  say "=== PHASE val: held-out transfer (14 VAL-LSD origins, 12q) ==="
  run_one "$OUTDIR/p2v_base.tsv" "$(cls_rows "$HERE/sample_p2val_all.tsv" 12)" \
    "${common[@]}" SAMPLE="$HERE/sample_p2val_all.tsv" QGRID_ZR="$QFULL" \
    ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1
  run_one "$OUTDIR/p2v_ship.tsv" "$(cls_rows "$HERE/sample_p2val_all.tsv" 12)" \
    "${common[@]}" SAMPLE="$HERE/sample_p2val_all.tsv" QGRID_ZR="$QFULL" \
    ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $SHIP
  for cls in $CLASSES; do
    smp="$HERE/sample_p2valc_${cls}.tsv"
    [ -f "$smp" ] || continue
    run_one "$OUTDIR/p2vc_${cls}.tsv" "$(cls_rows "$smp" 12)" "${common[@]}" \
      SAMPLE="$smp" QGRID_ZR="$QFULL" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" \
      $(cls_env "$cls")
  done
fi

if [[ " $PHASES " == *" timing "* ]]; then
  say "=== PHASE timing: solo wall (JOBS=1, RD_CACHE=off, q{40,65,85}) ==="
  tim=(RD_CACHE=off JOBS=1 QGRID_ZR="40 65 85")
  run_one "$OUTDIR/p2t_s6_plain.tsv" 72 "${common[@]}" "${tim[@]}" \
    SAMPLE="$SAMPLE_T26" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1"
  run_one "$OUTDIR/p2t_s6_size1ship.tsv" 72 "${common[@]}" "${tim[@]}" \
    SAMPLE="$SAMPLE_T26" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $SHIP
  for cls in $CLASSES; do
    smp="$HERE/sample_p2c_${cls}.tsv"
    [ -f "$smp" ] || continue
    run_one "$OUTDIR/p2t_c_${cls}.tsv" "$(cls_rows "$smp" 3)" "${common[@]}" \
      "${tim[@]}" SAMPLE="$smp" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" \
      $(cls_env "$cls")
  done
  run_one "$OUTDIR/p2t_intra7.tsv" 12 "${common[@]}" "${tim[@]}" \
    SAMPLE="$SAMPLE_TIM" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $I7
  run_one "$OUTDIR/p2t_intra7ship.tsv" 12 "${common[@]}" "${tim[@]}" \
    SAMPLE="$SAMPLE_TIM" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $SHIP $I7
  run_one "$OUTDIR/p2t_size1.tsv" 12 "${common[@]}" "${tim[@]}" \
    SAMPLE="$SAMPLE_TIM" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1
fi

say "chain complete."
