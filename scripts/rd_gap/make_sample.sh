#!/usr/bin/env bash
# Generate sample_images.tsv (image<TAB>w<TAB>h<TAB>family) from a corpus dir.
# family = first digit of the origin id in o_<id>..., so 7 = 7000-lilith-plots
# (synthetic screen content — AVIF-hostile regardless; the analyzer splits it out).
# Picks the largest (~1MP scale1024) rendition per origin, up to N_PER images per family.
set -uo pipefail
CORPUS="${CORPUS:-/mnt/v/output/clean-picker-corpus-2026-06-26}"
OUT="${OUT:-$(cd "$(dirname "$0")" && pwd)/sample_images.tsv}"
N_PER="${N_PER:-3}"
[ -d "$CORPUS" ] || { echo "corpus dir not found: $CORPUS (set CORPUS=...)"; exit 1; }
echo -e "image\tw\th\tfamily" > "$OUT"
declare -A cnt
for f in "$CORPUS"/o_*scale1024x*.png; do
  [ -f "$f" ] || continue
  bn=$(basename "$f"); id=${bn#o_}; fam=${id:0:1}
  wh=$(echo "$bn" | grep -oE 'scale[0-9]+x[0-9]+' | head -1 | sed 's/scale//')
  w=${wh%x*}; h=${wh#*x}
  [ -z "$w" ] || [ -z "$h" ] && continue
  [ "${cnt[$fam]:-0}" -ge "$N_PER" ] && continue
  cnt[$fam]=$(( ${cnt[$fam]:-0} + 1 ))
  printf '%s\t%s\t%s\t%s\n' "$f" "$w" "$h" "$fam" >> "$OUT"
done
echo "wrote $OUT ($(($(wc -l < "$OUT")-1)) images across families: ${!cnt[*]})"
