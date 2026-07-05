# Feature-driven AV1 encoder priors — design (2026-07-02)

**Status: design complete, implementation phased below (P0-P4).** Goal: use the zen feature
stack (zenanalyze / zenanalyze-api) + zenrav1e's own per-block statistics to drive encoder
choices "intuitively" — a smart *fast* default that makes near-best choices without intense
encode-loop search, plus per-SB hints that safely prune search. Companion to
`RD_GAP_VS_LIBAOM.md` (search-quality program) and `TUNE_SSIMULACRA2_PLAN.md` (metric tunes).

## Corrections to stale assumptions (verified against source)

1. **zenanalyze is 0.2.x with 97 default features** (+3 `experimental` → 100, +16 `hdr` →
   116). The "102 features, IDs 0-121, 0.1.x-forever" claim is retired —
   `zenanalyze/README.md:23-45` explicitly calls it stale. Canonical parquets carry "~97
   named zenanalyze" + 372 zensim `feat_0..371`.
2. **Train on `/mnt/v/output/canonical-picker-2026-07-01-zensimA/zenavif_lossy/`**
   (= `s3://zentrain/canonical/2026-07-01-zensimA/`, zensim profile A). The 2026-06-27 set is
   superseded (PreviewV0_2 metric, banned). 775,152 train rows, 48 cells = speed{2,4,6,8} ×
   qm{on,off} × {444,420} × bd{8,10} × {YCbCr,RGB}, q∈{5,15,30,50,70,85,95}, with
   `encode_ms`/`decode_ms` (`~/work/zen/DATA_PROVENANCE.md:571-832`).
3. **Critical drift caveat: canonical encodes used pre-parity zenrav1e `22a58d58`.** Master
   now has 5 RD fixes + 2 QM fixes + tune infra (`64a081d4`). **QUANTIFIED 2026-07-02
   (P0.2, see the P0 phase entry):** aggregate |Δbytes| p50 = 0.21% but knob-correlated —
   s2 −3.36% / s4 −1.23% signed p50 vs s6/s8 ≈ 0; ssim2 |Δ| p90 = 1.36 pts; encode_ms
   1.36-2.59× on every cell. Speed/qm heads + encode_ms LUTs need re-encoded labels;
   subsampling/bit-depth/color heads can train on existing labels.

## Recommended v1 scope

**(a1) Extend the existing global picker** (`src/auto_tune.rs`, ZNPR MLP + LUTs — the shipped
production pattern) with knob heads trainable from canonical data today: chroma subsampling,
bit depth, color model, qm — plus **budget-gated `rdo_tx_decision`**, the single largest known
lever (−5.7% median bytes AND better ssim2 at −Q80-95, 7.5× time; RD_GAP §6b). It was declined
as a *default* only to preserve matched-speed comparisons; the picker's encode_ms LUT +
`time_budget` machinery (`src/auto_tune.rs:422-461`) is exactly the principled place to spend
that time when a budget allows. **(b1) Per-SB `partition_range.min` narrowing** as the first
in-loop hint — pure speed lever for the fast mode, gated off in matched-speed/s2 mode.

## (A) Global feature→knob table

| Knob (plumbing) | Predictive features | Training signal | Expected win |
|---|---|---|---|
| speed, quality | (shipped picker v0.1.1, `auto_tune.rs`) | canonical | baseline |
| `chroma_subsampling` 420/444 (`with_chroma_subsampling`) | CbSharpness/CrSharpness (+per-axis), ChromaComplexity, ChromaLumaCovariance — zenanalyze's founding use case | **in canonical** (420/444 axis) | bytes at matched score; largest new head |
| `bit_depth` 8/10 | GradientFraction[Smooth] (banding), NoiseFloorY | **in canonical** (bd10 axis) | HQ-band bytes |
| `color_model` RGB/YCbCr | GrayscaleScore, DistinctColorBins | **in canonical** | niche |
| `with_qm` | flatness/synthetic features | **in canonical** (noqm axis); OAT: off = +4.2% median | keep on; model may find off-pockets |
| `with_rdo_tx_decision(Some(true))` at high q | Uniformity, HighFreqEnergyRatio, texture family + **pixel count × ms-LUT budget check** | needs small sweep (canonical lacks axis); RD_GAP §6b measured −5.7%/7.5× | biggest opt-in win |
| `vaq_strength` (≠1.0; 1.0 is a byte-proven no-op, `VARIANT_GENERATION.md`), `seg_boost`, `segmentation_complex` | AqMapMean/Std/percentiles (built for this), variance spread | OAT survivors (2-3%, 1-2.5%, 2.65% median; `RAV1E_PICKER_PLAN.md:9-30`) but n=117, pre-parity, and global-VAQ measured +2.8% BD-rate *worse* — needs fresh per-image sweep | conditional 1-3% |
| `partition_range` override (`expert.rs` `InternalParams.partition_range` → ravif `override_partition_range` av1encoder.rs:220/800/1260) | Uniformity, EdgeDensity, PatchFraction | OAT `coarse_16_64`: +2.75% bytes, faster → **fast-mode speed knob, not an RD knob** (widening regresses: RD_GAP "RULED OUT 3c") | speed |
| Yuv400 for grayscale | GrayscaleScore ≥ 0.99 | deterministic descriptor-gap rule, no model (README) | 30-40% on B&W |
| metric-tune selection (future) | screen-vs-photo pack (PatchFraction AUC 0.88, EdgeSlopeStdev 0.84) | after tunes exist; `Tune::Psychovisual` already default & worth ~9.5% | future |

