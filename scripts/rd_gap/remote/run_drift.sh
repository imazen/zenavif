#!/usr/bin/env bash
# P0 label-drift legs on the sweep box (docs/FEATURE_HINTS_PLAN.md P0.2).
#
#   ./run_drift.sh base     # registry zenrav1e 0.1.4 (the canonical route)
#   ./run_drift.sh master   # box-side dev-patch -> zenrav1e--drift-master export
#
# Each leg: (un)applies the box-side dev-patch, rebuilds the drift_reencode
# example, runs the sampled cells, and fetches the output TSV into
# results/drift/. The LOCAL repos are never patched by this script — the
# patch is applied to the box's synced copies only (and reverted by the next
# `base` invocation or re-sync).
#
# Prereqs: provision.sh + sync.sh (which must have shipped
# scripts/rd_gap/drift_sample.tsv's images + the zenrav1e--drift-master export).
set -euo pipefail
source "$(dirname "$0")/common.sh"
load_token
require_box_ip

leg="${1:-}"
[ "$leg" = base ] || [ "$leg" = master ] || die "usage: $0 base|master"

SAMPLE="$RD_GAP_DIR/drift_sample.tsv"
OUT_DIR="$REMOTE_OUT_ROOT/drift"
JOBS="${JOBS:-40}"

note "leg=$leg: preparing box-side dep state ..."
box_ssh 'bash -s' <<REMOTE
set -euo pipefail
export PATH="\$HOME/.cargo/bin:\$PATH"
cd /home/lilith/work/zen
python3 - "$leg" <<'PY'
import sys
leg = sys.argv[1]
PATCH = '''
# DEV-ONLY BOX-SIDE (P0 drift master leg): resolve zenrav1e from the exported
# master tree. Applied/reverted by run_drift.sh; never synced back.
[patch.crates-io]
zenrav1e = { path = "../zenrav1e--drift-master" }
'''
za = 'zenavif/Cargo.toml'
s = open(za).read().split('# DEV-ONLY BOX-SIDE')[0].rstrip() + '\n'
if leg == 'master':
    s += PATCH
open(za, 'w').write(s)
rv = 'ravif/ravif/Cargo.toml'
s = open(rv).read()
if leg == 'master':
    s = s.replace('zenrav1e = { version = "0.1.4"', 'zenrav1e = { version = "0.2.0"')
else:
    s = s.replace('zenrav1e = { version = "0.2.0"', 'zenrav1e = { version = "0.1.4"')
open(rv, 'w').write(s)
print(f'box-side dep state -> {leg}')
PY
cd /home/lilith/work/zen/zenavif
echo "=== building drift_reencode ($leg) ==="
cargo build --release --features __expert --example drift_reencode > /tmp/drift_build_$leg.log 2>&1 \
  || { tail -40 /tmp/drift_build_$leg.log; exit 1; }
grep -E "Compiling (zenrav1e|zenravif|zenavif)" /tmp/drift_build_$leg.log || true
mkdir -p $OUT_DIR/encoded_$leg
echo "=== running leg $leg ==="
./target/release/examples/drift_reencode \
  --sample $SAMPLE \
  --out $OUT_DIR/drift_$leg.tsv \
  --leg $leg --jobs $JOBS \
  --encoded-dir $OUT_DIR/encoded_$leg 2>&1 | tail -5
REMOTE

mkdir -p "$RESULTS_DIR/drift"
box_rsync -az "root@$BOX_IP:$OUT_DIR/drift_$leg.tsv" "$RESULTS_DIR/drift/"
note "leg $leg done -> $RESULTS_DIR/drift/drift_$leg.tsv"
