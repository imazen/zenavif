#!/usr/bin/env bash
# Run the full Phase 4b/4c pipeline: train_hybrid → bake_picker → LUTs.
#
# Inputs (from $ZENAVIF_DIR/benchmarks/):
#   rav1e_phase1a_<DATE>.tsv          (predictor_sweep output)
#   rav1e_phase1a_features_<DATE>.tsv (extract_features output)
#
# Outputs (also in benchmarks/):
#   rav1e_picker_v0_1.json             (sklearn training output)
#   rav1e_picker_v0_1.bin              (zenpredict-bake binary)
#   rav1e_picker_v0_1.manifest.json    (legacy manifest sidecar)
#   rav1e_encode_ms_lut_v0_1.json      (per-(speed, size) LUT)
#   rav1e_quality_lut_v0_1.json        (per-(cell, target_zq) → q LUT)
#
# Usage:
#   ~/work/zen/zenavif/scripts/train_bake_pipeline.sh
#
# Env overrides:
#   ZENAVIF_DIR    default ~/work/zen/zenavif
#   ZENANALYZE_DIR default ~/work/zen/zenanalyze
#   PARETO_TSV     default benchmarks/rav1e_phase1a_2026-04-30.tsv
#   FEATURES_TSV   default benchmarks/rav1e_phase1a_features_2026-04-30.tsv
#   OUT_PREFIX     default rav1e_picker_v0_1
#   ALLOW_UNSAFE   set to "1" to bake even if safety gates fail

set -euo pipefail

ZENAVIF_DIR="${ZENAVIF_DIR:-$HOME/work/zen/zenavif}"
ZENANALYZE_DIR="${ZENANALYZE_DIR:-$HOME/work/zen/zenanalyze}"
PARETO_TSV="${PARETO_TSV:-benchmarks/rav1e_phase1a_2026-04-30.tsv}"
FEATURES_TSV="${FEATURES_TSV:-benchmarks/rav1e_phase1a_features_2026-04-30.tsv}"
OUT_PREFIX="${OUT_PREFIX:-rav1e_picker_v0_1}"
ALLOW_UNSAFE="${ALLOW_UNSAFE:-0}"

cd "$ZENAVIF_DIR"

if [[ ! -f "$PARETO_TSV" ]]; then
  echo "error: $PARETO_TSV not found — run predictor_sweep first" >&2
  exit 1
fi
if [[ ! -f "$FEATURES_TSV" ]]; then
  echo "error: $FEATURES_TSV not found — run extract_features first" >&2
  exit 1
fi

echo "=== Phase 4b: train_hybrid ==="
TRAIN_LOG="benchmarks/${OUT_PREFIX}.log"
TRAIN_JSON="benchmarks/${OUT_PREFIX}.json"

# train_hybrid reads PARETO/FEATURES/OUT_JSON from the codec config
# module, so we keep the config the source of truth and don't re-pass
# the paths on the cmdline. The user can override with env vars if
# they swap dates.
PYTHONPATH="$ZENAVIF_DIR/training:$ZENANALYZE_DIR/zentrain/tools" \
  python3 "$ZENANALYZE_DIR/zentrain/tools/train_hybrid.py" \
    --codec-config rav1e_picker_config 2>&1 | tee "$TRAIN_LOG"
echo "  → $TRAIN_JSON"
echo "  → $TRAIN_LOG"

echo
echo "=== Phase 4c: bake_picker → ZNPR v2 ==="
BAKE_BIN="benchmarks/${OUT_PREFIX}.bin"
BAKE_FLAGS=()
if [[ "$ALLOW_UNSAFE" == "1" ]]; then
  BAKE_FLAGS+=(--allow-unsafe)
  echo "  ALLOW_UNSAFE=1 — bypassing safety gates"
fi

python3 "$ZENANALYZE_DIR/tools/bake_picker.py" \
  --model "$TRAIN_JSON" \
  --out "$BAKE_BIN" \
  --dtype f16 \
  --bake-bin "$ZENANALYZE_DIR/target/release/zenpredict-bake" \
  "${BAKE_FLAGS[@]}"
echo "  → $BAKE_BIN ($(du -h "$BAKE_BIN" | cut -f1))"

echo
echo "=== Build encode_ms LUT ==="
LUT_MS="benchmarks/rav1e_encode_ms_lut_${OUT_PREFIX#rav1e_picker_}.json"
python3 "$ZENAVIF_DIR/training/build_encode_ms_lut.py" \
  --pareto "$PARETO_TSV" \
  --output "$LUT_MS"
echo "  → $LUT_MS"

echo
echo "=== Build quality LUT (target_zq → q per cell) ==="
LUT_Q="benchmarks/rav1e_quality_lut_${OUT_PREFIX#rav1e_picker_}.json"
python3 "$ZENAVIF_DIR/training/build_quality_lut.py" \
  --pareto "$PARETO_TSV" \
  --output "$LUT_Q"
echo "  → $LUT_Q"

echo
echo "=== Pipeline complete ==="
echo "  Drop these into src/models/ and rebuild zenavif with --features auto-tune."
echo
ls -lh "$BAKE_BIN" "$LUT_MS" "$LUT_Q"