Not levers: trellis, global deltaq/VAQ-as-default, CfL toggles, filter_intra (all measured
dead/broken — RD_GAP "REJECTED"/"RULED OUT"). Screen content's real fix is palette-mode
implementation or family routing to another codec, not a knob.

## (B) Per-SB hints

**zenanalyze has no ROI API** (only `mirror_tile_packed` padding for tiny inputs,
`src/lib.rs:570-621`). Running `analyze_features_rgb8` on ~240 64×64 crops/MP repeats per-call
setup (~265 KB allocs, plan build) and degenerates the stripe sampler — **cost now measured
(P0.1, section D): 42-106 µs/call on percentile-free subsets = 10-25 ms/MP (fine), but 799
µs/call if the set includes percentile features (<128 px tile-refill), and ~90% of a 64² call
is per-call overhead** — so a proper **tiled single-pass mode is a zenanalyze v2 additive
feature** worth ~5-10×, not v1.
Meanwhile **zenrav1e already computes per-block features**: `ActivityMask::from_plane`
(per-8×8 luma variance, `activity.rs:23`) and per-block
`spatiotemporal_scores`/`segmentation_scores` (`encoder.rs:811,814,850`) feeding k-means
segmentation with per-segment QP deltas (`segmentation.rs:23,75`) — per-SB deltaq **already
structurally exists** via segmentation + `seg_boost` (`encoder.rs:901`).

| Hint | Hook | Win/risk |
|---|---|---|
| (i) per-SB `partition_range` narrowing (raise `min` on flat SBs; never touch `max`) | `must_split`/`can_split` read `fi.partition_range` at `encoder.rs:3276-3286`; candidate list 3307-3346 | speed only; risk = skipping depths content needed — the *premature-pruning bug class fixed 3× this week* (topdown, tx-type early-exit, SPLIT pessimism). Fast-mode only, conservative thresholds |
| (ii) intra mode-set budget | `num_modes_rdo` 7↔3 at `rdo.rs:1483-1493`; CDF-sort 1507 + SATD prescreen 1550-1585 already order modes | speed; per-SB 7→3 on smooth SBs |
| (iii) tx-type priors | `rdo_tx_type_decision` `rdo.rs:1896` (now exhaustive by design) | low value; re-introduces pruning risk — defer |
| (iv) per-SB deltaq / perceptual importance | blend external importance into `segmentation_scores` before `segmentation_optimize` (`segmentation.rs:23`) | metric-tune track; evaluate on zensim-A/butteraugli, **not** ssim2 (psy-tune already optimizes SSIM-shaped distortion; global deltaq was rejected under ssim2) |
| (v) early NONE/SPLIT forcing | degenerate case of (i) (`min==max`) | fold into (i) |
| (vi) **content-adaptive large-block gate** (NEW 2026-07-02, RD not speed): per-image or per-SB widen `partition_range.max` 16→32/64 only where large blocks help | same `fi.partition_range` hooks as (i), but raising `max` instead of `min` | the prange (4,64) re-test on the fixed estimate split 7/22 winners (−1.8..−2.5%) vs 15/22 losers (`benchmarks/rd_gap_prange_retest_2026-07-02.tsv`) — a global default loses, but the split correlates with content; a hint that predicts "this image/SB benefits from 64-blocks" captures the wins. Global per-image head first (cheap, no per-SB risk), per-SB later |
| (vii) **per-image `split_trial_depth` selection** (NEW 2026-07-02): choose trial-estimate depth 1 vs 2 per image | `split_trial_depth` knob landed at zenrav1e@2fac1af6 | depth-2 rescued the worst p64 outliers in the s1 ablation (o_5004 +12.3→+3.6) and improved 5/5 early p64 images but lost the wins+median rule overall at ~extra cost — i.e. it helps exactly where large-block ranking is hard. Candidate: predict when depth-2 pays (likely the same smooth/large-block-ambiguous content class as (vi) and the 8 s1 losers) |

