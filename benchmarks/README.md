# zenavif benchmarks

Committed benchmark data and how to reproduce it. Each dataset is a self-contained
TSV; most carry a `.meta` sidecar with the git commit, host, full grid, column
definitions, and a copy-paste run command. The numbers quoted in the top-level
[`README.md`](../README.md) are drawn from these files.

## Integrity rules

These benches follow the repo-wide benchmarking discipline:

- **No `-C target-cpu=native`.** Every repro command is a plain
  `cargo build --release` — runtime SIMD dispatch (archmage) is what ships, so
  that is what we measure.
- **One process per cell**, run serially, so peak-memory high-water marks don't
  bleed between cells.
- **Memory is measured, never extrapolated** — VmHWM (peak RSS) deltas and, where
  noted, heaptrack peak-heap, measured at each size rather than scaled from one.
- **I/O is outside the timed region** — corpus bytes are loaded before the
  measured call (e.g. `benches/decode_benchmark.rs` reads the file, then times
  `decode_with` on the in-memory bytes).
- **Same inputs across the contenders within a sweep** — same images, dimensions,
  pixel format, and quality target; stated per dataset.
- Large rotating data (`*.parquet`, the multi-MB feature/pareto TSVs) is
  gitignored and archived to block storage (see [`../.gitignore`](../.gitignore));
  only small, decision-supporting summaries live here.

The exact timed region lives in each harness source (see *Harnesses*); the `.meta`
`measure:` field documents what every column captured.

## Environment

The current sweeps were run on:

- **CPU** AMD Ryzen 9 7950X (16C / 32T)
- **RAM** ~59 GiB available (128 GiB box)
- **OS** Linux 6.18 (WSL2, `microsoft-standard-WSL2`)
- Build profile `release`, no `target-cpu=native`.

The precise commit for each run is recorded in its `.meta`.

## Datasets

| File | Measures | Grid | Commit |
|------|----------|------|--------|
| `avif_encode_fine_sweep_2026-04-16.tsv` | encode time / size / zensim vs speed | 512² photo (CID22), q80, 8-bit, speed 1–10 | — |
| `avif_resource_main_2026-06-14.tsv` | per-op peak-RSS delta + wall vs size | 256–2048², speed {4,6,8,10}, q75, 5 content classes | `ebf440a` |
| `vcpu_resource_sweep_2026-06-20.tsv` | peak mem / heap / wall vs **thread count** | 256/1024/2048², speed {6,10}, threads {1,2,4,8,16,28}, q75 8-bit | `7697b87` |
| `zenavif_encode_mem_2026-06-23.tsv` | encode peak-memory model calibration | 256–2048², photo/screenshot, speed {6,8,10}, q {50,85}, RGB8 | `097e86d0` |
| `lossless_speed_sweep_fixed_2026-06-11.tsv` | lossless size/speed monotonicity (path-patched fix) | lossless, speed sweep | — |
| `sweep_validate_2026-06-12.tsv` | sweep-axis liveness / fingerprint validation | encoder knob axes | — |

The thread-scaling and memory-calibration `.meta` files also record their findings
inline: AV1 tile parallelism scales near-linearly with image size, while encode
peak RSS is roughly thread-invariant (tiles are views into one shared frame, so
the allocator churns per-tile but the working set does not grow much with threads).

## Comparisons & baselines

- **Speed/quality and resource sweeps** compare zenavif's own encoder knobs
  (speed, quality, threads) against each other — there is no third-party codec in
  these files.
- **BD-rate** figures in the README ("vs upstream rav1e") use unmodified
  [rav1e](https://github.com/xiph/rav1e) as the baseline. zenavif encodes via
  [zenravif](https://github.com/imazen/zenrav1e) (our rav1e fork), so the
  comparison isolates the fork's quantization-matrix / still-image extras against
  the well-maintained encoder they build on.
- **Decode** behavior is cross-checked pixel-for-pixel against
  [libavif](https://github.com/AOMediaCodec/libavif), the AOMedia reference
  implementation (test vectors under `tests/vectors/`).

## Harnesses

Reproduce from a clean checkout:

```sh
git clone https://github.com/imazen/zenavif && cd zenavif
git checkout <commit-from-meta>      # pin the commit the numbers came from
```

| Harness | Build | Purpose |
|---------|-------|---------|
| `examples/avif_probe.rs` | `--features encode` (add `encode-threading` for the thread sweep) | per-op peak-RSS + wall probe (resource sweeps) |
| `examples/mem_probe_encode.rs` | `--features encode` | single-encode peak-memory probe |
| `examples/encode_sweep.rs` | `--features encode-imazen,encode-threading` | speed/quality encode sweep |
| `examples/lossless_speed_sweep.rs` | `--features encode-imazen` | lossless size/speed sweep |
| `examples/sweep_validate.rs` | `--features __expert` | encoder-axis liveness validation |
| `benches/decode_benchmark.rs` | default (zenbench) | decode throughput |

Heavy jobs run under `scripts/run-heavy` (cgroup memory cap + idle CPU/IO
priority) exactly as the `.meta` `reproducer:` blocks show. For the precise
command, environment tunables (e.g. `GLIBC_TUNABLES` to pin the WSL2 glibc arena),
and per-column definitions, read the dataset's `.meta` — it is the source of truth
for that run.
