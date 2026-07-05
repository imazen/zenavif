#!/usr/bin/env bash
# Encode one PNG with zenjpeg (the S10 program's scoreboard anchor: at
# ultra-fast speeds the competitor is JPEG, not aom), decode via zenjpeg's OWN
# decoder, score decoded-vs-source with fast-ssim2 (+ optional butteraugli).
# Uses zenjpeg's sweep-grammar cell ids (config_from_cell_id) so arms are
# canonical + fingerprintable — e.g. jp3_t0_small_420 (the shipped default
# stratum), moz_tr14.75+dc_small_420 (the mozjpeg-class trellis arm).
#
# Driven by the JPEG_SWEEP_CELL binary (zenjpeg example):
#   cargo build --release --example sweep_cell --features __expert -p zenjpeg
#
# Args:  IMG W H FAMILY Q TMP
# Env:   JPEG_SWEEP_CELL (binary), JPEG_CONFIG (cell id), SCORER, [BUTTER]
# Emits: encoder<TAB>fmt<TAB>q<TAB>bytes<TAB>bpp<TAB>ssim2<TAB>enc_ms<TAB>b3<TAB>bmax<TAB>enc_int_ms
#   enc_ms     = whole-process wall (PNG load + encode + decode + PNG write),
#                same convention as the other cells;
#   enc_int_ms = INTERNAL encode-only ms from sweep_cell — the number the
#                cross-codec ms ratios use (PNG I/O excluded on both sides).
set -uo pipefail
JPEG_SWEEP_CELL="${JPEG_SWEEP_CELL:?set JPEG_SWEEP_CELL to zenjpeg target/release/examples/sweep_cell}"
SCORER="${SCORER:?set SCORER to fast-ssim2-cli}"
CFG="${JPEG_CONFIG:-jp3_t0_small_420}"

IMG="$1"; W="$2"; H="$3"; FAM="$4"; Q="$5"; TMP="$6"
PX=$((W*H)); base=$(basename "$IMG" .png)
cfgtag=$(printf '%s' "$CFG" | tr -c 'A-Za-z0-9._-' '_')
jpg="$TMP/${base}.${cfgtag}.q${Q}.jpg"; decp="$TMP/${base}.${cfgtag}.q${Q}.dec.png"

source "$(dirname "${BASH_SOURCE[0]}")/cell_cache.sh"
rd_cache_row_key "$JPEG_SWEEP_CELL" "$IMG" "jpeg" "q=$Q" "cfg=$CFG" "butter=${BUTTER:+on}"
if row=$(rd_cache_row_get); then printf '%s\n' "$row"; exit 0; fi

t0=$(date +%s.%N)
out=$("$JPEG_SWEEP_CELL" "$IMG" "$jpg" "$CFG" "$Q" "$decp" 2> "$TMP/${base}.${cfgtag}.q${Q}.enc.log")
rc=$?; t1=$(date +%s.%N)
enc_ms=$(python3 -c "print(f'{($t1-$t0)*1000:.1f}')")
{ [ $rc -ne 0 ] || [ ! -s "$jpg" ]; } && { echo "ENCFAIL zenjpeg $CFG q$Q rc=$rc $(tail -1 "$TMP/${base}.${cfgtag}.q${Q}.enc.log" 2>/dev/null)"; exit 1; }

bytes=$(printf '%s' "$out" | grep -oE 'bytes=[0-9]+' | cut -d= -f2)
enc_int_ms=$(printf '%s' "$out" | grep -oE 'enc_ms=[0-9.]+' | cut -d= -f2)
[ -z "$bytes" ] && bytes=$(stat -c%s "$jpg")
[ -z "$enc_int_ms" ] && enc_int_ms="NA"
bpp=$(python3 -c "print(f'{$bytes*8/$PX:.5f}')")

[ -s "$decp" ] || { echo "DECFAIL zenjpeg $CFG q$Q (no decoded png)"; exit 1; }
ss=$("$SCORER" image "$IMG" "$decp" 2>/dev/null | grep -oE '[0-9.-]+' | head -1)
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

rm -f "$jpg" "$decp"
row=$(printf 'zenjpeg\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s' "$CFG" "$Q" "$bytes" "$bpp" "$ss" "$enc_ms" "$b3" "$bmax" "$enc_int_ms")
[ "$ss" != "NA" ] && rd_cache_row_put "$row"
printf '%s\n' "$row"
