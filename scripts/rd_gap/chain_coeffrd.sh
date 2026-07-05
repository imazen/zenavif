#!/usr/bin/env bash
# COEFF_RD_STACK chain (2026-07-05): the composed coefficient-level RD
# valuation stack — libaom's coupled "FP round-to-nearest quant + always-on
# per-coefficient RD descent" posture as ONE knob (docs/COEFF_RD_STACK.md;
# zenrav1e@3e5ff155 `EncoderConfig::coeff_rd_stack`, default None =
# byte-identical, 36/36 sha-gated vs a master rav1e). Binary chain:
# ravif--coeffrd devpatch (ravif@main d72304a + tune/palette/coeffrd env
# passthroughs) -> zenrav1e--coeffrd path dep @ master 3e5ff155.
#
# THE TWO PRIOR HALF-STACKS ARE MEASURED REJECTIONS (do not re-run):
# quant_rounding_bias=128 alone (+2.67 med, 20/23 ba vetoes, TUNER2 (2));
# enable_trellis forced alone (+0.32/+0.55 at 1.66x). This chain measures the
# COMPOSITION, which neither probe reached (the trellis was also hard-dead
# below ~Q80 via its ac_quant>=200 gate — bypassed by the armed posture).
#
# CONSTANTS ARE FIT PARAMETERS (iron lesson): the mechanism ports verbatim,
# the lambda scale / guards / rounding get fit here.
# Arm format: ZENRAVIF_COEFFRD="k:scale:guards:tuz"
#   A 128:0.1328:1:0  aom tune-ss2 posture verbatim (plane_rd_mult 17>>7)
#   B 128:0.35:1:0    mid lambda
#   C 128:1.0:1:0     rav1e-unit lambda policing, guards on
#   D 128:4.25:1:0    aom default-tune posture (17*8>>5) under our tune
#   E 128:1.0:0:0     control: unguarded (brackets the historical rejections)
#   F <winner>:...:1  winner + per-TU zero-out counterweight (row-13)
#
# Phases:
#   base     env-off t26 12q fresh baseline under the NEW binary
#            (tune-marginal-drift rule: never BD arms against stale store
#            rows) + BYTE-CONTINUITY gate vs the label store's zr-s2-tune
#            rows (bytes column must match 288/288 — proves the devpatch
#            binary is RD-identical to the store lineage before any arm).
#   coarse   arms A-E x t26 6q (s2).
#   dcbase   env-off doccharts 12q (the 6096-class near-lossless rescan
#            content is in-distribution HERE, not in train26).
#   dccoarse arms A-E x doccharts 6q.
#   full     WINNER t26 12q + doccharts 12q (+F if the per-family slices
#            show fam-7/flat over-keep).
#   s6       env-off + WINNER at SPEED=6, t26 6q (the wall shows at every
#            tier; fast-tier regression check).
#   s1probe  env-off + WINNER at SPEED=1, legacy 22-image corpus 12q — the
#            8-photo s1 residual class evidence (o_6629/o_5004/o_3008/
#            o_9051/... are VAL/TEST origins: REPORT-ONLY, never fit).
#   aomref   cpu2{iq,def}-ai reference replays (cache-hot) for gap columns.
#   timing   solo walls (RD_CACHE=off, JOBS=1): base vs WINNER (the armed
#            trellis runs in every RDO trial — expect >=1.66x; measure it).
#
# Every armed cell runs PALCONF (aomdec + rav1d-safe byte-agree) + BUTTER.
# Analysis: bd_arm.py ARM vs base (--all for the veto view); per-family
# FIRST per the 93b83401 evaluation policy (photos-merit KEEPABLE).
#
# Run ON THE BOX under nohup:
#   OUTDIR=/home/lilith/sweep_out/coeffrd_20260705 \
#     PHASES="base coarse dcbase dccoarse" \
#     nohup bash chain_coeffrd.sh > $OUTDIR/chain.log 2>&1 &
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUTDIR="${OUTDIR:?set OUTDIR (per-run output dir)}"
mkdir -p "$OUTDIR"

