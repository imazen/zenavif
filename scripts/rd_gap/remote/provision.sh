#!/usr/bin/env bash
# Provision the zenavif rd_gap sweep box (idempotent: re-running finds the
# existing box and just re-verifies deps). One box only — multi-box work goes
# through zenfleet (zenmetrics/scripts/jobsys), not copies of this script.
#
#   ./provision.sh            # create zenavif-sweep-1 (ccx63) if absent, install deps
#
# COST: ccx63 is ~EUR 1.61/h gross (fsn1/nbg1/hel1). Tear down when done:
#   ./teardown.sh --yes
source "$(dirname "$0")/common.sh"
load_token

if "$HCLOUD" server describe "$BOX_NAME" >/dev/null 2>&1; then
  note "box '$BOX_NAME' already exists — reusing it"
else
  created=""
  for loc in $BOX_LOCATIONS; do
    note "creating $BOX_NAME ($BOX_TYPE, $BOX_IMAGE) in $loc ..."
    if "$HCLOUD" server create --name "$BOX_NAME" --type "$BOX_TYPE" --image "$BOX_IMAGE" \
         --location "$loc" --ssh-key "$SSH_KEY_NAME" --label purpose=rd-gap-sweeps; then
      created="$loc"; break
    fi
    note "create failed in $loc (capacity?) — trying next location"
  done
  [ -n "$created" ] || die "could not create a $BOX_TYPE in any of: $BOX_LOCATIONS (try BOX_TYPE=ccx53 ./provision.sh)"
  rm -f "$KNOWN_HOSTS"   # fresh box => fresh host key
fi

require_box_ip
note "box IP: $BOX_IP — waiting for SSH ..."
up=""
for i in $(seq 1 60); do
  if box_ssh true 2>/dev/null; then up=1; break; fi
  sleep 5
done
[ -n "$up" ] || die "SSH to root@$BOX_IP not up after 5 min"

note "installing deps (idempotent) ..."
box_ssh 'bash -s' <<'REMOTE'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq build-essential cmake ninja-build nasm yasm pkg-config git curl rsync \
  perl python3 python3-numpy python3-pil > /dev/null
if ! [ -x "$HOME/.cargo/bin/cargo" ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
fi
"$HOME/.cargo/bin/rustc" -V
cmake --version | head -1
nasm -v
python3 -c 'import numpy, PIL; print("numpy", numpy.__version__, "pillow", PIL.__version__)'
mkdir -p /home/lilith/work/zen /home/lilith/sweep_out /home/lilith/sweep_in /mnt/v/output
echo "PROVISION OK"
REMOTE

echo
note "PROVISIONED: $BOX_NAME @ $BOX_IP ($BOX_TYPE, ~EUR 1.61/h gross)"
note "next: ./sync.sh && ./build_remote.sh    |    check: ./status.sh    |    when done: ./teardown.sh --yes"