## (C) Architecture

**STATUS 2026-07-03 — the FrameHints skeleton is REAL (first channel shipped,
release-gated).** zenrav1e@c4047cec landed `pub struct FrameHints { sb_q_scale:
Option<Box<[f32]>> }` (`#[non_exhaustive]`, so the per-SB partition/mode-budget
channels below are additive) carried as `Option<Arc<FrameHints>>` on
`FrameInvariants` via `FrameParameters.frame_hints`, applied through the landed
per-SB delta_q machinery; ravif@13b1ca4b passes it through
`expert::InternalParams.sb_q_scale` (release-gated `FRAME_HINTS_LIVE`); the
first consumer is the P4-track butteraugli diffmap two-pass
(zenavif@2e8e9912, `docs/DIFFMAP_TWO_PASS.md`) — external importance judged on
butteraugli exactly as (B)(iv) prescribed, but through real delta-q instead of
the segmentation blend (the boost work showed segmentation stacking
double-boosts).

- **zenrav1e**: one new optional input, no deps: `pub struct FrameHints {
  sb_partition_range: Option<Box<[PartitionRange]>>, sb_intra_mode_budget: Option<Box<[u8]>>,
  sb_importance: Option<Box<[f32]>> }` (SB raster order), carried as
  `Option<Arc<FrameHints>>` on `FrameInvariants` next to `activity_mask` (`encoder.rs:809`),
  consulted at the three hooks above. Standalone-usable; hints are plain data like
  ActivityMask. *(Shipped shape: `sb_q_scale` first; the other channels remain planned.)*
- **ravif**: pass-through builder, same shape as existing `override_*` plumbing
  (av1encoder.rs:1260). *(Shipped: `InternalParams.sb_q_scale`.)*
- **zenavif**: owns feature extraction + prediction (already depends on
  zenanalyze/zenpredict/zenanalyze-api under `auto-tune`, Cargo.toml:37-43, pinned api rev
  `47b4d0f5`); extends `auto_tune` with new heads and builds `FrameHints`. The Offer-reuse
  path (`auto_tune.rs:271-288`) stays the orchestrator contract.
- **v1 hint producer**: derive per-SB ranges from zenrav1e's *own* ActivityMask (zero
  cross-crate data); zenanalyze tiled mode is the v2 upgrade once its cost is measured.

## (D) Cost model — MEASURED 2026-07-02 (P0.1)

zenbench grid on **real photo crops** (4 distinct crops/size; 64²/256² from
clean-picker-corpus renditions, 1024²/2048² from FiveK photos, 4096² = 2×2 mosaics of
distinct FiveK 2048² crops), `analyze_features` RGB8, AVX2, no `target-cpu=native`.
Full data: `zenanalyze/benchmarks/feature_cost_grid_2026-07-02.tsv` (+ raw zenbench JSON at
`/mnt/v/output/zenanalyze/per_tier_cost_grid_2026-07-02.json`); harness
`zenanalyze/examples/per_tier_cost_grid.rs`; crops builder
`zenanalyze/scripts/make_costgrid_crops.py`. Medians of 4 crops:

| subset (cumulative passes) | 64² | 256² | 1024² (1 MP) | 2048² (4 MP) | 4096² (16.8 MP) |
|---|--:|--:|--:|--:|--:|
| t1 (full Tier-1 kernel) | 42 µs | 824 µs | 5.59 ms | 8.97 ms | 19.0 ms |
| t1+t2 | 43 µs | 837 µs | 5.45 ms | 8.59 ms | 18.9 ms |
| t1+t2+t3 | 106 µs | 2.12 ms | 7.45 ms | 12.4 ms | 28.1 ms |
| t1+t2+t3+palette | 106 µs | 2.04 ms | 7.45 ms | 12.6 ms | 27.4 ms |
| full `SUPPORTED` (97) | **799 µs** | 2.55 ms | 8.21 ms | 14.3 ms | 32.1 ms |

