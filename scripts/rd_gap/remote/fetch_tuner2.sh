#!/usr/bin/env bash
# Fetch the tuner2 chain outputs to the durable results dir + local results,
# then run the analyzer. Idempotent.
set -euo pipefail
source "$(dirname "$0")/common.sh"
load_token
require_box_ip

DEST_MV=/mnt/v/output/zenavif/tuner2-20260704
DEST_LOCAL="$RESULTS_DIR/tuner2_20260704"
mkdir -p "$DEST_MV" "$DEST_LOCAL"

note "fetching tuner2 outputs from $BOX_IP ..."
box_rsync "root@$BOX_IP:/home/lilith/sweep_out/tuner2_20260704/" "$DEST_LOCAL/"
rsync -a "$DEST_LOCAL/" "$DEST_MV/"
note "fetched to $DEST_LOCAL (mirrored to $DEST_MV)"

note "analyzer:"
nice -n19 python3 "$(dirname "$0")/../../hyperparam/analyze_tuner2.py" "$DEST_LOCAL" "$@"
