#!/usr/bin/env bash
# After Phase 1a sweep completes, run the full v0.1 finalization:
#   1. Re-extract features (idempotent --append; covers any new images
#      that landed during sweep)
#   2. Run train_bake_pipeline.sh
#   3. Copy artifacts into src/models/
#   4. Build zenavif with --features auto-tune
#   5. Run examples/auto_tune_smoke against a CID22 sample
#   6. Print a summary and exit 0 on success
set -euo pipefail

ZENAVIF_DIR="${ZENAVIF_DIR:-$HOME/work/zen/zenavif}"
ZENANALYZE_DIR="${ZENANALYZE_DIR:-$HOME/work/zen/zenanalyze}"
MANIFEST="${MANIFEST:-$HOME/work/codec-corpus/picker-train/manifest.tsv}"
SAMPLE_PNG="${SAMPLE_PNG:-$HOME/work/codec-corpus/CID22/CID22-512/training/1722183.png}"
PARETO="benchmarks/rav1e_phase1a_2026-04-30.tsv"
FEATURES="benchmarks/rav1e_phase1a_features_2026-04-30.tsv"
PREFIX="rav1e_picker_v0_1"

cd "$ZENAVIF_DIR"

if [[ ! -f "$PARETO" ]]; then
  echo "error: $PARETO not found — Phase 1a sweep didn't complete" >&2
  exit 1
fi

echo "=== 1. Re-extract features (incremental) ==="
cargo run --release --example extract_features --features encode-imazen -- \
  --manifest "$MANIFEST" \
  --output "$FEATURES" \
  --sizes 64,256,1024,4096 \
  --max-images 50 \
  --threads 4 \
  --append
echo

echo "=== 2. Train + bake + LUTs ==="
PARETO_TSV="$PARETO" FEATURES_TSV="$FEATURES" OUT_PREFIX="$PREFIX" \
  bash "$ZENAVIF_DIR/scripts/train_bake_pipeline.sh"
echo

echo "=== 3. Copy artifacts into src/models/ ==="
cp -v "benchmarks/${PREFIX}.bin" "src/models/${PREFIX}.bin"
cp -v "benchmarks/rav1e_encode_ms_lut_v0_1.json" "src/models/rav1e_encode_ms_lut_v0_1.json"
cp -v "benchmarks/rav1e_quality_lut_v0_1.json" "src/models/rav1e_quality_lut_v0_1.json"
echo

echo "=== 4. Build zenavif with --features auto-tune ==="
cargo build --release --features auto-tune,encode-imazen,encode-threading
echo

echo "=== 5. Smoke test ==="
cargo run --release --example auto_tune_smoke \
  --features auto-tune,encode-imazen,encode-threading -- \
  "$SAMPLE_PNG" 85.0 || {
  echo "smoke test FAILED — model didn't load or returned an error" >&2
  exit 1
}
echo

echo "=== 6. Summary ==="
ls -lh "src/models/${PREFIX}.bin" \
       "src/models/rav1e_encode_ms_lut_v0_1.json" \
       "src/models/rav1e_quality_lut_v0_1.json"
echo
echo "Done. Commit + push, then:"
echo "  jj describe -m 'feat(auto-tune): land v0.1 baked model + LUTs'"
echo "  jj bookmark set main -r @"
echo "  jj git push"