Findings (all measured, workstation 7950X):

1. **Scaling is strongly SUB-LINEAR** — the stripe/block sampling budgets are crate
   invariants, so per-pixel cost falls with size (t1: 10.3 ns/px at 64² → 1.13 ns/px at
   16.8 MP). A linear `total = α + β·px` fit is therefore a poor model shape: fitted over
   the whole grid it gives (α, β) = t1 (2.33 ms, 1.04 ns/px), t1t2 (2.40 ms, 1.02),
   t1t2t3 (3.32 ms, 1.53), t1t2t3_pal (3.30 ms, 1.50), full (3.42 ms, 1.79) — usable
   ONLY ≥1 MP; it over-predicts small sizes by ~50× because the true curve saturates.
   **Use the table (or the per-64² direct numbers) below 1 MP, the fit above.**
2. **The real fixed per-call overhead is the direct 64² measurement**: 42-106 µs
   (subset-dependent) — ~90% of a 64² call is per-call setup (~265 KB allocs + plan
   build + sampler floors), since the same pixels inside a 16.8 MP call cost ~1-1.7 ns/px.
3. **Tile-refill cliff at <128 px (per-SB landmine):** `full SUPPORTED` at 64² costs
   **799 µs — 7.5× t1t2t3** — because the percentile/windowed families
   (`aq_map_p*`, `noise_floor_*`, `quant_survival_*`, `luma_kurtosis`, …) drop below
   `MIN_TILE_DIM = 128` and trigger a mirror-tile to 128² plus a SECOND full analyze
   pass (zenanalyze `src/lib.rs` tiny-input refill). **Per-SB hinting must request a
   percentile-free subset** (t1 → t1t2t3_pal are all safe: 42-106 µs/call).
4. **Per-SB affordability at 64×64 (≈240 SB/MP):** t1 = 10.1 ms/MP, t1t2t3 = 25.4 ms/MP
   of added analysis. vs zenrav1e s2 ≈ 11 s/MP that is 0.09-0.23% — free. Even at the
   fast end (s8 ≈ 0.5-1 s/MP) it is ~1-3% — acceptable for a fast-mode-only hint, but
   the per-call fixed cost dominates (point 2), so the zenanalyze **v2 tiled
   single-pass mode remains the right upgrade** (shares setup across SBs; expected
   ~5-10× cheaper than 240 independent calls).
5. Full-frame analysis stays negligible at every size that matters: 8.2 ms at 1 MP /
   32 ms at 16.8 MP vs multi-second encodes.
6. Prior README-derived numbers ("9.5 ms at 4 MP full-SUPPORTED") were measured on
   different content; real-photo crops cost ~14 ms at 4 MP. Content matters ~±15%
   (crop spread in the TSV); the ranking and orders of magnitude are unchanged.

## (E) The hyperparameter expert (added 2026-07-02, user directive)

**Concept**: for encoder hyperparameters with **no globally-optimal value** — measured
repeatedly this week — a zenanalyze-features → hyperparameter-vector predictor (ZNPR MLP,
the shipped `auto_tune` pattern) picks per-image values. Evidence that "no perfect
solution" is the norm, not the exception:
- variance-boost strength: global winner 1.0, but smooth photos peak at 2.0 (5004_nps
  −15.0% at s=2) and o_6629-class flat gradients regress at ANY global strength
  (`benchmarks/rd_gap_deltaq_2026-07-02.tsv`);
- 64-block gate: 7/22 winners at −1.8..−2.5% vs 15/22 losers (`rd_gap_prange_retest`);
- `split_trial_depth` 2: rescued p64 outliers, lost the global rule;
- `rdo_tx_decision`: −5.7% median in its band at 7.5× time — pure budget question.

**Two products, in priority order:**
1. **Fast mode that needs less brute force** (the primary goal): predict per-image
   {partition_range min/max, intra mode budget, tx-RDO on/off + budget, trial depth,
   boost strength, tune selection} so an s4-s6-class speed approaches s2 RD — the
   predictor REPLACES search rather than adding to it. The P0 cost grid makes the
   analysis side free (≤14 ms at 4 MP full-SUPPORTED vs ~seconds of encode).
