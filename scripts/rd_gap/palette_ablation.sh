#!/usr/bin/env bash
# Isolates how much of the zenrav1e-vs-libaom RD gap (docs/RD_GAP_VS_LIBAOM.md) is
# specifically attributable to libaom's palette mode, which zenrav1e's encoder does
# not implement at all (see the doc's "Credible narrowing levers" #1 + zenrav1e#2).
# Runs aomenc TWICE per (image, cq) — default (palette on) and --enable-palette=0 —
# on the SAME sample corpus as run_gap.sh, format 420 only. Same cq-level between the
# two runs means no frontier interpolation is needed for the raw-bpp comparison; we
# still score+frontier for a matched-ssim2 comparison since mode decisions (and thus
# achieved quality at a given cq) can differ slightly with palette off.
#
# Required env: AOMENC, AOMDEC, SCORER (see README.md).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SAMPLE="${SAMPLE:-$HERE/sample_images.tsv}"
OUT="${OUT:-$HERE/palette_ablation_results.tsv}"
WORK="${WORK:-/mnt/v/output/zenavif/rd_gap_work/palette_ablation}"; mkdir -p "$WORK"
JOBS="${JOBS:-6}"
CQGRID_AOM="${CQGRID_AOM:-8 16 24 32 40 48 56 63}"
export COLOR="$HERE/color.py"

echo -e "image\tw\th\tfamily\tpalette\tencoder\tfmt\tq\tbytes\tbpp\tssim2\tenc_ms" > "$OUT"

worker() {
  local img="$1" w="$2" h="$3" fam="$4"
  local tmp="$WORK/$(basename "$img" .png)"; mkdir -p "$tmp"
  local part="$tmp/rows.tsv"; : > "$part"
  local bn; bn=$(basename "$img")
  local q r
  for q in $CQGRID_AOM; do
    r=$(AOM_EXTRA="" bash "$HERE/aom_cell.sh" "$img" "$w" "$h" "$fam" 420 "$q" "$tmp" 2>>"$tmp/err.log")
    [[ "$r" == libaom* ]] && printf '%s\t%s\t%s\t%s\t1\t%s\n' "$bn" "$w" "$h" "$fam" "$r" >> "$part"
    r=$(AOM_EXTRA="--enable-palette=0" bash "$HERE/aom_cell.sh" "$img" "$w" "$h" "$fam" 420 "$q" "$tmp" 2>>"$tmp/err.log")
    [[ "$r" == libaom* ]] && printf '%s\t%s\t%s\t%s\t0\t%s\n' "$bn" "$w" "$h" "$fam" "$r" >> "$part"
  done
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
echo "[palette_ablation] COMPLETE rows=$(($(wc -l < "$OUT")-1)) -> $OUT"
echo "[palette_ablation] analyze:  python3 $HERE/analyze_palette_ablation.py $OUT"
