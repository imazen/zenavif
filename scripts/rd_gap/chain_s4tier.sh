#!/usr/bin/env bash
# S4TIER chain (2026-07-04, FAST_TIER_PARITY_PLAN: the last open column —
# the s4-equivalent tier where aom cpu2iq-allintra leads the composed+i7
# mode by +4.40/+4.04 at 1.27x its wall). Phases:
#
#   cont     byte-continuity gate: re-encode the p2heads s6+size1 base and
#            i7 coarse arms under the NEW binary chain (zenrav1e 0d392334
#            with num_modes_rdo_override; env-off must be byte-identical:
#            compare TSV bytes vs the p2heads run offline).
#   i5axis   the NEW top-5 knob response: s6 size1+I5, s6 size1+ship+I5,
#            s8 size1+I5 (coarse, t26) — vs the cached i3 (base/ship) and
#            i7 arms from the cont phase / p2heads.
#   filters  hi-q filter-schedule probe (P1 lever 4, the one unswept axis):
#            s6 size1+ship + {CDEF=1, LRF=1} coarse — aom-ai keeps CDEF at
#            every q; ravif's table gates both at Q<~50.
#   composed the v3 (s4-tier rules) per-class 12q confirm, +i7 global:
#            sample_s4c_* classes.
#   composedi5  the same v3 classes with I5 instead of I7 (the global
#            intra-arm decision at mode level).
#   oraclex  full-tx oracle extras +i7 (NOT deployable — no honest gate;
#            quantifies the rule-vs-oracle residual): sample_s4x_*.
#   timing   solo walls (JOBS=1 RD_CACHE=off q{40,65,85}): plain s6, the
#            v3+i7 mode per-class, v3+i5, full extras, cdef arm, i5/i7
#            4-img marginals.
#   val      held-out transfer: sample_s4valc_* classes, 12q, +i7 AND +i5.
#
# Run ON THE BOX under nohup:
#   OUTDIR=/home/lilith/sweep_out/s4tier_20260704 \
#     PHASES="cont i5axis filters composed composedi5 oraclex timing val" \
#     nohup bash chain_s4tier.sh > $OUTDIR/chain.log 2>&1 &
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUTDIR="${OUTDIR:?set OUTDIR (per-run output dir)}"
mkdir -p "$OUTDIR"

export CAVIF="${CAVIF:-/home/lilith/work/zen/ravif--s4tier/target/release/cavif}"
export SAVE_PNG="${SAVE_PNG:-/home/lilith/work/zen/zenavif/target/release/examples/save_png}"
export SCORER="${SCORER:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}"
export AOMDEC="${AOMDEC:-/home/lilith/work/aom/build_slow/aomdec}"
export BUTTER="${BUTTER:-/home/lilith/work/butteraugli/target/release/butteraugli}"
export EXTRACT_AV1="${EXTRACT_AV1:-/home/lilith/work/zen/zenavif/target/release/examples/extract_av1}"
export IVF_RAW="${IVF_RAW:-/home/lilith/work/zen/zenavif/target/release/examples/ivf_raw}"
PHASES="${PHASES:-cont i5axis filters composed composedi5 oraclex timing val}"

SAMPLE_T26="$HERE/sample_images_train26.tsv"  # 24 images
SAMPLE_TIM="$HERE/sample_timing4.tsv"         # 4 images
QCOARSE="30 50 60 75 85 95"
QFULL="30 40 50 55 60 65 70 75 80 85 90 95"

say() { echo "[s4tier $(date -u +%H:%M:%SZ)] $*"; }

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
FULL="ZENRAVIF_TX_SIZE_RDO=1 ZENRAVIF_TX_TYPE_RDO=1"
# Partition rungs (p1part env conventions)
SHIP="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_4WM=0.0 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_VARG=2.0"
M32="ZENRAVIF_RECT_THR=16 ZENRAVIF_PART_MAX=32 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_VARG=2.0"
I7="ZENRAVIF_INTRA_MODES=7"
I5="ZENRAVIF_INTRA_MODES=5"

cls_env() { # <class>  (tx_part)
  case "$1" in
    none_ship)  echo "$SHIP" ;;
    none_m32)   echo "$M32" ;;
    size1_ship) echo "$SIZE1 $SHIP" ;;
    size1_m32)  echo "$SIZE1 $M32" ;;
    min_ship)   echo "$MIN $SHIP" ;;
    min_m32)    echo "$MIN $M32" ;;
    full_ship)  echo "$FULL $SHIP" ;;
    full_m32)   echo "$FULL $M32" ;;
    *) echo "BAD CLASS $1" >&2; exit 1 ;;
  esac
}
CLASSES="none_ship size1_ship size1_m32 min_ship min_m32"

cls_rows() { echo $(( ($(wc -l < "$1") - 1) * $2 )); }

if [[ " $PHASES " == *" cont "* ]]; then
  say "=== PHASE cont: byte-continuity vs p2heads (new binary, env-off identity) ==="
  run_one "$OUTDIR/s4_cont_base.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1
  run_one "$OUTDIR/s4_cont_intra7.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $I7
fi

if [[ " $PHASES " == *" i5axis "* ]]; then
  say "=== PHASE i5axis: the top-5 knob response (coarse, t26) ==="
  run_one "$OUTDIR/s4_s6_i5.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $I5
  run_one "$OUTDIR/s4_s6_i5ship.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $SHIP $I5
  run_one "$OUTDIR/s4_s8_i5.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=8 CAVIF_EXTRA="--threads 1" $SIZE1 $I5
fi