2. **Quality mode top-up**: the same heads at s1/s2 capture wins global constants
   provably cannot (the per-image residual vs cpu0-ss2tune).

**Training data is already being produced as a side effect**: every mechanism fit sweep
(strength arms, prange arms, depth arms — per-image × per-arm × per-q rows with ssim2 AND
butteraugli on train26) is exactly a labeled hyperparameter-response surface. Formalize:
accumulate fit-sweep TSVs into a label store (image → arm → RD outcome) instead of
treating them as one-shot artifacts; new fit sweeps append. When a head needs denser
labels, the cell cache + coarse-grid convention makes arm re-runs cheap.

**First size/crop-axis entry in the label store (2026-07-03, WEDGE-FINDER):**
`/mnt/v/output/rd-gap-wedge-2026-07-03/wedge_dataset.parquet` — 2,598 cells over K=16
train-LSD origins × sizes {256,512,1024,2048|native} × c50 quadrants × arms {zr-best,
zr-paletteoff, cpu2, cpu0}, each row carrying a `feature_join` key that joins EXACTLY
(verified 123/123) to its precomputed row in `imazen26_features_2026-06-23.parquet`.
These are the program's first labels below 1024px. Head candidates it directly feeds
(see docs/WEDGE_MAP_2026-07-03.md for the ranked list + correlations): size-conditional
tune/QM curve offset (parity at 256px, −13%→−1.15% median decay), per-image
variance-boost/QM-discount strength for the 9226 smooth-gradient family (+13.7 med,
32% of corpus), a zenanalyze palette gate (ported AA-aware detection stops firing on
ANY downscaled screen content — byte ratio 1.000 at ≤512 vs −12..−39% BD wins at
native), and per-SB/region hints for quadrant wedges (spread up to 61.7 BD points
inside one image; c50 crops of 1480/9908/7052).

**Method discipline** (unchanged from the rest of this plan): interpretable threshold
rules on 2-3 features FIRST, MLP head only where thresholds demonstrably underfit;
train26/LSD split hygiene; butteraugli in the selection rule (veto), not just ssim2;
dense-sampling rules from the sweep discipline when a head graduates to real training;
per-family reporting. Runtime side stays the shipped ZNPR/`auto_tune` machinery — new
heads, not new infrastructure.

**STATUS 2026-07-03 — label store BUILT + first three threshold cuts MEASURED**
(full report: `docs/HYPERPARAM_FIRST_CUT_2026-07-03.md`): the store is
`/mnt/v/output/zenavif/hyperparam-labels-2026-07-03/labels.parquet` (21,096 rows /
62 arms across tune-ss2 + deltaq + qmdist + lfsharp + desyncfix + wedge +
palette-ab + palette-mech; 100% feature-join on train26+wedge+mech26 corpora;
builder + append protocol
`scripts/hyperparam/build_label_store.py`; Tower-mirrored). Verdicts: **palette
gate `patch_fraction > 0.197` → Always is the graduating head** (LOOCV-stable,
val-firing sanity clean, fires where the ported detection is downscale-dead);
**size decay narrowed** — the
1024→512 step is entirely a high-quality-band loss on photo content, top suspect
ss2-QM curves, 768-cell isolation A/B specced; **per-image boost strength NOT
deployable** (LOOCV ≈ global-1.0 at n=24; fam-9226's residual is palette/QM-shaped,
not boost-shaped; needs val + dense-strength labels). MLP heads: not warranted on
any of the three yet — in every case the LABELS underfit before the rule does.

**STATUS 2026-07-04 (TUNER2) — the boost-strength head is CLOSED as a
triple-verified honest negative** (RD_GAP_VS_LIBAOM.md "TUNER2"): the named
val gap was filled (valstr arms, 14 held-out origins × str{0,1,2,3,4.5} × 12q),
and (a) the refit LOOCV still doesn't clear global-1.0, (b) the 2026-07-02
strength labels DRIFTED stale (qmdist+lfsharp subsumed 2-4 BD points of the
boost's marginal; 6018's deep headroom 1.3 → 0.4), (c) the frozen rule
regresses val (vetoadj +0.44 vs −0.03). The deeper-curve variant
(`variance_boost_deep` low-var ramp) never fires on the deep-AQ class (their
8×8 variance is above any sane ceiling) and loses on photos. The one live
derivative — an anti-boost OFF-gate (val document-charts 8103/5343 pay +5.8/
+7.3 at str1) — is blocked by a TRAIN-CORPUS GAP: train26 has no
document-chart member (best train rule fires 0/14 on val). **Data need for
any revival: 3-5 document-chart origins in the next train-corpus revision;
until then the strength dimension carries no head.** Fit scripts:
`scripts/hyperparam/refit_boost_strength_p3.py` + `fit_boost_gate.py`.

