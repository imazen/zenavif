#!/usr/bin/env bash
# gate-conformance: every pinned shipped-config encode must decode cleanly
# under libaom's reference decoder AND byte-agree (raw planar md5) with
# rav1d-safe — the PALCONF protocol as an executable gate (invariant A2 of
# docs/ENGINEERING_BASELINE.md).
#
# Cells are emitted fresh by `gate_kit cells` (pinned integer-synthetic
# content; product-path AVIFs across speed x quality x subsampling x depth),
# then each AVIF runs the protocol from scripts/rd_gap/zenrav1e_cell.sh:
#   extract_av1 -> obu_to_ivf.py -> aomdec clean + aomdec --rawvideo
#   vs ivf_raw (rav1d-safe) raw md5 agreement.
# Plus an optional palette/intraBC-armed rav1e-CLI leg on the emitted
# screen-content y4m inputs — those tools are release-gated off the product
# path until the dep bump, so arm coverage needs the sibling zenrav1e CLI.
#
# Env (the CALLER decides what runs — the justfile wires the defaults):
#   AOMDEC    path to aomdec — REQUIRED. It is the reference decoder;
#             without it this is not the PALCONF protocol (loud exit 2).
#   ZENRAV1E  path to the sibling zenrav1e CLI for the armed leg.
#             Empty = leg skipped, reported loudly in the summary line.
#   CI=1      reduced cell grid (gate_kit cells --ci).
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="${WORK:-/tmp/zenavif_gate_conformance.$$}"
AOMDEC="${AOMDEC:-}"
ZENRAV1E="${ZENRAV1E:-}"
OBU2IVF="$ROOT/scripts/rd_gap/obu_to_ivf.py"

if [ -z "$AOMDEC" ] || [ ! -x "$AOMDEC" ]; then
  echo "gate-conformance: FATAL — AOMDEC not set or not executable: '$AOMDEC'" >&2
  echo "  The PALCONF protocol needs libaom's reference decoder. Set AOMDEC" >&2
  echo "  (dev box canonical: /home/lilith/work/aom/build_slow/aomdec)." >&2
  exit 2
fi
if [ -n "$ZENRAV1E" ] && [ ! -x "$ZENRAV1E" ]; then
  echo "gate-conformance: FATAL — ZENRAV1E set but not executable: $ZENRAV1E" >&2
  echo "  Build it (cargo build --release in ../zenrav1e) or pass ZENRAV1E=''" >&2
  echo "  to skip the armed leg deliberately." >&2
  exit 2
fi

cd "$ROOT"
cargo build --release --features encode-imazen,encode-threading \
  --example gate_kit --example extract_av1 --example ivf_raw || exit 2
GATEKIT="$ROOT/target/release/examples/gate_kit"
EXTRACT_AV1="$ROOT/target/release/examples/extract_av1"
IVF_RAW="$ROOT/target/release/examples/ivf_raw"

mkdir -p "$WORK"
trap 'rm -rf "$WORK"' EXIT
ci_flag=""
[ -n "${CI:-}" ] && ci_flag="--ci"
"$GATEKIT" cells "$WORK/cells" $ci_flag || exit 2

# One AVIF cell: aomdec must decode cleanly AND byte-agree with rav1d-safe.
fail=0
cells=0
while IFS=$'\t' read -r name file w h; do
  [ "${name:0:1}" = "#" ] && continue
  cells=$((cells + 1))
  obudir="$WORK/obu"
  rm -rf "$obudir"; mkdir -p "$obudir"
  if ! "$EXTRACT_AV1" "$file" "$obudir" > /dev/null 2>&1; then
    echo "CONFFAIL $name extract_av1"; fail=$((fail + 1)); continue
  fi
  obu=$(ls "$obudir"/*.obu 2> /dev/null | head -1)
  if [ -z "$obu" ] || ! python3 "$OBU2IVF" "$obu" "$WORK/c.ivf" "$w" "$h" \
    > /dev/null 2>&1; then
    echo "CONFFAIL $name obu_to_ivf"; fail=$((fail + 1)); continue
  fi
  if ! "$AOMDEC" --summary -o /dev/null "$WORK/c.ivf" > /dev/null 2>&1; then
    echo "CONFFAIL $name aomdec-rejects"; fail=$((fail + 1)); continue
  fi
  if ! "$AOMDEC" --rawvideo -o "$WORK/aom.raw" "$WORK/c.ivf" > /dev/null 2>&1; then
    echo "CONFFAIL $name aomdec-raw"; fail=$((fail + 1)); continue
  fi
  if ! "$IVF_RAW" "$obu" "$WORK/rav1d.raw" > /dev/null 2>&1; then
    echo "CONFFAIL $name rav1d-safe-decode"; fail=$((fail + 1)); continue
  fi
  amd5=$(md5sum < "$WORK/aom.raw" | cut -d' ' -f1)
  rmd5=$(md5sum < "$WORK/rav1d.raw" | cut -d' ' -f1)
  if [ "$amd5" != "$rmd5" ]; then
    echo "CONFFAIL $name MD5DISAGREE aom=$amd5 rav1d=$rmd5"; fail=$((fail + 1))
  fi
done < "$WORK/cells/manifest.tsv"

# Armed leg: palette / palette+intraBC on the pinned screen-content y4m,
# encoded by the sibling zenrav1e CLI (still-picture, shipped-style flags).
armed=0
arm_leg="SKIPPED (ZENRAV1E unset — release-gated tools not covered)"
if [ -n "$ZENRAV1E" ]; then
  arm_leg="on ($ZENRAV1E)"
  for y4m in screen_420 screen_444; do
    for arm in "always" "auto --intrabc"; do
      armed=$((armed + 1))
      tag="$y4m/palette-${arm%% *}"
      # shellcheck disable=SC2086 — $arm intentionally word-splits
      if ! "$ZENRAV1E" "$WORK/cells/$y4m.y4m" --still-picture --threads 1 \
        -s 6 --quantizer 100 --palette $arm -o "$WORK/a.ivf" -y \
        > /dev/null 2>&1; then
        echo "CONFFAIL $tag rav1e-encode"; fail=$((fail + 1)); continue
      fi
      if ! "$AOMDEC" --summary -o /dev/null "$WORK/a.ivf" > /dev/null 2>&1; then
        echo "CONFFAIL $tag aomdec-rejects"; fail=$((fail + 1)); continue
      fi
      if ! "$AOMDEC" --rawvideo -o "$WORK/aom.raw" "$WORK/a.ivf" \
        > /dev/null 2>&1; then
        echo "CONFFAIL $tag aomdec-raw"; fail=$((fail + 1)); continue
      fi
      if ! "$IVF_RAW" "$WORK/a.ivf" "$WORK/rav1d.raw" > /dev/null 2>&1; then
        echo "CONFFAIL $tag rav1d-safe-decode"; fail=$((fail + 1)); continue
      fi
      amd5=$(md5sum < "$WORK/aom.raw" | cut -d' ' -f1)
      rmd5=$(md5sum < "$WORK/rav1d.raw" | cut -d' ' -f1)
      if [ "$amd5" != "$rmd5" ]; then
        echo "CONFFAIL $tag MD5DISAGREE aom=$amd5 rav1d=$rmd5"
        fail=$((fail + 1))
      fi
    done
  done
fi

echo "gate-conformance: $cells avif cells + $armed armed cells," \
  "$fail failures (armed leg: $arm_leg)"
if [ "$fail" -gt 0 ]; then
  echo "gate-conformance: FAIL"
  exit 1
fi
echo "gate-conformance: PASS"
