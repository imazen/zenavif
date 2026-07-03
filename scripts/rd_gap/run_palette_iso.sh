#!/usr/bin/env bash
# Isolated-config palette A/B driver: 3 palette arms x speeds x quantizer grid
# through palette_iso_cell.sh (rav1e CLI, color-exact 420 y4m, aomdec +
# rav1d-safe agreement). One y4m conversion per image, shared by all cells.
#
# Env:
#   SAMPLE       corpus tsv (image\tw\th\tfamily)
#   OUT          results tsv
#   RAV1E AOMDEC IVF_RAW SCORER   required binaries
#   BUTTER       butteraugli-cli (optional but the A/B runs with it ON)
#   ARMS         default "off always auto"
#   SPEEDS       default "2 6"
#   QGRID_ISO    default "60 100 140 180 220"  (rav1e quantizer, palette-ab grid)
#   JOBS         default 22
#   IVF_KEEP     set to a dir to persist every encoded stream (else discarded)
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SAMPLE="${SAMPLE:?set SAMPLE}"
OUT="${OUT:?set OUT}"
WORK="${WORK:-/tmp/palette_iso_work.$$}"; mkdir -p "$WORK"
ARMS="${ARMS:-off always auto}"
SPEEDS="${SPEEDS:-2 6}"
QGRID_ISO="${QGRID_ISO:-60 100 140 180 220}"
JOBS="${JOBS:-22}"
export COLOR="${COLOR:-$HERE/color.py}"
export RAV1E AOMDEC IVF_RAW SCORER
[ -n "${IVF_KEEP:-}" ] && export IVF_KEEP_DIR="$IVF_KEEP"

if [ -z "${RD_CACHE_DIR:-}" ] && [ -d /home/lilith/sweep_cache ]; then
  export RD_CACHE_DIR=/home/lilith/sweep_cache
fi
[ -n "${RD_CACHE_DIR:-}" ] && echo "[pal_iso] cell cache: $RD_CACHE_DIR (RD_CACHE=off to bypass)"

echo -e "image\tspeed\tq\tarm\tbytes\tenc_ms\tssim2\tbutter_max\tbutter_p3\tmd5_agree" > "$OUT"
echo "[pal_iso] arms='$ARMS' speeds='$SPEEDS' qgrid='$QGRID_ISO' corpus=$SAMPLE"

worker() {
  local img="$1"
  local base; base=$(basename "$img" .png)
  local tmp="$WORK/$base"; mkdir -p "$tmp"
  local part="$tmp/rows.tsv"; : > "$part"
  local y4m="$tmp/$base.y4m"
  if ! python3 "$COLOR" to_y4m "$img" 420 "$y4m" > /dev/null 2>&1; then
    echo "  [$(date -u +%H:%M:%SZ)] Y4MFAIL $base" >&2
    printf 'CELLFAIL\t%s\tto_y4m\t-\t-\n' "$base" >> "$WORK/failures.tsv"
    rm -rf "$tmp"; return
  fi
  local spd q arm r fails=0
  for spd in $SPEEDS; do for q in $QGRID_ISO; do for arm in $ARMS; do
    r=$(bash "$HERE/palette_iso_cell.sh" "$img" "$y4m" "$spd" "$q" "$arm" "$tmp" 2>>"$tmp/err.log")
    if [[ "$r" == "$base"$'\t'* ]]; then
      printf '%s\n' "$r" >> "$part"
    else
      fails=$((fails+1))
      echo "  [$(date -u +%H:%M:%SZ)] CELL FAILED $base s$spd q$q $arm: ${r:-<no output>}" >&2
      printf 'CELLFAIL\t%s\ts%s-q%s-%s\t%s\n' "$base" "$spd" "$q" "$arm" "${r:-none}" >> "$WORK/failures.tsv"
    fi
  done; done; done
  cat "$part" >> "$OUT"
  if (( fails > 0 )); then
    cp "$tmp/err.log" "$WORK/err.$base.log" 2>/dev/null || true
    echo "  [$(date -u +%H:%M:%SZ)] done $base rows=$(wc -l < "$part") FAILED_CELLS=$fails"
  else
    echo "  [$(date -u +%H:%M:%SZ)] done $base rows=$(wc -l < "$part")"
  fi
  rm -rf "$tmp"
}

running=0
while IFS=$'\t' read -r img w h fam; do
  [ -z "${img:-}" ] && continue
  [ -f "$img" ] || { echo "  skip missing: $img"; continue; }
  worker "$img" &
  running=$((running+1)); if (( running >= JOBS )); then wait -n; running=$((running-1)); fi
done < <(tail -n +2 "$SAMPLE")
wait
if [ -s "$WORK/failures.tsv" ]; then
  echo "[pal_iso] FAILURES: $(wc -l < "$WORK/failures.tsv") failed cells — results INCOMPLETE ($WORK/failures.tsv)" >&2
  exit 1
fi
echo "[pal_iso] COMPLETE rows=$(($(wc -l < "$OUT")-1)) -> $OUT"
