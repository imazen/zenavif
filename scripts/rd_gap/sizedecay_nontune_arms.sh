#!/usr/bin/env bash
# Size-decay NON-TUNE isolation A/B arm driver (wedge follow-up to
# sizedecay_arms.sh): the tune-OFF zenrav1e baseline decays vs aom cpu2 from
# ~parity @1024 to +4.17 (train) / +12.72 (val) median BD @256
# (benchmarks/hyperparam_size_decay_ab_2026-07-03.tsv). These arms isolate the
# DEFAULT coding-path suspects — the quality-keyed SpeedTweaks clamps in ravif
# (partition-range cap, rdo_tx off at hi-q, CDEF/LRF off above ~Q50,
# segmentation) plus chroma subsampling — one mechanism per arm, unconditional
# at every quality, via the ZENRAVIF_SD2_* dev passthroughs in the
# ravif--wedge private clone (env-unset == baseline byte-identical).
#
# All arms run tune-OFF (Psychovisual default) + ZENRAVIF_PALETTE=auto,
# matching the sizedecay `off` arm cell-for-cell (byte-identity verified vs
# the label store before any arm ran).
#
#   base       no SD2 env                       (== sizedecay off arm)
#   prange432  ZENRAVIF_SD2_PRANGE=4,32         (lift the hi-q (4,16) cap to 32)
#   rdotx      ZENRAVIF_SD2_RDOTX=1             (tx RDO also at high quality)
#   cdef       ZENRAVIF_SD2_CDEF=1              (CDEF also above ~Q50)
#   lrf        ZENRAVIF_SD2_LRF=1               (LRF also above ~Q50)
#   segoff     ZENRAVIF_SD2_SEG=off             (drop Complex segmentation)
#   yuv420     CAVIF_EXTRA="--yuv 420"          (4:2:0 instead of default 444)
#   combo32    prange432 + rdotx + cdef + lrf   (aom-parity coding-tools shape)
#
# prange464 (ZENRAVIF_SD2_PRANGE=4,64) is DROPPED from the default arm list:
# (4,64) at high quality (qi<80, where ravif also turns rdo_tx_decision OFF)
# produces bitstreams BOTH aomdec ("Corrupted segment_ids") and rav1d-safe
# reject on zenrav1e master b0098eb1 — the zenrav1e#28 leftover (unvalidated
# 64-dim transforms), reachable via the public override_partition_range.
# Repro: /mnt/v/output/zenavif/sizedecay-nontune-2026-07-03/bug_prange64_hiq/.
# Re-add the arm only after the upstream fix.
#
# All armed cells run with PALCONF=1 (aomdec decode + aomdec/rav1d-safe raw
# md5 agreement per cell) — the zero-corruption bar; RD_CACHE_EXTRA carries
# the palconf tag so armed rows never alias non-conformance-checked rows.
#
# Usage (local workstation or box):
#   SAMPLE=scripts/rd_gap/sample_sizedecay_train.tsv OUT=<dir>/arms.done \
#   CAVIF=~/work/zen/ravif--wedge/target/release/cavif JOBS=6 \
#     ~/work/zen/scripts/run-heavy -- bash sizedecay_nontune_arms.sh
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${OUT:?pass OUT (sentinel path; per-arm TSVs land in its directory)}"
OUTDIR="$(dirname "$OUT")"
mkdir -p "$OUTDIR"
export AOMENC=""            # zenrav1e-only cells; aom refs come from the label store
export ZENRAVIF_PALETTE="${ZENRAVIF_PALETTE:-auto}"   # constant across arms (cancels)
QGRID_ZR="${QGRID_ZR:-30 40 50 55 60 65 70 75 78 80 82 85 88 90 92 95}"
export QGRID_ZR
ARMS="${ARMS:-base prange432 rdotx cdef lrf segoff yuv420 combo32}"
# Conformance gate for every armed cell (see header). base is exempt only
# when its bytes were already proven identical to a conformance-passed arm.
PALCONF_ARMS="${PALCONF_ARMS:-1}"

echo "[sd_nontune] sample=$SAMPLE jobs=${JOBS:-?} qgrid='$QGRID_ZR'"
echo "[sd_nontune] arms: $ARMS"
fails=0
for arm in $ARMS; do
  unset ZENRAVIF_TUNE ZENRAVIF_SD2_PRANGE ZENRAVIF_SD2_RDOTX ZENRAVIF_SD2_CDEF \
        ZENRAVIF_SD2_LRF ZENRAVIF_SD2_SEG CAVIF_EXTRA PALCONF RD_CACHE_EXTRA 2>/dev/null || true
  case "$arm" in
    base) : ;;
    prange432) export ZENRAVIF_SD2_PRANGE=4,32 ;;
    prange464) export ZENRAVIF_SD2_PRANGE=4,64 ;;   # see header: bug-blocked on master
    rdotx) export ZENRAVIF_SD2_RDOTX=1 ;;
    cdef) export ZENRAVIF_SD2_CDEF=1 ;;
    lrf) export ZENRAVIF_SD2_LRF=1 ;;
    segoff) export ZENRAVIF_SD2_SEG=off ;;
    yuv420) export CAVIF_EXTRA="--yuv 420" ;;
    combo32) export ZENRAVIF_SD2_PRANGE=4,32 ZENRAVIF_SD2_RDOTX=1 \
                    ZENRAVIF_SD2_CDEF=1 ZENRAVIF_SD2_LRF=1 ;;
    ship*) echo "[sd_nontune] arm $arm needs explicit env (ship-shape trial)" ;;
    *) echo "[sd_nontune] unknown arm $arm" >&2; exit 2 ;;
  esac
  if [ "$arm" != "base" ] && [ "$PALCONF_ARMS" = "1" ]; then
    export PALCONF=1 RD_CACHE_EXTRA=palconf1
  fi
  out="$OUTDIR/sdn_${arm}.tsv"
  echo "[sd_nontune] === arm=$arm -> $out ==="
  if ! OUT="$out" bash "$HERE/run_gap.sh"; then
    echo "[sd_nontune] ARM FAILED: $arm" >&2
    fails=$((fails + 1))
  fi
done
if [ "$fails" -eq 0 ]; then
  echo "all arms complete $(date -u +%FT%TZ)" > "$OUT"
  echo "[sd_nontune] ALL ARMS DONE"
else
  echo "[sd_nontune] $fails ARM(S) FAILED — no sentinel written" >&2
  exit 1
fi
