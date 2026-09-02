#!/usr/bin/env bash
# C-reference decode timing for the 4-way decode benchmark: aomdec + dav1d on
# the SAME cells and SAME scope (frame 0 = first KEY frame, --limit=1) as the
# interleaved Rust pair. These run as separate back-to-back processes (NOT in
# the zenbench interleave) — cross-process C-vs-Rust ratios are less controlled
# than the interleaved Rust-vs-Rust pair; wall-clock includes process startup.
#
# Also emits a 4-way md5 agreement check (aomdec vs dav1d raw I420) so a wrong
# decode on the fresh mosaic encodes surfaces as a correctness finding.
#
# Usage: decode_4way_c_refs.sh <corpus_dir> <out_csv_append> [reps]
set -u
DIR="${1:-/root/zenav1-aom/conformance/data}"
OUT="${2:-/tmp/decode_c.csv}"
REPS="${3:-30}"
AOMDEC=/root/zenav1-aom/reference/libaom/build/aomdec
DAV1D=/root/dav1d-src/build/tools/dav1d

# cell: label|filename|width|height
CELLS=(
  "small-352x288-q00|av1-1-b8-00-quantizer-00.ivf|352|288"
  "small-352x288-q32|av1-1-b8-00-quantizer-32.ivf|352|288"
  "small-352x288-q63|av1-1-b8-00-quantizer-63.ivf|352|288"
  "2K-1920x1080-conf-intrabc|av1-1-b8-16-intra_only-intrabc-extreme-dv.ivf|1920|1080"
  "2K-1920x1080-photo-cq20|mosaic-2k-cq20.ivf|1920|1080"
  "2K-1920x1080-photo-cq40|mosaic-2k-cq40.ivf|1920|1080"
  "4K-3840x2160-photo-cq20|mosaic-4k-cq20.ivf|3840|2160"
  "4K-3840x2160-photo-cq40|mosaic-4k-cq40.ivf|3840|2160"
)

echo "== box state ==" >&2
uptime >&2
echo "rustc procs: $(pgrep -c rustc 2>/dev/null || echo 0)" >&2

median() { python3 -c "import sys,statistics; xs=[float(x) for x in sys.stdin.read().split()]; print(f'{statistics.median(xs):.6f}')"; }

for cell in "${CELLS[@]}"; do
  IFS='|' read -r label fname w h <<< "$cell"
  path="$DIR/$fname"
  if [ ! -f "$path" ]; then echo "!! $label: missing $path — skip" >&2; continue; fi
  px=$((w*h)); mp=$(python3 -c "print(f'{$px/1e6:.4f}')")

  # --- 4-way md5 correctness: aomdec vs dav1d raw I420 (frame 0) ---
  amd5=$("$AOMDEC" --codec=av1 --limit=1 --i420 --md5 "$path" 2>/dev/null | awk '{print $1}' | head -1)
  dmd5=$("$DAV1D" -i "$path" --limit 1 --muxer md5 -o - 2>/dev/null | tail -1 | awk '{print $1}')
  if [ -n "$amd5" ] && [ "$amd5" = "$dmd5" ]; then cnote="aomdec==dav1d md5 $amd5"; else cnote="C-MD5-DIVERGE aomdec=$amd5 dav1d=$dmd5"; fi
  echo "cell $label  $cnote" >&2

  # --- aomdec wall (N reps, median) + internal decode us ---
  aw=""
  for i in $(seq "$REPS"); do
    t0=$(date +%s.%N); "$AOMDEC" --codec=av1 --limit=1 -o /dev/null "$path" >/dev/null 2>&1; t1=$(date +%s.%N)
    aw+="$(python3 -c "print($t1-$t0)") "
  done
  aw_med=$(echo "$aw" | median)
  # internal decode time (us) from --summary, single run
  a_us=$("$AOMDEC" --codec=av1 --limit=1 -o /dev/null --summary "$path" 2>&1 | grep -oE 'in [0-9]+ us' | grep -oE '[0-9]+' | head -1)

  # --- dav1d wall (N reps, median) ---
  dw=""
  for i in $(seq "$REPS"); do
    t0=$(date +%s.%N); "$DAV1D" -i "$path" --limit 1 --muxer null -o - >/dev/null 2>&1; t1=$(date +%s.%N)
    dw+="$(python3 -c "print($t1-$t0)") "
  done
  dw_med=$(echo "$dw" | median)

  # Mpx/s
  amps=$(python3 -c "print(f'{$px/$aw_med/1e6:.2f}')")
  dmps=$(python3 -c "print(f'{$px/$dw_med/1e6:.2f}')")
  aw_ms=$(python3 -c "print(f'{$aw_med*1e3:.4f}')")
  dw_ms=$(python3 -c "print(f'{$dw_med*1e3:.4f}')")

  # aomdec-internal Mpx/s (decode-only, excludes process startup)
  if [ -n "$a_us" ] && [ "$a_us" -gt 0 ] 2>/dev/null; then
    ai_mps=$(python3 -c "print(f'{$px/($a_us/1e6)/1e6:.2f}')")
    ai_ms=$(python3 -c "print(f'{$a_us/1e3:.4f}')")
  else ai_mps="NA"; ai_ms="NA"; fi

  {
    printf '%s,%s,%s,%s,aomdec-wall,first-KEY,%s,%s,%s,%s,%s,%s\n' "$label" "$w" "$h" "$mp" "$aw_ms" "$aw_ms" "$amps" "$amps" "$cnote" "wall-clock incl. process startup (cross-process; directional)"
    printf '%s,%s,%s,%s,aomdec-internal,first-KEY,%s,%s,%s,%s,%s,%s\n' "$label" "$w" "$h" "$mp" "$ai_ms" "$ai_ms" "$ai_mps" "$ai_mps" "$cnote" "aomdec --summary internal decode time (excludes process startup)"
    printf '%s,%s,%s,%s,dav1d-wall,first-KEY,%s,%s,%s,%s,%s,%s\n' "$label" "$w" "$h" "$mp" "$dw_ms" "$dw_ms" "$dmps" "$dmps" "$cnote" "wall-clock incl. process startup (cross-process; directional)"
  } >> "$OUT"
  echo "$label aomdec ${aw_ms}ms(${amps}Mpx/s) internal ${ai_ms}ms(${ai_mps}Mpx/s)  dav1d ${dw_ms}ms(${dmps}Mpx/s)" >&2
done
echo "C-ref CSV appended to $OUT" >&2