export CAVIF="${CAVIF:-/home/lilith/work/zen/ravif--coeffrd/target/release/cavif}"
export SAVE_PNG="${SAVE_PNG:-/home/lilith/work/zen/zenavif/target/release/examples/save_png}"
export SCORER="${SCORER:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}"
export AOMDEC="${AOMDEC:-/home/lilith/work/aom/build_slow/aomdec}"
export AOMENC_BIN="${AOMENC:-/home/lilith/work/aom/build_slow/aomenc}"
export BUTTER="${BUTTER:-/home/lilith/work/butteraugli/target/release/butteraugli}"
export EXTRACT_AV1="${EXTRACT_AV1:-/home/lilith/work/zen/zenavif/target/release/examples/extract_av1}"
export IVF_RAW="${IVF_RAW:-/home/lilith/work/zen/zenavif/target/release/examples/ivf_raw}"
PHASES="${PHASES:-base coarse dcbase dccoarse}"

SAMPLE_T26="$HERE/sample_images_train26.tsv"    # 24 images
SAMPLE_DC="$HERE/sample_doccharts.tsv"          # 15 doc/chart origins
SAMPLE_LEG="$HERE/sample_images.tsv"            # legacy 22 (s1probe, REPORT-ONLY)
SAMPLE_TIM="$HERE/sample_timing4.tsv"
QCOARSE="30 50 60 75 85 95"
QFULL="30 40 50 55 60 65 70 75 80 85 90 95"

# The five coarse arms (see header). F is assembled from WINNER at `full`.
ARM_A="128:0.1328:1:0"
ARM_B="128:0.35:1:0"
ARM_C="128:1.0:1:0"
ARM_D="128:4.25:1:0"
ARM_E="128:1.0:0:0"

say() { echo "[coeffrd $(date -u +%H:%M:%SZ)] $*"; }

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
  say "=== PHASE base: env-off t26 12q fresh baseline (byte-continuity gate) ==="
  run_one "$OUTDIR/cr_base_t26.tsv" "$(rows "$SAMPLE_T26" 12)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_T26" QGRID_ZR="$QFULL"
fi

if [[ " $PHASES " == *" coarse "* ]]; then
  say "=== PHASE coarse: posture arms A-E (t26, 6q, s2) ==="
  for arm in A B C D E; do
    v="ARM_$arm"; spec="${!v}"
    run_one "$OUTDIR/cr_${arm}_t26.tsv" "$(rows "$SAMPLE_T26" 6)" run_gap.sh "${common[@]}" \
      SAMPLE="$SAMPLE_T26" QGRID_ZR="$QCOARSE" ZENRAVIF_COEFFRD="$spec"
  done
fi

if [[ " $PHASES " == *" dcbase "* ]]; then
  say "=== PHASE dcbase: env-off doccharts 12q ==="
  run_one "$OUTDIR/cr_base_dc.tsv" "$(rows "$SAMPLE_DC" 12)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_DC" QGRID_ZR="$QFULL" JOBS=15
fi

if [[ " $PHASES " == *" dccoarse "* ]]; then
  say "=== PHASE dccoarse: posture arms A-E (doccharts, 6q) ==="
  for arm in A B C D E; do
    v="ARM_$arm"; spec="${!v}"
    run_one "$OUTDIR/cr_${arm}_dc.tsv" "$(rows "$SAMPLE_DC" 6)" run_gap.sh "${common[@]}" \
      SAMPLE="$SAMPLE_DC" QGRID_ZR="$QCOARSE" JOBS=15 ZENRAVIF_COEFFRD="$spec"
  done
fi

