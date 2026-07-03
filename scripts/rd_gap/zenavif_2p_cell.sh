#!/usr/bin/env bash
# Encode one PNG through zenavif's two-pass example (single | twopass arm),
# decode via zenavif's own save_png, score with fast-ssim2 (+ optional
# butteraugli columns via BUTTER). Mirrors zenrav1e_cell.sh but drives the
# library two-pass driver instead of cavif (the driver is a zenavif API).
# Env: TP_CELL (two_pass_cell binary), SAVE_PNG, SCORER, [BUTTER],
#      TP_MODE=single|twopass, [TP_STRENGTH=1.0], [ZENRAV1E_SPEED=2],
#      [ZENRAVIF_TUNE] exported through to the encoder (dev-patch builds).
# Args:  IMG W H FAMILY Q TMP
# Emits: encoder \t fmt \t q \t bytes \t bpp \t ssim2 \t enc_ms \t b3 \t bmax
set -uo pipefail
TP_CELL="${TP_CELL:?set TP_CELL to zenavif target/release/examples/two_pass_cell}"
SAVE_PNG="${SAVE_PNG:?set SAVE_PNG to zenavif target/release/examples/save_png}"
SCORER="${SCORER:?set SCORER to fast-ssim2-cli}"
SPEED="${ZENRAV1E_SPEED:-2}"
MODE="${TP_MODE:?set TP_MODE=single|twopass}"
STRENGTH="${TP_STRENGTH:-1.0}"
CHROMA="${TP_CHROMA:-444}"
CLAMP_HI="${TP_CLAMP_HI:-2.5}"
METRIC="${TP_METRIC:-butteraugli}"
PROBE_Q="${TP_PROBE_Q:-none}"

IMG="$1"; W="$2"; H="$3"; FAM="$4"; Q="$5"; TMP="$6"
PX=$((W*H)); base=$(basename "$IMG" .png)
avif="$TMP/${base}.q${Q}.${MODE}.avif"; decp="$TMP/${base}.q${Q}.${MODE}.dec.png"

source "$(dirname "${BASH_SOURCE[0]}")/cell_cache.sh"
rd_cache_row_key "$TP_CELL" "$IMG" "zen2p" "q=$Q" "s=$SPEED" "mode=$MODE" "str=$STRENGTH" \
  "chroma=$CHROMA" "clamphi=$CLAMP_HI" "metric=$METRIC" "probeq=$PROBE_Q" "tune=${ZENRAVIF_TUNE:-}" "butter=${BUTTER:+on}"
if row=$(rd_cache_row_get); then printf '%s\n' "$row"; exit 0; fi

t0=$(date +%s.%N)
stats=$("$TP_CELL" "$IMG" "$avif" "$Q" "$SPEED" "$MODE" "$STRENGTH" "$CHROMA" "$CLAMP_HI" "$METRIC" "$PROBE_Q" 2> "$TMP/${base}.q${Q}.${MODE}.enc.log")
rc=$?; t1=$(date +%s.%N)
enc_ms=$(python3 -c "print(f'{($t1-$t0)*1000:.1f}')")
{ [ $rc -ne 0 ] || [ ! -s "$avif" ]; } && { echo "ENCFAIL zenavif-2p $MODE Q$Q rc=$rc $(tail -1 "$TMP/${base}.q${Q}.${MODE}.enc.log" 2>/dev/null)"; exit 1; }

bytes=$(stat -c%s "$avif")
bpp=$(python3 -c "print(f'{$bytes*8/$PX:.5f}')")

"$SAVE_PNG" "$avif" "$decp" > /dev/null 2>&1 || { echo "DECFAIL zenavif-2p $MODE Q$Q"; exit 1; }
ss=$("$SCORER" image "$IMG" "$decp" 2>/dev/null | grep -oE '[0-9.]+' | head -1)
[ -z "$ss" ] && ss="NA"

b3="NA"; bmax="NA"
if [ -n "${BUTTER:-}" ]; then
  bout=$("$BUTTER" --json "$IMG" "$decp" 2>/dev/null | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
    print("%.6f %.6f" % (d["pnorm_3"], d["score"]))
except Exception:
    print("NA NA")')
  b3="${bout%% *}"; bmax="${bout##* }"
  [ -z "$b3" ] && b3="NA"; [ -z "$bmax" ] && bmax="NA"
fi

rm -f "$avif" "$decp"
row=$(printf 'zenavif-2p\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s' "$MODE" "$Q" "$bytes" "$bpp" "$ss" "$enc_ms" "$b3" "$bmax")
[ "$ss" != "NA" ] && rd_cache_row_put "$row"
printf '%s\n' "$row"
