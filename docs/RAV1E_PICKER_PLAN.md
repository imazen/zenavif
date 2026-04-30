# zenavif knob predictor — master plan

**Status:** drafted 2026-04-30. v0.1 baked + integrated 2026-04-30.
Phase 2 OAT complete 2026-04-30 — 8 of 16 candidate knobs survived the
cull threshold. Phase 3 nightly LHS sweep installed via cron; rotates
through 64 LHS-sampled v0.2 tuples (one per night, full coverage in ~9
weeks). Sunday 06:00 retrain consumes the accumulated v0.2 TSV.

## Phase 2 OAT outcome (2026-04-30)

117 (image, size) cells tested at speed=4, q=60. Cull rule: median
|Δ% bytes| < 0.5 % AND p90 < 1.5 %.

**Survivors (8):**
- `qm` — +4.2 % bytes when off (median); promotes to CATEGORICAL_AXES
- `partition_range coarse_16_64` — +2.75 % bytes / -0.27 zensim, faster
- `rdo_tx_decision off` — +2.6 % bytes saves ~1 s encode time
- `vaq_strength` (non-1.0) — 2-3 % savings at strength 2-3
- `seg_boost` (1.5-2.0) — 1-2.5 % savings
- `segmentation_complex on` — 2.65 % savings (median)
- `encode_bottomup on` — 0.48 % savings, but p90 high → keep
- `lrf on` — +0.63 % bytes, +0.082 zensim — small but consistent

**Culled (8):**
`cdef`, `complex_prediction_modes`, `fast_deblock`, `lru_on_skip`,
`sgr_full`, `trellis`, `tune_still`, `vaq` (at default strength 1.0;
non-default strengths surface via the `vaq_strength` scalar).

