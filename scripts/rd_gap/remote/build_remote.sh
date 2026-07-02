#!/usr/bin/env bash
# Build the full rd_gap toolchain on the box, using all cores (dedicated box —
# no nice needed). Idempotent: libaom is only rebuilt when the synced rev
# changes; cargo rebuilds are incremental. Verifies every binary at the end
# and fails loudly if any is missing.
#
# Builds:
#   1. libaom aomenc/aomdec   cmake -DCMAKE_BUILD_TYPE=Release, same codec config
#                             as the local build_slow (defaults); docs/tests are
#                             trimmed — build-scope only, no effect on the codec.
#   2. cavif                  ~/work/zen/ravif (picks up the [patch.crates-io]
#                             zenrav1e--phase2v2 working tree, i.e. your WIP)
#   3. zenavif examples       save_png extract_av1 decode_avif (default features)
#   4. fast-ssim2-cli         ~/work/zen/fast-ssim2
source "$(dirname "$0")/common.sh"
load_token
require_box_ip

note "building on $BOX_NAME ($BOX_IP) ..."
box_ssh 'bash -s' <<'REMOTE'
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
NPROC=$(nproc)
t0=$(date +%s)
phase() { echo; echo "=== [$(date -u +%H:%M:%SZ)] $* ==="; }

phase "libaom (rev-stamped, skip when unchanged)"
cd /home/lilith/work/aom
want=$(cat .synced_rev)
have=$(cat build_slow/.built_rev 2>/dev/null || echo none)
if [ -x build_slow/aomenc ] && [ -x build_slow/aomdec ] && [ "$have" = "$want" ]; then
  echo "aom already built at ${want:0:12} — skip"
else
  rm -rf build_slow
  cmake -B build_slow -DCMAKE_BUILD_TYPE=Release -DENABLE_DOCS=0 -DENABLE_TESTS=0 -DENABLE_TESTDATA=0 \
    > /tmp/aom_cmake.log 2>&1 || { tail -40 /tmp/aom_cmake.log; exit 1; }
  cmake --build build_slow -j "$NPROC" > /tmp/aom_build.log 2>&1 || { tail -40 /tmp/aom_build.log; exit 1; }
  echo "$want" > build_slow/.built_rev
  echo "aom built at ${want:0:12}"
fi

# Full cargo output goes to a log; on failure print the last 60 lines (never
# truncate errors), on success just the tail.
cbuild() {  # cbuild <log-name> <dir> <cargo args...>
  local log="/tmp/$1.log" dir="$2"; shift 2
  cd "$dir"
  if ! cargo "$@" > "$log" 2>&1; then
    echo "CARGO FAILED in $dir: cargo $*"; tail -60 "$log"; exit 1
  fi
  tail -2 "$log"
}

phase "cavif (ravif; zenrav1e picked by ravif's [patch.crates-io])"
grep -A3 '^\[patch.crates-io\]' /home/lilith/work/zen/ravif/Cargo.toml | grep -m1 zenrav1e || echo "(no patch — registry zenrav1e)"
cbuild cavif_build /home/lilith/work/zen/ravif build --release

phase "fast-ssim2-cli"
# The CLI lives in the ssimulacra2_bin/ member (package name fast-ssim2-cli);
# the plain workspace-root build is the documented way to get it.
cbuild fastssim2_build /home/lilith/work/zen/fast-ssim2 build --release

phase "zenavif examples (save_png extract_av1 decode_avif)"
# Primary: build from the synced tree. Fallback (LOUD): the decoder binaries
# sync.sh shipped from the workstation — the same decoders the local harness
# runs — for when a sibling-repo contract change mid-flight breaks the WIP
# build. No fallback available => hard fail.
EXDIR=/home/lilith/work/zen/zenavif/target/release/examples
FBDIR=/home/lilith/decoder_fallback
cd /home/lilith/work/zen/zenavif
if cargo build --release --example save_png --example extract_av1 --example decode_avif \
     > /tmp/zenavif_build.log 2>&1; then
  tail -2 /tmp/zenavif_build.log
  rm -f "$EXDIR/.synced_from_workstation"
  echo "zenavif decoders: BUILT FROM SOURCE on the box"
elif [ -x "$FBDIR/save_png" ] && [ -x "$FBDIR/extract_av1" ] && [ -x "$FBDIR/decode_avif" ]; then
  echo '!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!'
  echo '!! zenavif examples DO NOT BUILD from the current synced tree. Tail:'
  tail -12 /tmp/zenavif_build.log | sed 's/^/!!   /'
  echo '!! FALLING BACK to decoder binaries synced from the workstation'
  echo '!! (identical to what the local harness runs):'
  sed 's/^/!!   /' "$FBDIR/decoder_fallback_manifest.txt" 2>/dev/null || true
  echo '!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!'
  mkdir -p "$EXDIR"
  cp -f "$FBDIR/save_png" "$FBDIR/extract_av1" "$FBDIR/decode_avif" "$EXDIR/"
  cp -f "$FBDIR/decoder_fallback_manifest.txt" "$EXDIR/.synced_from_workstation"
else
  echo "CARGO FAILED building zenavif examples AND no synced fallback binaries exist:"
  tail -60 /tmp/zenavif_build.log
  exit 1
fi

phase "verify binaries"
fail=0
for b in \
  /home/lilith/work/aom/build_slow/aomenc \
  /home/lilith/work/aom/build_slow/aomdec \
  /home/lilith/work/zen/ravif/target/release/cavif \
  /home/lilith/work/zen/zenavif/target/release/examples/save_png \
  /home/lilith/work/zen/zenavif/target/release/examples/extract_av1 \
  /home/lilith/work/zen/zenavif/target/release/examples/decode_avif \
  /home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli
do
  if [ -x "$b" ]; then
    echo "OK  $(sha256sum "$b" | cut -c1-16)  $(stat -c '%8s' "$b")  $b"
  else
    echo "MISSING: $b"; fail=1
  fi
done
[ "$fail" = 0 ] || { echo "BUILD INCOMPLETE"; exit 1; }
if [ -f "$EXDIR/.synced_from_workstation" ]; then
  echo "NOTE: zenavif decoders are workstation-synced binaries (see warning above), NOT built on the box."
fi
echo "versions:"
/home/lilith/work/aom/build_slow/aomenc --help 2>&1 | grep -m1 -i 'AV1 Encoder' || true
/home/lilith/work/zen/ravif/target/release/cavif --version || true
/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli --version 2>/dev/null || true
echo
echo "BUILD OK ALL in $(( $(date +%s) - t0 ))s on $NPROC cores"
REMOTE
