#!/usr/bin/env bash
# Run a harness command on the box with the rd_gap env prewired (all paths are
# the mirrored absolute layout), stream the log live, then auto-fetch the run's
# output dir (the OUT tsv + anything else the run wrote there) into
# remote/results/<run-id>/ locally.
#
#   ./run_remote.sh [VAR=value ...] <script.sh | command> [args ...]
#
# Examples:
#   ./run_remote.sh run_gap.sh                          # full sweep, both encoders
#   ./run_remote.sh AOMENC= QGRID_ZR="60 80" OUT=smoke.tsv run_gap.sh   # zenrav1e-only
#   ./run_remote.sh AOM_CPU=0 AOM_EXTRA="--tune=ssim" OUT=aom_cpu0.tsv aom_only.sh
#   ./run_remote.sh SAMPLE=/home/lilith/sweep_in/smoke3.tsv OUT=s.tsv run_gap.sh
#
# Prewired (each overridable by passing VAR=... yourself; VAR= empty disables —
# e.g. AOMENC= makes run_gap.sh sweep zenrav1e only):
#   CAVIF SAVE_PNG SCORER AOMENC AOMDEC   -> the build_remote.sh binaries
#   JOBS=22                               -> per-image workers (corpus has 22)
#   OUT (relative)                        -> placed in the per-run dir on the box
# Commands run with cwd = scripts/rd_gap on the box; *.sh get `bash` prefixed.
source "$(dirname "$0")/common.sh"
load_token
require_box_ip

ENVS=()
while [ $# -gt 0 ] && [[ "$1" =~ ^[A-Za-z_][A-Za-z0-9_]*= ]]; do ENVS+=("$1"); shift; done
[ $# -ge 1 ] || die "usage: run_remote.sh [VAR=val ...] <script|command> [args...]"

have() { local e; for e in ${ENVS[@]+"${ENVS[@]}"}; do [[ "$e" == "$1="* ]] && return 0; done; return 1; }
getv() { local e; for e in ${ENVS[@]+"${ENVS[@]}"}; do [[ "$e" == "$1="* ]] && { echo "${e#*=}"; return 0; }; done; return 0; }

have CAVIF    || ENVS+=("CAVIF=/home/lilith/work/zen/ravif/target/release/cavif")
have SAVE_PNG || ENVS+=("SAVE_PNG=/home/lilith/work/zen/zenavif/target/release/examples/save_png")
have SCORER   || ENVS+=("SCORER=/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli")
have AOMENC   || ENVS+=("AOMENC=/home/lilith/work/aom/build_slow/aomenc")
have AOMDEC   || ENVS+=("AOMDEC=/home/lilith/work/aom/build_slow/aomdec")
have JOBS     || ENVS+=("JOBS=22")

TS="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_ID="${TS}_$(printf '%s' "$(basename "$1")" | tr -cs 'A-Za-z0-9._-' '_')"
RREMOTE="$REMOTE_OUT_ROOT/$RUN_ID"

# Relative OUT lands in the per-run dir on the box (outside the synced trees,
# so a later sync.sh can never clobber results).
OUT_VAL="$(getv OUT)"; [ -n "$OUT_VAL" ] || OUT_VAL="rd_gap_results.tsv"
[[ "$OUT_VAL" == /* ]] || OUT_VAL="$RREMOTE/$OUT_VAL"
NEW=(); for e in "${ENVS[@]}"; do [[ "$e" == OUT=* ]] || NEW+=("$e"); done
ENVS=("${NEW[@]}" "OUT=$OUT_VAL")

CMD=("$@")
[[ "${CMD[0]}" == *.sh ]] && CMD=(bash "${CMD[@]}")

printf -v envq '%q ' "${ENVS[@]}"
printf -v cmdq '%q ' "${CMD[@]}"
LOCAL_DIR="$RESULTS_DIR/$RUN_ID"; mkdir -p "$LOCAL_DIR"
note "run-id: $RUN_ID"
note "remote: cd $RD_GAP_DIR && env ${ENVS[*]} ${CMD[*]}"
note "log:    $LOCAL_DIR/run.log (streaming)"
t0=$(date +%s)
set +e
box_ssh "mkdir -p $RREMOTE && cd $RD_GAP_DIR && env $envq$cmdq" 2>&1 | tee "$LOCAL_DIR/run.log"
rc=${PIPESTATUS[0]}
set -e
note "remote exit=$rc after $(( $(date +%s) - t0 ))s — fetching $RREMOTE ..."
box_rsync -az "root@$BOX_IP:$RREMOTE/" "$LOCAL_DIR/" || note "fetch: nothing to fetch (run wrote no files?)"

found=0
for f in "$LOCAL_DIR"/*.tsv; do
  [ -f "$f" ] || continue
  found=1
  echo "[remote] RESULT: $f  ($(( $(wc -l < "$f") - 1 )) data rows)"
done
[ "$found" = 1 ] || note "no .tsv fetched — outputs (if any) are in $LOCAL_DIR/"
exit "$rc"