**STATUS 2026-07-04 — heads 2+3 SHIPPED (release-gated) + the intra axis
measured: FAST_TIER_PARITY P2 complete.** `src/fast_heads.rs` (auto_tune-wired):
per-image TX budget {Largest|Size1|Min} (razor-edge withhold `pf>0.8505 &&
dcty>100`, VAL-attribution-revised conjunctive form; smooth-content deepen
`pf≤0.8505 && dcty<8.352`) at s6-8 + partition budget {Ship|Max32}
(`gradient_fraction_smooth<0.4105`) at s6. Composed s6 mode: train26 −4.38
med vs s6+size1 base (deviators −5.13 mean vs global-ship), VAL −3.98
(deviators −2.41, worst +0.32); photos-vs-cpu4iq-ai +0.57/−0.94 med — inside
the ±1% parity band. The intra-mode-budget axis measured NOT-a-head (broad
global −0.56/−1.17 med win, no per-image structure; no top-5 knob exists —
`num_modes_rdo` hardcoded 7|3). MLP heads: still nowhere warranted. Full
record `docs/HYPERPARAM_P2_HEADS_2026-07-04.md` +
`benchmarks/rd_gap_p2heads_2026-07-04.tsv`.

**STATUS 2026-07-03 (later) — the palette gate CONFIRMED on val + LANDED as the
first deterministic descriptor head** (the "Yuv400-for-grayscale"-shaped rule
family in table A, now real): mechanism A/B across palette {off,always,auto} ×
sizes {256,512,1024} × configs {isolated rav1e CLI s2+s6, shipped cavif s2+s6}
on the wedge fired/quiet/photo subset + a 14-origin VAL corpus. Shipped s6 val
where detection is dead: rule −10..−39% BD (auto ≈0); s2 val @1024 −3.3..−15.2;
zero val miss&won at s2, false fires ≈0-cost (max +3.46 vetoed), fired-file
time 1.06× median. Threshold 0.197 re-validated (pooled refit = identical fire
set; val-only refit wants ~0.05-0.07 — an s6-only phenomenon → documented
speed-conditional follow-up). Runtime: `src/palette_gate.rs`
(`palette_gate_for_rgb8` = Offer-reuse or one-feature Tier-1 pass, degrades to
Auto), wired into `auto_tune`, forwarded to the encoder **at the zenrav1e dep
bump** (CLAUDE.md checklist). Full record:
`docs/HYPERPARAM_FIRST_CUT_2026-07-03.md` rule-1 status block +
`benchmarks/hyperparam_palette_mech_ab_2026-07-03.tsv`.

## Phases + gates

