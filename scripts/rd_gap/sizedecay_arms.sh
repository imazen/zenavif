#!/usr/bin/env bash
# Size-decay isolation A/B arm driver (HYPERPARAM_FIRST_CUT rule 2 / wedge #3):
# leave-one-out Tune::Ssimulacra2 mechanism arms across rendition sizes on the
# photo-like wedge (train) / palette-val (val) corpora.
#
# Arms (all zenrav1e-only, cavif from the ravif--wedge private clone whose path
# dep targets the zenrav1e--sizedecay workspace with the ZENRAV1E_SD_DISABLE
# dev gates; env-unset == master byte-identical, verified local + box):
#   full        ZENRAVIF_TUNE=ssimulacra2                       (shipped tune)
#   off         (no tune env — Psychovisual baseline)
#   no_<mech>   tune + ZENRAV1E_SD_DISABLE=<mech> for each of
#               chromadq qmcurves boost qmdist lfsharp
#
# Usage (via run_remote.sh; OUT names the sentinel file, per-arm TSVs land
# next to it in the same per-run dir):
#   ./run_remote.sh AOMENC= SAMPLE=/home/lilith/sweep_in/sample_sizedecay_train.tsv \
#       CAVIF=/home/lilith/work/zen/ravif--wedge/target/release/cavif \
#       OUT=arms.done JOBS=24 sizedecay_arms.sh
#   ARMS="full off" ... sizedecay_arms.sh     # subset (e.g. the val confirm)
#
# Q grid: the standard 12-pt grid + {78,82,88,92} — dense q75-95 coverage
# because the 1024->512 decay is entirely a high-quality-band phenomenon
# (benchmarks/hyperparam_size_decay_2026-07-03.tsv).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${OUT:?pass OUT (sentinel path; per-arm TSVs land in its directory)}"
OUTDIR="$(dirname "$OUT")"
mkdir -p "$OUTDIR"
export AOMENC=""            # zenrav1e-only cells; aom refs come from the wedge dataset
export ZENRAVIF_PALETTE="${ZENRAVIF_PALETTE:-auto}"   # constant across arms (cancels)
QGRID_ZR="${QGRID_ZR:-30 40 50 55 60 65 70 75 78 80 82 85 88 90 92 95}"
export QGRID_ZR
ARMS="${ARMS:-full off no_chromadq no_qmcurves no_boost no_qmdist no_lfsharp}"

echo "[sizedecay_arms] sample=$SAMPLE jobs=${JOBS:-?} qgrid='$QGRID_ZR'"
echo "[sizedecay_arms] arms: $ARMS"
fails=0
for arm in $ARMS; do
  unset ZENRAVIF_TUNE ZENRAV1E_SD_DISABLE ZENRAV1E_SD_RAMP
  case "$arm" in
    full) export ZENRAVIF_TUNE=ssimulacra2 ;;
    off) : ;;
    no_*) export ZENRAVIF_TUNE=ssimulacra2 ZENRAV1E_SD_DISABLE="${arm#no_}" ;;
    # ramp_<mech>_<m256>[_<lo>_<hi>] -> ZENRAV1E_SD_RAMP=<mech>:<m256>[:<lo>:<hi>]
    ramp_*) spec="${arm#ramp_}"; export ZENRAVIF_TUNE=ssimulacra2 \
              ZENRAV1E_SD_RAMP="$(echo "$spec" | tr '_' ':')" ;;
    *) echo "[sizedecay_arms] unknown arm $arm" >&2; exit 2 ;;
  esac
  out="$OUTDIR/sd_${arm}.tsv"
  echo "[sizedecay_arms] === arm=$arm -> $out (tune=${ZENRAVIF_TUNE:-unset} sd_disable=${ZENRAV1E_SD_DISABLE:-unset} sd_ramp=${ZENRAV1E_SD_RAMP:-unset}) ==="
  if ! OUT="$out" bash "$HERE/run_gap.sh"; then
    echo "[sizedecay_arms] ARM FAILED: $arm" >&2
    fails=$((fails + 1))
  fi
done
if [ "$fails" -eq 0 ]; then
  echo "all arms complete $(date -u +%FT%TZ)" > "$OUT"
  echo "[sizedecay_arms] ALL ARMS DONE"
else
  echo "[sizedecay_arms] $fails ARM(S) FAILED — no sentinel written" >&2
  exit 1
fi
