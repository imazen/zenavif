# AV1 decode backend spike — HW vs SW (2026-05-23)

## Question

Should zenavif gain platform-native HW AV1 decode backends (VA-API,
D3D11VA, DXVA2, NVDEC/CUDA) alongside the existing rav1d-safe pure-Rust
decoder, and would they actually be faster for the still-image AVIF
workload imageflow cares about?

## Method

New backend dispatch trait modeled on `zen/heic/src/backend.rs`
(`src/backend.rs`) plus a single ffmpeg-shellout backend
(`src/backend_ffmpeg.rs`) that exercises each platform's hwaccel
through `ffmpeg -hwaccel <method>`. The bench harness
(`examples/bench_backends.rs`) decodes every AVIF in the link-u corpus
(150 files: 8/10/12-bit, 4:0:0 mono / 4:2:0 / 4:2:2 / 4:4:4, 64×64 to
3082×2048) and reports per-file median wall-time over N timed
iterations plus ffmpeg's internal `-benchmark` rtime (which excludes
subprocess-startup overhead).

ffmpeg gets fed raw AV1 OBU bitstream (the AVIF primary item payload)
with a temporal-delimiter OBU prepended — the rust path's container
parse via `zenavif-parse` runs identically inside the FfmpegBackend
before it spawns ffmpeg, so the comparison is decoder-only.

**Hardware:** AMD Ryzen 9 7950X (Raphael, 16C/32T), 128 GB RAM, NVIDIA
card available to CUDA. WSL2 Ubuntu 22.04 + Windows 11 PowerShell
dual-shell setup. iGPU not exposed to WSL (`/dev/dri` absent), so the
WSL VA-API column is empty by necessity.

**Software:**

- Linux (WSL2): ffmpeg 4.4.2 with libdav1d 0.9.2, rav1d-safe 0.5.4
- Windows 11: ffmpeg 7.1.1-full (gyan.dev winget build), rustc nightly
  1.98 / cargo 1.98

**Iters:** 7 timed runs after 2 warmup on Linux, 5 timed runs after
2 warmup on Windows. Per-file median reported. ffmpeg subprocess time
(cold start every iteration) included in `*_ms`; `*_bench_ms` is
ffmpeg's internal real time minus subprocess startup.

## Results

### Linux (WSL2, libdav1d 0.9.2 via ffmpeg 4.4)

Raw TSV: `benchmarks/backends_linux_2026-05-23.tsv` (150 rows).

Mean across all 150 vectors:

| Backend          | Wall mean | -benchmark mean | vs rust (internal) | Notes |
|------------------|-----------|-----------------|--------------------|-------|
| **rust**         | 60.6 ms   | (in-process)    | 1.00×              | rav1d-safe 0.5.4 |
| **ffmpeg-cpu**   | 93.7 ms   | **42.9 ms**     | **0.71×**          | libdav1d 0.9.2; subprocess overhead ≈ 51 ms |
| **vaapi**        | —         | —               | —                  | No `/dev/dri` in this WSL; `Device creation failed -542398533`. Code lands; needs real Linux host to measure. |
| **cuda** (NVDEC) | 334.7 ms  | 239.2 ms        | 5.6× *slower*      | Per-call context init + GPU→sysmem readback dominates |

By image-size bucket (mean internal time, ms):

| Bucket          | n   | rust   | ffmpeg-cpu (internal) | cuda (internal) |
|-----------------|----:|-------:|----------------------:|-----------------:|
| tiny (<256²)    | 32  | 4.8    | 15.2                  | 202.8            |
| small (<1 MP)   | 76  | 46.7   | 35.6                  | 242.9            |
| large (>1 MP)   | 42  | 128.1  | **77.4**              | 260.4            |

**Library-internal libdav1d beats rav1d-safe by ~25-40 % at every size
that takes long enough to measure cleanly.** At tiny (<256²) the
subprocess overhead even inside ffmpeg's `-benchmark` rtime dominates,
so rav1d-safe wins trivially.

### Windows 11 (ffmpeg 7.1.1, native d3d11va / dxva2 / cuda)

Raw TSV: `benchmarks/backends_windows_v2_2026-05-23.tsv` (150 rows;
HW backends only populated for the AV1 profiles they accept).

Mean across vectors a backend actually handled:

| Backend          |   n | Wall mean | -benchmark mean | vs rust (internal) | Notes |
|------------------|----:|----------:|----------------:|--------------------|-------|
| **rust**         | 150 | 55.7 ms   | (in-process)    | 1.00×              | rav1d-safe 0.5.4 |
| **ffmpeg-cpu**   | 150 | 57.5 ms   | **22.2 ms**     | **0.40×**          | libdav1d via ffmpeg 7.1; subprocess ≈ 35 ms |
| **d3d11va**      |   8 | 239.9 ms  | 46.0 ms         | 0.83×              | Rejects 4:0:0 mono, 4:2:2, 4:4:4, 12-bit |
| **dxva2**        |   8 | 387.8 ms  | 40.2 ms         | 0.72×              | Same profile gaps as d3d11va |
| **cuda** (NVDEC) |   5 | 175.8 ms  | 39.6 ms         | 0.71×              | Limited sample after consecutive-failure disable |

