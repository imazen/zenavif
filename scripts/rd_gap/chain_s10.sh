#!/usr/bin/env bash
# S10 program chain (2026-07-05, docs/S10_PROGRAM.md): make the ultra-fast
# tier competitive with JPEG as the scoreboard anchor (user direction: at
# this speed class the competitor is JPEG, not aom). The SPEED_LADDER s10
# cliff: +32.6/+49.0 vs matched aom arms; partition (16,16), rects dead,
# CDEF/LRF off, tx_domain_rate on.
#
# Phases:
#   p1jpeg   zenjpeg anchor arms on train26 + doccharts (sweep-grammar cells:
#            jp3_t0_small_420 = shipped default stratum, moz_tr14.75+dc =
#            mozjpeg-class trellis, jp3_tr14.5 = jpegli-class trellis), 19-q.
#   p1reg    registry-config cavif (primary ravif -> registry zenrav1e 0.1.4,
#            no tune available there): s8 s9 s10, coarse 6-q.
#   p1mas    master-with-gated-arms cavif (ravif--s10 -> zenrav1e--s10):
#            s8 composed (tune + size1 + part-prune ship triple = the crossed
#            FAST_TIER config) + s9/s10 tune-only (the cliff as shipped).
#   p2grid   the s10/s9 pruned-liveness exploration grid (single-axis probes
#            vs the p1mas s10 base): partition floor 8x16 / 8x32, rect
#            liveness + prune triple, txdr off, cdef on, fdi on, reduced-tx
#            off, tx size1 — each coarse 6-q on train26.
#   timing   solo walls (JOBS=1 RD_CACHE=off, q{40,65,85}, sample_timing4):
#            every deployable arm above + the JPEG anchors. Internal
#            enc_int_ms column captures encoder-only cost on both sides.
#   p1doc    doccharts supplement for the verdict arms.
#
# Run ON THE BOX under nohup:
#   OUTDIR=/home/lilith/sweep_out/s10_$(date +%Y%m%d) \
#     PHASES="p1jpeg p1reg p1mas p2grid timing p1doc" \
#     nohup bash chain_s10.sh > $OUTDIR/chain.log 2>&1 &
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUTDIR="${OUTDIR:?set OUTDIR (per-run output dir)}"
mkdir -p "$OUTDIR"

# Master-leg binaries (the s10 program dev trees).
export CAVIF="${CAVIF:-/home/lilith/work/zen/ravif--s10/target/release/cavif}"
# Registry-leg cavif (primary ravif -> registry zenrav1e).
CAVIF_REG="${CAVIF_REG:-/home/lilith/work/zen/ravif/target/release/cavif}"
export SAVE_PNG="${SAVE_PNG:-/home/lilith/work/zen/zenavif/target/release/examples/save_png}"
export SCORER="${SCORER:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}"
export AOMDEC="${AOMDEC:-/home/lilith/work/aom/build_slow/aomdec}"
export BUTTER="${BUTTER:-/home/lilith/work/butteraugli/target/release/butteraugli}"
export EXTRACT_AV1="${EXTRACT_AV1:-/home/lilith/work/zen/zenavif/target/release/examples/extract_av1}"
export IVF_RAW="${IVF_RAW:-/home/lilith/work/zen/zenavif/target/release/examples/ivf_raw}"
export JPEG_SWEEP_CELL="${JPEG_SWEEP_CELL:-/home/lilith/work/zen/zenjpeg/target/release/examples/sweep_cell}"
PHASES="${PHASES:-p1jpeg p1reg p1mas p2grid timing p1doc}"

SAMPLE_T26="$HERE/sample_images_train26.tsv"   # 24 images
SAMPLE_DOC="$HERE/sample_doccharts.tsv"        # 15 images
SAMPLE_TIM="$HERE/sample_timing4.tsv"          # 4 images
QCOARSE="30 50 60 75 85 95"
QTIM="40 65 85"

JPEG_ARMS="jp3_t0_small_420 moz_tr14.75+dc_small_420 jp3_tr14.5_small_420"

say() { echo "[s10 $(date -u +%H:%M:%SZ)] $*"; }

