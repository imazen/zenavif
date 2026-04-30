#!/usr/bin/env bash
# Nightly LHS sweep — picks one tuple from training/rav1e_lhs_tuples_v0_2.json
# (round-robin by day-of-year), invokes predictor_sweep with the right flags,
# and appends to the year-stamped Phase 3 TSV.
#
# Run from cron via install_predictor_cron.sh's Phase 3+4 line.
set -euo pipefail

ZENAVIF_DIR="${ZENAVIF_DIR:-$HOME/work/zen/zenavif}"
TUPLES_JSON="${TUPLES_JSON:-$ZENAVIF_DIR/training/rav1e_lhs_tuples_v0_2.json}"
MANIFEST="${MANIFEST:-$HOME/work/codec-corpus/picker-train/manifest.tsv}"
YEAR="$(date +%Y)"
DOY="$(date +%j)" # day of year, 1..366
LOG_DIR="${LOG_DIR:-$ZENAVIF_DIR/cron-logs}"

cd "$ZENAVIF_DIR"
mkdir -p "$LOG_DIR"

if [[ ! -f "$TUPLES_JSON" ]]; then
  echo "error: $TUPLES_JSON not found — run training/lhs_tuples.py" >&2
  exit 1
fi

# Pick today's tuple (DOY mod N).
TUPLE=$(python3 -c "
import json, sys
data = json.load(open('$TUPLES_JSON'))
n = data['n']
i = int('$DOY') % n
t = data['tuples'][i]
print(i, t['qm'], t['vaq_strength'], t['seg_boost'], t['rdo_tx_off'],
      t['seg_complex_on'], t['bottomup_on'], t['lrf_on'], t['partition_range_idx'])
")
read -r IDX QM VAQ_STRENGTH SEG_BOOST RDO_TX_OFF SEG_COMPLEX_ON BOTTOMUP_ON LRF_ON PR_IDX <<< "$TUPLE"

# vaq is enabled iff vaq_strength != 1.0 (1.0 is preset/default).
if [[ "$VAQ_STRENGTH" == "1.0" ]]; then
  VAQ=false
else
  VAQ=true
fi

# qm bool from int.
if [[ "$QM" == "1" ]]; then QM_BOOL=true; else QM_BOOL=false; fi

OUTPUT="benchmarks/rav1e_phase3_${YEAR}.tsv"
echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) tuple[$IDX] qm=$QM vaq_strength=$VAQ_STRENGTH seg_boost=$SEG_BOOST rdo_tx_off=$RDO_TX_OFF seg_complex_on=$SEG_COMPLEX_ON bottomup_on=$BOTTOMUP_ON lrf_on=$LRF_ON pr_idx=$PR_IDX → $OUTPUT" \
  | tee -a "$LOG_DIR/lhs_sweep.log"

cargo run --release --example predictor_sweep \
  --features encode-imazen,encode-threading -- \
  --manifest "$MANIFEST" \
  --output "$OUTPUT" \
  --speeds 1..=10 \
  --qualities 5..=100:5 \
  --sizes 64,256,1024,4096 \
  --max-images 200 \
  --threads 16 --enc-threads 1 \
  --append \
  --qm "$QM_BOOL" \
  --vaq "$VAQ" \
  --vaq-strength "$VAQ_STRENGTH" \
  --tune-still true \
  --seg-boost "$SEG_BOOST" \
  --rdo-tx-off "$([[ $RDO_TX_OFF == 1 ]] && echo true || echo false)" \
  --seg-complex-on "$([[ $SEG_COMPLEX_ON == 1 ]] && echo true || echo false)" \
  --bottomup-on "$([[ $BOTTOMUP_ON == 1 ]] && echo true || echo false)" \
  --lrf-on "$([[ $LRF_ON == 1 ]] && echo true || echo false)" \
  --partition-range-idx "$PR_IDX" \
  >> "$LOG_DIR/lhs_sweep.log" 2>&1