`complex_prediction_modes=on` correctly culled: it would save 0.09 %
bytes but loses 32.7 zensim points — confirmed broken (zenrav1e#5).

## Goal

Train a hybrid-heads MLP (ZNPR v2 binary, loadable via zenpredict) that maps

  `(zenanalyze features, image dimensions, target_zensim)` → `(speed, quality, qm, vaq, vaq_strength, tune_still_image, color_model, alpha_color_mode, [internal speed-knob overrides])`

replacing brute-force search at higher speed levels. Drop into `zenavif::EncoderConfig::auto_tune(target_zensim)` behind an `auto-tune` cargo feature.

## Search space (verified against ravif/src/av1encoder.rs)

### User-visible (direct ravif builders)
| Knob | Domain | Notes |
|---|---|---|
| `quality` | f32, 1..100 | Continuous |
| `speed` | u8, 0..10 | Sets ~16 internal knobs via `SpeedSettings::from_preset` |
| `alpha_quality` | Option<f32> | Independent from color quality |
| `qm` | bool | `with_qm` — ~10 % BD-rate win on most images |
| `vaq` + `vaq_strength` | bool + f64 (0..4) | `with_vaq(enable, strength)` |
| `tune_still_image` | bool | Tune::StillImage vs Tune::Psychovisual |
| `seg_boost` | f64 | `with_seg_boost` — segment QP amplification |
| `trellis` | bool | `with_trellis` — currently always-on by default |
| `color_model` | enum {YCbCr, RGB} | RGB only useful for lossless |
| `alpha_color_mode` | enum {UnassociatedClean, UnassociatedDirty, Premultiplied} | Semantics-driven, not perf |
| `chroma_subsampling` | enum {444, 422, 420, 400} | `with_chroma_subsampling` |

### Internal speed-knob overrides exposed by ravif as `Option<bool>`
`Some(b)` overrides the speed-preset default; `None` keeps preset behavior.

| Knob | ravif builder | Speed-preset baseline (s=0..10) |
|---|---|---|
| `cdef` | `with_cdef(Option<bool>)` | always true |
| `rdo_tx_decision` | `with_rdo_tx_decision` | true for s<6, false for s≥6 |
| `sgr_full` | `with_sgr_full` | true for s<5 (Full), false for s≥5 (Reduced) |
| `lru_on_skip` | `with_lru_on_skip` | true for s=0, false for s≥1 |
| `segmentation_complex` | `with_segmentation_complex` | true for s=0, false for s≥1 |
| `encode_bottomup` | `with_encode_bottomup` | true for s<4, false for s≥4 |
| `trellis` | `with_trellis(bool)` | (always-on; no Option) |

### Internal knobs **NOT** plumbed through ravif (out-of-scope for v1)
`partition_range`, `prediction_modes`, `motion.*`, `multiref`, `scene_detection_mode`, `lrf`, `fast_deblock`, `rdo_lookahead_frames`. Adding plumbing would require ravif + zenrav1e API extensions. **Defer to v2.**

### Total v1 search-space cardinality

Naive grid:

  11 (speed) × 100 (quality bins) × 2 (qm) × 2 (vaq) × 5 (vaq_strength) × 2 (tune_still) × 2 (chroma_subsampling) × 2³ (3 surviving Option<bool> overrides post-Phase-2 cull) = **563,200 configs per (image, size)**.

At 200 sources × 4 sizes × 0.5 s/encode median = **6.25 years of CPU**. Untrainable.

## Culling strategy (5 stages)

### Stage 0 — decompose, don't cross-product

Two predictors, not one joint output:

- **Macro head** (categorical-cell + per-cell scalar regression, hybrid-heads style):
  - cells: `(speed_bucket ∈ {0,1,2,3,4,5,6,7,8,9,10}) × (chroma_subsampling ∈ {420, 444})`
  - per-cell scalars: `quality`, `vaq_strength`, `seg_boost`
  - per-cell categoricals: `qm`, `vaq`, `tune_still`
- **Micro head** (conditional on macro): which of the surviving 3–5 internal `Option<bool>` overrides to flip from preset.

Decoupling cuts joint cardinality from 10⁵ × 2³ to 10⁴ + 2³ = additive, not multiplicative.

### Stage 1 — coarse Pareto baseline (quick, narrow)

Goal: establish per-(speed, image-size, content-class) Pareto frontier on quality axis only. Find which q ranges are interesting — i.e. where (zensim, bytes) curves are non-degenerate.

- Corpus: **50 sources** stratified across photo (60 %) / screen (20 %) / illustration (15 %) / synthetic (5 %), drawn from `~/work/codec-corpus/picker-train/manifest.tsv` if those classes are tagged, else hand-pick from CID22 + clic2025 + screen-corpus + line-art-corpus.
- Sizes: tiny (64×64 area-equiv), small (256×256), medium (1024×1024), large (4096×4096) — resize via Mitchell-Netravali, **skip upscaling**.
- Axes: 11 speeds × 21 q-points (q=5..100 step 5, plus q=2,3) × all-other-knobs-default.
- Output: `benchmarks/rav1e_phase1a_<DATE>.tsv` with `(image, size_bucket, w, h, content_class, speed, q, bytes, zensim, encode_ms)`.
- Budget: 50 × 4 × 11 × 21 = **46,200 encodes**. ~6 h on the 7950X.

**Decision gate:** if any (size_bucket, content_class) shows trivial speed-curve (i.e. all speeds collapse to same Pareto), drop that bucket from later phases.

### Stage 2 — internal-knob sensitivity (one-at-a-time)

Goal: cull the 7 ravif `Option<bool>` overrides + `seg_boost` to the ones that actually move BD-rate.

- For each (image, size) cell from a 30-sample subset of Phase-1a images:
  - Pick the speed-N baseline config at the per-image median Pareto-knee q.
  - Encode 8 perturbations: each of 7 booleans flipped + `seg_boost ∈ {1.0 default, 1.5, 2.0}` swept (3 values).
- Compute ΔBD-rate vs baseline using the 21-q frontier from Phase 1a.
- **Cull rule:** drop knob if median |Δ BD-rate| < 0.5 % AND p90 |ΔBD-rate| < 1.5 %.
- Budget: 30 × 4 × (7 + 2) × 21 = **22,680 encodes**. ~3 h.

Expected survivors based on prior knob-eval work in `coefficient/docs/ZENJPEG_KNOB_SURFACE.md`: probably `qm`, `vaq`, `tune_still`, `encode_bottomup`, maybe `rdo_tx_decision`. Others likely cull.

### Stage 3 — macro-knob discrimination

Goal: identify q ranges and content classes where the surviving macro knobs (`qm`, `vaq`, `vaq_strength`, `tune_still`, `chroma_subsampling`) discriminate.

- For Phase-1a 50-image corpus, encode at every cross-product of *surviving* macro knobs at *every* q × *every* speed.
- Surviving knob count post-Stage-2 likely 4–5 macros + 2–3 internals.
- Budget: 50 × 4 × 11 × 21 × ~16 (macro combos) = **~740 k encodes**. **Too many.**

Smarter: **Latin Hypercube** sample 256 macro-knob tuples uniformly across the joint space. Run LHS at every (image, size, speed, q):

- 50 × 4 × 11 × 21 × 256 LHS = ~12 M encodes — still too many.

Even smarter: **stratified-by-q Latin Hypercube**, only at 7 representative q values (q ∈ {10, 25, 50, 65, 80, 90, 95}) and 4 speeds (s ∈ {0, 4, 7, 10}):

- 50 × 4 × 4 × 7 × 256 = **1.4 M encodes** ≈ 80 h. Still too many.

Final budget: **drop LHS to 64 samples**, **drop speeds to 3 (s ∈ {0, 6, 10})**, full q grid (21), full image set (50, all 4 sizes):

- 50 × 4 × 3 × 21 × 64 = **~800 k encodes** ≈ 50 h.

This is borderline. Plan: kick off as a 2–3 day background sweep on the 16-core box.

### Stage 4 — full-corpus joint sweep on best-tuple region

Goal: scale up to 200 sources × 4 sizes for the **converged** best-tuple region from Stage 3 (the cells that tend to win on photo/screen/lineart respectively).

- Budget: 200 × 4 × 11 (speeds full) × 21 q × ~8 surviving config tuples = **~1.5 M encodes** ≈ 4 days.
- Run in background continuously, append-mode to `benchmarks/rav1e_phase4_<DATE>.tsv`.

### Stage 5 — feature extraction + train + bake

- Extract full ~100-feature zenanalyze vector for all 200 sources × 4 sizes via zenanalyze CLI.
- Write `zenavif/training/rav1e_picker_config.py` (codec config module).
- Run `zenpicker/tools/train_hybrid.py --codec-config rav1e_picker_config --hidden 192,192,192` (or 256,256,256 if validation supports).
- Run `feature_ablation.py` + `feature_group_ablation.py` to cull from ~100 → 25–35 load-bearing features.
- Retrain on culled feature set; re-validate against held-out 20 % of corpus.
- Bake → ZNPR v2 (f16) via `bake_picker.py`.
- Pass safety gates: `bake_roundtrip_check.py`, `adversarial_probe.py`, `size_invariance_probe.py`.

### Stage 6 — integrate

- Add `auto-tune` cargo feature.
- Add 7 missing builder methods to `zenavif::EncoderConfig`: `with_cdef`, `with_rdo_tx_decision`, `with_sgr_full`, `with_lru_on_skip`, `with_segmentation_complex`, `with_encode_bottomup`, `with_seg_boost`.
- New API: `EncoderConfig::auto_tune(target_zensim: f32, image: ImgRef<...>) -> EncoderConfig` — runs zenanalyze, predicts, applies.
- ZNPR blob lives at `zenavif/src/models/rav1e_picker_v1.bin` via `include_bytes!`.

## Output schema (v1 hybrid heads)

Inputs (post-feature-ablation, ~30 features):
- ~25 zenanalyze features (TBD by ablation; likely `variance`, `edge_density`, `chroma_complexity`, `uniformity`, `colourfulness`, `laplacian_variance`, `dct_compressibility_y/uv`, `screen_content_likelihood`, `text_likelihood`, `line_art_score`, `noise_floor_y/uv`, `aq_map_mean/std`, `gradient_fraction`, `patch_fraction`, `skin_tone_fraction`, `hdr_present`, `effective_bit_depth`, `is_grayscale`, `alpha_present`, `alpha_used_fraction`, `alpha_bimodal_score`)
- `log_pixels`, `log_min_dim`, `log_max_dim`, `log_aspect_abs`
- `target_zensim_norm` (target zensim ∈ [0,1])
- Engineered: `target_zensim²`, `log_pixels × feat[i]` for top features

Outputs (per cell):
- `bytes` (regression, log domain)
- `quality` (regression, [0, 100])
- `vaq_strength` (regression, [0, 4])
- `seg_boost` (regression, [1, 3])
- `qm` (binary)
- `vaq` (binary)
- `tune_still` (binary)
- micro-head: per-internal-override binary (only for surviving knobs)

Cells: 11 speed × 2 chroma_subsampling = 22 cells. (Or post-Stage-3 cull, fewer.)

## Compute budget summary

| Stage | Encodes | Wall-clock |
|---|---|---|
| 1 baseline | 46 k | ~6 h |
| 2 sensitivity | 23 k | ~3 h |
| 3 macro discrimination | 800 k | ~50 h |
| 4 full-corpus joint | 1.5 M | ~96 h |
| **Subtotal encodes** | **2.4 M** | **~155 h ≈ 6.5 days** |
| 5 features + train + bake | — | ~2 h |
| 6 integration | — | ~3 h |

This is large but tractable. Phases 1+2 (~10 h compute) deliver the cull decision; phases 3+4 are the bulk and are append-only and resumable.

## Open questions / decisions to confirm

1. **Scope: v1 = ravif's already-plumbed knobs only?** (Defer partition_range, prediction_modes, etc. to v2.) — Recommended.
2. **Target metric: zensim** (matching existing zenpicker pipeline) — yes.
3. **Corpus: codec-corpus/picker-train manifest** as the 200-source set, with explicit content-class tags — needs verification.
4. **Compute box: 7950X local** vs distributed — local for now; results commit to `benchmarks/`.
5. **Output: scheduled bake on git push trigger?** — manual for v1, automated later.

## Rollout plan

1. Land plan (this doc) + sweep harness extension as one PR.
2. Run Phase 1+2 in background, summarize results in chat after ~12 h.
3. Decide Phase 3 budget based on Phase 1+2 findings (may be able to cut further).
4. Run Phase 3+4 in background, ~5–7 days.
5. Phase 5+6 in one session.

Total elapsed: **~10 days** of mostly-unattended compute + 2–3 active sessions.

## Alternative if budget too tight

If 6.5-day compute budget is infeasible, drop Stage 4 to **80 sources × 4 sizes × 6 speeds × 11 q × 4 surviving tuples = ~85 k encodes** (~6 h) at the cost of held-out validation accuracy. Stage 1+2+truncated-Stage-4 path delivers a v1 model in ~24 h compute at ~3–5 % BD-rate degradation vs full-budget version.

## v0.1 limitations (post-bake snapshot, 2026-04-30)

The first bake landed with these safety-gate violations, all bypassed via
`ALLOW_UNSAFE=1` on the bake step. They have known root causes that the
cron-driven backfill + Phase 2/3 sweeps will close:

| Gate | v0.1 value | Threshold | Why | Resolves when |
|---|---|---|---|---|
| OVERFIT | train→val gap +5.57 pp (2.75 % → 8.32 %) | 2.0 pp | 50-image corpus, model fits training data | corpus grows past ~150 images via cron |
| LOW_ARGMIN | val argmin_acc 15.7 % | 30 % | Only `speed` varies in Phase 1a, so many cells are equivalent at high q (encoder genuinely doesn't differentiate, picker can't either) | Phase 2 adds qm/vaq/tune_still macro-knob axes; argmin gains real choices |
| PER_ZQ_TAIL | zq=88 p99 overhead 97.4 % | 80 % | Tail dominated by handful of (image, size) cells where the picker happens to mis-rank speed at a specific target | More images smooths the tail |
| DATA_STARVED_SIZE | 84/120 (size_class, zq) cells have <50 train rows; tiny size class is worst (1–2 rows per zq) | 50 | 50-image corpus thin on 64×64 variants, since most CID22 sources are 512px (downscale to 64+256+512 only) | Cron pulls in larger sources from clic2025-1024 + gb82-sc + kadid10k; tiny variants accumulate |

Today's bake is therefore a **runtime smoke** — it lights up the
`auto_tune` API path so callers can integrate against it, but its
predictions are not yet better than picking `speed=4` blindly. The cron
+ Phase 2/3 backfill is the path to real accuracy.

### Acceptance criteria for v0.2

- [ ] OVERFIT < 2.0 pp on the latest cron snapshot.
- [ ] LOW_ARGMIN: val argmin_acc ≥ 30 %.
- [ ] PER_ZQ_TAIL: zq p99 ≤ 60 %.
- [ ] DATA_STARVED_SIZE: < 5 % of (size_class, zq) cells under 50 rows.
- [ ] Phase 2 OAT-confirmed knobs added to CATEGORICAL_AXES /
      SCALAR_AXES — picker sees real choices.
- [ ] No more `ALLOW_UNSAFE=1` on the bake.