run_one() { # <out.tsv> <expected_rows> [ENV=val ...]
  local out="$1" want="$2"; shift 2
  if [ -s "$out" ] && [ "$(($(wc -l < "$out") - 1))" -ge "$want" ]; then
    say "SKIP (complete $(($(wc -l < "$out") - 1))/$want): $(basename "$out")"
    return 0
  fi
  local t0=$(date +%s)
  say "RUN $(basename "$out") [$*]"
  if env "$@" OUT="$out" bash "$HERE/run_gap.sh" > "$out.log" 2>&1; then
    local rows=$(($(wc -l < "$out") - 1))
    say "DONE $(basename "$out") rows=$rows/$want in $(( $(date +%s) - t0 ))s"
    if [ "$rows" -lt "$want" ]; then
      say "WARNING: INCOMPLETE $(basename "$out") ($rows/$want) -- see $out.log"
      grep -h "CELLFAIL\|CONFFAIL\|ENCFAIL\|DECFAIL" "$out.log" | tail -5 || true
    fi
  else
    say "FAILED run $(basename "$out") -- see $out.log"; tail -5 "$out.log"
  fi
}

# Common env: PALCONF on every zr cell (fast-tier TX_MODE_LARGEST / reduced-tx
# paths + the new s9/s10 arms must stay conformance-clean); butteraugli on.
common=(AOMENC= PALCONF=1 AOMDEC="$AOMDEC" EXTRACT_AV1="$EXTRACT_AV1" IVF_RAW="$IVF_RAW"
        BUTTER="$BUTTER" JOBS=24)
# The tune is mandatory + nearly-free at fast tiers (SPEED_LADDER) — every
# master-leg arm carries it. Registry-leg arms CANNOT (release-gated).
TUNE="ZENRAVIF_TUNE=ssimulacra2 ZENRAVIF_PALETTE=auto"
# The landed s8 composed config (FAST_TIER_PARITY: size1 + part-prune ship).
SIZE1="ZENRAVIF_TX_SIZE_RDO=1 ZENRAVIF_TX_SIZE_DEPTH=1"
SHIP="ZENRAVIF_RECT_THR=16 ZENRAVIF_PRUNE_4WM=0.0 ZENRAVIF_PRUNE_BK=1.0 ZENRAVIF_PRUNE_VARG=2.0"

if [[ " $PHASES " == *" p1jpeg "* ]]; then
  say "=== PHASE p1jpeg: zenjpeg scoreboard anchors (train26, 3 configs x 19q) ==="
  run_one "$OUTDIR/s10_jpeg_t26.tsv" $((24*3*19)) "${common[@]}" SAMPLE="$SAMPLE_T26" \
    ZR=off JPEG_CONFIGS="$JPEG_ARMS"
fi

if [[ " $PHASES " == *" p1reg "* ]]; then
  say "=== PHASE p1reg: registry-config cavif s8/s9/s10 (train26, 6q) ==="
  for sp in 8 9 10; do
    run_one "$OUTDIR/s10_reg_s${sp}_t26.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
      CAVIF="$CAVIF_REG" QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=$sp CAVIF_EXTRA="--threads 1"
  done
fi

if [[ " $PHASES " == *" p1mas "* ]]; then
  say "=== PHASE p1mas: master arms — s8 composed + s9/s10 tune-only cliff (train26, 6q) ==="
  run_one "$OUTDIR/s10_mas_s8c_t26.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=8 CAVIF_EXTRA="--threads 1" $TUNE $SIZE1 $SHIP
  for sp in 9 10; do
    run_one "$OUTDIR/s10_mas_s${sp}_t26.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
      QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=$sp CAVIF_EXTRA="--threads 1" $TUNE
  done
fi

if [[ " $PHASES " == *" p2grid "* ]]; then
  say "=== PHASE p2grid: s10/s9 pruned-liveness single-axis probes (train26, 6q) ==="
  # s10 axes vs the s10-tune base (p1mas): each is ONE amputation undone.
  run_one "$OUTDIR/s10_g_p816.tsv"    144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE \
    ZENRAVIF_PART_MIN=8 ZENRAVIF_PART_MAX=16
  run_one "$OUTDIR/s10_g_p832.tsv"    144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE \
    ZENRAVIF_PART_MIN=8 ZENRAVIF_PART_MAX=32
  run_one "$OUTDIR/s10_g_rects.tsv"   144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE \
    ZENRAVIF_PART_MIN=8 ZENRAVIF_PART_MAX=16 $SHIP
  run_one "$OUTDIR/s10_g_txdr0.tsv"   144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE \
    ZENRAVIF_TXDR=0
  run_one "$OUTDIR/s10_g_cdef.tsv"    144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE \
    ZENRAVIF_CDEF=1
  run_one "$OUTDIR/s10_g_fdi.tsv"     144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE \
    ZENRAVIF_FDI=1
  run_one "$OUTDIR/s10_g_redtx0.tsv"  144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE \
    ZENRAVIF_REDUCED_TX=0
  run_one "$OUTDIR/s10_g_size1.tsv"   144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE \
    $SIZE1
  # s9 probes: the same partition axes (s9 shares the (16,16) floor; its
  # remaining deltas vs s8 are cdef-lowq-only + reduced_tx + inter_tx_split).
  run_one "$OUTDIR/s10_g9_p816.tsv"   144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=9 CAVIF_EXTRA="--threads 1" $TUNE \
    ZENRAVIF_PART_MIN=8 ZENRAVIF_PART_MAX=16
  run_one "$OUTDIR/s10_g9_rects.tsv"  144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=9 CAVIF_EXTRA="--threads 1" $TUNE \
    ZENRAVIF_PART_MIN=8 ZENRAVIF_PART_MAX=16 $SHIP
