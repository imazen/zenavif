# backend-probe

An exploratory still-image comparison of zenavif's three AV1 encoders at
their latest heads. It encodes 768×512 4:2:0 photos at 8, 10 and 12 bits,
at four quantizer points, on one thread. FFmpeg's libdav1d decodes every
output for PSNR.

The results and their limits are in
`benchmarks/backend_probe_2026-10-15.md`.

```sh
# Inputs: four CLIC 2025 final-test photos, cover-scaled to 768x512.
mkdir -p in && i=0
for f in $(ls ~/work/codec-corpus/clic2025/final-test/*.png | awk 'NR%7==1' | head -4); do
  i=$((i+1))
  for fmt in yuv420p yuv420p10le yuv420p12le; do
    ffmpeg -v error -y -i "$f" \
      -vf "scale=768:512:force_original_aspect_ratio=increase:flags=lanczos,crop=768:512" \
      -pix_fmt $fmt -f rawvideo in/img$i.$fmt.yuv
  done
done
cargo build --release
./target/release/av1probe > cells.csv
python3 psnr.py > results.csv
python3 summary.py
```
