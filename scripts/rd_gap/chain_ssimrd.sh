#!/usr/bin/env bash
# SSIMRD chain (2026-07-05): per-16x16 ssim-rdmult lambda scaling port —
# the LAST unported iq/ss2 rdmult mechanism (aom av1_set_mb_ssim_rdmult_scaling
# / av1_set_ssim_rdmult at rev 632172a4; docs/TUNE_SSIMULACRA2_PLAN.md (a2),
# docs/RD_GAP_VS_LIBAOM.md TUNER2 "what remains"). Binary chain:
# ravif--ssimrd devpatch -> zenrav1e--ssimrd @ the knob change
# (EncoderConfig::ssim_rdmult_strength, default None = byte-identical; local
# gate: knob-off byte-identical to master-built cavif 36/36 + Some(0.0)==None
# + strength 1.0 byte-live). CONSTANTS ARE FIT PARAMETERS: the aom curve
# SHAPE ports verbatim, the strength (exponent blend on the normalized
# factor; 1.0 = aom curve) gets fit here. Phases:
#
#   base    env-off t26 12q under the NEW binary: the fresh current-master
#           baseline every arm below is BD'd against (tune-marginal-drift
#           rule: never diff arms against stale store rows).
#   coarse  strength arms {0.25, 0.5, 1.0, 2.0} x t26 6q — the response
#           shape (expected inverted-U by the boost/qmdist precedents).
#   valbase env-off val 12q (14 held-out origins).
#   full    WINNER (env WINNER=s) t26 12q + val 12q — TRAIN fit / VAL
#           confirm at the chosen strength.
#   s6      fast-tier parity spot check: env-off + WINNER at SPEED=6,
#           t26 6q (the mechanism must not regress the composed fast mode).
#   aomref  cpu2iq-ai + cpu2def-ai reference replays (cache-hot from the
#           speed-ladder program) for the class-movement columns
#           (1236 / 9094-family / 6018 vs aom-cpu2iq-ai).
#   timing  solo walls (RD_CACHE=off, JOBS=1): base vs WINNER.
#
# Every armed cell runs PALCONF (aomdec + rav1d-safe byte-agree) + BUTTER
# (per-cell butteraugli veto columns). Analysis: bd_arm.py ARM vs base
# (--all for the veto view), analyze.py vs the aomref TSVs.
#
# Run ON THE BOX under nohup:
#   OUTDIR=/home/lilith/sweep_out/ssimrd_20260705 \
#     PHASES="base coarse valbase" \
#     nohup bash chain_ssimrd.sh > $OUTDIR/chain.log 2>&1 &
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUTDIR="${OUTDIR:?set OUTDIR (per-run output dir)}"
mkdir -p "$OUTDIR"

export CAVIF="${CAVIF:-/home/lilith/work/zen/ravif--ssimrd/target/release/cavif}"
export SAVE_PNG="${SAVE_PNG:-/home/lilith/work/zen/zenavif/target/release/examples/save_png}"
export SCORER="${SCORER:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}"
export AOMDEC="${AOMDEC:-/home/lilith/work/aom/build_slow/aomdec}"
export AOMENC_BIN="${AOMENC:-/home/lilith/work/aom/build_slow/aomenc}"
export BUTTER="${BUTTER:-/home/lilith/work/butteraugli/target/release/butteraugli}"
export EXTRACT_AV1="${EXTRACT_AV1:-/home/lilith/work/zen/zenavif/target/release/examples/extract_av1}"
export IVF_RAW="${IVF_RAW:-/home/lilith/work/zen/zenavif/target/release/examples/ivf_raw}"
PHASES="${PHASES:-base coarse valbase}"

SAMPLE_T26="$HERE/sample_images_train26.tsv"    # 24 images
SAMPLE_VAL="$HERE/sample_p2val_all.tsv"         # 14 held-out origins
SAMPLE_TIM="$HERE/sample_timing4.tsv"
QCOARSE="30 50 60 75 85 95"
QFULL="30 40 50 55 60 65 70 75 80 85 90 95"

say() { echo "[ssimrd $(date -u +%H:%M:%SZ)] $*"; }

