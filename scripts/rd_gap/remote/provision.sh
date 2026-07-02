#!/usr/bin/env bash
# Provision the zenavif rd_gap sweep box (idempotent: re-running finds the
# existing box and just re-verifies deps). One box only — multi-box work goes
# through zenfleet (zenmetrics/scripts/jobsys), not copies of this script.
#
#   ./provision.sh            # create zenavif-sweep-1 (ccx63) if absent, install deps
#   FROM_SNAPSHOT=auto ./provision.sh   # restore from the newest zenavif-sweep snapshot
#   FROM_SNAPSHOT=<image-id> ./provision.sh
#
# COST: ccx63 is ~EUR 1.61/h gross (fsn1/nbg1/hel1). The box is EXPENSIVE to
# keep idle — when the day's sweeps are done, snapshot+delete instead of
# leaving it running:  ./teardown.sh --snapshot --yes
# (restore later with FROM_SNAPSHOT=auto; deps/toolchain/aom-build all come
# back with the disk, so provisioning from snapshot skips the apt/rustup step.)
source "$(dirname "$0")/common.sh"
load_token

from_snapshot=""
if "$HCLOUD" server describe "$BOX_NAME" >/dev/null 2>&1; then
  note "box '$BOX_NAME' already exists — reusing it"
else
  image="$BOX_IMAGE"
  if [ -n "${FROM_SNAPSHOT:-}" ]; then
    if [ "$FROM_SNAPSHOT" = auto ]; then
      image="$("$HCLOUD" image list --type snapshot -o noheader -o columns=id,description \
        | awk '/zenavif-sweep/ {print $1}' | tail -1)"
      [ -n "$image" ] || die "FROM_SNAPSHOT=auto: no snapshot with 'zenavif-sweep' in its description found"
    else
      image="$FROM_SNAPSHOT"
    fi
    from_snapshot=1
    note "restoring from snapshot image $image"
  fi
  created=""
  for loc in $BOX_LOCATIONS; do
    note "creating $BOX_NAME ($BOX_TYPE, image $image) in $loc ..."
    if "$HCLOUD" server create --name "$BOX_NAME" --type "$BOX_TYPE" --image "$image" \
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

if [ -n "$from_snapshot" ]; then
  note "snapshot restore: verifying baked deps instead of reinstalling ..."
  box_ssh '"$HOME/.cargo/bin/rustc" -V && cmake --version | head -1 && python3 -c "import numpy, PIL" && echo "SNAPSHOT DEPS OK"' \
    || die "snapshot box missing baked deps — provision from scratch instead (unset FROM_SNAPSHOT)"
  note "PROVISIONED (from snapshot): $BOX_NAME @ $BOX_IP"
  note "next: ./sync.sh && ./build_remote.sh   (sync is a delta — fast on a snapshot restore)"
  exit 0
fi

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
