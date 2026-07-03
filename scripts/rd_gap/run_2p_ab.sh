#!/usr/bin/env bash
# Two-pass butteraugli A/B driver: sweeps one corpus TSV through
# zenavif_2p_cell.sh for one arm (TP_MODE=single|twopass). Same pool
# structure as aom_only.sh; emits the run_gap.sh row schema so
# bd_arm.py works unchanged (fmt column carries the arm mode).
#
#   TP_CELL=... SAVE_PNG=... SCORER=... BUTTER=... TP_MODE=twopass \
#   [TP_STRENGTH=1.0] [ZENRAVIF_TUNE=ssimulacra2] [SAMPLE=...] OUT=... \
#     ~/work/zen/scripts/run-heavy -- bash run_2p_ab.sh
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SAMPLE="${SAMPLE:-$HERE/sample_images.tsv}"
OUT="${OUT:-$HERE/zenavif_2p_results.tsv}"
WORK="${WORK:-/tmp/rd_gap_work_2p.$$}"; mkdir -p "$WORK"  # local disk: see run_gap.sh WORK note
JOBS="${JOBS:-6}"
QGRID_ZR="${QGRID_ZR:-30 40 50 55 60 65 70 75 80 85 90 95}"

echo -e "image\tw\th\tfamily\tencoder\tfmt\tq\tbytes\tbpp\tssim2\tenc_ms\tbutteraugli_3n\tbutteraugli_max" > "$OUT"
echo "[2p_ab] mode=${TP_MODE:?} strength=${TP_STRENGTH:-1.0} speed=${ZENRAV1E_SPEED:-2} tune='${ZENRAVIF_TUNE:-}' q='$QGRID_ZR'"

worker() {
  local img="$1" w="$2" h="$3" fam="$4"
  local tmp="$WORK/$(basename "$img" .png)"; mkdir -p "$tmp"
  local part="$tmp/rows.tsv"; : > "$part"
  local bn; bn=$(basename "$img")
  local q r
  local fails=0
  for q in $QGRID_ZR; do
    r=$(bash "$HERE/zenavif_2p_cell.sh" "$img" "$w" "$h" "$fam" "$q" "$tmp" 2>>"$tmp/err.log")
    if [[ "$r" == zenavif-2p* ]]; then
      printf '%s\t%s\t%s\t%s\t%s\n' "$bn" "$w" "$h" "$fam" "$r" >> "$part"
    else
      fails=$((fails+1))
      echo "  [$(date -u +%H:%M:%SZ)] CELL FAILED $bn 2p-$TP_MODE q$q: ${r:-<no output>}" >&2
      printf 'CELLFAIL\t%s\t2p-%s\tq%s\t%s\n' "$bn" "$TP_MODE" "$q" "${r:-none}" >> "$WORK/failures.tsv"
    fi
  done
  flock "$OUT" -c "cat '$part' >> '$OUT'"
  if (( fails > 0 )); then
    cp "$tmp/err.log" "$WORK/err.$bn.log" 2>/dev/null || true
    echo "  [$(date -u +%H:%M:%SZ)] done $bn rows=$(wc -l < "$part") FAILED_CELLS=$fails (err: $WORK/err.$bn.log)"
  else
    echo "  [$(date -u +%H:%M:%SZ)] done $bn rows=$(wc -l < "$part")"
  fi
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
if [ -s "$WORK/failures.tsv" ]; then
  echo "[2p_ab] WARNING: $(wc -l < "$WORK/failures.tsv") FAILED CELLS -- results are INCOMPLETE. See $WORK/failures.tsv + $WORK/err.*.log" >&2
fi
echo "[2p_ab] COMPLETE rows=$(($(wc -l < "$OUT")-1)) -> $OUT"
