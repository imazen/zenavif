#!/usr/bin/env bash
# G6 (GOAL_PARETO): integration-honest reproduction from PUSHED revisions only.
# Fresh-clones the armed chain into a scratch dir, builds, encodes one leg, and
# byte-compares against the dev-tree binary's output. No machine-local paths
# beyond the documented layout (the chain expects ../ravif--cooptloop and
# ../zenrav1e siblings, recreated here from their public remotes).
#
# PINS (update on re-pin):
ZENAVIF_REV=86dfbd56cc183e17869e35408298ff158a5b35d9
RAVIF_REV=8ccba4a41c83162289bec717d2583eda1db81309          # imazen/cavif-rs branch cooptloop
ZENRAV1E_REV=8552e2f0629b66dfe3e2a3506c0da401b3deea23       # imazen/zenrav1e master
set -eu
WORK="${WORK:-$(mktemp -d /tmp/g6repro.XXXX)}"
echo "[g6] scratch: $WORK"
cd "$WORK"
git clone -q https://github.com/imazen/zenavif zen-zenavif && git -C zen-zenavif checkout -q "$ZENAVIF_REV"
git clone -q https://github.com/imazen/cavif-rs ravif--cooptloop && git -C ravif--cooptloop checkout -q "$RAVIF_REV"
git clone -q https://github.com/imazen/zenrav1e zenrav1e && git -C zenrav1e checkout -q "$ZENRAV1E_REV"
# the chain's relative path deps: zenavif -> ../ravif--cooptloop/ravif -> ../../zenrav1e
mv zen-zenavif zenavif
cd "$WORK/ravif--cooptloop" && nice -n 19 cargo build --release -j "${JOBS:-8}" 2>&1 | tail -1
CAVIF="$WORK/ravif--cooptloop/target/release/cavif"
IMG="${IMG:-/mnt/v/output/rd-gap-train26-2026-07-02/1236_interiors_ornate-arched-interior_casa-vicens-barcelona_zfold7_iso400-f2p2_20260319-165702_4000x3000.sdr.s1024.png}"
REF_CAVIF="${REF_CAVIF:-/home/lilith/work/zen/ravif--cooptloop/target/release/cavif}"
mkdir -p "$WORK/out"
ok=1
for q in 30 60 85; do
  "$CAVIF" -f -Q $q -s 6 --yuv 420 -o "$WORK/out/repro_q$q.avif" "$IMG" >/dev/null 2>&1
  "$REF_CAVIF" -f -Q $q -s 6 --yuv 420 -o "$WORK/out/ref_q$q.avif" "$IMG" >/dev/null 2>&1
  if cmp -s "$WORK/out/repro_q$q.avif" "$WORK/out/ref_q$q.avif"; then
    echo "[g6] q$q: BYTE-IDENTICAL ($(stat -c%s "$WORK/out/repro_q$q.avif") B)"
  else
    echo "[g6] q$q: MISMATCH ($(stat -c%s "$WORK/out/repro_q$q.avif") vs $(stat -c%s "$WORK/out/ref_q$q.avif") B)"; ok=0
  fi
done
[ $ok -eq 1 ] && echo "[g6] REPRODUCTION PASS" || { echo "[g6] REPRODUCTION FAIL"; exit 1; }
