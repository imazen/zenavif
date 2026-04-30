#!/usr/bin/env bash
# Install crontab entries for the rav1e knob predictor pipeline.
#
# Two jobs land in the user's crontab (idempotent — re-run-safe):
#
#   1. Nightly Phase 3+4 sweep on the full picker-train corpus.
#      Resumable via --append; runs single-threaded per-encode with
#      16-way rayon across the (image, size, speed, q) tuple space.
#      Appends to a shared rav1e_phase34_<YEAR>.tsv that grows over weeks.
#
#   2. Weekly retrain + bake + verify gate (after Phase 5 lands).
#      Pulls latest benchmarks/ TSVs, runs zentrain/tools/train_hybrid.py +
#      bake_picker.py, runs round-trip + adversarial gates, and writes a
#      new model bin. Manual review before committing.
#
# Usage:
#   ~/work/zen/zenavif/scripts/install_predictor_cron.sh         # install
#   ~/work/zen/zenavif/scripts/install_predictor_cron.sh --remove # uninstall

set -euo pipefail

ZENAVIF_DIR="${HOME}/work/zen/zenavif"
ZENANALYZE_DIR="${HOME}/work/zen/zenanalyze"
MANIFEST="${HOME}/work/codec-corpus/picker-train/manifest.tsv"
LOG_DIR="${HOME}/work/zen/zenavif/cron-logs"
YEAR="$(date +%Y)"

MARKER_BEGIN="# >>> zenavif rav1e predictor cron (managed by install_predictor_cron.sh) >>>"
MARKER_END="# <<< zenavif rav1e predictor cron <<<"

remove_block() {
  if crontab -l 2>/dev/null | grep -q "^${MARKER_BEGIN}"; then
    crontab -l | sed "/^${MARKER_BEGIN}\$/,/^${MARKER_END}\$/d" | crontab -
    echo "Removed existing zenavif predictor cron block."
  fi
}

if [[ "${1:-}" == "--remove" ]]; then
  remove_block
  exit 0
fi

if [[ ! -d "$ZENAVIF_DIR" ]]; then
  echo "error: $ZENAVIF_DIR does not exist" >&2
  exit 1
fi
if [[ ! -f "$MANIFEST" ]]; then
  echo "error: manifest $MANIFEST not found" >&2
  exit 1
fi

mkdir -p "$LOG_DIR"
remove_block

# ---- Build the new block ----
TMP="$(mktemp)"
{
  crontab -l 2>/dev/null || true
  cat <<EOF
${MARKER_BEGIN}
# Phase 3+4 — nightly large-corpus sweep at 2:30am local. Resumable.
# Appends to rav1e_phase34_${YEAR}.tsv; new (image, size, speed, q) tuples
# only (existing rows skipped via --append key).
30 2 * * * cd $ZENAVIF_DIR && /usr/bin/env -i PATH="\$HOME/.cargo/bin:/usr/bin:/bin" cargo run --release --example predictor_sweep --features encode-imazen,encode-threading -- --manifest $MANIFEST --output benchmarks/rav1e_phase34_${YEAR}.tsv --speeds 1..=10 --qualities 5..=100:5 --sizes 64,256,1024,4096 --max-images 200 --threads 16 --enc-threads 1 --append >> $LOG_DIR/phase34.log 2>&1

# Phase 4a — feature extraction nightly at 2:00am local (before sweep).
# Idempotent via --append; new (image, size) tuples only.
0 2 * * * cd $ZENAVIF_DIR && /usr/bin/env -i PATH="\$HOME/.cargo/bin:/usr/bin:/bin" cargo run --release --example extract_features --features encode-imazen -- --manifest $MANIFEST --output benchmarks/rav1e_phase34_features_${YEAR}.tsv --sizes 64,256,1024,4096 --max-images 200 --threads 4 --append >> $LOG_DIR/features.log 2>&1

# Weekly retrain + bake + safety gates Sundays 6am local.
# Manual review of the produced model before committing.
0 6 * * 0 cd $ZENAVIF_DIR && /usr/bin/env -i PATH="\$HOME/.cargo/bin:/usr/bin:/bin" PYTHONPATH=$ZENAVIF_DIR/training:$ZENANALYZE_DIR/zentrain/tools python3 $ZENANALYZE_DIR/zentrain/tools/train_hybrid.py --codec-config rav1e_picker_config >> $LOG_DIR/train.log 2>&1
${MARKER_END}
EOF
} > "$TMP"

crontab "$TMP"
rm -f "$TMP"

echo "Installed:"
crontab -l | sed -n "/^${MARKER_BEGIN}\$/,/^${MARKER_END}\$/p"
echo
echo "Logs in: $LOG_DIR"
echo "Remove with: $0 --remove"
