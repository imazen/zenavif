# Shared config + helpers for the zenavif rd_gap remote sweep box.
# Source me from the sibling scripts; do not execute directly.
#
# Everything here fails LOUD: missing token, missing key, missing box => exit 1.
# The Hetzner API token is loaded from ~/.config/hetzner/credentials and is
# NEVER printed, logged, or synced to the box (the box needs no hcloud access).

set -euo pipefail

REMOTE_HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

HCLOUD="${HCLOUD:-$HOME/.local/bin/hcloud}"
BOX_NAME="${BOX_NAME:-zenavif-sweep-1}"
BOX_TYPE="${BOX_TYPE:-ccx63}"                      # 48 dedicated AMD vCPU, 192 GB RAM
BOX_LOCATIONS="${BOX_LOCATIONS:-fsn1 nbg1 hel1}"   # EU only, tried in order
BOX_IMAGE="${BOX_IMAGE:-ubuntu-24.04}"
SSH_KEY_NAME="${SSH_KEY_NAME:-zen-arm-dev-20260528}"  # existing hcloud key
SSH_KEY_FILE="${SSH_KEY_FILE:-$HOME/.ssh/zen-arm-dev}" # matching local private key
KNOWN_HOSTS="$HOME/.ssh/known_hosts.zenavif-sweep"     # per-box file (IPs get recycled)

# The box MIRRORS the local absolute layout (/home/lilith/work/..., /mnt/v/...).
# Why: ravif/Cargo.toml's dev-only [patch.crates-io] points at an ABSOLUTE path
# (workspace paths when a dev-patch targets one) and sample_images.tsv lists absolute
# /mnt/v/... corpus paths. Mirroring means zero path rewriting anywhere: the same
# TSVs, scripts, and manifests work verbatim on both ends.
#
# zenpixels + zencodec ride along because zenavif's local (gitignored)
# .cargo/config.toml `paths`-overrides into them; that config file syncs with
# the zenavif tree, so its targets must exist on the box too.
# zenrav1e--tune was removed at the tune-ss2 landing (2026-07-02). A trailing
# '?' marks a tree OPTIONAL: sync.sh skips it with a note instead of dying.
# zenrav1e--drift-master is the P0 drift-check dev-patch target (regenerate:
#   mkdir -p ~/work/zen/zenrav1e--drift-master && \
#     git -C ~/work/zen/zenrav1e archive <rev> | tar -x -C ~/work/zen/zenrav1e--drift-master
# see FEATURE_HINTS_PLAN.md P0 + run_drift.sh).
ZEN_REPOS=(ravif zenrav1e 'zenrav1e--drift-master?' zenavif zenanalyze fast-ssim2 zenpixels zencodec)
AOM_SRC="$HOME/work/aom"
AOM_PIN="632172a468f5e91c5b40daaa0a91f4a291c63af4"  # docs/RD_GAP_VS_LIBAOM.md pinned rev
RD_GAP_DIR="/home/lilith/work/zen/zenavif/scripts/rd_gap"  # same path on both ends
REMOTE_OUT_ROOT="/home/lilith/sweep_out"   # remote run outputs (OUTSIDE the synced trees)
REMOTE_IN_DIR="/home/lilith/sweep_in"      # remote home for ad-hoc sample TSVs
RESULTS_DIR="$REMOTE_HERE/results"         # local fetched results (gitignored)

die()  { echo "FATAL: $*" >&2; exit 1; }
note() { echo "[remote] $*"; }

[ -x "$HCLOUD" ] || die "hcloud CLI not found at $HCLOUD"
[ -f "$SSH_KEY_FILE" ] || die "ssh private key missing: $SSH_KEY_FILE"

load_token() {
  local f="$HOME/.config/hetzner/credentials"
  [ -f "$f" ] || die "missing $f (Hetzner API token). Never print or commit it."
  HCLOUD_TOKEN="$(grep -E '^api_token=' "$f" | head -1 | cut -d= -f2- | tr -d ' \r')"
  [ -n "$HCLOUD_TOKEN" ] || die "no api_token= line in $f"
  export HCLOUD_TOKEN
}

SSH_OPTS=(-i "$SSH_KEY_FILE" -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new
          -o UserKnownHostsFile="$KNOWN_HOSTS" -o ConnectTimeout=10 -o ServerAliveInterval=30)

# Sets BOX_IP or dies. Needs load_token first.
require_box_ip() {
  BOX_IP="$("$HCLOUD" server ip "$BOX_NAME" 2>/dev/null || true)"
  [ -n "$BOX_IP" ] || die "box '$BOX_NAME' not found — run provision.sh first"
}

box_ssh()   { ssh "${SSH_OPTS[@]}" "root@$BOX_IP" "$@"; }
box_rsync() { rsync -e "ssh ${SSH_OPTS[*]}" "$@"; }
