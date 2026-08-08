#!/usr/bin/env bash
# Wait for the main sweep to finish, then run the 1024 frontier top-up and the
# whole reduction. One chain so the remaining hours need no supervision; every
# stage appends to sweep.log and each analysis lands in its own file.
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${OUT:-$HOME/tmp/encrd2}"
export OUT

while pgrep -f run_full_sweep.sh >/dev/null 2>&1; do sleep 30; done
echo "=== $(date -u +%H:%M:%SZ) main sweep process gone" | tee -a "$OUT/sweep.log"

if grep -q "ALL DONE" "$OUT/sweep.log" 2>/dev/null; then
  bash "$REPO/scripts/encode_rd/run_topup_1024.sh"
else
  echo "=== main sweep did NOT reach ALL DONE — skipping top-up, analysing what exists" \
    | tee -a "$OUT/sweep.log"
fi

bash "$REPO/scripts/encode_rd/merge_and_analyze.sh" 2>&1 | tee -a "$OUT/sweep.log"
echo "=== $(date -u +%H:%M:%SZ) PIPELINE COMPLETE" | tee -a "$OUT/sweep.log"
