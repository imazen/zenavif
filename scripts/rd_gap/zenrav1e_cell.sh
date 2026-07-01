#!/usr/bin/env bash
# Encode one PNG with cavif (zenrav1e, best speed s2), decode via zenavif's OWN
# decoder (the save_png example — dogfoods the full zenavif encode+decode roundtrip),
# score decoded-vs-source with fast-ssim2.
#   s1 == s2 byte-identical and there is no s0, so s2 is zenrav1e's max-effort setting.
# Args:  IMG W H FAMILY Q TMP
# Emits (tab-separated): encoder<TAB>fmt<TAB>q<TAB>bytes<TAB>bpp<TAB>ssim2<TAB>enc_ms
set -uo pipefail
CAVIF="${CAVIF:?set CAVIF to ravif/target/release/cavif}"
SAVE_PNG="${SAVE_PNG:?set SAVE_PNG to zenavif target/release/examples/save_png}"
SCORER="${SCORER:?set SCORER to fast-ssim2-cli}"
SPEED="${ZENRAV1E_SPEED:-2}"
DEPTH="${ZENRAV1E_DEPTH:-8}"   # cavif defaults to 10-bit → decodes as Rgb16 (save_png handles RGB8/RGBA8);
                              # 8-bit keeps the roundtrip scorable AND symmetric with the 8-bit libaom side.

IMG="$1"; W="$2"; H="$3"; FAM="$4"; Q="$5"; TMP="$6"
PX=$((W*H)); base=$(basename "$IMG" .png)
avif="$TMP/${base}.q${Q}.avif"; decp="$TMP/${base}.q${Q}.dec.png"

t0=$(date +%s.%N)
"$CAVIF" -f -Q "$Q" -s "$SPEED" --depth "$DEPTH" -o "$avif" "$IMG" > "$TMP/${base}.q${Q}.enc.log" 2>&1
rc=$?; t1=$(date +%s.%N)
enc_ms=$(python3 -c "print(f'{($t1-$t0)*1000:.1f}')")
{ [ $rc -ne 0 ] || [ ! -s "$avif" ]; } && { echo "ENCFAIL zenrav1e Q$Q rc=$rc $(tail -1 "$TMP/${base}.q${Q}.enc.log" 2>/dev/null)"; exit 1; }

bytes=$(stat -c%s "$avif")
bpp=$(python3 -c "print(f'{$bytes*8/$PX:.5f}')")
"$SAVE_PNG" "$avif" "$decp" > /dev/null 2>&1 || { echo "DECFAIL zenrav1e Q$Q"; exit 1; }
ss=$("$SCORER" image "$IMG" "$decp" 2>/dev/null | grep -oE '[0-9.]+' | head -1)
[ -z "$ss" ] && ss="NA"

rm -f "$avif" "$decp"
printf 'zenrav1e\tdefault\t%s\t%s\t%s\t%s\t%s\n' "$Q" "$bytes" "$bpp" "$ss" "$enc_ms"
