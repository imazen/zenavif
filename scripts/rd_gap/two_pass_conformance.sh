#!/usr/bin/env bash
# Both-sampling conformance for the butteraugli two-pass encodes (the per-SB
# delta_q VALUES change when the loop is on, so every coded-value change gets
# the zero-corruption bar): for each cell, extract the AV1 payload, require
# (a) aomdec — the reference decoder — decodes it cleanly and (b) aomdec's
# raw I420/I444 output byte-agrees with rav1d-safe's (ivf_raw example).
#
#   TP_CELL=... EXTRACT_AV1=... IVF_RAW=... AOMDEC=... OBU2IVF=... \
#   [SAMPLE=...] [QGRID="30 50 60 75 90"] [CHROMAS="444 420"] [JOBS=10] \
#   [ZENRAVIF_TUNE=ssimulacra2] OUT=conformance.tsv \
#     ~/work/zen/scripts/run-heavy -- bash two_pass_conformance.sh
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
TP_CELL="${TP_CELL:?}"
EXTRACT_AV1="${EXTRACT_AV1:?}"
IVF_RAW="${IVF_RAW:?}"
AOMDEC="${AOMDEC:?}"
OBU2IVF="${OBU2IVF:-$HERE/obu_to_ivf.py}"
SAMPLE="${SAMPLE:-$HERE/sample_images.tsv}"
OUT="${OUT:?}"
WORK="${WORK:-/tmp/tp_conf.$$}"; mkdir -p "$WORK"
QGRID="${QGRID:-30 50 60 75 90}"
CHROMAS="${CHROMAS:-444 420}"
JOBS="${JOBS:-10}"
SPEED="${ZENRAV1E_SPEED:-2}"

echo -e "image\tchroma\tq\tverdict" > "$OUT"

cell() {
  local img="$1" q="$2" chroma="$3"
  local bn; bn=$(basename "$img" .png)
  local tmp="$WORK/$bn.$chroma.q$q"; mkdir -p "$tmp"
  local avif="$tmp/x.avif"
  if ! "$TP_CELL" "$img" "$avif" "$q" "$SPEED" twopass 1.0 "$chroma" > /dev/null 2> "$tmp/enc.log"; then
    echo "ENCFAIL"; return
  fi
  rm -rf "$tmp/obu"; mkdir -p "$tmp/obu"
  "$EXTRACT_AV1" "$avif" "$tmp/obu" > /dev/null 2>&1 || { echo "EXTRACTFAIL"; return; }
  local obu; obu=$(ls "$tmp/obu"/*.obu 2>/dev/null | head -1)
  [ -n "$obu" ] || { echo "NOOBU"; return; }
  local W H
  read -r W H < <(python3 -c "from PIL import Image; im=Image.open('$img'); print(im.size[0], im.size[1])")
  python3 "$OBU2IVF" "$obu" "$tmp/x.ivf" "$W" "$H" > /dev/null 2>&1 || { echo "IVFFAIL"; return; }
  "$AOMDEC" --summary -o /dev/null "$tmp/x.ivf" > /dev/null 2>&1 || { echo "AOMDEC-REJECT"; return; }
  "$AOMDEC" --rawvideo -o "$tmp/aom.raw" "$tmp/x.ivf" > /dev/null 2>&1 || { echo "AOMDEC-RAWFAIL"; return; }
  "$IVF_RAW" "$obu" "$tmp/rav1d.raw" > /dev/null 2>&1 || { echo "RAV1D-FAIL"; return; }
  if cmp -s "$tmp/aom.raw" "$tmp/rav1d.raw"; then echo "OK"; else echo "RAW-MISMATCH"; fi
}

worker() {
  local img="$1"
  local bn; bn=$(basename "$img")
  local rows=""
  for chroma in $CHROMAS; do for q in $QGRID; do
    local v; v=$(cell "$img" "$q" "$chroma")
    rows+="$bn\t$chroma\t$q\t$v\n"
    [ "$v" != "OK" ] && echo "  [conf] FAIL $bn $chroma q$q: $v" >&2
  done; done
  flock "$OUT" -c "printf '$rows' >> '$OUT'"
  rm -rf "$WORK/$(basename "$img" .png)".*
  echo "  [conf] done $bn"
}

running=0
while IFS=$'\t' read -r img w h fam; do
  [ -z "${img:-}" ] && continue
  [ -f "$img" ] || continue
  worker "$img" &
  running=$((running+1)); if (( running >= JOBS )); then wait -n; running=$((running-1)); fi
done < <(tail -n +2 "$SAMPLE")
wait
total=$(($(wc -l < "$OUT")-1)); ok=$(grep -c $'\tOK$' "$OUT" || true)
echo "[conf] $ok/$total OK -> $OUT"
[ "$ok" = "$total" ] || echo "[conf] FAILURES PRESENT" >&2
