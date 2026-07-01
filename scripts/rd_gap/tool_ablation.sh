#!/usr/bin/env bash
# Generic libaom tool-ablation harness: measures how much bpp (at matched ssim2) each
# named aomenc flag variant costs relative to baseline, on the same corpus/settings as
# run_gap.sh. Complements palette_ablation.sh (which is palette-specific); this one
# takes an arbitrary VARIANTS list so any future lever (restoration, cdef, tune, ...)
# can be probed the same way without editing the script. See docs/RD_GAP_VS_LIBAOM.md
# "Credible narrowing levers" for why this matters: cheaply isolating which AV1 tools
# libaom actually leans on for the PHOTO gap (not just the palette-dominated plots gap)
# tells us where to point zenrav1e RDO-completeness work next.
#
# VARIANTS: space-separated label=flag pairs. Empty flag = baseline (no extra args).
#   e.g. VARIANTS="baseline= norestore=--enable-restoration=0 nocdef=--enable-cdef=0"
#
# Required env: AOMENC, AOMDEC, SCORER (see README.md).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SAMPLE="${SAMPLE:-$HERE/sample_images.tsv}"
OUT="${OUT:-$HERE/tool_ablation_results.tsv}"
WORK="${WORK:-/mnt/v/output/zenavif/rd_gap_work/tool_ablation}"; mkdir -p "$WORK"
JOBS="${JOBS:-6}"
CQGRID_AOM="${CQGRID_AOM:-8 16 24 32 40 48 56 63}"
VARIANTS="${VARIANTS:?set VARIANTS, e.g. 'baseline= norestore=--enable-restoration=0'}"
export COLOR="$HERE/color.py"

echo -e "image\tw\th\tfamily\tvariant\tencoder\tfmt\tq\tbytes\tbpp\tssim2\tenc_ms" > "$OUT"

worker() {
  local img="$1" w="$2" h="$3" fam="$4"
  local tmp="$WORK/$(basename "$img" .png)"; mkdir -p "$tmp"
  local part="$tmp/rows.tsv"; : > "$part"
  local bn; bn=$(basename "$img")
  local q r label flag
  for q in $CQGRID_AOM; do
    for v in $VARIANTS; do
      label="${v%%=*}"; flag="${v#*=}"; [ "$flag" = "$v" ] && flag=""
      r=$(AOM_EXTRA="$flag" bash "$HERE/aom_cell.sh" "$img" "$w" "$h" "$fam" 420 "$q" "$tmp" 2>>"$tmp/err.log")
      [[ "$r" == libaom* ]] && printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$bn" "$w" "$h" "$fam" "$label" "$r" >> "$part"
    done
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
echo "[tool_ablation] COMPLETE rows=$(($(wc -l < "$OUT")-1)) -> $OUT"
echo "[tool_ablation] analyze:  python3 $HERE/analyze_tool_ablation.py $OUT baseline"