fi

if [[ " $PHASES " == *" timing "* ]]; then
  say "=== PHASE timing: solo walls (JOBS=1, RD_CACHE=off, 4 img x 3 q) ==="
  tcommon=(AOMENC= BUTTER= RD_CACHE=off JOBS=1)
  run_one "$OUTDIR/s10_tim_jpeg.tsv" $((4*3*3)) "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    ZR=off JPEG_CONFIGS="$JPEG_ARMS" QGRID_JPEG="$QTIM"
  run_one "$OUTDIR/s10_tim_reg_s10.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    CAVIF="$CAVIF_REG" QGRID_ZR="$QTIM" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1"
  run_one "$OUTDIR/s10_tim_reg_s9.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    CAVIF="$CAVIF_REG" QGRID_ZR="$QTIM" ZENRAV1E_SPEED=9 CAVIF_EXTRA="--threads 1"
  run_one "$OUTDIR/s10_tim_mas_s8c.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=8 CAVIF_EXTRA="--threads 1" $TUNE $SIZE1 $SHIP
  run_one "$OUTDIR/s10_tim_mas_s9.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=9 CAVIF_EXTRA="--threads 1" $TUNE
  run_one "$OUTDIR/s10_tim_mas_s10.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE
  # p2 candidates (timing before verdicts so the budget fit is same-day):
  run_one "$OUTDIR/s10_tim_g_rects.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE \
    ZENRAVIF_PART_MIN=8 ZENRAVIF_PART_MAX=16 $SHIP
  run_one "$OUTDIR/s10_tim_g_txdr0.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE ZENRAVIF_TXDR=0
  run_one "$OUTDIR/s10_tim_g_p816.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE \
    ZENRAVIF_PART_MIN=8 ZENRAVIF_PART_MAX=16
fi

if [[ " $PHASES " == *" p1doc "* ]]; then
  say "=== PHASE p1doc: doccharts supplement (verdict arms) ==="
  run_one "$OUTDIR/s10_jpeg_doc.tsv" $((15*3*19)) "${common[@]}" SAMPLE="$SAMPLE_DOC" \
    ZR=off JPEG_CONFIGS="$JPEG_ARMS"
  run_one "$OUTDIR/s10_reg_s10_doc.tsv" 90 "${common[@]}" SAMPLE="$SAMPLE_DOC" \
    CAVIF="$CAVIF_REG" QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1"
  run_one "$OUTDIR/s10_mas_s10_doc.tsv" 90 "${common[@]}" SAMPLE="$SAMPLE_DOC" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE
  run_one "$OUTDIR/s10_mas_s8c_doc.tsv" 90 "${common[@]}" SAMPLE="$SAMPLE_DOC" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=8 CAVIF_EXTRA="--threads 1" $TUNE $SIZE1 $SHIP
fi


# ---- ROUND 2 (2026-07-05, after the single-axis verdicts): composed arms ----
# Single-axis truth: txdr0 -7.45/-5.86/-6.15 med at 1.14x (22/22), cdef-on
# -1.70/-2.45/-1.89 at 1.04x (22-23/23), size1 -7.82/-13.3/-13.0 at 1.49x,
# rects -4.27/-5.78/-3.84 at 2.31x, p816 -1.58 at 1.84x; p832 catastrophic
# (+10/+18, ruled out), fdi/redtx0 null. s9: p816 -13.5/-17.1/-20.7 (23/23)
# at 1.89x, rects -18.9/-21.9/-24.8 at ~4x — the s10 partition win is
# txdr-masked, so the composed grid re-tests partitions on top of txdr0.
TXDR0="ZENRAVIF_TXDR=0"
CDEF1="ZENRAVIF_CDEF=1"
P816="ZENRAVIF_PART_MIN=8 ZENRAVIF_PART_MAX=16"
IM1="ZENRAVIF_INTRA_MODES=1"
IM2="ZENRAVIF_INTRA_MODES=2"

