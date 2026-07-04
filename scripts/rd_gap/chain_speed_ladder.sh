#!/usr/bin/env bash
# SPEED-LADDER GAP MAP chain (2026-07-04): zenrav1e s{2,4,6,8,10} x {tune-ss2+palette, tune-off}
# vs libaom --allintra cpu{2,4,6,8,9} x {default, --tune=iq}, on train26 + legacy corpora,
# BUTTER on, PALCONF=1 on every zenrav1e RD cell (zero-corruption bar for the fast tiers),
# plus cached GOOD-mode anchor replays (continuity) and a solo RD_CACHE=off timing pass.
#
# Designed to run ON THE SWEEP BOX under nohup (survives ssh drops); every sub-run
# lands its own TSV in $OUTDIR and is SKIPPED on re-run when its row count is complete,
# so re-launching after an interruption only redoes missing work (the cell cache makes
# even redone arms cheap). Heartbeats go to stdout (redirect to chain.log and tail it).
#
# Env (all overridable): OUTDIR (required), CAVIF SAVE_PNG SCORER AOMENC AOMDEC BUTTER,
# EXTRACT_AV1 IVF_RAW, PHASES (default "rd aom anchors timing").
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUTDIR="${OUTDIR:?set OUTDIR (per-run output dir, e.g. /home/lilith/sweep_out/<runid>)}"
mkdir -p "$OUTDIR"

export CAVIF="${CAVIF:-/home/lilith/work/zen/ravif/target/release/cavif}"
export SAVE_PNG="${SAVE_PNG:-/home/lilith/work/zen/zenavif/target/release/examples/save_png}"
export SCORER="${SCORER:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}"
export AOMENC_BIN="${AOMENC:-/home/lilith/work/aom/build_slow/aomenc}"
export AOMDEC="${AOMDEC:-/home/lilith/work/aom/build_slow/aomdec}"
export BUTTER="${BUTTER:-/home/lilith/work/butteraugli/target/release/butteraugli}"
export EXTRACT_AV1="${EXTRACT_AV1:-/home/lilith/work/zen/zenavif/target/release/examples/extract_av1}"
export IVF_RAW="${IVF_RAW:-/home/lilith/work/zen/zenavif/target/release/examples/ivf_raw}"
PHASES="${PHASES:-rd aom anchors timing}"

SAMPLE_LEG="$HERE/sample_images.tsv"        # 22 images
SAMPLE_T26="$HERE/sample_images_train26.tsv" # 24 images
SAMPLE_TIM="$HERE/sample_timing4.tsv"        # 4 images
SPEEDS="2 4 6 8 10"
CPUS="2 4 6 8 9"

say() { echo "[chain $(date -u +%H:%M:%SZ)] $*"; }

# run_one <out.tsv> <expected_rows> <driver.sh> [ENV=val ...]
# Skips when the TSV already has >= expected rows (idempotent resume).
run_one() {
  local out="$1" want="$2" driver="$3"; shift 3
  if [ -s "$out" ] && [ "$(($(wc -l < "$out") - 1))" -ge "$want" ]; then
    say "SKIP (complete $(($(wc -l < "$out") - 1))/$want): $(basename "$out")"
    return 0
  fi
  local t0=$(date +%s)
  say "RUN $(basename "$out") [$*]"
  if env "$@" OUT="$out" bash "$HERE/$driver" > "$out.log" 2>&1; then
    local rows=$(($(wc -l < "$out") - 1))
    say "DONE $(basename "$out") rows=$rows/$want in $(( $(date +%s) - t0 ))s"
    if [ "$rows" -lt "$want" ]; then
      say "WARNING: INCOMPLETE $(basename "$out") ($rows/$want) -- see $out.log"
      grep -h "CELLFAIL\|CONFFAIL\|FAILED" "$out.log" | tail -5 || true
    fi
  else
    say "FAILED run $(basename "$out") -- see $out.log"; tail -5 "$out.log"
  fi
}

# ---- phase: zr RD ladder (cache on, PALCONF on, --threads 1) ----------------
zr_env_common=(CAVIF_EXTRA="--threads 1" PALCONF=1 AOMDEC="$AOMDEC" EXTRACT_AV1="$EXTRACT_AV1" IVF_RAW="$IVF_RAW" AOMENC=)
if [[ " $PHASES " == *" rd "* ]]; then
  say "=== PHASE rd: zenrav1e ladder (5 speeds x 2 cfg x 2 corpora, 12-q, PALCONF) ==="
  for corpus in t26 leg; do
    case $corpus in t26) S=$SAMPLE_T26; N=24; J=24;; leg) S=$SAMPLE_LEG; N=22; J=22;; esac
    for s in $SPEEDS; do
      for cfg in tune off; do
        envs=("${zr_env_common[@]}" SAMPLE="$S" JOBS="$J" ZENRAV1E_SPEED="$s" BUTTER="$BUTTER")
        [ $cfg = tune ] && envs+=(ZENRAVIF_TUNE=ssimulacra2 ZENRAVIF_PALETTE=auto)
        run_one "$OUTDIR/zr_${corpus}_s${s}_${cfg}.tsv" $((N*12)) run_gap.sh "${envs[@]}"
      done
    done
  done