- **P0 (measure) — DONE 2026-07-02.**
  **(1) Cost grid**: measured, see section (D). 64×64 per-SB analysis is affordable
  (42-106 µs/call on percentile-free subsets; 0.09-0.23% of s2 encode time) with two
  hard constraints: request a percentile-free FeatureSet (else the <128 px tile-refill
  costs 7.5×) and treat the per-call fixed overhead (~90% of a 64² call) as the v2
  tiled-mode motivation.
  **(2) Label drift** (`benchmarks/drift_check_2026-07-02.tsv`, raw at
  `/mnt/v/output/zenavif/drift-2026-07-02/` + Tower; harness
  `examples/drift_reencode.rs` + `scripts/rd_gap/sample_drift_cells.py` +
  `scripts/rd_gap/remote/run_drift.sh`, Hetzner ccx63): 8 images × 6 cells
  (s2/s4/s4-bd10/s6-420/s6-rgb/s8-noqm) × 7 q = 336 cells, two legs.
  *Control leg* (registry zenrav1e 0.1.4): reproduced the canonical dataset
  **336/336 byte-identical, ssim2 exact** — route, planner fingerprints (336/336
  match), decoder, and scorer all validated; every master-leg delta is pure zenrav1e.
  *Master leg* (zenrav1e `64a081d4`): |Δbytes| **p50 = 0.21%** (aggregate gate ≤2%
  PASSES), p90 = 4.50%, max 12.3%; 139/336 byte-identical. **BUT the drift is
  knob-correlated, not noise**: signed Δbytes p50 by cell — s2 **−3.36%** (0/56
  identical), s4 −1.23%, s4-bd10 −1.13%, s6-420 ±0 (45/56 identical), s6-rgb ±0
  (38/56), s8-noqm ±0 (56/56 identical). ssim2 |Δ| p90 = **1.36 pts** (max 5.5,
  mean +0.21 — better quality AND smaller files on slow speeds, as the parity fixes
  intend). encode_ms drifted **everywhere**: master/base p50 ratio 1.36× (s6-420) to
  2.59× (s2) — even byte-identical s8-noqm costs 1.66×.
  **Verdict — P1 go/no-go: SPLIT.** The aggregate ≤2% gate passes, but knob heads
  that must *rank speeds/qm* (and any encode_ms/time-budget LUT) would train on
  labels biased exactly along the ranked axis (s2 bytes shrink 3.4% while s8 doesn't
  move; s2 encode cost 2.6×). → **(a) speed/qm heads + all encode_ms LUTs: re-encode
  labels after the zenrav1e dep bump** (fresh sweep, not retrofit). **(b)
  subsampling/bit-depth/color-model heads: existing labels are usable now** — their
  cells' bytes are stable (420/rgb/bd10-vs-bd8 contrasts drift ≤1.1% and mostly 0),
  so a P1 scoped to those heads can proceed on `2026-07-01-zensimA` labels. No
  retraining was started (per P0 scope).
- **P1 (global, existing data)**: retrain picker on `2026-07-01-zensimA/zenavif_lossy` with
  heads for subsampling/bit-depth/color/qm + budget-gated rdo_tx_decision rule + grayscale
  rule. Gate: held-out test-split bytes at matched zensim-A ≥2% below current picker, no p95
  regression, `scripts/rd_gap/` median stays ≤0%.
- **P2 (global, new labels)**: post-release-bump OAT-style sweep (reuse `sweep.rs` planner +
  fingerprints + `RAV1E_PICKER_PLAN.md` Stage-2 protocol) for
  vaq_strength/seg_boost/segmentation_complex/tune_still; train micro-head only for
  survivors.
- **P3 (per-SB)**: zenrav1e `FrameHints` + ActivityMask-derived partition-min hint, fast-mode
  only. Gate: ≥20% encode-time cut at ≤0.1% median BD-rate delta on the rd_gap harness;
  matched-speed default untouched.
- **P4 (metric-tune deltaq)**: external importance → segmentation blend, judged on
  zensim-A/butteraugli.

**Prime risk, stated plainly**: every per-SB pruning hint is deliberate re-introduction of the
search-truncation family that caused the +5.7% gap — hints must be opt-in (fast mode),
conservatively thresholded, and re-run through `scripts/rd_gap/` on every change.

## Key file:line index for implementers

zenrav1e — `src/encoder.rs:3276` (must_split), `:3285` (can_split), `:3307-3346` (candidates),
`:722` (partition_range), `:809` (activity_mask), `:811/:850/:901` (scores/seg_boost),
`:1139-1143` (tx_mode_select); `src/rdo.rs:1483-1493/:1550-1585` (intra budget/prescreen),
`:1896` (tx-type), `:767` (tx size/type); `src/segmentation.rs:23/:75`; `src/activity.rs:23`;
`src/api/config/speedsettings.rs:115-194`. ravif — `ravif/src/av1encoder.rs:1536-1610`
(SpeedTweaks; rdo_tx gate `speed<=4 && !high_quality` ~:1577; partition table :1546),
`:220/:800/:1260` (override plumbing). zenavif — `src/auto_tune.rs` (whole flow; Offer reuse
:271-288), `src/expert.rs:71+` (InternalParams.partition_range), `src/encoder.rs:531-665`
(with_* builders), `docs/RAV1E_PICKER_PLAN.md`, `docs/RD_GAP_VS_LIBAOM.md`,
`docs/VARIANT_GENERATION.md`. Data —
`/mnt/v/output/canonical-picker-2026-07-01-zensimA/zenavif_lossy/` (train on this, not
06-27), `~/work/zen/DATA_PROVENANCE.md:571-832`. Contract — `zenanalyze/zenanalyze-api/`
(zenavif pins git rev `47b4d0f5`; sibling head has a newer NamedFeature-based surface —
migration is orthogonal to this work).
