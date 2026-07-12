#!/usr/bin/env bash
# Q2 (GOAL queue) coarse SELECTION over the graft arms — candidate search, not
# verdict (rule: DECISION_RULE_Q2_HOLE.md). For each graft arm: 12-job median
# wall + banded ssim2 BD + veto vs the SOLO svt p1t4 / p2t4 reference cells.
# Winners advance to the dense confirm + JOBS=1 timing pass.
set -eu
D="${1:-/mnt/v/output/cooptloop/q2-hole-2026-07-12}"
REF="${2:-/mnt/v/output/cooptloop/svt-ref-solo-2026-07-12/svt_ref.tsv}"
OUT="${3:-$D/q2_selection.tsv}"
RG="$(cd "$(dirname "$0")" && pwd)"
for p in p1t4 p2t4; do
  awk -F'\t' -v P="$p" 'BEGIN{OFS="\t"} NR==1{print;next} $6==P{$5="zenrav1e";sub(/\.png$/,"",$1);print}' \
    "$REF" > "/tmp/q2ref_$p.tsv"
done
printf 'arm\tmed_wall_ms\tref\tref_med_wall\tmass_ssim2\tband_low\tband_mid\tband_high\tveto\n' > "$OUT"
for arm in base i7 prune txd2 txmin i7prune; do
  A0="$D/s9_$arm.tsv"; [ -f "$A0" ] || { echo "# missing $A0" >&2; continue; }
  A="/tmp/q2arm_$arm.tsv"
  awk -F'\t' 'BEGIN{OFS="\t"} NR==1{print;next} {sub(/\.png$/,"",$1);print}' "$A0" > "$A"
  W=$(awk -F'\t' 'NR>1{print $11}' "$A" | sort -n | awk '{a[NR]=$1} END{print a[int(NR/2)+1]}')
  for p in p1t4 p2t4; do
    RW=$(awk -F'\t' 'NR>1{print $11}' "/tmp/q2ref_$p.tsv" | sort -n | awk '{a[NR]=$1} END{print a[int(NR/2)+1]}')
    python3 - "$RG" "/tmp/q2ref_$p.tsv" "$A" "$arm" "$W" "$p" "$RW" >> "$OUT" <<'PYEOF'
import json, subprocess, sys
rg, base, arm, an, w, p, rw = sys.argv[1:8]
j = json.loads(subprocess.run(
    ["python3", f"{rg}/objective.py", base, arm, "--json"],
    capture_output=True, text=True, check=True).stdout)
b = j["band_ssim2_bd"]
def g(k):
    for kk in b:
        if kk.startswith(k):
            v = b[kk]
            return f"{v:+.2f}" if v == v else "NA"
    return "NA"
print("\t".join([an, w, p, rw, f"{j['mass_ssim2_bd']:+.2f}",
                 g("low"), g("mid"), g("high"), str(j["vetoed"])]))
PYEOF
  done
done
column -t "$OUT"
