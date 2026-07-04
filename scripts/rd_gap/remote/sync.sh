#!/usr/bin/env bash
# Rsync the LOCAL WORKING TREES (including uncommitted WIP — that is the point:
# the box tests exactly what your checkout builds), the pinned libaom source,
# and the corpus subset referenced by sample_images.tsv, all to MIRRORED
# absolute paths on the box. Fast delta on re-runs.
#
#   ./sync.sh                      # repos + aom + default corpus subset
#   ./sync.sh /path/extra.tsv ...  # ALSO sync these sample TSVs (to
#                                  #   /home/lilith/sweep_in/<basename> on the box)
#                                  #   + the images they reference
#   SYNC_DELETE=1 ./sync.sh        # exact mirror (delete remote files gone locally)
#
# Synced repos: see common.sh ZEN_REPOS (a trailing '?' marks a tree optional —
# skipped with a note when absent locally, e.g. a dev-patch export that was
# cleaned up). zenanalyze is a zenavif path dev-dependency (needed to build the
# examples). If you point a dev-patch at a different workspace/export, add that
# tree to ZEN_REPOS.
source "$(dirname "$0")/common.sh"
load_token
require_box_ip

t0=$(date +%s)
RSYNC_BASE=(-az --info=stats1 --human-readable
  --exclude 'target/' --exclude '.git/' --exclude '.jj/' --exclude '.workongoing'
  --exclude '__pycache__/' --exclude '*.profraw')
# Big, build-irrelevant subtrees (anchored at each repo root):
declare -A EXTRA_EXCLUDES=(
  [zenavif]="--exclude /fuzz/ --exclude /benchmarks/ --exclude /cron-logs/"
  [zenanalyze]="--exclude /benchmarks/"
  [zenpixels]="--exclude /benchmarks/"
  [zencodec]="--exclude /benchmarks/"
)
[ "${SYNC_DELETE:-0}" = 1 ] && RSYNC_BASE+=(--delete)

for r in "${ZEN_REPOS[@]}"; do
  optional=""
  case "$r" in *\?) optional=1; r="${r%\?}";; esac
  src="$HOME/work/zen/$r"
  if [ ! -d "$src" ]; then
    [ -n "$optional" ] && { note "SKIP optional zen/$r (not present locally)"; continue; }
    die "missing local repo: $src (ZEN_REPOS in common.sh is stale?)"
  fi
  note "sync zen/$r ..."
  # shellcheck disable=SC2086  # EXTRA_EXCLUDES is a flag string, split intended
  box_rsync "${RSYNC_BASE[@]}" ${EXTRA_EXCLUDES[$r]:-} "$src/" "root@$BOX_IP:/home/lilith/work/zen/$r/"
done

# Provenance: the ravif [patch.crates-io] target decides WHICH zenrav1e tree the
# box's cavif builds from (concurrent sessions toggle it for A/B measurements) —
# print what this sync just shipped so every run is attributable.
note "ravif patch state shipped: $(grep -A3 '^\[patch.crates-io\]' "$HOME/work/zen/ravif/Cargo.toml" | grep -m1 zenrav1e || echo '(no patch — registry zenrav1e)')"

# Decoder fallback: the zenavif decode examples occasionally don't build from
# the WIP tree (e.g. a sibling-repo contract change mid-flight). Ship the
# CURRENT LOCAL binaries — the exact decoders the local harness runs — so
# build_remote.sh can fall back LOUDLY instead of leaving the box unusable.
EXDIR="$HOME/work/zen/zenavif/target/release/examples"
if [ -x "$EXDIR/save_png" ] && [ -x "$EXDIR/extract_av1" ] && [ -x "$EXDIR/decode_avif" ] && [ -x "$EXDIR/ivf_raw" ]; then
  note "sync decoder fallback binaries (local target/release/examples) ..."
  { for b in save_png extract_av1 decode_avif ivf_raw; do
      echo "$(sha256sum "$EXDIR/$b" | cut -c1-16)  built $(date -u -r "$EXDIR/$b" +%Y-%m-%dT%H:%MZ)  $b"
    done; } > /tmp/decoder_fallback_manifest.txt
  box_ssh "mkdir -p /home/lilith/decoder_fallback"
  box_rsync -az "$EXDIR/save_png" "$EXDIR/extract_av1" "$EXDIR/decode_avif" "$EXDIR/ivf_raw" \
    /tmp/decoder_fallback_manifest.txt "root@$BOX_IP:/home/lilith/decoder_fallback/"
else
  note "WARNING: local decoder examples not all built — no fallback synced (source build must succeed on the box)"
fi

# butteraugli (lives outside ~/work/zen): scorer for the metric-gaming guard
# columns (BUTTER env in the cell scripts).
if [ -d "$HOME/work/butteraugli" ]; then
  note "sync butteraugli ..."
  box_rsync "${RSYNC_BASE[@]}" --exclude 'reference-sources/' --exclude '*.out.*' --exclude 'perf.data*' \
    "$HOME/work/butteraugli/" "root@$BOX_IP:/home/lilith/work/butteraugli/"
fi

# libaom source at the pinned rev (build dirs stay local; the box builds its own).
aom_rev="$(git -C "$AOM_SRC" rev-parse HEAD)"
if [ "$aom_rev" != "$AOM_PIN" ] && [ "${ALLOW_AOM_DRIFT:-0}" != 1 ]; then
  die "local ~/work/aom is at $aom_rev but the harness pins $AOM_PIN (ALLOW_AOM_DRIFT=1 to sync anyway)"
fi
note "sync aom source (rev ${aom_rev:0:12}) ..."
box_rsync "${RSYNC_BASE[@]}" --exclude 'build*/' "$AOM_SRC/" "root@$BOX_IP:/home/lilith/work/aom/"
box_ssh "echo $aom_rev > /home/lilith/work/aom/.synced_rev"

# Corpus: every image referenced by the default sample TSV + any TSVs passed as
# args, copied to the SAME absolute path on the box => TSVs need no rewriting.
list="$(mktemp)"; trap 'rm -f "$list"' EXIT
tsvs=("$RD_GAP_DIR/sample_images.tsv" "$@")
for tsv in "${tsvs[@]}"; do
  [ -f "$tsv" ] || die "sample tsv not found: $tsv"
  tail -n +2 "$tsv" | cut -f1
done | sort -u | sed 's|^/||' > "$list"
missing=0
while read -r rel; do
  [ -f "/$rel" ] || { echo "MISSING corpus image: /$rel" >&2; missing=1; }
done < "$list"
[ "$missing" = 0 ] || die "corpus images missing locally — regenerate the TSV (make_sample.sh)"
note "sync corpus ($(wc -l < "$list") images) ..."
box_rsync -az --info=stats1 --files-from="$list" / "root@$BOX_IP:/"

# Ad-hoc TSVs land in sweep_in/ (they already contain valid absolute image paths).
for tsv in "$@"; do
  box_rsync -az "$tsv" "root@$BOX_IP:$REMOTE_IN_DIR/$(basename "$tsv")"
  note "extra sample synced — use with:  ./run_remote.sh SAMPLE=$REMOTE_IN_DIR/$(basename "$tsv") ..."
done

note "SYNC DONE in $(( $(date +%s) - t0 ))s"
