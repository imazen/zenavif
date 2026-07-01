# rd_gap — zenrav1e vs libaom-slow RD-gap harness

Measures the SSIMULACRA2 rate-distortion gap between our AVIF encoder (zenrav1e, via
`cavif`) and the reference (libaom `aomenc` at slow), so every zenrav1e change can be
scored against the current baseline. Background + full plan: [`../../docs/RD_GAP_VS_LIBAOM.md`](../../docs/RD_GAP_VS_LIBAOM.md).

**Current gap (2026-06-30 baseline):** our AVIF is ~**10–18% larger on photos** at matched
ssim2 (BD-rate +25%, 28/28 images) — an RD-completeness gap, not a speed handicap (zenrav1e
s1==s2 is already its best; no s0). Baseline: [`../../benchmarks/rd_gap_baseline_2026-06-30.tsv`](../../benchmarks/rd_gap_baseline_2026-06-30.tsv).

## How it works

Per corpus image, both encoders sweep a rate grid; each encode is decoded and scored
decoded-vs-source with the **same** `fast-ssim2-cli` that produced the canonical `score_ssim2`:

- **zenrav1e** (`zenrav1e_cell.sh`): `cavif -s2 -Q<grid>` → decode with zenavif's own
  `save_png` example (dogfoods the full zenavif encode+decode roundtrip) → score.
- **libaom** (`aom_cell.sh`): `color.py` → color-exact y4m (BT.601-full, matching zenravif's
  math — a naive ffmpeg path capped ssim2 ~66) → `aomenc --cpu-used=2` → `aomdec` → `color.py`
  → score. Formats {420, 444, rgb-identity}, frontier over all.

`run_gap.sh` drives the sweep (bounded job pool) into one unified TSV; `analyze.py` builds the
per-image RD frontiers and reports the paired bpp gap at ssim2 {82,85,88,90}, split photos vs
plots, plus integrated BD-rate.

## Prerequisites (external binaries, not vendored)

```bash
# zenrav1e encoder CLI
( cd ~/work/zen/ravif && cargo build --release )                 # -> target/release/cavif
# zenavif decoder (this repo)
cargo build --release --example save_png                         # -> target/release/examples/save_png
# fast-ssim2 scorer
( cd ~/work/zen/fast-ssim2 && cargo build --release )            # -> target/release/fast-ssim2-cli
# libaom reference (OPTIONAL — unset AOMENC to sweep zenrav1e only). Pin the rev from the doc.
git clone https://aomedia.googlesource.com/aom && cd aom && git checkout 632172a4 \
  && cmake -B build_slow -DCMAKE_BUILD_TYPE=Release && cmake --build build_slow -j   # -> aomenc, aomdec
```

Python: `numpy`, `Pillow` (for `color.py`), `matplotlib` optional.

## Run

```bash
cd scripts/rd_gap
./make_sample.sh                      # -> sample_images.tsv (from clean-picker-corpus; CORPUS=... to override)

export CAVIF=~/work/zen/ravif/target/release/cavif \
       SAVE_PNG="$(git rev-parse --show-toplevel)/target/release/examples/save_png" \
       SCORER=~/work/zen/fast-ssim2/target/release/fast-ssim2-cli \
       AOMENC=~/work/aom/build_slow/aomenc AOMDEC=~/work/aom/build_slow/aomdec

# ALWAYS under the resource guard on the shared box:
~/work/zen/scripts/run-heavy -- bash run_gap.sh
python3 analyze.py rd_gap_results.tsv
```

## Regression tracking

After any zenrav1e RD change, rebuild `cavif` + re-run. The photo-gap column in `analyze.py`
is the number to drive down (today ~10–18% → target < 5%). To iterate on zenrav1e alone without
libaom, `unset AOMENC` — the harness then emits only the zenrav1e RD frontier; diff its median
bpp per ssim2 against the previous run.

## Known v1 limitations (honest)

- **cavif format parity:** the zenrav1e side sweeps `-Q` at cavif's default subsampling; the
  libaom side takes the frontier over {420,444,rgb}. If cavif's default under-serves a content
  class, that widens the apparent gap. Add a cavif format axis when the CLI exposes it, for a
  fully symmetric frontier.
- **8-bit only (v1):** cavif is forced to `--depth 8` because zenavif's `save_png` decodes only
  RGB8/RGBA8 (a 10-bit AVIF decodes as `Rgb16`, which `save_png` rejects → no PNG → `NA` score).
  8-bit matches the libaom side (fair, symmetric) but drops zenrav1e's 10-bit RD option, which the
  2026-06-30 measurement found modestly *helps* zenrav1e — so this harness is mildly conservative
  toward our encoder. Teach `save_png` to emit `Rgb16`→8-bit (or score 16-bit) to restore 10-bit.
- **Decoder asymmetry:** zenrav1e is decoded by zenavif, libaom by `aomdec`+`color.py`. Both are
  scored decoded-vs-source; the color-exact converter keeps the libaom path faithful, but this is
  a full-pipeline comparison (encode+decode), not encode-only. That's the right thing for "what a
  user gets," but keep it in mind when attributing a change to the encoder vs the decoder.
- Sample is ~1 MP (corpus max). AVIF's relative strength is size-dependent; larger images would
  shift absolute numbers (not the sign).
