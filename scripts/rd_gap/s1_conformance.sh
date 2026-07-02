#!/usr/bin/env bash
# Conformance sweep for a cavif/zenrav1e config: encode every corpus image at
# QGRID (default the 110-cell 22x5 grid), then require BOTH
#   (a) aomdec — libaom's reference decoder — decodes the extracted AV1
#       payload cleanly (bitstream conformance; rav1d-safe alone is NOT
#       sufficient, it has silently accepted spec-invalid streams before), and
#   (b) zenavif's own decode path (save_png / rav1d-safe) roundtrips it,
#       scoring the roundtrip with fast-ssim2 so quality-scaling sanity is
#       visible in the output.
# Emits OUT tsv: image  q  bytes  aomdec  rav1d  ssim2
# ANY non-OK cell fails the run (exit 1) — a conformance sweep with failures
# is a blocker, never a warning.
#
# Env (run_remote.sh prewires the binaries):
#   CAVIF SAVE_PNG SCORER AOMDEC   required
#   EXTRACT_AV1                    default zenavif's release example
#   SAMPLE                         corpus tsv (default sample_images.tsv)
#   QGRID                          default "30 50 60 75 90"
#   ZENRAV1E_SPEED                 default 1 (this script exists for s1)
#   ZENRAV1E_DEPTH                 default 8
#   JOBS                           default 22
#   OUT                            default s1_conformance.tsv
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SAMPLE="${SAMPLE:-$HERE/sample_images.tsv}"
OUT="${OUT:-$HERE/s1_conformance.tsv}"
WORK="${WORK:-/tmp/s1_conformance_work}"; mkdir -p "$WORK"
CAVIF="${CAVIF:?set CAVIF}"; SAVE_PNG="${SAVE_PNG:?set SAVE_PNG}"
AOMDEC="${AOMDEC:?set AOMDEC}"; SCORER="${SCORER:?set SCORER}"
EXTRACT_AV1="${EXTRACT_AV1:-/home/lilith/work/zen/zenavif/target/release/examples/extract_av1}"
OBU2IVF="$HERE/obu_to_ivf.py"
SPEED="${ZENRAV1E_SPEED:-1}"
DEPTH="${ZENRAV1E_DEPTH:-8}"
QGRID="${QGRID:-30 50 60 75 90}"
JOBS="${JOBS:-22}"

[ -x "$EXTRACT_AV1" ] || { echo "FATAL: EXTRACT_AV1 not executable: $EXTRACT_AV1"; exit 1; }
[ -f "$OBU2IVF" ] || { echo "FATAL: missing $OBU2IVF"; exit 1; }

echo -e "image\tq\tbytes\taomdec\trav1d\tssim2" > "$OUT"
echo "[s1_conformance] speed=$SPEED depth=$DEPTH qgrid='$QGRID' corpus=$SAMPLE"

worker() {
  local img="$1" w="$2" h="$3"
  local base; base=$(basename "$img" .png)
  local tmp="$WORK/$base"; mkdir -p "$tmp"
  local part="$tmp/rows.tsv"; : > "$part"
  local q avif obu ivf dec
  for q in $QGRID; do
    avif="$tmp/q${q}.avif"; ivf="$tmp/q${q}.ivf"; dec="$tmp/q${q}.png"
    "$CAVIF" -f -Q "$q" -s "$SPEED" --depth "$DEPTH" -o "$avif" "$img" >/dev/null 2>&1
    if [ ! -s "$avif" ]; then
      printf '%s\t%s\tENCFAIL\tENCFAIL\tENCFAIL\tNA\n' "$base" "$q" >> "$part"; continue
    fi
    local bytes; bytes=$(stat -c%s "$avif")
    # (a) reference-decoder conformance
    local aomres=CORRUPT
    rm -rf "$tmp/obu"; mkdir -p "$tmp/obu"
    if "$EXTRACT_AV1" "$avif" "$tmp/obu" >/dev/null 2>&1; then
      obu=$(ls "$tmp/obu"/*.obu 2>/dev/null | head -1)
      if [ -n "$obu" ] && python3 "$OBU2IVF" "$obu" "$ivf" "$w" "$h" >/dev/null 2>&1 \
         && "$AOMDEC" --summary -o /dev/null "$ivf" >/dev/null 2>&1; then
        aomres=OK
      fi
    fi
    # (b) our-own-decoder roundtrip + score
    local ravres=DECFAIL ss=NA
    if "$SAVE_PNG" "$avif" "$dec" >/dev/null 2>&1; then
      ravres=OK
      ss=$("$SCORER" image "$img" "$dec" 2>/dev/null | grep -oE '[0-9.]+' | head -1)
      [ -n "$ss" ] || ss=NA
    fi
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$base" "$q" "$bytes" "$aomres" "$ravres" "$ss" >> "$part"
    rm -f "$avif" "$ivf" "$dec"
  done
  cat "$part" >> "$OUT"
  echo "  [$(date -u +%H:%M:%SZ)] done $base"
  rm -rf "$tmp"
}

running=0
while IFS=$'\t' read -r img w h _fam; do
  [ -z "${img:-}" ] && continue
  [ -f "$img" ] || { echo "  skip missing: $img"; continue; }
  worker "$img" "$w" "$h" &
  running=$((running+1)); if (( running >= JOBS )); then wait -n; running=$((running-1)); fi
done < <(tail -n +2 "$SAMPLE")
wait

total=$(( $(wc -l < "$OUT") - 1 ))
bad=$(awk -F'\t' 'NR>1 && ($4!="OK" || $5!="OK")' "$OUT" | wc -l)
echo "[s1_conformance] $total cells, $bad failures"
if [ "$bad" -gt 0 ]; then
  echo "[s1_conformance] FAILING CELLS:"
  awk -F'\t' 'NR>1 && ($4!="OK" || $5!="OK")' "$OUT"
  exit 1
fi
echo "[s1_conformance] ALL CLEAN"
