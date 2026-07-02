#!/usr/bin/env bash
# Delete the sweep box. Requires --yes. Salvages any un-fetched remote results
# into remote/results/_salvage/ first (best-effort). Deletes ONLY the exact
# box named $BOX_NAME — never touches any other server on the account.
source "$(dirname "$0")/common.sh"
load_token

if ! "$HCLOUD" server describe "$BOX_NAME" >/dev/null 2>&1; then
  echo "[teardown] no '$BOX_NAME' box exists — nothing to delete."
  exit 0
fi

if [ "${1:-}" != "--yes" ]; then
  echo "[teardown] this would DELETE Hetzner server '$BOX_NAME':"
  "$HCLOUD" server describe "$BOX_NAME" | grep -E '^(ID|Name|Status|Created)' || true
  echo "[teardown] confirm with:  $0 --yes"
  exit 1
fi

require_box_ip
note "salvaging any remote results from $REMOTE_OUT_ROOT ..."
mkdir -p "$RESULTS_DIR/_salvage"
box_rsync -az "root@$BOX_IP:$REMOTE_OUT_ROOT/" "$RESULTS_DIR/_salvage/" 2>/dev/null \
  && note "salvaged into $RESULTS_DIR/_salvage/" \
  || note "salvage skipped (box unreachable or nothing to fetch)"

"$HCLOUD" server delete "$BOX_NAME"
note "deleted '$BOX_NAME'. Remaining rd-gap-sweeps servers (should be none):"
"$HCLOUD" server list -l purpose=rd-gap-sweeps