if [[ " $PHASES " == *" r2comp "* ]]; then
  say "=== PHASE r2comp: s10/s9 composed arms (train26, 6q) ==="
  run_one "$OUTDIR/s10_c1_cheap.tsv"   144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1
  run_one "$OUTDIR/s10_c2_p816.tsv"    144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $P816
  run_one "$OUTDIR/s10_c3_size1.tsv"   144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $SIZE1
  run_one "$OUTDIR/s10_c4_ps.tsv"      144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $P816 $SIZE1
  run_one "$OUTDIR/s10_c5_full.tsv"    144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $P816 $SHIP $SIZE1
  # SATD-decides (phase-3 candidate measured early: RD only the SATD-best
  # intra mode; im2 = top-2 hedge). On the base AND on the c4 composition.
  run_one "$OUTDIR/s10_c6_satd1.tsv"   144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $IM1
  run_one "$OUTDIR/s10_c7_ps_satd1.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $P816 $SIZE1 $IM1
  run_one "$OUTDIR/s10_c8_ps_satd2.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $P816 $SIZE1 $IM2
  # s9 compositions (txdr already off at s9; cdef is lowq-gated -> force-on axis)
  run_one "$OUTDIR/s10_c9_s9ps.tsv"    144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=9 CAVIF_EXTRA="--threads 1" $TUNE $P816 $SIZE1
  run_one "$OUTDIR/s10_c10_s9psc.tsv"  144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=9 CAVIF_EXTRA="--threads 1" $TUNE $P816 $SIZE1 $CDEF1
fi

if [[ " $PHASES " == *" r2tim "* ]]; then
  say "=== PHASE r2tim: composed-arm solo walls ==="
  tcommon=(AOMENC= BUTTER= RD_CACHE=off JOBS=1)
  run_one "$OUTDIR/s10_tim_c1.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1
  run_one "$OUTDIR/s10_tim_c2.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $P816
  run_one "$OUTDIR/s10_tim_c4.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $P816 $SIZE1
  run_one "$OUTDIR/s10_tim_c5.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $P816 $SHIP $SIZE1
  run_one "$OUTDIR/s10_tim_c7.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $P816 $SIZE1 $IM1
  run_one "$OUTDIR/s10_tim_c9.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=9 CAVIF_EXTRA="--threads 1" $TUNE $P816 $SIZE1
  run_one "$OUTDIR/s10_tim_satd1.tsv" 12 "${tcommon[@]}" SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $IM1
fi


# ---- ROUND 3: the tier-row candidates + class supplement ----
if [[ " $PHASES " == *" r3rows "* ]]; then
  say "=== PHASE r3rows: s10'/s9' row candidates ==="
  # s10' candidate: txdr0+cdef+SATD-decides (target ~old-s10 wall)
  run_one "$OUTDIR/s10_c11_row10.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $IM1
  # s9'-preset expression check: c7 config expressed through the s9 preset
  # (reduced_tx on there — measured null; everything else identical).
  run_one "$OUTDIR/s10_c13_row9.tsv" 144 "${common[@]}" SAMPLE="$SAMPLE_T26" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=9 CAVIF_EXTRA="--threads 1" $TUNE $CDEF1 $P816 $SIZE1 $IM1
  run_one "$OUTDIR/s10_tim_c11.tsv" 12 AOMENC= BUTTER= RD_CACHE=off JOBS=1 SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $IM1
  run_one "$OUTDIR/s10_tim_c13.tsv" 12 AOMENC= BUTTER= RD_CACHE=off JOBS=1 SAMPLE="$SAMPLE_TIM" \
    QGRID_ZR="$QTIM" ZENRAV1E_SPEED=9 CAVIF_EXTRA="--threads 1" $TUNE $CDEF1 $P816 $SIZE1 $IM1
  # doccharts class supplement for the two row candidates
  run_one "$OUTDIR/s10_c11_doc.tsv" 90 "${common[@]}" SAMPLE="$SAMPLE_DOC" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=10 CAVIF_EXTRA="--threads 1" $TUNE $TXDR0 $CDEF1 $IM1
  run_one "$OUTDIR/s10_c13_doc.tsv" 90 "${common[@]}" SAMPLE="$SAMPLE_DOC" \
    QGRID_ZR="$QCOARSE" ZENRAV1E_SPEED=9 CAVIF_EXTRA="--threads 1" $TUNE $CDEF1 $P816 $SIZE1 $IM1
fi

say "CHAIN COMPLETE — outputs in $OUTDIR"
