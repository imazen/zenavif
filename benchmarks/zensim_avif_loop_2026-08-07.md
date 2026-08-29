# zensim AVIF target-hitting loop — harness + pre-registered study (2026-08-07)

The AVIF analogue of jxl-encoder's zensim loop series (direct extension of
`jxl-encoder/benchmarks/zensim_loop_beatbutter_2026-08-07.md`, whose
controller defaults this harness adopts). Registered in the sota944
campaign as **appendix AC.4** (zensim
`benchmarks/sota944_campaign_2026-08-03.md`): harness-first, one-cell
smoke THIS session; **the full matrix runs when the wave-12 candidate
bake lands**, with shipped C as the control arm. **This registration was
committed before any matrix run.**

## Purpose

User directive chain: *"iterate towards a jxl perfect model better than
the butter loop"* → *"for an avif loop"* → *"do it"*. The jxl loop's
proven pieces — pure-proportional controller (exp 1.0, per-step clamp
2.0, beats-butter adoption), h3-mag magnitude steering from the fused
binned attribution, emit-best — transplanted onto zenavif's CQ dial and
per-superblock quantizer hints.

## The harness

`examples/zensim_cq_rd.rs` (build:
`cargo build --release -p zenavif --features encode-imazen,two-pass-butteraugli --example zensim_cq_rd`),
runner `scripts/zensim-loop/run_avif_loop.sh` (phases `smoke` / `matrix`).
CLI mirrors `zensim_diffmap_rd`: `--corpus-file` (path\tname\tclass TSV),
`--zensim-targets 70,80,88`, `--arms baseline|h3-mag|outer`, `--bake
<path>|profile:c`, `--iters K`, `--label`, `--out-dir`. Env:
`AVIF_ZENSIM_CTRL_EXP` (1.0), `AVIF_ZENSIM_CTRL_CLAMP` (2.0),
`ZENSIM_ATTR_BIN` (8), `AVIF_ZENSIM_EMIT_BEST`, `AVIF_ZENSIM_H3_GAIN`
(10.0), `AVIF_ZENSIM_FACTOR_MAX` (1.15), `AVIF_ZENSIM_SEED_CQ`,
`AVIF_ZENSIM_SPEED` (6), `AVIF_ZENSIM_SAVE_AVIF`.

Manifest `target_ab_<label>.tsv` carries the jxl series schema (seed_d →
seed_cq) so `analyze_23shot.cells_stats` reads it unchanged; a
per-iteration `trace_<label>.tsv` records `trace_id iter cq qindex score
bytes sb_min sb_max iter_ms`.

### Registered constants

- Encoder: zenavif speed **6**, **4:4:4** (the rd_gap/cavif harness
  convention), 8-bit, **threads 1** (deterministic; the box carries HDR
  scoring containers). Env-overridable, registered at these values.
- Scorer: folded-944 extraction (`compute_folded720_append2_features`)
  on (ref, decoded) + `score_features_with_profile` with the mounted
  bake. Default bake = `/mnt/v/output/zensim/bakes/sota944/bakes/
  W10L9_s4003_packed.bin` (Profile C's bytes; caller width **944**, the
  pruned-bake sizing rule). The harness REFUSES bakes with caller width
  < 720 — the folded-944 layout zeroes f156-371, so a 372-class bake
  would silently mis-score (the `--regime 944` known-bug class).
- Because scoring runs on the ACTUAL decoded bitstream,
  `achieved_inloop == achieved_decoded` structurally (no jxl-style
  internal-recon proxy gap; both columns kept for schema parity).
