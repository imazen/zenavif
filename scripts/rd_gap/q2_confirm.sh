#!/usr/bin/env bash
# Q2 (GOAL queue) dense CONFIRM for a coarse-selected graft winner
# (rule: DECISION_RULE_Q2_HOLE.md). Two legs per graft:
#   1. dense-grid RD leg (Q1's 16-point grid, standard dispatch)
#   2. JOBS=1 timing leg on the 6q coarse grid (the wall claim)
# Usage: q2_confirm.sh GRAFT [OUTDIR]
set -eu
G="${1:?graft name (i7|prune|txd2|txmin|i7prune|base)}"
D="${2:-/mnt/v/output/cooptloop/q2-hole-2026-07-12}"
HERE="$(cd "$(dirname "$0")" && pwd)"
export SAVE_PNG="${SAVE_PNG:-$HERE/../../target/release/examples/save_png}"
export SCORER="${SCORER:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}"
export BUTTER="${BUTTER:-/home/lilith/work/butteraugli/target/release/butteraugli}"
export CAVIF="${CAVIF:-/home/lilith/work/zen/ravif--cooptloop/target/release/cavif}"
export SAMPLE="$HERE/sample_images_train26.tsv" RD_CACHE=off
export CAVIF_EXTRA="--yuv 420" ZENRAV1E_SPEED=9
unset AOMENC || true
export ZENRAVIF_Q2_GRAFT="$([ "$G" = base ] && echo "" || echo "$G")"
export QGRID_ZR="5 10 15 20 25 30 35 40 45 50 55 60 70 80 90 95"
OUT="$D/confirm_${G}_dense.tsv" bash "$HERE/run_gap.sh"
echo "[q2-confirm] $G dense done ($(date -u +%H:%M:%SZ))"
export QGRID_ZR="30 50 60 75 85 95"
JOBS=1 OUT="$D/confirm_${G}_solo6q.tsv" bash "$HERE/run_gap.sh"
echo "[q2-confirm] $G solo timing done ($(date -u +%H:%M:%SZ))"