run_one() { # <out.tsv> <expected_rows> <script> [ENV=val ...]
  local out="$1" want="$2" script="$3"; shift 3
  if [ -s "$out" ] && [ "$(($(wc -l < "$out") - 1))" -ge "$want" ]; then
    say "SKIP (complete $(($(wc -l < "$out") - 1))/$want): $(basename "$out")"
    return 0
  fi
  local t0=$(date +%s)
  say "RUN $(basename "$out") [$*]"
  if env "$@" OUT="$out" bash "$HERE/$script" > "$out.log" 2>&1; then
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

if [[ " $PHASES " == *" base "* ]]; then
  say "=== PHASE base: env-off t26 12q fresh baseline ==="
  run_one "$OUTDIR/sr_base_t26.tsv" "$(rows "$SAMPLE_T26" 12)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_T26" QGRID_ZR="$QFULL"
fi

if [[ " $PHASES " == *" coarse "* ]]; then
  say "=== PHASE coarse: strength arms (t26, 6q) ==="
  for s in 0.25 0.5 1.0 2.0; do
    run_one "$OUTDIR/sr_str_${s}.tsv" "$(rows "$SAMPLE_T26" 6)" run_gap.sh "${common[@]}" \
      SAMPLE="$SAMPLE_T26" QGRID_ZR="$QCOARSE" ZENRAVIF_SSIMRD="$s"
  done
fi

if [[ " $PHASES " == *" valbase "* ]]; then
  say "=== PHASE valbase: env-off val 12q ==="
  run_one "$OUTDIR/sr_base_val.tsv" "$(rows "$SAMPLE_VAL" 12)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_VAL" QGRID_ZR="$QFULL"
fi

if [[ " $PHASES " == *" full "* ]]; then
  : "${WINNER:?set WINNER=<strength> after coarse analysis}"
  say "=== PHASE full: winner $WINNER t26+val 12q ==="
  run_one "$OUTDIR/sr_full_t26_${WINNER}.tsv" "$(rows "$SAMPLE_T26" 12)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_T26" QGRID_ZR="$QFULL" ZENRAVIF_SSIMRD="$WINNER"
  run_one "$OUTDIR/sr_full_val_${WINNER}.tsv" "$(rows "$SAMPLE_VAL" 12)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_VAL" QGRID_ZR="$QFULL" ZENRAVIF_SSIMRD="$WINNER"
fi

if [[ " $PHASES " == *" s6 "* ]]; then
  : "${WINNER:?set WINNER=<strength> after coarse analysis}"
  say "=== PHASE s6: fast-tier parity spot check (t26, 6q) ==="
  run_one "$OUTDIR/sr_s6_base.tsv" "$(rows "$SAMPLE_T26" 6)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_T26" QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6
  run_one "$OUTDIR/sr_s6_${WINNER}.tsv" "$(rows "$SAMPLE_T26" 6)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_T26" QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6 ZENRAVIF_SSIMRD="$WINNER"
fi

if [[ " $PHASES " == *" aomref "* ]]; then
  say "=== PHASE aomref: cpu2{iq,def}-ai reference replays (cache-hot) ==="
  run_one "$OUTDIR/aom_t26_cpu2iq.tsv" "$(rows "$SAMPLE_T26" 8)" aom_only.sh \
    SAMPLE="$SAMPLE_T26" JOBS=24 AOMENC="$AOMENC_BIN" AOM_CPU=2 \
    AOM_EXTRA="--allintra --tune=iq" AOMFMTS=420 BUTTER="$BUTTER"
  run_one "$OUTDIR/aom_t26_cpu2def.tsv" "$(rows "$SAMPLE_T26" 8)" aom_only.sh \
    SAMPLE="$SAMPLE_T26" JOBS=24 AOMENC="$AOMENC_BIN" AOM_CPU=2 \
    AOM_EXTRA="--allintra" AOMFMTS=420 BUTTER="$BUTTER"
  run_one "$OUTDIR/aom_val_cpu2iq.tsv" "$(rows "$SAMPLE_VAL" 8)" aom_only.sh \
    SAMPLE="$SAMPLE_VAL" JOBS=14 AOMENC="$AOMENC_BIN" AOM_CPU=2 \
    AOM_EXTRA="--allintra --tune=iq" AOMFMTS=420 BUTTER="$BUTTER"
fi

if [[ " $PHASES " == *" timing "* ]]; then
  : "${WINNER:?set WINNER=<strength>}"
  say "=== PHASE timing: solo walls (RD_CACHE=off, JOBS=1, 4 img x 3q) ==="
  run_one "$OUTDIR/sr_tim_base.tsv" "$(rows "$SAMPLE_TIM" 3)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_TIM" QGRID_ZR="40 65 85" JOBS=1 RD_CACHE=off
  run_one "$OUTDIR/sr_tim_${WINNER}.tsv" "$(rows "$SAMPLE_TIM" 3)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_TIM" QGRID_ZR="40 65 85" JOBS=1 RD_CACHE=off ZENRAVIF_SSIMRD="$WINNER"
fi

say "chain done."
