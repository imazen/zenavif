#!/usr/bin/env bash
# TUNER2 chain (2026-07-04, the P3-residual handoffs: iq-AQ class + 6096
# coefficient-level no-skip; docs/RD_GAP_VS_LIBAOM.md "Near-lossless rescans
# residual"). Binary chain: ravif--tuner2 devpatch -> zenrav1e--tuner2 @ the
# knobs change (variance_boost_strength / variance_boost_deep /
# quant_rounding_bias; local gate: knobs-off byte-identical to master-built
# cavif 36/36 + str1.0==None, all three knobs byte-live). Phases:
#
#   cont    byte-continuity: 8-image env-off s2+tune 12q re-encode under the
#           NEW binary; offline-compare bytes vs the label store's
#           speedladder/zr-s2-tune rows (proves the store rows remain valid
#           same-binary base curves for every arm below).
#   valstr  THE data gap the parked boost head named: strength arms
#           {0,1,2,3,4.5} x 14 held-out val origins x 12q, s2+tune.
#           Gives the refit its VAL labels (6091 1-bit rescan + 9165
#           illustration are the deep-AQ transfer probes).
#   deep    the deeper-curve tune arms (aom {36,64}-style spread): the
#           deep-flat ramp at {3.0:4, 4.5:4} on train26, 6q coarse.
#   dz      the 6096 dead-zone/rounding probe: QROUND {118, 128} on
#           train26, 6q coarse (128 = aom sharpness!=0 parity, dead-zone
#           removal; the +8%-bytes-at-same-q no-skip mechanism).
#   deepval deep winner on the val origins (6q) — run after coarse analysis.
#   dzfull  QROUND winner full 12q on train26 — run after coarse analysis.
#   timing  solo walls for landing candidates (RD_CACHE=off, JOBS=1).
#
# Run ON THE BOX under nohup:
#   OUTDIR=/home/lilith/sweep_out/tuner2_20260704 \
#     PHASES="cont valstr deep dz" \
#     nohup bash chain_tuner2.sh > $OUTDIR/chain.log 2>&1 &
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUTDIR="${OUTDIR:?set OUTDIR (per-run output dir)}"
mkdir -p "$OUTDIR"

export CAVIF="${CAVIF:-/home/lilith/work/zen/ravif--tuner2/target/release/cavif}"
export SAVE_PNG="${SAVE_PNG:-/home/lilith/work/zen/zenavif/target/release/examples/save_png}"
export SCORER="${SCORER:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}"
export AOMDEC="${AOMDEC:-/home/lilith/work/aom/build_slow/aomdec}"
export BUTTER="${BUTTER:-/home/lilith/work/butteraugli/target/release/butteraugli}"
export EXTRACT_AV1="${EXTRACT_AV1:-/home/lilith/work/zen/zenavif/target/release/examples/extract_av1}"
export IVF_RAW="${IVF_RAW:-/home/lilith/work/zen/zenavif/target/release/examples/ivf_raw}"
PHASES="${PHASES:-cont valstr deep dz}"

SAMPLE_T26="$HERE/sample_images_train26.tsv"    # 24 images
SAMPLE_VAL="$HERE/sample_p2val_all.tsv"         # 14 held-out origins
SAMPLE_C8="$HERE/sample_tuner2_cont8.tsv"       # 8-image continuity subset
SAMPLE_TIM="$HERE/sample_timing4.tsv"
QCOARSE="30 50 60 75 85 95"
QFULL="30 40 50 55 60 65 70 75 80 85 90 95"

say() { echo "[tuner2 $(date -u +%H:%M:%SZ)] $*"; }

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
        ZENRAV1E_SPEED=2 CAVIF_EXTRA="--threads 1" JOBS=24)

rows() { echo $(( ($(wc -l < "$1") - 1) * $2 )); }

if [[ " $PHASES " == *" cont "* ]]; then
  say "=== PHASE cont: 8-image env-off byte-continuity vs speedladder/zr-s2-tune ==="
  run_one "$OUTDIR/t2_cont8.tsv" "$(rows "$SAMPLE_C8" 12)" "${common[@]}" \
    SAMPLE="$SAMPLE_C8" QGRID_ZR="$QFULL"
fi

