#!/usr/bin/env bash
# Encode one PNG with aomenc (libaom, cpu-used=2 by default), decode with aomdec,
# score decoded-vs-source with fast-ssim2. Uses the color-exact color.py so the ONLY
# round-trip error is AV1 compression, not RGB<->YUV conversion (a naive ffmpeg path
# capped ssim2 ~66). Feeds aomenc the same YUV zenravif would produce → fair ENCODER
# comparison. Formats match zenravif frontier cell types: {420-ycc, 444-ycc, rgb-identity}.
# Args:  IMG W H FAMILY FMT CQ TMP
# Emits (tab-separated): encoder<TAB>fmt<TAB>cq<TAB>bytes<TAB>bpp<TAB>ssim2<TAB>enc_ms
set -uo pipefail
AOMENC="${AOMENC:?set AOMENC to a libaom aomenc build}"
AOMDEC="${AOMDEC:?set AOMDEC to a libaom aomdec build}"
SCORER="${SCORER:?set SCORER to fast-ssim2-cli}"
COLOR="${COLOR:-$(cd "$(dirname "$0")" && pwd)/color.py}"
CPU="${AOM_CPU:-2}"
EXTRA="${AOM_EXTRA:-}"

IMG="$1"; W="$2"; H="$3"; FAM="$4"; FMT="$5"; CQ="$6"; TMP="$7"
PX=$((W*H)); base=$(basename "$IMG" .png)
y4m="$TMP/${base}.${FMT}.y4m"
[ -f "$y4m" ] || python3 "$COLOR" to_y4m "$IMG" "$FMT" "$y4m"

case "$FMT" in
  420) IN="--i420"; MC="--matrix-coefficients=bt470bg" ;;
  444) IN="--i444"; MC="--matrix-coefficients=bt470bg" ;;
  rgb) IN="--i444"; MC="--matrix-coefficients=identity" ;;
  *)   echo "ENCFAIL libaom bad-fmt $FMT"; exit 1 ;;
esac

obu="$TMP/${base}.${FMT}.q${CQ}.obu"; decy="$TMP/${base}.${FMT}.q${CQ}.dec.y4m"; decp="$TMP/${base}.${FMT}.q${CQ}.dec.png"
t0=$(date +%s.%N)
"$AOMENC" --cpu-used="$CPU" --end-usage=q --cq-level="$CQ" $IN --passes=1 --lag-in-frames=0 \
  --color-primaries=bt709 --transfer-characteristics=srgb $MC $EXTRA --output="$obu" "$y4m" > "$TMP/${base}.${FMT}.enc.log" 2>&1
rc=$?; t1=$(date +%s.%N)
enc_ms=$(python3 -c "print(f'{($t1-$t0)*1000:.1f}')")
[ $rc -ne 0 ] && { echo "ENCFAIL libaom $FMT $CQ rc=$rc $(tail -1 "$TMP/${base}.${FMT}.enc.log")"; exit 1; }

bytes=$(stat -c%s "$obu")
bpp=$(python3 -c "print(f'{$bytes*8/$PX:.5f}')")
"$AOMDEC" --output-bit-depth=8 -o "$decy" "$obu" > /dev/null 2>&1
python3 "$COLOR" from_y4m "$decy" "$FMT" "$IMG" "$decp"
ss=$("$SCORER" image "$IMG" "$decp" 2>/dev/null | grep -oE '[0-9.]+' | head -1)
[ -z "$ss" ] && ss="NA"

rm -f "$obu" "$decy" "$decp"
printf 'libaom\t%s\t%s\t%s\t%s\t%s\t%s\n' "$FMT" "$CQ" "$bytes" "$bpp" "$ss" "$enc_ms"
