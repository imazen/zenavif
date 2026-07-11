#!/usr/bin/env bash
# COOPT Phase-1 dataset generator: decision traces over a corpus slice.
#
# For each image in SAMPLE (image<TAB>w<TAB>h<TAB>family) x each Q in QS:
# convert to PPM, run zenrav1e's cooptloop_trace_dump (threads=1, the trace
# discipline), store the trace TSV + one manifest row. Traces are LARGE
# (5-50 MB each) -> they go to OUTDIR on block storage, never git; the
# per-encode SUMMARY (analyze_cooptloop_trace.py --json per trace, one row
# each) is the small artifact that gets committed.
#
# Usage:
#   SAMPLE=sample_images_train26.tsv QS="60 100 160" SPEED=6 \
#   OUTDIR=/mnt/v/output/cooptloop/traces-$(date +%F) \
#   ~/work/zen/scripts/run-heavy -- bash gen_trace_corpus.sh
#
# Note QS are zenrav1e QUANTIZERS (0-255, higher = coarser), not cavif
# quality — the dump example takes the encoder-native knob.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
SAMPLE="${SAMPLE:-$HERE/sample_images_train26.tsv}"
QS="${QS:-60 100 160}"
SPEED="${SPEED:-6}"
OUTDIR="${OUTDIR:?set OUTDIR (block storage, e.g. /mnt/v/output/cooptloop/traces-YYYY-MM-DD)}"
DUMP="${DUMP:-/home/lilith/work/zen/zenrav1e/target/release/examples/cooptloop_trace_dump}"
ANALYZE="$HERE/analyze_cooptloop_trace.py"
LIMIT="${LIMIT:-0}"   # 0 = all sample rows

[ -x "$DUMP" ] || { echo "missing $DUMP (cargo build --release --features cooptloop_trace --example cooptloop_trace_dump)" >&2; exit 2; }
mkdir -p "$OUTDIR"
manifest="$OUTDIR/manifest.tsv"
summary="$OUTDIR/summary.tsv"
echo -e "image\tfamily\tspeed\tquantizer\ttrace\trows\tbytes" > "$manifest"
wrote_summary_header=0

n=0
tail -n +2 "$SAMPLE" | while IFS=$'\t' read -r img w h fam; do
  [ -f "$img" ] || { echo "skip missing $img" >&2; continue; }
  n=$((n+1)); [ "$LIMIT" -gt 0 ] && [ "$n" -gt "$LIMIT" ] && break
  base=$(basename "$img" .png)
  ppm="$OUTDIR/.tmp_$base.ppm"
  convert "$img" "$ppm" || { echo "convert failed: $img" >&2; continue; }
  for q in $QS; do
    trace="$OUTDIR/trace_${base}_s${SPEED}_q${q}.tsv"
    log=$("$DUMP" "$trace" "$ppm" --speed "$SPEED" --quantizer "$q" 2>&1) || {
      echo "dump failed: $base q$q: $log" >&2; continue; }
    rows=$(($(wc -l < "$trace") - 1))
    ebytes=$(echo "$log" | grep -oE '\-> [0-9]+ B' | grep -oE '[0-9]+' | head -1)
    echo -e "$base\t$fam\t$SPEED\t$q\t$(basename "$trace")\t$rows\t${ebytes:-NA}" >> "$manifest"
    # One summary row per encode (flat JSON -> TSV via python).
    python3 - "$ANALYZE" "$trace" "$base" "$fam" "$SPEED" "$q" "$summary" "$wrote_summary_header" <<'EOF'
import json, subprocess, sys
analyze, trace, base, fam, speed, q, summary, wrote = sys.argv[1:9]
out = subprocess.run(["python3", analyze, trace, "--json"],
                     capture_output=True, text=True, check=True).stdout
r = json.loads(out)
g = r.get("runner_up_gap", {})
row = {
  "image": base, "family": fam, "speed": speed, "quantizer": q,
  "n_evals": r["n_evals"], "n_decisions": r["n_decisions"],
  "scaled_frac": r["scaled_eval_fraction"],
  "lambda_min": r["lambda"]["min"], "lambda_max": r["lambda"]["max"],
  "lambda_distinct": r["lambda"]["distinct"],
  "gap_p25": g.get("p25"), "gap_p50": g.get("p50"), "gap_p75": g.get("p75"),
  "contested_lt_2pct": g.get("contested_lt_2pct"), "gap_n": g.get("n"),
  "evals_per_scope_p50": r["evals_per_scope_p50"],
  "skip_frac": r["skip_fraction"],
}
import os
new = not os.path.exists(summary) or os.path.getsize(summary) == 0
with open(summary, "a") as f:
    if new:
        f.write("\t".join(row.keys()) + "\n")
    f.write("\t".join(str(v) for v in row.values()) + "\n")
EOF
  done
  rm -f "$ppm"
  echo "[trace-corpus] $n: $base done ($(date -u +%H:%M:%SZ))"
done
echo "[trace-corpus] manifest: $manifest"
echo "[trace-corpus] summary:  $summary"
