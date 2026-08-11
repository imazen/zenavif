#!/usr/bin/env bash
# Merge the per-tier cells TSVs from run_full_sweep.sh into one grid and run the
# full reduction over it.
#
# The tiers are separate run_grid.py invocations (each interleaves all four
# arms; only the ladder set differs, because the four-way time overlap moves
# with image size). The analysis is row-oriented, so concatenating them — one
# header, every "# " provenance line kept — is exactly equivalent to one run
# for every section except the timing-drift ones, which are per-tier anyway.

set -uo pipefail
OUT="${OUT:-$HOME/tmp/encrd2}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO/scripts/encode_rd" || exit 1

RD="$OUT/cells_rd.tsv"        # the RD grid: 64 / 256 / 1024, full rate density
FIT="$OUT/cells_fit.tsv"      # the size-scaling grid: adds 2048 and 4096

merge () {                    # merge <dest> <src...>
  local dest="$1"; shift
  : > "$dest"
  local first=1
  for f in "$@"; do
    [ -s "$f" ] || { echo "  (skip missing $f)" >&2; continue; }
    if [ $first -eq 1 ]; then
      cat "$f" >> "$dest"; first=0
    else
      # keep every provenance comment, drop only the repeated column header
      grep '^#' "$f" >> "$dest"
      tail -n +2 <(grep -v '^#' "$f") >> "$dest"
    fi
  done
  echo "  $dest: $(grep -vc '^#' "$dest") rows (incl. header)" >&2
}

merge "$RD"  "$OUT/cells_t64.tsv" "$OUT/cells_t256.tsv" \
             "$OUT/cells_t1024a.tsv" "$OUT/cells_t1024b.tsv" \
             "$OUT/cells_tu1024a.tsv" "$OUT/cells_tu1024b.tsv"
merge "$FIT" "$OUT/cells_fit_photo.tsv" "$OUT/cells_fit_screen.tsv"

# Quality targets: even density low and high. ssim2_floor is the encoder-only
# view (ceiling 100); ssim2_ref includes the fixed 4:2:0 round-trip cost and is
# run second so both are on the record.
for metric in ssim2_floor ssim2_ref; do
  python3 analyze_matched.py "$RD" --metric "$metric" \
      --targets 20,30,40,50,60,70,80,88 > "$OUT/analysis_rd_$metric.txt" 2>&1
  echo "  wrote $OUT/analysis_rd_$metric.txt ($(wc -l < "$OUT/analysis_rd_$metric.txt") lines)" >&2
done

python3 bdrate_matched.py "$RD" --metric ssim2_floor --qgrid 20:88:2 \
    > "$OUT/bdrate_rd_ssim2_floor.txt" 2>&1
python3 bdrate_matched.py "$RD" --metric ssim2_ref --qgrid 20:80:2 \
    > "$OUT/bdrate_rd_ssim2_ref.txt" 2>&1
# A second metric family, because ssim2 saturating near the 4:2:0 floor is a
# known hazard of this harness. zensim disagreeing with ssim2 is a finding.
python3 bdrate_matched.py "$RD" --metric zensim_floor --qgrid 20:88:2 \
    > "$OUT/bdrate_rd_zensim_floor.txt" 2>&1

# The size model wants every size that exists, so it reads the union.
cat "$RD" > "$OUT/cells_all.tsv"
grep '^#' "$FIT" >> "$OUT/cells_all.tsv"
tail -n +2 <(grep -v '^#' "$FIT") >> "$OUT/cells_all.tsv"
python3 analyze_matched.py "$OUT/cells_all.tsv" --metric ssim2_floor \
    --targets 30,50,70,88 > "$OUT/analysis_sizemodel.txt" 2>&1
echo "  wrote $OUT/analysis_sizemodel.txt" >&2

python3 payload_identity.py "$OUT/cells_all.tsv" --artifacts "$OUT/artifacts" \
    --pairs svtc:svtrs --examples 12 > "$OUT/payload_identity.txt" 2>&1
echo "  wrote $OUT/payload_identity.txt" >&2
echo "ANALYSIS DONE" >&2
