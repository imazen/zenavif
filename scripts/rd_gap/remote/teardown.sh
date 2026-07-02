#!/usr/bin/env bash
# Delete the sweep box. Requires --yes. Salvages any un-fetched remote results
# into remote/results/_salvage/ first (best-effort). Deletes ONLY the exact
# box named $BOX_NAME — never touches any other server on the account.
#
#   ./teardown.sh --snapshot --yes   # snapshot the disk first (restore later
#                                    # with FROM_SNAPSHOT=auto ./provision.sh),
#                                    # then delete. Preferred: the box is too
#                                    # expensive to idle, snapshots are cheap.
source "$(dirname "$0")/common.sh"
load_token

want_snapshot=""
if [ "${1:-}" = "--snapshot" ]; then want_snapshot=1; shift; fi

if ! "$HCLOUD" server describe "$BOX_NAME" >/dev/null 2>&1; then
  echo "[teardown] no '$BOX_NAME' box exists — nothing to delete."
  exit 0
fi

if [ "${1:-}" != "--yes" ]; then
  echo "[teardown] this would ${want_snapshot:+SNAPSHOT then }DELETE Hetzner server '$BOX_NAME':"
  "$HCLOUD" server describe "$BOX_NAME" | grep -E '^(ID|Name|Status|Created)' || true
  echo "[teardown] confirm with:  $0 ${want_snapshot:+--snapshot }--yes"
  exit 1
fi

require_box_ip
note "salvaging any remote results from $REMOTE_OUT_ROOT ..."
mkdir -p "$RESULTS_DIR/_salvage"
box_rsync -az "root@$BOX_IP:$REMOTE_OUT_ROOT/" "$RESULTS_DIR/_salvage/" 2>/dev/null \
  && note "salvaged into $RESULTS_DIR/_salvage/" \
  || note "salvage skipped (box unreachable or nothing to fetch)"

if [ -n "$want_snapshot" ]; then
  desc="$BOX_NAME-$(date +%s)"
  note "snapshotting disk as '$desc' (takes a few minutes) ..."
  "$HCLOUD" server shutdown "$BOX_NAME" >/dev/null 2>&1 || true
  sleep 10
  "$HCLOUD" server create-image --type snapshot --description "$desc" "$BOX_NAME" \
    || die "snapshot failed — NOT deleting the box"
  note "snapshot '$desc' created. Old zenavif-sweep snapshots (delete extras in the console):"
  "$HCLOUD" image list --type snapshot | grep zenavif-sweep || true
fi

"$HCLOUD" server delete "$BOX_NAME"
note "deleted '$BOX_NAME'. Remaining rd-gap-sweeps servers (should be none):"
"$HCLOUD" server list -l purpose=rd-gap-sweeps