if [[ " $PHASES " == *" full "* ]]; then
  : "${WINNER:?set WINNER=<k:scale:guards:tuz> after coarse analysis}"
  wtag="${WINNER//:/_}"
  say "=== PHASE full: winner $WINNER t26+doccharts 12q ==="
  run_one "$OUTDIR/cr_full_t26_${wtag}.tsv" "$(rows "$SAMPLE_T26" 12)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_T26" QGRID_ZR="$QFULL" ZENRAVIF_COEFFRD="$WINNER"
  run_one "$OUTDIR/cr_full_dc_${wtag}.tsv" "$(rows "$SAMPLE_DC" 12)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_DC" QGRID_ZR="$QFULL" JOBS=15 ZENRAVIF_COEFFRD="$WINNER"
fi

if [[ " $PHASES " == *" s6 "* ]]; then
  : "${WINNER:?set WINNER=<spec> after coarse analysis}"
  wtag="${WINNER//:/_}"
  say "=== PHASE s6: fast-tier check (t26, 6q) ==="
  run_one "$OUTDIR/cr_s6_base.tsv" "$(rows "$SAMPLE_T26" 6)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_T26" QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6
  run_one "$OUTDIR/cr_s6_${wtag}.tsv" "$(rows "$SAMPLE_T26" 6)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_T26" QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=6 ZENRAVIF_COEFFRD="$WINNER"
fi

if [[ " $PHASES " == *" s1probe "* ]]; then
  : "${WINNER:?set WINNER=<spec> after coarse analysis}"
  wtag="${WINNER//:/_}"
  say "=== PHASE s1probe: legacy 22-image s1 (REPORT-ONLY val/test origins) ==="
  run_one "$OUTDIR/cr_s1_base_leg.tsv" "$(rows "$SAMPLE_LEG" 12)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_LEG" QGRID_ZR="$QFULL" ZENRAV1E_SPEED=1 JOBS=22
  run_one "$OUTDIR/cr_s1_${wtag}_leg.tsv" "$(rows "$SAMPLE_LEG" 12)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_LEG" QGRID_ZR="$QFULL" ZENRAV1E_SPEED=1 JOBS=22 ZENRAVIF_COEFFRD="$WINNER"
fi

if [[ " $PHASES " == *" aomref "* ]]; then
  say "=== PHASE aomref: cpu2{iq,def}-ai replays (cache-hot) ==="
  run_one "$OUTDIR/aom_t26_cpu2iq.tsv" "$(rows "$SAMPLE_T26" 8)" aom_only.sh \
    SAMPLE="$SAMPLE_T26" JOBS=24 AOMENC="$AOMENC_BIN" AOM_CPU=2 \
    AOM_EXTRA="--allintra --tune=iq" AOMFMTS=420 BUTTER="$BUTTER"
  run_one "$OUTDIR/aom_dc_cpu2iq.tsv" "$(rows "$SAMPLE_DC" 8)" aom_only.sh \
    SAMPLE="$SAMPLE_DC" JOBS=15 AOMENC="$AOMENC_BIN" AOM_CPU=2 \
    AOM_EXTRA="--allintra --tune=iq" AOMFMTS=420 BUTTER="$BUTTER"
fi

if [[ " $PHASES " == *" timing "* ]]; then
  : "${WINNER:?set WINNER=<spec>}"
  wtag="${WINNER//:/_}"
  say "=== PHASE timing: solo walls (RD_CACHE=off, JOBS=1, 4 img x 3q) ==="
  run_one "$OUTDIR/cr_tim_base.tsv" "$(rows "$SAMPLE_TIM" 3)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_TIM" QGRID_ZR="40 65 85" JOBS=1 RD_CACHE=off
  run_one "$OUTDIR/cr_tim_${wtag}.tsv" "$(rows "$SAMPLE_TIM" 3)" run_gap.sh "${common[@]}" \
    SAMPLE="$SAMPLE_TIM" QGRID_ZR="40 65 85" JOBS=1 RD_CACHE=off ZENRAVIF_COEFFRD="$WINNER"
fi

say "chain done."
