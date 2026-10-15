#!/usr/bin/env bash
# Differential check of zenmp4 against FFmpeg: FFmpeg writes MP4s (moov at
# the end, faststart, fragmented, B-frames with ctts; AV1+Opus, H.264+AAC,
# VP9, MPEG-4 Part 2 with B-frames + AAC, FLAC) and zenmp4's packets must match ffprobe's per stream: size, key flag,
# and pts/dts once zenmp4's carried edit delay is subtracted (FFmpeg applies
# edit lists to timestamps; zenmp4 reports them on the track).
# Needs ffmpeg/ffprobe (libaom, libopus, libvpx; libx264 optional) and cargo.
# Usage: zenmp4/scripts/ffmpeg-oracle.sh [work-dir]
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
work=${1:-$here/../target/mp4-ffmpeg-oracle}
mkdir -p "$work"
cd "$work"
ff() { ffmpeg -v error -y "$@"; }
src_av=( -f lavfi -i testsrc2=size=96x64:rate=25 -f lavfi -i sine=frequency=440:sample_rate=48000 -t 1.6 )
ff "${src_av[@]}" -c:v libaom-av1 -cpu-used 8 -g 10 -c:a libopus av1_opus.mp4
ff "${src_av[@]}" -c:v libaom-av1 -cpu-used 8 -g 10 -c:a libopus -movflags +faststart av1_opus_faststart.mp4
ff "${src_av[@]}" -c:v libaom-av1 -cpu-used 8 -g 10 -c:a libopus \
  -movflags frag_keyframe+empty_moov+default_base_moof av1_opus_frag.mp4
ff -f lavfi -i testsrc2=size=64x48:rate=30 -t 1 -c:v libvpx-vp9 -g 10 vp9.mp4
# B-frames (ctts / trun composition offsets) and AAC priming (edit list),
# with FFmpeg's own encoders.
ff -f lavfi -i testsrc2=size=64x48:rate=24 -f lavfi -i sine=sample_rate=44100 -t 1.2 \
  -c:v mpeg4 -bf 2 -g 12 -c:a aac mpeg4_aac.mp4
ff -f lavfi -i testsrc2=size=64x48:rate=24 -f lavfi -i sine=sample_rate=44100 -t 1.2 \
  -c:v mpeg4 -bf 2 -g 12 -c:a aac -movflags frag_keyframe+empty_moov+default_base_moof mpeg4_aac_frag.mp4
ff -f lavfi -i sine=sample_rate=48000 -t 0.5 -c:a flac flac.mp4
if ffmpeg -hide_banner -encoders 2>/dev/null | grep -q libx264; then
  ff -f lavfi -i testsrc2=size=64x48:rate=24 -f lavfi -i sine=sample_rate=44100 -t 1.2 \
    -c:v libx264 -bf 2 -g 12 -c:a aac h264_aac.mp4
  ff -f lavfi -i testsrc2=size=64x48:rate=24 -t 1 -c:v libx264 -bf 2 -g 12 \
    -movflags frag_keyframe+empty_moov h264_frag.mp4
fi
(cd "$here" && cargo build -q --release --example mp4_dump)
dump=$(cd "$here" && cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/release/examples/mp4_dump
fail=0
for f in *.mp4; do
  "$dump" "$f" > "$f.zen.txt" 2> "$f.zen.err" || { echo "FAIL $f: zenmp4 refused: $(cat "$f.zen.err")"; fail=1; continue; }
  ffprobe -v error -show_entries packet=stream_index,pts,dts,size,flags -of csv=p=0 "$f" > "$f.ffprobe.csv"
  if python3 - "$f" <<'PY'
import sys
f = sys.argv[1]
edit = {}
ours = {}
for line in open(f + ".zen.txt"):
    if line.startswith("track "):
        parts = line.split()
        idx = int(parts[1])
        e = [p for p in parts if p.startswith("edit=")][0][5:]
        edit[idx] = int(e[5:-1]) if e.startswith("Some(") else 0
        continue
    t, pts, dts, size, key = line.strip().split(",")
    ours.setdefault(int(t), []).append((int(dts) - edit[int(t)], int(pts) - edit[int(t)], int(size), int(key)))
theirs = {}
for line in open(f + ".ffprobe.csv"):
    s, pts, dts, size, flags = line.strip().split(",")[:5]
    theirs.setdefault(int(s), []).append((int(dts), int(pts), int(size), int("K" in flags)))
ok = True
for s in sorted(set(ours) | set(theirs)):
    a = sorted(ours.get(s, [])); b = sorted(theirs.get(s, []))
    if a != b:
        ok = False
        print(f"  stream {s}: zenmp4 {len(a)} packets, ffprobe {len(b)}")
        for x, y in zip(a, b):
            if x != y:
                print(f"    first difference: zenmp4 {x} ffprobe {y}")
                break
sys.exit(0 if ok else 1)
PY
  then echo "ok   $f: $(grep -vc '^track' "$f.zen.txt") packets match ffprobe"; else echo "FAIL $f"; fail=1; fi
done
exit $fail
