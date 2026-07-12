#!/usr/bin/env bash
# G2 reference-side sweep: SVT-AV1 still cells over a corpus TSV, emitting the
# run_gap/objective.py schema. Configs: presets x tunes x CRF grid (defaults =
# the G2 sampling set: p{2,6,10} at tune 4 [MS_SSIM/SSIMULACRA2-optimized, the
# strongest reference on our metric] + p6 tune 1 [PSNR default], crf 15..65).
#
# Usage: OUT=/mnt/v/output/cooptloop/svt-ref-<date> \
#        ~/work/zen/scripts/run-heavy -- bash svt_sweep.sh
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
SAMPLE="${SAMPLE:-$HERE/sample_images_train26.tsv}"
OUT="${OUT:?set OUT dir}"
CONFIGS="${CONFIGS:-2 4|6 4|10 4|6 1}"   # preset tune pairs, |-separated
CRFS="${CRFS:-15 25 35 45 55 65}"
mkdir -p "$OUT"
TSV="$OUT/svt_ref.tsv"
echo -e "image\tw\th\tfamily\tencoder\tfmt\tq\tbytes\tbpp\tssim2\tenc_ms\tbutteraugli_3n\tbutteraugli_max" > "$TSV"
TMP=$(mktemp -d /tmp/svtsweep.XXXX); trap 'rm -rf "$TMP"' EXIT
n=0
tail -n +2 "$SAMPLE" | while IFS=$'\t' read -r img w h fam; do
  [ -f "$img" ] || continue
  echo "$CONFIGS" | tr '|' '\n' | while read -r preset tune; do
    for crf in $CRFS; do
      row=$(bash "$HERE/svt_cell.sh" "$img" "$fam" "$preset" "$crf" "$tune" "$TMP" 2>/dev/null) \
        && echo "$row" >> "$TSV"
    done
  done
  n=$((n+1)); echo "[svt-sweep] $n images done ($(date -u +%H:%M:%SZ))"
done
wc -l "$TSV"
