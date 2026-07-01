#!/usr/bin/env bash
# RD-gap sweep driver: zenrav1e (cavif, s2, Q-grid) vs libaom (aomenc, cpu2, fmt x cq-grid).
# Reads a corpus TSV (image<TAB>w<TAB>h<TAB>family), encodes+decodes+scores every cell with
# BOTH encoders, writes one unified TSV. The libaom side is OPTIONAL: unset AOMENC to sweep
# only zenrav1e (then diff vs the committed baseline to track a change).
#
# Run it under the resource guard so it can't peg the shared box:
#   ~/work/zen/scripts/run-heavy -- bash run_gap.sh
#
# Required env (see README.md): CAVIF, SAVE_PNG, SCORER  (+ AOMENC, AOMDEC for the gap).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SAMPLE="${SAMPLE:-$HERE/sample_images.tsv}"
OUT="${OUT:-$HERE/rd_gap_results.tsv}"
WORK="${WORK:-/mnt/v/output/zenavif/rd_gap_work}"; mkdir -p "$WORK"
JOBS="${JOBS:-6}"
QGRID_ZR="${QGRID_ZR:-30 40 50 55 60 65 70 75 80 85 90 95}"    # cavif quality (higher = better)
CQGRID_AOM="${CQGRID_AOM:-8 16 24 32 40 48 56 63}"             # aomenc cq-level (lower = better)
AOMFMTS="${AOMFMTS:-420 444}"
export COLOR="$HERE/color.py"

echo -e "image\tw\th\tfamily\tencoder\tfmt\tq\tbytes\tbpp\tssim2\tenc_ms" > "$OUT"
[ -n "${AOMENC:-}" ] && echo "[rd_gap] both encoders (zenrav1e + libaom)" || echo "[rd_gap] zenrav1e only (AOMENC unset)"

worker() {
  local img="$1" w="$2" h="$3" fam="$4"
  local tmp="$WORK/$(basename "$img" .png)"; mkdir -p "$tmp"
  local part="$tmp/rows.tsv"; : > "$part"
  local bn; bn=$(basename "$img")
  local q fmt r
  for q in $QGRID_ZR; do
    r=$(bash "$HERE/zenrav1e_cell.sh" "$img" "$w" "$h" "$fam" "$q" "$tmp" 2>>"$tmp/err.log")
    [[ "$r" == zenrav1e* ]] && printf '%s\t%s\t%s\t%s\t%s\n' "$bn" "$w" "$h" "$fam" "$r" >> "$part"
  done
  if [ -n "${AOMENC:-}" ]; then
    for fmt in $AOMFMTS; do for q in $CQGRID_AOM; do
      r=$(bash "$HERE/aom_cell.sh" "$img" "$w" "$h" "$fam" "$fmt" "$q" "$tmp" 2>>"$tmp/err.log")
      [[ "$r" == libaom* ]] && printf '%s\t%s\t%s\t%s\t%s\n' "$bn" "$w" "$h" "$fam" "$r" >> "$part"
    done; done
  fi
  cat "$part" >> "$OUT"
  echo "  [$(date -u +%H:%M:%SZ)] done $bn rows=$(wc -l < "$part")"
  rm -rf "$tmp"
}

running=0
while IFS=$'\t' read -r img w h fam; do
  [ -z "${img:-}" ] && continue
  [ -f "$img" ] || { echo "  skip missing: $img"; continue; }
  worker "$img" "$w" "$h" "$fam" &
  running=$((running+1)); if (( running >= JOBS )); then wait -n; running=$((running-1)); fi
done < <(tail -n +2 "$SAMPLE")
wait
echo "[rd_gap] COMPLETE rows=$(($(wc -l < "$OUT")-1)) -> $OUT"
echo "[rd_gap] analyze:  python3 $HERE/analyze.py $OUT"
