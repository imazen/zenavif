#!/usr/bin/env bash
# Commit a tier's cells TSV the moment it exists, and refresh the interim
# analysis over everything landed so far.
#
# The 1024 tiers are ~1.7 h each. Waiting for the whole grid before committing
# anything means one interruption costs every measured tier, and the encodes
# themselves are the expensive part. Each tier is independently valid data — the
# analysis is row-oriented and skips absent tiers — so each one gets committed
# as it lands.
#
#     bash land_tier.sh <tier-name>...      # blocks until each appears
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${OUT:-$HOME/tmp/encrd2}"
B="$REPO/benchmarks"

for tier in "$@"; do
  src="$OUT/cells_$tier.tsv"
  while [ ! -s "$src" ]; do
    # Give up only if NOTHING is producing tiers any more. The pattern must cover
    # every driver AND run_grid.py itself: an earlier version listed only
    # run_full_sweep.sh, so when the sweep was relaunched as run_remaining2.sh the
    # lander declared "driver gone" and exited for all six tiers while the 1024
    # tier was still being scored.
    if ! pgrep -f 'run_full_sweep|run_remaining|run_topup_1024|cut_and_finish|run_grid.py' \
         >/dev/null 2>&1; then
      [ -s "$src" ] || { echo "land_tier: $tier never produced a TSV; nothing is running"; break; }
    fi
    sleep 20
  done
  [ -s "$src" ] || continue
  rows=$(grep -vc '^#' "$src")
  # zstd anything over ~30 KB, per the repo rule on committed benchmark data
  if [ "$(wc -c < "$src")" -gt 30000 ]; then
    zstd -19 -q -f "$src" -o "$B/encode_rd_sweep_${tier}_2026-08-08.tsv.zst"
  else
    cp "$src" "$B/encode_rd_sweep_${tier}_2026-08-08.tsv"
  fi
  [ -s "$OUT/cells_${tier}_floor.tsv" ] && \
    cp "$OUT/cells_${tier}_floor.tsv" "$B/encode_rd_sweep_${tier}_floor_2026-08-08.tsv"
  cd "$REPO" || exit 1
  # Start a FRESH change first. Without this, if @ happens to be an
  # already-pushed commit (because a human/agent described and pushed it
  # directly rather than leaving an empty working copy), `jj describe` would
  # rewrite that commit's message and diverge it from origin.
  jj new >/dev/null 2>&1
  jj describe -m "bench(encode-rd): tier $tier of the full sweep — $rows cells

Committed as it landed rather than at the end of the grid: the 1024 tiers are
~1.7 h each and the encodes are the expensive part, so one interruption must
not cost a measured tier. Each tier is independently valid — the analysis is
row-oriented and skips absent tiers.

$(grep '^# grid:' "$src" | head -1)
$(grep '^# scheduling:' "$src" | head -1)" >/dev/null 2>&1
  jj bookmark set main -r @ >/dev/null 2>&1
  jj git push --bookmark main >/dev/null 2>&1 && echo "LANDED+PUSHED $tier ($rows cells)" \
    || echo "LANDED $tier ($rows cells) — push failed, commit is local"
  jj new >/dev/null 2>&1
done