The small HW sample sizes are real: D3D11VA / DXVA2 / NVDEC all reject
monochrome and 4:4:4/4:2:2 AV1, and the alphabetic file ordering hits
30+ rejected vectors in a row early on, tripping the harness's
auto-disable. The library-internal numbers we *did* get show HW within
10-20 % of rav1d-safe internally — barely faster — and 4-7 × slower
wall once setup + readback is paid.

## Findings

1. **`libdav1d` (C) via ffmpeg's internal time consistently beats
   `rav1d-safe` (Rust)** by 25-60 % at every image size large enough to
   measure cleanly. The performance ceiling for in-process AV1 decode
   on this hardware is libdav1d, not platform HW. If raw speed is the
   goal, upgrading or wrapping libdav1d delivers more than any HW path.

2. **HW decoders are net-negative for still AVIF on this hardware.**
   Even subtracting ffmpeg-subprocess overhead, platform HW internal
   time loses to ffmpeg-cpu internal and only marginally beats
   rav1d-safe. The GPU per-frame decode itself is fine; per-call
   context init + GPU→system-memory readback for a single still swamps
   the savings. Video streaming amortizes those costs across hundreds
   of frames; AVIF stills don't.

3. **Mono (4:0:0) AV1 + 4:4:4 + 4:2:2 + 12-bit are widely unsupported
   by HW decoders.** D3D11VA / DXVA2 / NVDEC reject all four. A native
   HW backend would need a CPU fallback for every profile outside Main
   8/10-bit 4:2:0, doubling the maintenance surface and removing the
   "always faster" hand-wave.

4. **VA-API on WSL2 isn't usable from this dev machine** (no
   `/dev/dri/renderD*` exposed by the default WSL kernel/userspace).
   The native-Linux path is writable but unmeasurable here. A re-run on
   bare-metal Linux with a working VA-API driver would close that gap,
   but the priority is low given the Windows results.

5. **The ffmpeg-shellout `FfmpegBackend` is genuinely useful as a
   long-term fallback / comparison harness** even if we don't ship
   native HW backends. It works on every platform with ffmpeg
   installed, validates correctness against rav1d-safe, and keeps the
   `Av1DecoderBackend` trait surface honest for future native FFI
   work.

## Decision

**Don't ship native libva / Media Foundation / D3D11VA backends for
zenavif right now.** The data says HW decode is the wrong knob for
still AVIF perf on commodity x86-64 hardware. Higher-impact moves:

- Upgrade or replace `rav1d-safe` with something that matches
  libdav1d's speed (or wrap libdav1d directly behind a feature flag).
  Internal time delta of 25-60 % on every >256² image is real money.
- Profile YUV→RGB in zenavif — that's a non-trivial fraction of total
  decode time and a clean in-process Rust optimization target.
- For mobile (Android MediaCodec, iOS VideoToolbox) HW *might* still
  win on power efficiency even at wall-time parity. Revisit on actual
  mobile hardware before committing engineering time.

## What this spike leaves behind

- `src/backend.rs` — `DecodeBackend` enum + `Av1DecoderBackend` trait
  + allowlist dispatcher, modeled on `zen/heic/src/backend.rs`. Lands
  cleanly regardless of the HW decision.
- `src/backend_ffmpeg.rs` — `FfmpegBackend` (single struct,
  configurable hwaccel string). Useful as a permanent comparison /
  fallback backend.
- `examples/bench_backends.rs` — TSV bench harness, will keep working
  for future rav1d-safe vs libdav1d sweeps.
- `benchmarks/backends_linux_2026-05-23.tsv`,
  `benchmarks/backends_windows_v2_2026-05-23.tsv` — raw measurements.

## How to reproduce

```bash
# Linux
cargo run --release --features backend-ffmpeg --example bench_backends -- \
  --vectors tests/vectors/link-u --iters 7 --warmup 2 \
  --backends rust,ffmpeg-cpu,vaapi,cuda \
  --out benchmarks/backends_linux_$(date +%Y-%m-%d).tsv

# Windows (from WSL via pwsh.exe)
pwsh.exe -NoProfile -Command '
  cd \\wsl.localhost\Ubuntu-22.04\home\lilith\work\zen\zenavif--av1-backends-spike
  $env:CARGO_TARGET_DIR = "C:\target\zenavif-spike"
  cargo build --release --features backend-ffmpeg --example bench_backends
  & "C:\target\zenavif-spike\release\examples\bench_backends.exe" `
    --vectors \\wsl.localhost\Ubuntu-22.04\home\lilith\work\zen\zenavif\tests\vectors\link-u `
    --iters 5 --warmup 2 `
    --backends rust,ffmpeg-cpu,d3d11va,dxva2,cuda `
    --out benchmarks\backends_windows_$(Get-Date -Format yyyy-MM-dd).tsv
'
```
