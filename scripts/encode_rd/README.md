# encode_rd — matched-wall-clock encode RD harness

Compares four AV1 still-image encoders at **equal measured encode time**, not
at equal nominal speed setting.

That distinction is the whole point. Measured on the committed validation grid
(`benchmarks/encode_rd_validate_2026-08-08.*`), one photo at 1024 px, every arm
set to its own nominal **6**:

| arm | nominal "6" | |
|---|---|---|
| `svtc` | 26.9 ms | |
| `aom` | 75.3 ms | |
| `svtrs` | 204.4 ms | |
| `zenrav1e` | 405.5 ms | **15× spread** |

Any table that puts those four numbers in one row because the knob says 6 is not
a comparison. So this harness treats the speed knob as a free parameter,
eliminates it, and asks the question that has an answer:

> at the same measured encode time, which encoder needs fewer bytes to reach
> the same quality?

Two halves:

- **`run_grid.py`** — drives the encoders, times them, persists every encoded
  bitstream content-addressed, decodes them all with one decoder, scores them
  with `zenmetrics`. Emits one TSV row per cell.
- **`analyze_matched.py`** — eliminates the ladder and reports the matched-time
  comparison, plus the `t = α + β·pixels` fit and the reproducibility stats.

---

## 1. The arms, and exactly how each is driven

Every arm: **8-bit, 4:2:0, one key frame, still-picture coding, one thread, one
tile, constant-quantizer rate control**, every adaptive-QP / psychovisual layer
left at that encoder's own default.

| arm | binary | in → out | time ladder | rate knob | still-picture |
|---|---|---|---|---|---|
| `zenrav1e` | `zenrav1e/target/release/rav1e` | y4m → IVF | `-s 0..10` | `--quantizer 0..255` (qindex) | `--still-picture` |
| `aom` | `aomenc` (libaom) | y4m → IVF | `--cpu-used 0..9` | `--cq-level 0..63` | `--allintra --limit=1` |
| `svtc` | `SvtAv1EncApp` (C reference) | y4m → IVF | `--preset 0..13` | `--qp 1..63` | `--avif 1` |
| `svtrs` | `zenav1-svt` `examples/identity_run` | PNG → raw I420 → OBU | `preset 0..13` | `cli_qp 0..63` | always |

Threading is pinned to 1 and tiling to a single tile for all four. Tiles are not
free — they change the bitstream, and zenavif's own FASTWINS measurement put the
default tiling policy at +7.4 % ssim2 BD-rate — so folding thread count into the
time axis would silently trade RD for speed. Thread scaling is a separate sweep.

### The rate scales are NOT aligned, deliberately

`--quantizer 40` (qindex, of 255) and `--qp 40` (of 63) are different quality
regimes: on the validation cell they land at ssim2 87.2 and 46.5 respectively.
Each arm therefore sweeps **its own** quantizer grid, and the analysis
interpolates on **achieved quality**, exactly as BD-rate does. Hand-aligning the
knobs would be a fudge; what each arm must actually do is *span* the quality
range of interest, and `analyze_matched.py` section B prints each arm's achieved
span and refuses to interpolate outside it.

The default grids are 20 points spaced for **even coverage in achieved
quality**, which is not the same as even spacing of the knob — and getting this
wrong is not cosmetic. The first version of these grids *was* uniform in the
quantizer, and measured on the probe it put **half its points below ssim2 10**,
where ssim2 is not even monotone in rate any more (cq 58 → 8.49, cq 62 → 9.88),
while leaving the useful 30–90 band with three samples. RD curves built on that
are noise.

So the knob→quality curve was measured and inverted. It is piecewise-linear with
a knee: on the 63-scale, ~0.91 ssim2 per unit up to qp 31, then ~1.93 per unit
beyond. The grids target ssim2 in ~7-point steps from ~92 down to ~15. Note the
consequence: **even quality spacing comes out denser at the high end of the
knob** — the opposite of a naive uniform grid — which is what "low-q density
equal to high-q density" actually requires for web-focused work.

This is a coverage heuristic fit on one photo, not a calibration; other content
shifts the mapping. It does not need to be exact, because the analysis
interpolates on achieved quality regardless. Section B prints the achieved span
per arm so a bad fit is visible rather than silent.

