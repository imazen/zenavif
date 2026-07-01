#!/usr/bin/env bash
# Per-block AV1 decode-decision diff between zenrav1e (cavif) and libaom (aomenc) at
# matched settings, using aom's OWN bitstream inspector (examples/inspect, built with
# -DCONFIG_INSPECTION=1 -DCONFIG_ACCOUNTING=1) -- decodes either encoder's output and
# dumps per-4x4-cell partition/mode/txType/skip/palette/cfl plus per-syntax-element bit
# cost (via libaom's Accounting API). This is how the encode_partition_topdown gap
# (docs/RD_GAP_VS_LIBAOM.md: PARTITION_HORZ/VERT hardcoded out of the candidate list)
# was found: `inspect` works on ANY valid AV1 bitstream, not just aom's own.
#
# Args: IMG W H Q_ZENRAV1E CQ_LIBAOM OUTDIR
# Emits OUTDIR/{zenrav1e,libaom}.json ; pick Q/CQ to land close in bytes first (a quick
# `ls -la` on the .obu files after a first pass tells you how far off you are).
set -uo pipefail
CAVIF="${CAVIF:?set CAVIF to a zenrav1e-backed cavif build}"
AOMENC="${AOMENC:?set AOMENC to a libaom aomenc build}"
INSPECT="${INSPECT:?set INSPECT to aom's examples/inspect (build_inspect, -DCONFIG_INSPECTION=1 -DCONFIG_ACCOUNTING=1)}"
EXTRACT_AV1="${EXTRACT_AV1:?set EXTRACT_AV1 to zenavif's target/release/examples/extract_av1}"
COLOR="${COLOR:-$(cd "$(dirname "$0")" && pwd)/color.py}"
HERE="$(cd "$(dirname "$0")" && pwd)"

IMG="$1"; W="$2"; H="$3"; Q_ZR="$4"; CQ_AOM="$5"; OUT="$6"
mkdir -p "$OUT"

"$CAVIF" -s2 -Q "$Q_ZR" --depth 8 -o "$OUT/zenrav1e.avif" "$IMG"
"$EXTRACT_AV1" "$OUT/zenrav1e.avif" "$OUT" >/dev/null

python3 "$COLOR" to_y4m "$IMG" 420 "$OUT/src.420.y4m"
"$AOMENC" --cpu-used=2 --end-usage=q --cq-level="$CQ_AOM" --i420 --passes=1 --lag-in-frames=0 \
  --color-primaries=bt709 --transfer-characteristics=srgb --matrix-coefficients=bt470bg \
  --obu --output="$OUT/libaom.obu" "$OUT/src.420.y4m" >"$OUT/libaom.enc.log" 2>&1

python3 "$HERE/obu_to_ivf.py" "$OUT/zenrav1e.obu" "$OUT/zenrav1e.ivf" "$W" "$H"
python3 "$HERE/obu_to_ivf.py" "$OUT/libaom.obu" "$OUT/libaom.ivf" "$W" "$H"

"$INSPECT" "$OUT/zenrav1e.ivf" --all > "$OUT/zenrav1e.json" 2>"$OUT/zenrav1e.inspect.log"
"$INSPECT" "$OUT/libaom.ivf" --all > "$OUT/libaom.json" 2>"$OUT/libaom.inspect.log"

zb=$(stat -c%s "$OUT/zenrav1e.obu"); ab=$(stat -c%s "$OUT/libaom.obu")
echo "zenrav1e: $zb bytes -> $OUT/zenrav1e.json"
echo "libaom:   $ab bytes -> $OUT/libaom.json  (byte ratio libaom/zenrav1e: $(python3 -c "print(f'{$ab/$zb:.3f}')"))"
[ -n "${VERBOSE:-}" ] || rm -f "$OUT"/*.y4m "$OUT"/*.ivf
echo "analyze:  python3 $HERE/analyze_inspect_diff.py $OUT/zenrav1e.json $OUT/libaom.json"
