#!/usr/bin/env bash
# Q4 (GOAL queue) verdict — rule: DECISION_RULE_Q4_TWOPASS.md (read FIRST).
# Leg 1 (KERNEL-WORTHY test): twopass_s6 vs single_s6 — banded ssim2 BD
#   <= -2.0% in >=2 bands, veto-clean.
# Leg 2 (ladder-candidate test): twopass_s6 vs svt p0t4 solo + aom cpu2-ss2ai.
set -eu
D="${1:-/mnt/v/output/cooptloop/q4-twopass-2026-07-12}"
Q1D="${2:-/mnt/v/output/cooptloop/q1-densegrid-2026-07-12}"
SVTREF="${3:-/mnt/v/output/cooptloop/svt-ref-solo-2026-07-12/svt_ref.tsv}"
RG="$(cd "$(dirname "$0")" && pwd)"
norm() { awk -F'\t' -v E="${3:-}" -v P="${4:-}" 'BEGIN{OFS="\t"} NR==1{print;next}
  { if (P != "" && $6 != P) next; if (E != "") $5=E; sub(/\.png$/,"",$1); print }' "$1" > "$2"; }
run() { python3 "$RG/objective.py" "$1" "$2" 2>/dev/null | tail -4; }
echo "== KERNEL-WORTHY leg: twopass_s6 vs single_s6 (need <=-2.0% in >=2 bands, veto-clean) =="
norm "$D/single_s6.tsv" /tmp/q4_base.tsv
norm "$D/twopass_s6.tsv" /tmp/q4_arm.tsv
run /tmp/q4_base.tsv /tmp/q4_arm.tsv
echo "== ladder leg A: twopass_s6 vs svt p0t4 (solo) =="
norm "$SVTREF" /tmp/q4_svt.tsv zenrav1e p0t4
run /tmp/q4_svt.tsv /tmp/q4_arm.tsv
echo "== ladder leg B: twopass_s6 vs aom cpu2-ss2-allintra =="
norm "$Q1D/aom_cpu2ss2ai.tsv" /tmp/q4_aom.tsv zenrav1e
run /tmp/q4_aom.tsv /tmp/q4_arm.tsv
echo "== walls (median enc_ms): single=$(awk -F'\t' 'NR>1{print $11}' "$D/single_s6.tsv" | sort -n | awk '{a[NR]=$1} END{print a[int(NR/2)+1]}') twopass=$(awk -F'\t' 'NR>1{print $11}' "$D/twopass_s6.tsv" | sort -n | awk '{a[NR]=$1} END{print a[int(NR/2)+1]}') =="
