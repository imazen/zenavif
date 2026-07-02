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
   now has 5 RD fixes (through `b073182c`, parity −0.65% median). Knob→bytes labels drifted
   1-5%; Phase 0 must quantify before training knob heads.

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
setup (~265 KB allocs, plan build) and degenerates the stripe sampler — cost unmeasured,
assume bad; a proper **tiled single-pass mode is a zenanalyze v2 additive feature**, not v1.
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

## (C) Architecture

- **zenrav1e**: one new optional input, no deps: `pub struct FrameHints {
  sb_partition_range: Option<Box<[PartitionRange]>>, sb_intra_mode_budget: Option<Box<[u8]>>,
  sb_importance: Option<Box<[f32]>> }` (SB raster order), carried as
  `Option<Arc<FrameHints>>` on `FrameInvariants` next to `activity_mask` (`encoder.rs:809`),
  consulted at the three hooks above. Standalone-usable; hints are plain data like
  ActivityMask.
- **ravif**: pass-through builder, same shape as existing `override_*` plumbing
  (av1encoder.rs:1260).
- **zenavif**: owns feature extraction + prediction (already depends on
  zenanalyze/zenpredict/zenanalyze-api under `auto-tune`, Cargo.toml:37-43, pinned api rev
  `47b4d0f5`); extends `auto_tune` with new heads and builds `FrameHints`. The Offer-reuse
  path (`auto_tune.rs:271-288`) stays the orchestrator contract.
- **v1 hint producer**: derive per-SB ranges from zenrav1e's *own* ActivityMask (zero
  cross-crate data); zenanalyze tiled mode is the v2 upgrade once its cost is measured.

## (D) Cost model

Measured: full-SUPPORTED zenanalyze = **9.5 ms at 4 MP** RGB8 (AVX2, no native; README
"Performance"); tier costs at 4 MP: T1 ≈1 ms, T2 ≈2 ms, T3 ≈3 ms, Palette ≈1 ms. zenrav1e s2
≈ **0.088 Mpx/s ≈ 11 s/MP** (RD_GAP); even s8 is ≫100× the analysis cost. Analysis is free at
any speed tier; no 1 MP/64×64 numbers exist — P0 measures them (zenbench, 4-size ×
tier-subset grid per the sweep discipline).

## Phases + gates

- **P0 (measure)**: (1) zenbench zenanalyze cost grid incl. 64×64; (2) drift check: re-encode
  22-image × 7q × 8-cell sample on post-parity zenrav1e vs canonical rows — if |Δbytes|
  median >2%, knob heads need fresh labels.
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
