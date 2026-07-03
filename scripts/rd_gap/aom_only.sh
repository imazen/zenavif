#!/usr/bin/env bash
# libaom-only sweep driver: same corpus/format/cq methodology as run_gap.sh's aom side,
# but no cavif — for measuring alternative libaom operating points (cpu-used, tune) as
# reference baselines. Reuses aom_cell.sh (AOM_CPU / AOM_EXTRA env passthrough).
#
#   AOMENC=... AOMDEC=... SCORER=... AOM_CPU=0 [AOM_EXTRA="--tune=ssimulacra2"] \
#   OUT=aom_cpu0.tsv [AOMFMTS=420] [CQGRID_AOM="8 16 24 32 40 48 56 63"] \
#     ~/work/zen/scripts/run-heavy -- bash aom_only.sh
#
# Emits the same row schema as run_gap.sh so analyze.py / bd_rate helpers work unchanged.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SAMPLE="${SAMPLE:-$HERE/sample_images.tsv}"
OUT="${OUT:-$HERE/aom_only_results.tsv}"
# PID-suffixed default: two concurrent runs sharing one WORK dir clobber each
# other's per-image temp dirs and silently LOSE ROWS (same fix as run_gap.sh).
WORK="${WORK:-/mnt/v/output/zenavif/rd_gap_work_aomonly.$$}"; mkdir -p "$WORK"
JOBS="${JOBS:-6}"
CQGRID_AOM="${CQGRID_AOM:-8 16 24 32 40 48 56 63}"
AOMFMTS="${AOMFMTS:-420}"
export COLOR="$HERE/color.py"

# Header matches run_gap.sh (aom_cell.sh emits the butteraugli columns whenever
# BUTTER is set; the old 11-col header silently misaligned those rows).
echo -e "image\tw\th\tfamily\tencoder\tfmt\tq\tbytes\tbpp\tssim2\tenc_ms\tbutteraugli_3n\tbutteraugli_max" > "$OUT"
echo "[aom_only] cpu=${AOM_CPU:-2} extra='${AOM_EXTRA:-}' fmts='$AOMFMTS' cq='$CQGRID_AOM'"

worker() {
  local img="$1" w="$2" h="$3" fam="$4"
  local tmp="$WORK/$(basename "$img" .png)"; mkdir -p "$tmp"
  local part="$tmp/rows.tsv"; : > "$part"
  local bn; bn=$(basename "$img")
  local q fmt r
  local fails=0
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
  cat "$part" >> "$OUT"
  if (( fails > 0 )); then
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
  echo "[aom_only] WARNING: $(wc -l < "$WORK/failures.tsv") FAILED CELLS -- results are INCOMPLETE. See $WORK/failures.tsv + $WORK/err.*.log" >&2
fi
echo "[aom_only] COMPLETE rows=$(($(wc -l < "$OUT")-1)) -> $OUT"
