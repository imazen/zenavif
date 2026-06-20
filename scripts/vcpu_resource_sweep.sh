#!/usr/bin/env bash
# vCPU resource sweep for zenavif — peak heap / peak RSS / marginal WS / wall
# across (size x speed x THREAD-COUNT). zenavif wraps zenravif/AV1 which is
# natively TILE-parallel: threads matter only up to the tile count, so speedup
# saturates at the tile count (not the thread count) — this sweep measures it.
# estimate_encode is thread-independent (its doc says "divide by thread count").
#
# Two runs per cell: clean (wall + VmHWM peak/delta + est_*) and heaptrack
# (PEAK_HEAP) at a thread subset. ONE PROCESS PER CELL, run-heavy, SERIAL.
#
# Usage: scripts/vcpu_resource_sweep.sh <driver_bin> <img_dir> <out.tsv>
set -uo pipefail
DRIVER="${1:?driver}"; IMGDIR="${2:?img dir}"; OUT="${3:?out tsv}"
HT_DIR="${HT_DIR:-/tmp/zenavif_vcpu_heaptrack}"; mkdir -p "$HT_DIR"
TMPOUT="${TMPOUT:-/tmp/zenavif_vcpu_out.avif}"
export GLIBC_TUNABLES=glibc.malloc.mmap_threshold=131072

IMAGES=( "256:photo" "1024:photo" "2048:photo" )
SPEEDS=( 6 10 )                  # rav1e speed (0=slowest/best .. 10=fastest); q75
THREADS=( 1 2 4 8 16 28 )
HT_THREADS="${HT_THREADS:-1 8 28}"
QUALITY=75; DEPTH=8; ALPHA=rgb

parse_ht() { heaptrack_print "$1" 2>/dev/null | python3 -c '
import sys,re
ph=pr=0
def kb(v,u): f={"B":1/1024,"K":1,"M":1024,"G":1024*1024}.get(u[0].upper(),0); return f*float(v)
for ln in sys.stdin:
    m=re.search(r"peak heap memory consumption:\s*([\d.]+)\s*([KMGB])",ln)
    if m: ph=kb(m.group(1),m.group(2))
    m=re.search(r"peak RSS[^:]*:\s*([\d.]+)\s*([KMGB])",ln)
    if m: pr=kb(m.group(1),m.group(2))
print(f"{int(ph)} {int(pr)}")'; }
getf() { sed -n "s/.*\b$2=\([^ ]*\).*/\1/p" <<<"$1"; }

echo -e "codec\tcontent_class\tsrc\twidth\theight\tpixels\tpath\teffort\tthreads\test_min_kb\test_typ_kb\test_max_kb\test_time_ms\tmeas_peak_heap_kb\tmeas_peak_rss_kb\tmeas_vmhwm_kb\tmeas_delta_kb\tmeas_wall_ms\tmeas_user_ms\tmeas_sys_ms\tbytes\tok" > "$OUT"

total=$(( ${#IMAGES[@]} * ${#SPEEDS[@]} * ${#THREADS[@]} )); i=0
for spec in "${IMAGES[@]}"; do
  label="${spec%%:*}"; cls="${spec##*:}"; png="$IMGDIR/${label}.png"
  [[ -f "$png" ]] || { echo "MISSING $png" >&2; continue; }
  for sp in "${SPEEDS[@]}"; do
    for t in "${THREADS[@]}"; do
      i=$((i+1))
      printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "claude-resource-harness" \
        "avif vcpu sweep $i/$total ${label} s${sp} t${t}" > .workongoing 2>/dev/null || true
      echo "[$i/$total] ${label}^2 speed${sp} t${t}" >&2
      line=$("$DRIVER" "$png" encode "$QUALITY" "$sp" "$DEPTH" "$ALPHA" "$TMPOUT" "$t" 2>/dev/null)
      [[ -z "$line" ]] && { echo "  FAIL clean" >&2; continue; }
      delta=$(getf "$line" delta_kb); vmhwm=$(getf "$line" peak_kb)
      wall=$(getf "$line" wall_ms);   user=$(getf "$line" user_ms)
      sys=$(getf "$line" sys_ms);     bytes=$(getf "$line" bytes)
      emin=$(getf "$line" est_min_kb); etyp=$(getf "$line" est_typ_kb)
      emax=$(getf "$line" est_max_kb); etime=$(getf "$line" est_time_ms)
      ph=""; pr=""
      if [[ " $HT_THREADS " == *" $t "* ]]; then
        htf="$HT_DIR/${label}_s${sp}_t${t}"; rm -f "${htf}.zst"
        heaptrack -o "$htf" "$DRIVER" "$png" encode "$QUALITY" "$sp" "$DEPTH" "$ALPHA" "$TMPOUT" "$t" >/dev/null 2>&1
        read -r ph pr < <(parse_ht "${htf}.zst")
      fi
      px=$((label*label))
      echo -e "zenavif\t${cls}\t${label}.png\t${label}\t${label}\t${px}\tlossy\t${sp}\t${t}\t${emin}\t${etyp}\t${emax}\t${etime}\t${ph}\t${pr}\t${vmhwm}\t${delta}\t${wall}\t${user}\t${sys}\t${bytes}\t1" >> "$OUT"
    done
  done
done
echo "wrote $OUT ($(( $(wc -l < "$OUT") - 1 )) rows)" >&2