### What could NOT be driven to a comparable config — stated plainly

- **`svtrs` reads a PNG, not the y4m.** Its driver (`identity_run`) does its own
  RGB→I420. That would be a fatal asymmetry, except the two transforms are
  byte-identical — `run_grid.py --verify-yuv` sha256s `identity_run`'s `.yuv`
  against `rd_tool prep`'s and **aborts the run** if they ever diverge. Verified
  equal on the validation grid. What remains is a *timing* asymmetry, not an
  input one: `svtrs` pays a PNG decode inside its timed region that the y4m arms
  do not. See §3.
- **`zenav1-svt` has the right timing discipline in the wrong tool.** Its
  `examples/perf_encode.rs` times *only* `encode_frame_420` on a fresh pipeline,
  excludes init, and warms up — matching the C harness `tools/perf_c_encode`.
  But it only generates synthetic content. `identity_run` takes real images but
  has no timer. Adding a timer to only one arm would make the cross-encoder
  comparison *worse*, not better, so this harness uses a uniform process-wall
  clock for all four (§3) and leaves `perf_encode`'s tighter clock to the
  zenav1-svt-internal speed-gap work, where both sides use it.
- **QP 0 is unreachable on the SVT arms.** `SvtAv1EncApp --qp` is documented
  `[1-63]`, and `zenav1-svt` rejects `base_qindex 0` with a typed
  `UnsupportedConfig` by design (zenav1-svt#5). The grids start at 4.
- **AVIF container accounting is not implemented.** `payload_bytes()` strips IVF
  and passes bare OBU through, but returns −1 for an `ftyp` file. Plane 2 (§6)
  needs it; plane 1 does not.

---

## 2. Everything around the encoder is byte-identical

An RD comparison is only as good as its symmetry. `rd_tool` (an example in this
repo, `examples/rd_tool.rs`) owns both ends:

```
source PNG ──prep──▶ ref.png            (the scoring reference)
                 └─▶ src.y4m / src.yuv  (ONE canonical input, all four arms)

bitstream ──decode──▶ rav1d-safe, 1 thread ──▶ fixed YUV→RGB ──▶ decoded PNG
```

- **One input.** Every arm encodes the same I420 bytes, so no arm can win or
  lose on its private RGB→YUV.
- **One decoder.** rav1d-safe, single-threaded, for IVF, bare OBU and AVIF
  alike. The older `scripts/rd_gap` harness decoded our encoder with zenavif and
  libaom with `aomdec` and listed the asymmetry as a known limitation; this one
  does not have it.
- **One colour pair.** BT.601 limited-range integer, forward with a 2×2 box
  average, inverse with *nearest* chroma upsampling — chosen because nearest is
  the inverse of a box average, so the pair adds no directional blur. It is
  byte-identical to the transform in zenav1-svt's `identity_run`.
- **Downscale only, Lanczos3, even dims.** Upscaling would fabricate
  high-frequency detail that no encoder should be judged on. Because of that,
  `--sizes` is a *cap*, not a promise: asking for 1024 from a 576 px source
  gives 576. The `size_tag` column therefore reports the **achieved** long edge
  and `size_req` the request, and the run prints a NOTE for every input where
  they differ. (They were the same column at first, which quietly labelled a
  576 px cell "1024" and would have corrupted every per-size number.)

### Two references, and why the floor matters

Every variant is scored against **both**:

| reference | what it measures | ceiling |
|---|---|---|
| `ref.png` | product reality — includes the fixed 4:2:0 round-trip cost | the floor value |
| `floor.png` | the encoder alone | 100 |

`floor.png` is `src.y4m` put through the inverse transform with **no encode**.
No arm can beat it, so quoting an absolute ssim2 without it is misleading — and
on the validation grid the floor turns out to vary enormously with content:

| ssim2 floor | 64 px | 256 px | 1024 px |
|---|---|---|---|
| photo | 64.5 | 91.9 | 91.2 |
| screen / text | **26.9** | 74.6 | 78.5 |
| line-art | 71.2 | 89.7 | 88.3 (@796) |

**On screen content, 4:2:0 chroma alone costs more quality than the encoder
does**, and at 64 px the ceiling (26.9) sits below most useful quality targets.
That is why the analysis defaults to `ssim2_floor` (ceiling 100) — it is the
encoder comparison. `ssim2_ref` is one flag away when you want the product view.

Absolute scores from this harness are **not** comparable to another harness's:
the nearest-neighbour chroma upsample depresses them, identically for all arms.
Compare arms to each other and to the floor.

---

## 3. What the clock contains — the core measurement decision

**`wall_ms_med` is the full process wall clock**: spawn, dynamic linking, input
parse, encode, output write. Median of ≥5 repeats, never niced.

### On macOS, `nice` is not the thing that ruins a timing — measure, don't assume

The standing project rule is "no `nice` on a timed run, because macOS maps nice
to the E-cores." That was checked here rather than taken on faith, with a fixed
6M-iteration integer loop on this M4 Pro (8P + 4E):

| launch policy | calibration loop |
|---|---|
| plain, `nice 5` (inherited from the agent harness) | **452.6 ms** |
| `nice -n 19` | **471.7 ms** (+4 %) |
| `taskpolicy -b` (true darwin-background QoS) | **2678.7 ms** (**5.9×**) |

So `nice` and E-core relegation are **different mechanisms**. `nice` barely
moves a single-threaded job on an idle box — it only bites under contention.
What costs 5.9× is the **darwin background QoS class**, which `nice` does not
set and which every child of a backgrounded process inherits silently.

The practical consequence is that "I didn't type `nice`" is not evidence of a
valid timing. So `run_grid.py` **gates on it**: at startup it runs that loop
normally and again under `taskpolicy -b`, and if being explicitly backgrounded
is not meaningfully slower, the process is already there and the run **exits
rather than measuring**. The ratio is written into the TSV header of every run.
The check is relative, so it needs no machine-specific constant.

(Watch out for `taskpolicy -c user-initiated` as a "fix" — that is not valid
syntax, it prints usage and runs nothing. Timing it produces a beautiful,
entirely fictional 42× speed-up, which is how this whole thread started.)

Process wall is the only definition applicable *uniformly* to all four arms
without modifying three repositories, and it is what an image pipeline actually
pays per image. Its cost is a per-arm constant that differs between arms —
`svtrs`'s PNG decode, each encoder's init.

That constant is not hand-waved away. It is **measured**, via the size sweep the
project's sweep discipline mandates anyway. But the measurement produced a
result worth stating up front:

> **`t = α + β·pixels` does not hold across a 256× pixel range.** Per-pixel
> encode cost *falls sharply* with image size — measured on the probe, `svtc`
> preset 8 costs **843 ms/MP at 64×64 and 35 ms/MP at 576×576**, a 24× swing
> for one arm at one ladder rung; `aom` cpu-used 4 swings 2921 → 2177 ms/MP.
> The log-log exponent is 0.27–0.92 (all sub-linear). For `aom` cpu-used 4 the
> curvature is strong enough that the least-squares intercept comes out
> **negative** (−14.75 ms), which is not an overhead but proof the straight
> line is the wrong shape — despite r² = 0.997, because with three points
> spanning 256× the largest point dominates the fit and a high r² proves
> nothing.

So section C reports three things, in this order:

- **C1 — the per-size table.** Median ms and ms/MP at each size. This is the
  honest primary: any single ms/MP figure is wrong at every size but one.
- **C2 — the `α + β·MP` fit**, with r², the log-log exponent, and α as a
  percentage of a 1 MP encode. It flags `alpha<0: LINEAR MODEL REJECTED`.
- **C3 — a local α from the two smallest sizes only**, where the curve really
  is near-linear. This is the usable estimate of genuine fixed cost: it lands
  at **3.0–5.7 ms for every arm** (spawn + init + parse + write), except
  zenrav1e at its slowest rung.

Two cross-checks on α, both recorded per cell:

- **`self_ms`** — the encoder's own reported encode time, where it prints one
  (`aomenc` µs/frame; `SvtAv1EncApp` "Total Encoding Time"; rav1e's fps
  inverted). `wall_ms_med − self_ms` is an independent estimate of the same
  overhead. On the validation cell: svtc 10.27 wall vs 8.00 self, zenrav1e
  227.68 vs 224.01.
- **`bytes_file` vs `bytes_av1`** — the same discipline on the byte axis. IVF
  costs 32 header + 12 per frame = **44 bytes**; two arms write IVF and one
  writes a bare OBU, so comparing file sizes would hand the OBU arm 44 free
  bytes (~5 % at 64×64). All RD uses `bytes_av1`.

---

## 4. The matched-time reduction, step by step

Given cells indexed by (image, size, arm, **ladder**, rate):

1. **Per (image, arm, ladder), build the RD curve** over the rate grid and
   Pareto-filter it: drop any point another point beats on *both* bytes and
   quality. Rate ladders are not always monotone and a kink becomes a fake
   interpolation crossing if left in.
2. **Interpolate `bytes @ Q*` and `time @ Q*`** along that curve, in log space
   for both (bytes-vs-quality is near-exponential over a short interval; linear
   interpolation across a 2× gap biases high). A target outside the arm's
   achieved span at that ladder is **NA — never extrapolated**.
3. **Per (image, arm) that gives points `{(time, bytes)}` indexed by ladder** —
   the arm's *time-vs-bytes-at-fixed-quality frontier*. Pareto-filter again: a
   rung both slower and bigger than another rung is never the right choice.
4. **Compare two arms** by interpolating log(bytes) against log(time) along each
   frontier — time spans decades, so log-log — and report the bytes ratio at a
   geometric grid of times.

### Non-overlapping ladders

**Only the overlap of the two arms' measured time ranges is reported.** Outside
it, `analyze_matched.py` prints `LADDERS DO NOT OVERLAP IN TIME` with both
ranges, and no number is invented. If an arm contributes fewer than two
undominated rungs there is nothing to interpolate along and it says that too.
This is the case the methodology has to get right, because the tempting move —
extrapolating one arm's frontier to meet the other — is exactly how a fabricated
result gets published.

---

## 5. Box hygiene

This box is shared with other agents' builds and test runs, so contention is a
real, ongoing condition rather than a transient.

- Before every timed sample the harness polls `ps` and **waits** (up to
  `--idle-budget`, default 120 s) for the box to settle. Waiting beats
  discarding, and both beat publishing a contaminated ladder.
- The gate is on **free cores**, not on one process's %CPU. The project rule
  ("discard over 25 % CPU") was written for a box where one hog meant real
  contention; on a 12-core M4 Pro one neighbour at 100 % is one core of twelve
  and a single-threaded encode still lands on a free performance core. The gate
  requires `nperf − threads − 1` foreign cores free.
- **Both numbers are recorded per cell** anyway — `foreign_cpu_pct` (worst
  single process) and `foreign_cores` (total) — so a reader who wants the strict
  25 % rule can re-filter after the fact without re-running.
- Arms are **interleaved and the visit order is rotated** each repeat, so no arm
  always runs first, into a cold cache or a thermal state another arm created.
- `ps -o %cpu` on macOS is a *lifetime* average, not an instantaneous sample. For
  a freshly-spawned hog it reads true; for a long-lived process it over-reports,
  which errs toward waiting — the safe direction for a timing harness.

Section A of the analysis reports the per-arm timing spread across repeats. That
is the empirical answer to "was the box quiet enough", and it beats assuming.

---

## 6. What the instrument was validated to do

Grid: 3 images (photo / screen / line-art) × 3 sizes × 4 arms × 3 ladder rungs ×
8 rates × 5 reps = **864 cells, 4320 encodes, 0 failures, 1037 s**. Plus an
independent 256 px re-run: 288 cells, 1440 encodes, 127 s. Full record in
`benchmarks/encode_rd_validate_2026-08-08.meta`.

| check | result |
|---|---|
| bytes deterministic across 5 repeats | **864/864 cells** identical |
| bytes identical across two independent runs | **288/288 cells** identical |
| within-run timing spread | median **1.2–3.7 %**, p90 **5.8–9.4 %** |
| within-run outliers | only **8/864** cells over 20 %; 7 of them at 64 px, 6 of 8 with a median under 10 ms |
| run-to-run timing | geometric mean **0.988–0.997**, \|ln ratio\| p90 **0.75–2.90 %**, max 5.80 % |
| input symmetry | **9/9** prep inputs sha256-identical to `identity_run`'s own I420 |

**The instrument's resolution is ~1–3 %.** An A/B difference smaller than the
run-to-run p90 is not measurable on this box in one sitting; do not report one.

Two things fell out of the validation that are findings in their own right:

- **`svtc` and `svtrs` agree byte-for-byte on 180/216 shared cells**, and the 36
  that differ do so by **≤ 0.08 %** — all on the larger sizes. The pure-Rust SVT
  port is at or very near bitstream parity with the C reference on real content.
  (A 24-cell probe had shown 24/24; the full grid is what found the 36.)
- **The measured slope ratio `svtrs β / svtc β` is 4.7× (preset 4), 5.4×
  (preset 6), 6.2× (preset 8)** — the zenav1-svt speed-gap number, independently
  reproduced here on real images. The repo's own figure is 3.5–4.9×, so preset 4
  agrees and the faster presets are worse than that range; the two harnesses use
  different content and different presets, and this one has not been reconciled
  against `perf_encode`'s tighter encode-only clock.

## 7. Scope: this is plane 1

- **Plane 1 (implemented): y4m → AV1 bitstream.** Pure encoder comparison,
  perfect input symmetry. Everything above.
- **Plane 2 (not implemented): PNG → AVIF** via `cavif` / zenavif's own encode
  path and `avifenc`. Measures what a user actually gets, including each
  encoder's own colour handling and the container. Needs AVIF payload
  accounting in `payload_bytes()` and a `cavif`/`avifenc` arm. `rd_tool decode`
  already handles AVIF, so the decode side is ready.

---

## Prerequisites

```bash
# this repo
cargo build --release --example rd_tool

# scorer  (png + cpu-metrics is enough; avoids pulling every codec)
( cd ~/work/zen/zenmetrics && cargo build --release -p zenmetrics-cli \
    --no-default-features --features png,cpu-metrics )

# zenrav1e CLI
( cd ~/work/zen/zenrav1e && cargo build --release --features binaries --bin rav1e )

# zenav1-svt Rust driver
( cd ~/work/zen/zenav1-svt/rust && cargo build --release --example identity_run )

# SVT-AV1 C reference — prebuilt in-tree at zenav1-svt/Bin/Release/SvtAv1EncApp
# libaom — `brew install aom` provides aomenc
```

Every path is overridable by environment variable: `RD_TOOL`, `ZENMETRICS`,
`RAV1E`, `SVTC`, `SVTRS`, `AOMENC`, `CORPUS`.

## Run

```bash
# validation grid (what benchmarks/encode_rd_validate_*.tsv was produced with)
python3 run_grid.py \
  --images gb82/city-lossless.png,gb82-sc/gui.png,gb82-sc/graph.png \
  --sizes 64,256,1024 --arms aom,svtc,zenrav1e,svtrs \
  --ladder 4,8 --rate-stride 4 --reps 5 --verify-yuv \
  --out ~/tmp/encrd/cells.tsv

python3 analyze_matched.py ~/tmp/encrd/cells.tsv --metric ssim2_floor
```

Analysis sections: **A0** byte determinism · **A1** cross-arm byte agreement ·
**A** timing spread (pooled and by size) · **B** achieved-quality span per arm ·
**C** time vs image size (C1 per-size table, C2 the α+β fit, C3 local α) ·
**D** time-vs-bytes frontier · **E** the matched-time comparison · **E2** the
geometric-mean aggregate by content class · **G** measured per-cell cost and the
projection basis for sizing a bigger grid · **Z** caveats.

Within-run spread is not the same question as run-to-run agreement, and only the
second one tells you whether a measured A/B difference is real. Run the grid
twice and diff:

```bash
python3 reproducibility.py runA.tsv runB.tsv
```

It reports the geometric mean and the |ln ratio| distribution of
`wall_ms_med(B)/wall_ms_med(A)` per arm and per size, and separately checks that
the two runs produced identical bytes — which must be exact, since anything else
means the runs did not measure the same thing.

Encoded bitstreams land in `--artifacts` (default `~/tmp/encrd/artifacts`) named
`<sha256>.<ext>`; every cell row carries its `enc_sha256`, so scalar scores can
be rejoined to the exact bitstream forever. The run **aborts** if no artifacts
landed, before any grid gets scaled up.