if [[ " $PHASES " == *" valstr "* ]]; then
  say "=== PHASE valstr: strength arms on the 14 val origins (12q) ==="
  for s in 0.0 1.0 2.0 3.0 4.5; do
    run_one "$OUTDIR/t2_valstr_${s}.tsv" "$(rows "$SAMPLE_VAL" 12)" "${common[@]}" \
      SAMPLE="$SAMPLE_VAL" QGRID_ZR="$QFULL" ZENRAVIF_VB_STRENGTH="$s"
  done
fi

if [[ " $PHASES " == *" deep "* ]]; then
  say "=== PHASE deep: deep-flat ramp arms (t26, coarse) ==="
  for d in 3.0:4 4.5:4; do
    run_one "$OUTDIR/t2_deep_${d/:/_}.tsv" "$(rows "$SAMPLE_T26" 6)" "${common[@]}" \
      SAMPLE="$SAMPLE_T26" QGRID_ZR="$QCOARSE" ZENRAVIF_VB_DEEP="$d"
  done
fi

if [[ " $PHASES " == *" dz "* ]]; then
  say "=== PHASE dz: quantizer rounding-bias arms (t26, coarse) ==="
  for k in 118 128; do
    run_one "$OUTDIR/t2_dz_${k}.tsv" "$(rows "$SAMPLE_T26" 6)" "${common[@]}" \
      SAMPLE="$SAMPLE_T26" QGRID_ZR="$QCOARSE" ZENRAVIF_QROUND="$k"
  done
fi

if [[ " $PHASES " == *" drift "* ]]; then
  say "=== PHASE drift: strength-response stability across binary generations ==="
  # The 2026-07-02 deltaq train labels predate qmdist+lfsharp landing into the
  # tune. If BD(str4.5 vs str0) on 6018/2000/9118 under THIS binary matches the
  # old labels (-4.14/-2.46/-4.10), train-fit rules transfer to the new-binary
  # val labels without a drift confound. str1 comes from the cont rows.
  for s in 0.0 4.5; do
    run_one "$OUTDIR/t2_drift_${s}.tsv" "$(rows "$HERE/sample_tuner2_drift3.tsv" 12)" "${common[@]}" \
      SAMPLE="$HERE/sample_tuner2_drift3.tsv" QGRID_ZR="$QFULL" ZENRAVIF_VB_STRENGTH="$s"
  done
fi

if [[ " $PHASES " == *" deepval "* ]]; then
  say "=== PHASE deepval: deep winner on val (6q) ==="
  run_one "$OUTDIR/t2_deepval_${DEEP_WINNER/:/_}.tsv" "$(rows "$SAMPLE_VAL" 6)" "${common[@]}" \
    SAMPLE="$SAMPLE_VAL" QGRID_ZR="$QCOARSE" ZENRAVIF_VB_DEEP="${DEEP_WINNER:?set DEEP_WINNER=s:c}"
fi

if [[ " $PHASES " == *" dzfull "* ]]; then
  say "=== PHASE dzfull: QROUND winner full grid (t26) ==="
  run_one "$OUTDIR/t2_dzfull_${DZ_WINNER:?set DZ_WINNER}.tsv" "$(rows "$SAMPLE_T26" 12)" "${common[@]}" \
    SAMPLE="$SAMPLE_T26" QGRID_ZR="$QFULL" ZENRAVIF_QROUND="$DZ_WINNER"
fi

if [[ " $PHASES " == *" timing "* ]]; then
  say "=== PHASE timing: solo walls (RD_CACHE=off, JOBS=1, 4 img x 3q) ==="
  for arm in base "QROUND=${DZ_WINNER:-128}" "VB_DEEP=${DEEP_WINNER:-4.5:4}"; do
    tag=${arm/=/_}; tag=${tag/:/_}
    extra=()
    [ "$arm" != base ] && extra=("ZENRAVIF_${arm%%=*}=${arm#*=}")
    run_one "$OUTDIR/t2_tim_${tag}.tsv" "$(rows "$SAMPLE_TIM" 3)" "${common[@]}" \
      SAMPLE="$SAMPLE_TIM" QGRID_ZR="40 65 85" JOBS=1 RD_CACHE=off \
      "${extra[@]+"${extra[@]}"}"
  done
fi

say "chain done."
