#!/usr/bin/env bash
# Q1 (GOAL queue): banded verdicts over the dense low-q re-grid.
# For each ref leg x armed leg: relabel the ref's encoder to the arm's so
# objective.py's single-encoder filter joins them, then record the banded
# ssim2 + ba3n BD masses. Output: one meta TSV (committed to benchmarks/).
set -eu
D="${1:-/mnt/v/output/cooptloop/q1-densegrid-2026-07-12}"
OUT="${2:-$D/q1_banded_meta.tsv}"
RG="$(cd "$(dirname "$0")" && pwd)"
relabel() { # $1 src -> $2 dst, force encoder col (5) to zenrav1e
  awk -F'\t' 'BEGIN{OFS="\t"} NR==1{print;next} {$5="zenrav1e";print}' "$1" > "$2"
}
printf 'ref\tarm\tmass_ssim2\tmass_ba3n\tband_low\tband_mid\tband_high\tba_bad\tba_mid\tba_good\tveto\n' > "$OUT"
for ref in aom_cpu6ss2ai aom_cpu4ss2ai aom_cpu2ss2ai aom_cpu0ss2ai svt_p0t4 svt_p2t4 svt_p6t4 svt_p10t4; do
  [ -f "$D/$ref.tsv" ] || { echo "# missing $ref.tsv — skipped" >&2; continue; }
  relabel "$D/$ref.tsv" "/tmp/q1v_$ref.tsv"
  for arm in arm420_s6 arm420_s9 arm420_s10; do
    python3 - "$RG" "/tmp/q1v_$ref.tsv" "$D/$arm.tsv" "$ref" "$arm" >> "$OUT" <<'PYEOF'
import json, subprocess, sys
rg, base, arm, rn, an = sys.argv[1:6]
j = json.loads(subprocess.run(
    ["python3", f"{rg}/objective.py", base, arm, "--json"],
    capture_output=True, text=True, check=True).stdout)
b = j.get("band_ssim2_bd") or j.get("band_bd") or {}
ba = j.get("band_ba3n_bd") or {}
def g(d, *keys):
    for k in keys:
        if k in d and d[k] is not None: return f"{d[k]:+.2f}"
    return "NA"
print("\t".join([rn, an,
    f"{j['mass_ssim2_bd']:+.2f}", f"{j['mass_butteraugli_3n_bd']:+.2f}",
    g(b, "low(<50)", "low"), g(b, "mid(50-75)", "mid"), g(b, "high(>75)", "high"),
    g(ba, "ba_bad(>3)"), g(ba, "ba_mid(1-3)"), g(ba, "ba_good(<1)"),
    str(j["vetoed"])]))
PYEOF
  done
done
column -t "$OUT"