fi

# ---- phase: aom allintra ladder ---------------------------------------------
if [[ " $PHASES " == *" aom "* ]]; then
  say "=== PHASE aom: libaom --allintra ladder (5 cpu x {default,iq} x 2 corpora, cq-grid) ==="
  for corpus in t26 leg; do
    case $corpus in t26) S=$SAMPLE_T26; N=24; J=24;; leg) S=$SAMPLE_LEG; N=22; J=22;; esac
    for cpu in $CPUS; do
      for t in def iq; do
        extra="--allintra"; [ $t = iq ] && extra="--allintra --tune=iq"
        run_one "$OUTDIR/aom_${corpus}_cpu${cpu}${t}.tsv" $((N*8)) aom_only.sh \
          SAMPLE="$S" JOBS="$J" AOMENC="$AOMENC_BIN" AOM_CPU="$cpu" AOM_EXTRA="$extra" AOMFMTS=420 BUTTER="$BUTTER"
      done
    done
  done
fi

# ---- phase: cached GOOD-mode anchors (continuity with committed tables) -----
if [[ " $PHASES " == *" anchors "* ]]; then
  say "=== PHASE anchors: GOOD-mode reference replays (cache hits) ==="
  for corpus in t26 leg; do
    case $corpus in t26) S=$SAMPLE_T26; N=24; J=24;; leg) S=$SAMPLE_LEG; N=22; J=22;; esac
    run_one "$OUTDIR/aomgood_${corpus}_cpu2.tsv"    $((N*8)) aom_only.sh SAMPLE="$S" JOBS="$J" AOMENC="$AOMENC_BIN" AOM_CPU=2 AOMFMTS=420 BUTTER="$BUTTER"
    run_one "$OUTDIR/aomgood_${corpus}_cpu0def.tsv" $((N*8)) aom_only.sh SAMPLE="$S" JOBS="$J" AOMENC="$AOMENC_BIN" AOM_CPU=0 AOMFMTS=420 BUTTER="$BUTTER"
    run_one "$OUTDIR/aomgood_${corpus}_cpu0ss2.tsv" $((N*8)) aom_only.sh SAMPLE="$S" JOBS="$J" AOMENC="$AOMENC_BIN" AOM_CPU=0 AOM_EXTRA="--tune=ssimulacra2" AOMFMTS=420 BUTTER="$BUTTER"
  done
fi

# ---- phase: timing (solo, RD_CACHE=off, no PALCONF, JOBS=1) -----------------
if [[ " $PHASES " == *" timing "* ]]; then
  say "=== PHASE timing: solo wall-time pass (4 img x 3 q x every arm, RD_CACHE=off) ==="
  for s in $SPEEDS; do
    for cfg in tune off; do
      envs=(SAMPLE="$SAMPLE_TIM" JOBS=1 RD_CACHE=off QGRID_ZR="40 65 85" ZENRAV1E_SPEED="$s" CAVIF_EXTRA="--threads 1" AOMENC= BUTTER="$BUTTER")
      [ $cfg = tune ] && envs+=(ZENRAVIF_TUNE=ssimulacra2 ZENRAVIF_PALETTE=auto)
      run_one "$OUTDIR/timing_zr_s${s}_${cfg}.tsv" 12 run_gap.sh "${envs[@]}"
    done
  done
  for cpu in $CPUS; do
    for t in def iq; do
      extra="--allintra"; [ $t = iq ] && extra="--allintra --tune=iq"
      run_one "$OUTDIR/timing_aom_cpu${cpu}${t}.tsv" 12 aom_only.sh \
        SAMPLE="$SAMPLE_TIM" JOBS=1 RD_CACHE=off CQGRID_AOM="16 32 48" AOMENC="$AOMENC_BIN" AOM_CPU="$cpu" AOM_EXTRA="$extra" AOMFMTS=420 BUTTER="$BUTTER"
    done
  done
fi

say "CHAIN COMPLETE. TSVs in $OUTDIR:"
ls -la "$OUTDIR"/*.tsv 2>/dev/null | awk '{print "  " $NF " (" $5 "b)"}'
say "failure scan:"
grep -l "CELLFAIL\|CONFFAIL" "$OUTDIR"/*.log 2>/dev/null || echo "  no CELLFAIL/CONFFAIL in any run log"