- Budget convention: `--iters K` = K steps after the seed = **K+1
  encodes per cell** (k3 ↔ jxl k3's 4 compares; outer j3 = 4 encodes).
  No early-stop tolerance (the beats-butter runs' `TARGET_TOL=-1`
  convention); `AVIF_ZENSIM_EMIT_BEST=1` for all registered runs.

### The CQ domain + controller (identical for every arm)

CQ = continuous AV1 qindex [1, 255]. zenavif's public dial is `quality`;
the harness inverts zenravif's piecewise `quality_to_quantizer` (mirror +
a 1..=255 roundtrip self-check that panics on ±1 drift) so the controller
works in the quantizer domain. Update rule (the adopted jxl template,
quantizer-domain mirror — jxl's qf ∝ 1/q):

```
achieved_loss = max(100 − score, 0.05)
target_loss   = max(100 − target, 0.05)
g       = clamp((achieved_loss / target_loss)^exp, 1/clamp, clamp)   # exp 1.0, clamp 2.0
next_cq = clamp(cq / g, 1, 255)
```

Pure proportional in the score-error → log-quantizer domain:
`Δlog cq = −exp · (log achieved_loss − log target_loss)`, per-step
multiplicative clamp 2.0. The SAME controller runs in every inner arm, so
arms isolate the steering variable alone.

### h3-mag steering (per-64px superblock)

Gradient `s` = `score_features_fd_gradient_with_profile` at the SEED
compare's folded-944 features (batched FD; panics if identically zero).
Steered iterations 1+ score through the fused binned entry
`compute_folded944_score_and_attribution_binned` (bin **8** — 64px SB
rects are bin-aligned ⇒ `query_rect` exact; features/score bit-identical
to the plain extraction). Per SB `b` (raster `ceil(w/64) × ceil(h/64)`,
the `FrameHints::sb_q_scale` grid):

```
tile_q[b]  = attr.query_rect(sb rect)                    # score units; + = damaged = wants bits
factor     = clamp(1 + gain·tile_q[b], 1/fmax, fmax)     # gain 10.0, fmax 1.15/step
scale[b]   = clamp(scale[b] / factor, 0.5, 2.0)          # ÷: quantizer is inverse-precision
scale     /= mean(scale)                                  # mean quantizer == controller's CQ
```

applied to the NEXT encode via `EncoderConfig::with_sb_q_scale` (hints
first land on encode 2 — the jxl timeline: the seed compare derives the
gradient, encode 1's compare produces the first map). Engagement gates
in-binary: unknown `--arms` values hard-error; h3-mag panics unless
`FRAME_HINTS_LIVE` AND a live probe (same-map determinism + differing
bitstream under a 0.5/2.0 map) passes.

### The comparator (`--arms outer`)

Zensim-judged CQ **bisection** at one full re-encode per step: bracket
[1, 255], first probe at the shared seed CQ, integer midpoints; judge =
the same folded-944 forward on the decoded output. In AVIF, inner and
outer steps cost the same (one full encode each — unlike jxl, whose inner
compares skip entropy coding), so inner-vs-outer isolates the UPDATE RULE
(+ steering) at equal per-step cost.

## SEEDS (calibrated 2026-08-07, registered)

Single-encode probes, city.png 576² (speed 6, 4:4:4, W10L9 bake):

| CQ | 40 | 45 | 50 | 60 | 80 | 100 | 120 | 140 | 160 | 180 |
|---|---|---|---|---|---|---|---|---|---|---|
| score | 87.89 | 87.13 | 86.42 | 85.13 | 81.66 | 78.22 | 71.67 | 65.03 | 53.55 | 44.48 |
| bytes | 107811 | 101328 | 95944 | 87213 | 72596 | 60035 | 46429 | 34452 | 25457 | 17136 |

Registered seed table: **t88 → CQ 40, t80 → CQ 90, t70 → CQ 125**
(bracket interpolation, rounded; coarse by design — the controller owns
convergence). `AVIF_ZENSIM_SEED_CQ` overrides for seed studies.

## Cells (frozen; series-identical)

The beats-butter 9-ref corpus VERBATIM (recipe in
`scripts/zensim-loop/run_avif_loop.sh`, copied from jxl-encoder
`run_beatbutter.sh`): coherence refs city/dog/girl 576², CID22-512
validation 1025469/1418519/1189261, gb82-sc 576² crops (+512+256)
wiki/gui/imessage × targets {70, 80, 88} = 27 cells per run. Arms
{baseline, h3-mag} × k{2, 3} for candidate AND control bakes + the outer
comparator at j{2, 3}.

## Registered gates + outcomes

- **G-AV1 (engagement)**: h3-mag traces must show a non-neutral
  `sb_min/sb_max` band and the in-binary hint probe must pass (panic
  otherwise); unknown arms hard-error. Status THIS session: the probe
  correctly REFUSES — see ENGAGEMENT STATUS below.
- **G-AV2 (smoke, this session)**: ONE cell (city t80 k3) end-to-end with
  a sane trace before anything else. **PASS — see SMOKE.**
- **G-AV3 (the matrix — registered, NOT run this session)**: runs when
  the wave-12 candidate lands (appendix AC.2/AC.3), candidate + C-control
  on every arm. Outcome frame (beats-butter shape): inner arms vs the
  outer comparator — own-units **±2 census** over 27 cells + med |err| +
  med bytes at k2-vs-j2 and k3-vs-j3, stats via
  `analyze_23shot.cells_stats` (the jxl owner; no hand-rolled medians).
  The wave-12 candidate's loop census vs C feeds appendix AC's G-AC2
  "AVIF-facing axis" arm.

Matrix command (when the candidate lands):

```
scripts/zensim-loop/run_avif_loop.sh matrix /path/to/wave12_candidate.bin
```

## SMOKE (G-AV2, measured 2026-08-07)

Substrate: zenavif this commit (workspace on main@origin 5fe50853 +
this change), zensim `f4588e9d` (path dev-dep `zensim03`), W10L9 C bake,
box CONTENDED (HDR containers, load ~15-19; ms columns load-sensitive as
registered across the series; value columns deterministic —
single-threaded encodes). Data committed:
`benchmarks/zensim_avif_loop_smoke_2026-08-07.tsv` (+ `_traces_`);
decoded outputs mirrored at `/mnt/v/output/zensim/avif-loop-2026-08-07/`.

| cell | arm | seed_cq | achieved | \|err\| | bytes | encodes |
|---|---|---|---|---|---|---|
| city t80 k3 | baseline | 90 | **80.009** | **0.009** | 67086 | 4 |
| city t80 j3 | outer (bisect) | 90 | **80.009** | **0.009** | 67086 | 4 |
| city t70 k3, seed FORCED 60 | baseline | 60 | **71.67** | 1.67 | 46429 | 4 |

- Baseline k3: the calibrated seed lands on-target (80.0093 at CQ 90);
  the controller correctly parks (g ≈ 0.9995 → qindex stays 90).
- Outer j3: bisection from the same on-target seed THRASHES by
  construction (90 → 173 → 132 → 111; judged 80.01 → 46.78 → 67.56 →
  74.91) — emit-best rescues iterate 0. As-emitted (emit-last) it would
  read 74.91/err 5.09: with a good seed, bisection's bracket exploration
  is pure waste. This asymmetry is the registered comparison's substance.
- Controller dynamics (seed deliberately forced 2× off): 60 → 120
  (per-step clamp 2.0 ENGAGED at the floor: g clamped to 0.5) → 127.1 →
  119.5; scores 85.13 → 71.67 → 68.08 → 72.12; emit-best picks 71.67
  (±2 hit from a badly wrong seed in a 4-encode budget). Pure-proportional
  damped oscillation, exactly the jxl exp-1.0 shape.
- Cost: ~68-81 ms/compare (scoring+steering) on 576²; full cells
  2.5-2.6 s wall. The 27-cell × 10-run matrix is a ~20-minute job.
- h3-mag smoke: loud refusal as designed (below), captured in
  `run_avif_loop.log` — NOT a silent fall-through to baseline.

## ENGAGEMENT STATUS — h3-mag is HARNESS-READY but gate-blocked (measured)

`zenavif::FRAME_HINTS_LIVE == false` on today's dep chain: registry
zenrav1e 0.1.4 has no `FrameHints` (the input lands on zenrav1e master
past 0.1.4, rev `c4047cec`); zenravif has the full passthrough plumbed
but constant-gated with the hinted send commented
(`ravif/src/av1encoder.rs`). MEASURED, two independent ways:

1. `tests/sb_q_scale_hint.rs` (this commit): a 0.5/2.0 per-SB map leaves
   the bitstream **byte-identical** on the gated build (and the same test
   flips to asserting engagement when the gate goes live — it never
   silently skips).
2. The harness's h3-mag probe refuses with the exact unblock chain
   (panic, not fall-through).

Unblock (owned by the ravif repo lane, NOT this repo): zenravif dep bump
past zenrav1e 0.1.4 + flip `FRAME_HINTS_LIVE` + uncomment the hinted
send. The moment that lands, `zensim_cq_rd --arms h3-mag` runs with ZERO
changes here (the additive `EncoderConfig::with_sb_q_scale` hook forwards
maps today; they are accepted-but-inert until the gate flips).

## Additive API surface added by this change

- `EncoderConfig::with_sb_q_scale(Option<Box<[f32]>>)` +
  `sb_q_scale_value()` (feature `two-pass-butteraugli`) — public access
  to the existing crate-internal per-SB hint field the two-pass driver
  already forwards; release-gate documented on the method; gated by
  `tests/sb_q_scale_hint.rs`.
- Dev-dep `zensim03` (renamed path dep on sibling zensim 0.3.0) — the
  registry 0.2.4 dev/product deps are untouched (same-name coexistence
  would E0464 the lib-test target); collapse at the zensim 0.3.0 publish.
- CI `clone-siblings` now also clones `imazen/zensim`.

## CONTROL CENSUS RUN (2026-08-27) — the AC.4 control-baseline subset

The candidate/h3-mag arms stay gated (wave-12 bake never landed;
FRAME_HINTS_LIVE still false — smoke re-confirmed the loud refusal today).
The CONTROL-baseline subset is the GOAL criterion-4 instrument census and
ran today on the registered corpus9 × t{70,80,88}, control bake
(`W10L9_s4003_packed`), emit-best:

| k | median \|err\| (decoded) | ±2 hits | photo | nonphoto | med iters |
|---|---|---|---|---|---|
| 2 | **0.756** | 23/27 | 0.425 | 1.965 | 3.0 |
| 3 | **0.336** | 23/27 | 0.190 | 0.514 | 4.0 |

Best target-hitter of the three censused encoders (jxl k2 ctrl 0.832,
zenwebp k2 1.859 — different judges, stated for scale not ranking). Smoke
row: city t80 k3 |err| 0.010. TSVs alongside; cells at
`/mnt/v/output/zenavif/instrument-census-2026-08-27/`.

## MATRIX RESULTS (2026-08-29) — the registered G-AV3 run; candidate = north-anchor

The wave-12 candidate landed (`W10L9PH_s4004_packed`, the frozen SDR
candidate-of-record "north-anchor"); the registered matrix ran on the
frozen 9-ref × t{70,80,88} corpus, control = shipped C bytes
(`W10L9_s4003_packed`, "gray-tower"). Cells:
`benchmarks/avifloop-matrix-2026-08-29/` (registered constants; speed 6,
4:4:4, emit-best).

| arm | med \|err\| | ±2 | total bytes |
|---|---|---|---|
| **cand (north-anchor) k3** | **0.180** | **24/27** | 826,038 |
| ctrl (gray-tower) k3 | 0.336 | 23/27 | 824,575 |
| cand k2 | 0.387 | 19/27 | 835,280 |
| ctrl k2 | 0.756 | 23/27 | 832,762 |
| outer j3 (bisection comparator) | 1.120 | 21/27 | 803,050 |
| outer j2 | 2.450 | 13/27 | 907,527 |

**Readings:** (1) the proportional inner controller dominates outer
bisection at equal per-step cost (0.18-0.34 vs 1.12 at 3 steps) — the
jxl-proven update rule transfers to the CQ domain; (2) **north-anchor
halves the control's k2 error and beats it at k3 on both stats** — its
finer top-zone dial structure (the G-GRAN-audited calibration) pays
directly in target-hitting on AVIF, the priority codec; (3) byte
totals are arm-comparable (±1%).

**h3-mag arms: REFUSED as registered** — `FRAME_HINTS_LIVE == false` in
the shipped zenravif build (per-SB q-scale maps accepted but not applied
below the zenrav1e 0.2.0 FrameHints input, master `c4047cec`). The
refusal fired loudly and instantly, exactly per the harness contract.
The unblock (zenravif dep bump + const flip + hinted send) is in flight
in the ravif repo; the h3-mag arms re-run when it compiles + its
determinism/differing-bitstream probe passes.

## h3-mag ARMS — first avif diffmap results, PARTIAL (2026-08-29 ~04:3xZ)

FRAME_HINTS went LIVE upstream (zenravif git f6c883b6 against zenrav1e
0.2.0 e4883037 — the dep bump had landed concurrently; the local ravif
staging was redundant and reverted). Candidate own-map arms completed;
the control h3 arms were stopped mid-run (task killed — lane HELD):

| arm | med \|err\| | ±2 | dBytes vs own baseline |
|---|---|---|---|
| cand (north-anchor) h3 k3 | 0.291 | 18/27 | **−5.40%** |
| cand h3 k2 | 0.668 | 18/27 | −2.57% |
| (scalar baselines) | 0.180 / 0.387 | 24 / 19 | — |

**Reading: own-map h3 steering DEGRADES avif target-hitting for the
scorer** (error up, hit-rate down) while saving bytes — the map
under-allocates. Same signature as jxl's "A + own map makes A worse"
(0.343→0.404), stronger here. The pair pattern (A scores + river-lantern
steers) is exactly the untested cell: split-role support
(`AVIF_ZENSIM_MAP_BAKE`, the jxl fd2f4351 mirror) is STAGED in
`examples/zensim_cq_rd.rs` (uncommitted-pending-build; inert without the
env). Protocol note for the rerun: the staged change requires a rebuild,
so an R0-identity re-run of one own-map arm gates arm-comparability
before any split cell is read.

## h3-mag COMPLETE ARMS + split-role amendment (2026-08-29 ~04:4xZ)

Control own-map arms (new-substrate binary; note the R0 rerun's cells are
byte-level different from the pre-split-role binary but STATISTICALLY
IDENTICAL — cand h3 k3 0.291/18 on both — so the split-role code is
inert-in-stats as designed; within-binary discipline kept anyway):

| arm | med \|err\| | ±2 | vs own scalar |
|---|---|---|---|
| ctrl (gray-tower) h3 k3 | 0.495 | 19/27 | scalar 0.336 / 23 — DEGRADES |
| ctrl h3 k2 | 0.867 | 17/27 | scalar 0.756 / 23 — DEGRADES |
| cand (north-anchor) h3 k3 (r0) | 0.291 | 18/27 | scalar 0.180 / 24 — DEGRADES |

**Own-map h3 steering degrades target-hitting for BOTH bakes on avif** —
the map under-allocates and costs hits; the jxl own-map signature is
codec-general. First anchor-lantern attempt PANICKED mid-corpus:
**river-lantern's FD gradient is identically zero on screen-content
cells** (f16 forward plateaus on flat content), and where it engaged on
sc_gui it steered badly (t70 → achieved 37.6). **Harness amendment
(registered here before the rerun):** with a mounted MAP bake, a zero
gradient now falls back to UNSTEERED for that cell (logged loudly) — the
product-realistic semantics; the own-map zero-gradient panic is kept (a
mount bug, not a content property). Pair arms re-running under the
amended contract; fresh same-binary scalar baselines queued.

## PAIR ARMS UNDER THE AMENDED CONTRACT + substrate caveat (2026-08-29 ~04:5xZ)

pair (north-anchor scores + river-lantern maps): k3 0.409 (18/27), k2
0.655 (19/27). Same-window scalar v2 baseline: k3 0.392 (19/27), k2
0.514 (19/27). **On avif the pair does NOT beat the scalar loop** —
every map arm (own or companion) ties or degrades. PRELIMINARY verdict
pending the tri-arm clean batch, because a SUBSTRATE SHIFT intervened:
the concurrent-session encoder changes pulled by the rebase moved the
scalar baseline 0.180 → 0.392 across binaries (cross-binary comparisons
invalid; the original 0.180-vs-0.336 candidate-vs-control read was
within-binary and stands). A single same-binary {scalar, own-map, pair}
k3 batch is queued to settle arm ordering on the current substrate.
