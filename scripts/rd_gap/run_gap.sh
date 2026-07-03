#!/usr/bin/env bash
# RD-gap sweep driver: zenrav1e (cavif, s2, Q-grid) vs libaom (aomenc, cpu2, fmt x cq-grid).
# Reads a corpus TSV (image<TAB>w<TAB>h<TAB>family), encodes+decodes+scores every cell with
# BOTH encoders, writes one unified TSV. The libaom side is OPTIONAL: unset AOMENC to sweep
# only zenrav1e (then diff vs the committed baseline to track a change).
#
# Run it under the resource guard so it can't peg the shared box:
#   ~/work/zen/scripts/run-heavy -- bash run_gap.sh
#
# Required env (see README.md): CAVIF, SAVE_PNG, SCORER  (+ AOMENC, AOMDEC for the gap).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SAMPLE="${SAMPLE:-$HERE/sample_images.tsv}"
OUT="${OUT:-$HERE/rd_gap_results.tsv}"
# PID-suffixed default: two concurrent runs sharing one WORK dir clobber
# each other's per-image temp dirs and silently LOSE ROWS (the `rm -rf $tmp`
# per worker races the other run's `cat $part`). Override WORK explicitly
# only with per-run-unique paths.
# WORK on LOCAL disk by default: /mnt/v (drvfs) stalls under WSL memory
# reclaim (mini_init drop_caches) and transiently EIOs the per-cell temp
# churn — a 2026-07-03 stall failed ~60 cells across 6 workers in one
# burst. Only durable outputs belong on /mnt/v; temps go local.
WORK="${WORK:-/tmp/rd_gap_work.$$}"; mkdir -p "$WORK"
# Deterministic cell cache (see cell_cache.sh): auto-enable when the standard
# cache dir exists (on the sweep box it lives inside the disk snapshot, so it
# survives teardown/restore). RD_CACHE=off bypasses; timing sweeps MUST bypass.
if [ -z "${RD_CACHE_DIR:-}" ] && [ -d /home/lilith/sweep_cache ]; then
  export RD_CACHE_DIR=/home/lilith/sweep_cache
fi
[ -n "${RD_CACHE_DIR:-}" ] && echo "[rd_gap] cell cache: $RD_CACHE_DIR (RD_CACHE=off to bypass; cached rows replay original enc_ms)"
JOBS="${JOBS:-6}"
QGRID_ZR="${QGRID_ZR:-30 40 50 55 60 65 70 75 80 85 90 95}"    # cavif quality (higher = better)
CQGRID_AOM="${CQGRID_AOM:-8 16 24 32 40 48 56 63}"             # aomenc cq-level (lower = better)
AOMFMTS="${AOMFMTS:-420 444}"
export COLOR="$HERE/color.py"

echo -e "image\tw\th\tfamily\tencoder\tfmt\tq\tbytes\tbpp\tssim2\tenc_ms\tbutteraugli_3n\tbutteraugli_max" > "$OUT"
[ -n "${AOMENC:-}" ] && echo "[rd_gap] both encoders (zenrav1e + libaom)" || echo "[rd_gap] zenrav1e only (AOMENC unset)"

worker() {
  local img="$1" w="$2" h="$3" fam="$4"
  local tmp="$WORK/$(basename "$img" .png)"; mkdir -p "$tmp"
  local part="$tmp/rows.tsv"; : > "$part"
  local bn; bn=$(basename "$img")
  local q fmt r
  local fails=0
  for q in $QGRID_ZR; do
    r=$(bash "$HERE/zenrav1e_cell.sh" "$img" "$w" "$h" "$fam" "$q" "$tmp" 2>>"$tmp/err.log")
    if [[ "$r" == zenrav1e* ]]; then
      printf '%s\t%s\t%s\t%s\t%s\n' "$bn" "$w" "$h" "$fam" "$r" >> "$part"
    else
      fails=$((fails+1))
      echo "  [$(date -u +%H:%M:%SZ)] CELL FAILED $bn zenrav1e q$q: ${r:-<no output>}" >&2
      printf 'CELLFAIL\t%s\tzenrav1e\tq%s\t%s\n' "$bn" "$q" "${r:-none}" >> "$WORK/failures.tsv"
    fi
  done
  if [ -n "${AOMENC:-}" ]; then
    for fmt in $AOMFMTS; do for q in $CQGRID_AOM; do
      r=$(bash "$HERE/aom_cell.sh" "$img" "$w" "$h" "$fam" "$fmt" "$q" "$tmp" 2>>"$tmp/err.log")
      if [[ "$r" == libaom* ]]; then
        printf '%s\t%s\t%s\t%s\t%s\n' "$bn" "$w" "$h" "$fam" "$r" >> "$part"
      else
        fails=$((fails+1))
        echo "  [$(date -u +%H:%M:%SZ)] CELL FAILED $bn libaom-$fmt cq$q: ${r:-<no output>}" >&2
        printf 'CELLFAIL\t%s\tlibaom-%s\tcq%s\t%s\n' "$bn" "$fmt" "$q" "${r:-none}" >> "$WORK/failures.tsv"
      fi
    done; done
  fi
  # flock: concurrent worker appends to one file are NOT atomic on /mnt/v
  # (drvfs) — cache-hit workers finishing simultaneously lost rows (261/576
  # landed, 2026-07-03). Serialize the append through a lock on the output.
  flock "$OUT" -c "cat '$part' >> '$OUT'"
  if (( fails > 0 )); then
    # keep err.log for postmortem instead of deleting it with the tmp dir
    cp "$tmp/err.log" "$WORK/err.$bn.log" 2>/dev/null || true
    echo "  [$(date -u +%H:%M:%SZ)] done $bn rows=$(wc -l < "$part") FAILED_CELLS=$fails (err: $WORK/err.$bn.log)"
  else
    echo "  [$(date -u +%H:%M:%SZ)] done $bn rows=$(wc -l < "$part")"
  fi
  rm -rf "$tmp"
}

running=0
while IFS=$'\t' read -r img w h fam; do
  [ -z "${img:-}" ] && continue
  [ -f "$img" ] || { echo "  skip missing: $img"; continue; }
  worker "$img" "$w" "$h" "$fam" &
  running=$((running+1)); if (( running >= JOBS )); then wait -n; running=$((running-1)); fi
done < <(tail -n +2 "$SAMPLE")
wait
if [ -s "$WORK/failures.tsv" ]; then
  echo "[rd_gap] WARNING: $(wc -l < "$WORK/failures.tsv") FAILED CELLS -- results are INCOMPLETE. See $WORK/failures.tsv + $WORK/err.*.log" >&2
fi
echo "[rd_gap] COMPLETE rows=$(($(wc -l < "$OUT")-1)) -> $OUT"
echo "[rd_gap] analyze:  python3 $HERE/analyze.py $OUT"