if [[ " $PHASES " == *" filters "* ]]; then
  say "=== PHASE filters: hi-q CDEF/LRF probe (coarse, t26, on size1+ship) ==="
  run_one "$OUTDIR/s4_s6_cdef.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $SHIP \
    ZENRAVIF_CDEF=1 RD_CACHE_EXTRA=cdef1
  run_one "$OUTDIR/s4_s6_lrf.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $SHIP \
    ZENRAVIF_LRF=1 RD_CACHE_EXTRA=lrf1
fi

if [[ " $PHASES " == *" composed "* ]]; then
  say "=== PHASE composed: v3 classes 12q +i7 (t26) ==="
  for cls in $CLASSES; do
    smp="$HERE/sample_s4c_${cls}.tsv"
    [ -f "$smp" ] || { say "no $smp — skipping"; continue; }
    run_one "$OUTDIR/s4c_${cls}_i7.tsv" "$(cls_rows "$smp" 12)" "${common[@]}" \
      SAMPLE="$smp" QGRID_ZR="$QFULL" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" \
      $(cls_env "$cls") $I7
  done
fi

if [[ " $PHASES " == *" composedi5 "* ]]; then
  say "=== PHASE composedi5: v3 classes 12q +i5 (t26) ==="
  for cls in $CLASSES; do
    smp="$HERE/sample_s4c_${cls}.tsv"
    [ -f "$smp" ] || continue
    run_one "$OUTDIR/s4c_${cls}_i5.tsv" "$(cls_rows "$smp" 12)" "${common[@]}" \
      SAMPLE="$smp" QGRID_ZR="$QFULL" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" \
      $(cls_env "$cls") $I5
  done
fi

if [[ " $PHASES " == *" oraclex "* ]]; then
  say "=== PHASE oraclex: full-tx oracle extras 12q +i7 ==="
  for cls in full_ship full_m32; do
    smp="$HERE/sample_s4x_${cls}.tsv"
    [ -f "$smp" ] || continue
    run_one "$OUTDIR/s4x_${cls}_i7.tsv" "$(cls_rows "$smp" 12)" "${common[@]}" \
      SAMPLE="$smp" QGRID_ZR="$QFULL" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" \
      $(cls_env "$cls") $I7
  done
fi

if [[ " $PHASES " == *" timing "* ]]; then
  say "=== PHASE timing: solo walls (JOBS=1, RD_CACHE=off, q{40,65,85}) ==="
  tim=(RD_CACHE=off JOBS=1 QGRID_ZR="40 65 85")
  run_one "$OUTDIR/s4t_plain.tsv" 72 "${common[@]}" "${tim[@]}" \
    SAMPLE="$SAMPLE_T26" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1"
  for cls in $CLASSES; do
    smp="$HERE/sample_s4c_${cls}.tsv"
    [ -f "$smp" ] || continue
    run_one "$OUTDIR/s4t_${cls}_i7.tsv" "$(cls_rows "$smp" 3)" "${common[@]}" \
      "${tim[@]}" SAMPLE="$smp" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" \
      $(cls_env "$cls") $I7
    run_one "$OUTDIR/s4t_${cls}_i5.tsv" "$(cls_rows "$smp" 3)" "${common[@]}" \
      "${tim[@]}" SAMPLE="$smp" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" \
      $(cls_env "$cls") $I5
  done
  for cls in full_ship full_m32; do
    smp="$HERE/sample_s4x_${cls}.tsv"
    [ -f "$smp" ] || continue
    run_one "$OUTDIR/s4t_${cls}_i7.tsv" "$(cls_rows "$smp" 3)" "${common[@]}" \
      "${tim[@]}" SAMPLE="$smp" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" \
      $(cls_env "$cls") $I7
  done
  run_one "$OUTDIR/s4t_cdef4.tsv" 12 "${common[@]}" "${tim[@]}" \
    SAMPLE="$SAMPLE_TIM" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $SHIP \
    ZENRAVIF_CDEF=1 RD_CACHE_EXTRA=cdef1
  run_one "$OUTDIR/s4t_ship4.tsv" 12 "${common[@]}" "${tim[@]}" \
    SAMPLE="$SAMPLE_TIM" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1 $SHIP
fi

if [[ " $PHASES " == *" val "* ]]; then
  say "=== PHASE val: v3 classes on val14, 12q, +i7 and +i5 ==="
  for cls in none_ship size1_ship size1_m32 min_ship min_m32; do
    smp="$HERE/sample_s4valc_${cls}.tsv"
    [ -f "$smp" ] || continue
    run_one "$OUTDIR/s4v_${cls}_i7.tsv" "$(cls_rows "$smp" 12)" "${common[@]}" \
      SAMPLE="$smp" QGRID_ZR="$QFULL" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" \
      $(cls_env "$cls") $I7
    run_one "$OUTDIR/s4v_${cls}_i5.tsv" "$(cls_rows "$smp" 12)" "${common[@]}" \
      SAMPLE="$smp" QGRID_ZR="$QFULL" ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" \
      $(cls_env "$cls") $I5
  done
  # val bases for BD-vs-base rows (fresh binary => cache re-encode anyway)
  run_one "$OUTDIR/s4v_base.tsv" "$(cls_rows "$HERE/sample_p2val_all.tsv" 12)" \
    "${common[@]}" SAMPLE="$HERE/sample_p2val_all.tsv" QGRID_ZR="$QFULL" \
    ZENRAV1E_SPEED=6 CAVIF_EXTRA="--threads 1" $SIZE1
fi

say "chain complete."
