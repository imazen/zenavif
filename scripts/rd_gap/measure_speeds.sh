#!/usr/bin/env bash
# measure_speeds.sh CAVIF LABEL IMG PX QLIST OUT_TSV  (SAVE_PNG, SCORER from env)
set -uo pipefail
CAVIF="$1"; LABEL="$2"; IMG="$3"; PX="$4"; QLIST="$5"; OUT="$6"
SAVE_PNG="${SAVE_PNG:?}"; SCORER="${SCORER:?}"
TMP=$(mktemp -d /tmp/statusmeasure/run.XXXXXX)
base=$(basename "$IMG" .png)
for q in $QLIST; do
 for s in $(seq 1 10); do
  avif="$TMP/${LABEL}.s${s}.q${q}.avif"; dec="$TMP/${LABEL}.s${s}.q${q}.png"
  t0=$(date +%s.%N)
  "$CAVIF" -f -Q "$q" -s "$s" --depth 8 -o "$avif" "$IMG" >"$TMP/enc.log" 2>&1
  rc=$?; t1=$(date +%s.%N)
  if [ $rc -ne 0 ] || [ ! -s "$avif" ]; then
    printf '%s\t%s\t%s\tENCFAIL\tNA\tNA\tNA\n' "$LABEL" "$s" "$q" >>"$OUT"; continue
  fi
  ms=$(python3 -c "print(f'{($t1-$t0)*1000:.0f}')")
  bytes=$(stat -c%s "$avif"); md5=$(md5sum "$avif" | cut -c1-12)
  "$SAVE_PNG" "$avif" "$dec" >/dev/null 2>&1 && \
    ss=$("$SCORER" image "$IMG" "$dec" 2>/dev/null | grep -oE '[0-9.]+' | head -1) || ss=DECFAIL
  bpp=$(python3 -c "print(f'{$bytes*8/$PX:.4f}')")
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$LABEL" "$s" "$q" "$bytes" "$bpp" "$ms" "$ss" >>"$OUT"
 done
done
rm -rf "$TMP"
