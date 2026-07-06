#!/usr/bin/env bash
# Cross-scale transfer test for the thumbnail-probe monotonicity idea.
# For each origin: encode s4 & s6 at FULL res and at a 384px THUMBNAIL, score ssim2,
# and check whether "does s4 give better RD than s6" is the SAME at both scales.
# If the thumbnail winner matches the full winner, a ~1.1x thumbnail probe is valid.
set -u
CAVIF=/tmp/statusmeasure/cavif-armed
SAVE_PNG=/home/lilith/work/zen/zenavif/target/release/examples/save_png
SCORER=/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli
D=/mnt/v/output/rd-gap-train26-2026-07-02
Q=80
T=$(mktemp -d /tmp/statusmeasure/xs.XXXXXX)

# rd_winner <ref.png> <label>  -> encodes s4,s6; echoes "s4|s6|tie bytes4 ss4 bytes6 ss6 ms4 ms6"
rd_pair() {
  local ref=$1
  local b4 s4 m4 b6 s6 m6
  for s in 4 6; do
    local t0 t1
    t0=$(date +%s.%N)
    "$CAVIF" -f -Q $Q -s $s --depth 8 -o "$T/e$s.avif" "$ref" >/dev/null 2>&1
    t1=$(date +%s.%N)
    "$SAVE_PNG" "$T/e$s.avif" "$T/d$s.png" >/dev/null 2>&1
    local by ss ms
    by=$(stat -c%s "$T/e$s.avif")
    ss=$("$SCORER" image "$ref" "$T/d$s.png" 2>/dev/null | grep -oE '[0-9.]+'|head -1)
    ms=$(echo "($t1-$t0)*1000"|bc -l)
    if [ "$s" = 4 ]; then b4=$by; s4=$ss; m4=$ms; else b6=$by; s6=$ss; m6=$ms; fi
  done
  # verdict: does s4 dominate s6 (fewer-or-equal bytes AND >= ssim2, meaningfully)?
  local verdict
  verdict=$(python3 -c "
b4,ss4,b6,ss6=$b4,$s4,$b6,$s6
smaller = b4 <= b6*0.99
better  = ss4 >= ss6+0.2
notworse= b4 <= b6 and ss4 >= ss6
if notworse and (smaller or better): print('s4')
elif (b6 <= b4*0.99 or ss6 >= ss4+0.2) and (b6<=b4 and ss6>=ss4): print('s6')
else: print('tie')
")
  printf "%s %d %.2f %d %.2f %.0f %.0f" "$verdict" "$b4" "$s4" "$b6" "$s6" "$m4" "$m6"
}

printf "%-8s %-6s | %-28s | %-28s | %s\n" origin class "FULL (s4B/ss vs s6B/ss ->win)" "THUMB384 (s4B/ss vs s6B/ss ->win)" "transfer?"
for spec in "7028 hurts" "7050 hurts" "7058 hurts" "8414 hurts" "6096 helps" "8302 helps" "8268 helps" "6018 helps"; do
  set -- $spec; o=$1; cls=$2
  img=$(ls $D/${o}_*.png 2>/dev/null | head -1)
  [ -z "$img" ] && { printf "%-8s %-6s | (no png)\n" "$o" "$cls"; continue; }
  # full
  read fv fb4 fs4 fb6 fs6 fm4 fm6 <<<"$(rd_pair "$img")"
  # thumbnail: longest edge 384
  convert "$img" -resize 384x384 "$T/thumb.png" 2>/dev/null
  read tv tb4 ts4 tb6 ts6 tm4 tm6 <<<"$(rd_pair "$T/thumb.png")"
  match=$([ "$fv" = "$tv" ] && echo "YES" || echo "no ($fv vs $tv)")
  printf "%-8s %-6s | %6dB/%.2f v %6dB/%.2f ->%-3s | %5dB/%.2f v %5dB/%.2f ->%-3s | %s\n" \
    "$o" "$cls" "$fb4" "$fs4" "$fb6" "$fs6" "$fv" "$tb4" "$ts4" "$tb6" "$ts6" "$tv" "$match"
done
echo "# thumbnail encode times (ms) shown as tm; full as fm — probe cost ~ (tm4+tm6)/fm6"
rm -rf "$T"
