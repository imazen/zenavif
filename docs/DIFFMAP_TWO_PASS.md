# Butteraugli-diffmap-guided second pass (spatial closed loop)

**Status 2026-07-03: mechanism SHIPPED (release-gated), A/B measurement in
progress.** Companion to `TUNE_SSIMULACRA2_PLAN.md` (the scalar metric-tune
program — global curves per frame) and `FEATURE_HINTS_PLAN.md` §C/P4 (this is
the first real `FrameHints` consumer). This document is the record of the
evaluate-first verdict, the mechanism, where each piece landed, and the
dep-bump checklist.

The question that spawned it: *"do we use metric diffmap APIs in closed
loop?"* — the answer was no. The butteraugli crate's `with_compute_diffmap`
per-pixel map fed nothing; every landed tune mechanism is open-loop (source
statistics → curves). libaom has had the closed-loop analog for years:
`tune=butteraugli`.

## The libaom mechanism (rev 632172a4, `av1/encoder/tune_butteraugli.c`)

Single encode call, internally two-stage: (1) downscale the source 2×, encode
it at **fixed q96**, (2) butteraugli diffmap between half-res source and that
preliminary recon, (3) pool per 16×16-equivalent block:
`dbutteraugli = (Σ d^12)^(1/12)`, `dmse` = YUV MSE, block weight
`min(dmse/dbutteraugli, 5) + K` (K = 0.4 recode path / 0.3 preliminary path),
geometric-mean normalized, clamped `[0.4, 2.5]`, (4) re-encode the real frame
with per-block **rdmult** (λ) scaled by the weight. Where butteraugli says the
error is more visible than MSE suggests → weight < 1 → lower λ → more bits;
over-served blocks give bits back. The quantizer itself never moves; it is
λ-only.

Two shim quirks worth knowing (not copied): the aom shim feeds libyuv "ARGB"
(B,G,R,A byte order) into libjxl as RGBA — an R/B channel swap applied to
*both* inputs, so the metric still measures a real perceptual difference, just
of channel-swapped pixels; and it requires the long-removed
`JxlButteraugliApi` (libjxl ≤ 0.8).

## Evaluate-first verdict: GO

Per the program discipline (aom constants transferred badly twice; mechanisms
transferred well), the mechanism was measured on aom's own encoder before
porting anything. `CONFIG_TUNE_BUTTERAUGLI=1` needs libjxl ≤ 0.8: built
libjxl **v0.8.2** static (`~/work/libjxl-0.8-for-aom`, prefix assembled with
`libjxl.a/libhwy.a/libskcms.a/libbrotli*-static.a` + `jxl/butteraugli.h`;
`-DCMAKE_POLICY_VERSION_MINIMUM=3.5` for the vendored brotli) and an aom
variant `build_butteraugli` at the pinned rev 632172a with
`-DSTATIC_LINK_JXL=1` + preseeded `LIBJXL_*` cache vars. Both A/B arms ran the
**same binary** (flag = only variable), aom_only.sh, 420, cq {8..63}, BUTTER
columns on.

BD-rate, tune=butteraugli vs default (negative = tune wins), **photos scope**
(committed summary: `benchmarks/aom_tune_butteraugli_eval_2026-07-03.tsv`;
raw: `/mnt/v/output/zenavif/aom-butter-eval-2026-07-03/` + Tower mirror):

| corpus | cpu | butteraugli-3n med | ba-max med | ssim2 med | time |
|---|---|---|---|---|---|
| legacy (19ph) | 2 | **−3.48%** (15/19) | −3.08% | −0.76% | 1.09× |
| legacy | 6 | **−2.66%** (14/19) | −2.53% | −0.56% | 1.43× |
| train26 (20ph) | 2 | **−2.52%** (15/20) | −2.62% | −1.86% | 1.27× |
| train26 | 6 | **−2.40%** (16/20) | −1.53% | −0.81% | 1.51× |

All four cells clear the pre-registered ≥1% butteraugli-BD GO bar, and ssim2
*also improves* everywhere on photos — the spatial loop is not
metric-gaming: it moves both metrics the right way on still images. Port
green-lit.

## Our mechanism (what shipped)

True two-pass, driven zenavif-side (the encoder stays metric-free):

1. **Pass 1**: normal encode at the caller's config (the shipped composed
   config — tune, QM, palette, everything).
2. **Decode** with zenavif's own decoder (rav1d-safe) — the pixels a user
   gets. (The encoder recon now byte-matches decoders after #32-#35, so a
   recon-based variant is a valid future cost optimization; the decode path
   was chosen for v1 because it needs no extra plumbing and validates the
   real output.)
3. **Diffmap**: `butteraugli` crate (OURS), `with_compute_diffmap(true)`,
   full resolution, defaults matching this program's evaluation scoring
   (hf_asymmetry 1.0, intensity 80 nits; aom's 0.8 is exposed as a refit
   knob). ~0.16 s/MP single-threaded — negligible next to the second encode.
4. **Pool per 64×64 superblock** (the delta_q syntax grain): aom's exact
   formula (12-norm pool, `min(mse/ba,5)+K`, geomean-normalize, clamp
   [0.4,2.5]) with RGB MSE, then **translated from the λ domain to a
   quantizer scale**: `q_scale = weight^(strength/2)` (λ ∝ q²; strength 1.0 =
   λ-parity, the `TwoPassOptions::strength` knob steers harder/softer).
5. **Pass 2**: re-encode with the per-SB map applied through zenrav1e's
   **landed per-SB delta_q machinery** (real delta-q syntax d125713f + the
   `(ac_q(base)/ac_q(sb))²` RDO distortion follow) — deliberately *stronger*
   than aom's λ-only scaling: the coded quantizer actually moves per SB, the
   Variance Boost composition point (`Tune::Ssimulacra2`) is respected (the
   scale applies ON TOP of the boosted per-SB qindex), and segmentation is
   disabled when the hints activate delta-q exactly like the boost path (the
   seg+delta-q composed-qindex path is deliberately not exercised until
   validated).

Cost: 1 extra encode + 1 decode + 1 butteraugli ≈ **2.05× single-pass wall**
(measured on the o_1015 smoke: 33.6 s → 67.3 s at 0.79 MP, s2+tune).

## Where each piece landed (all pushed 2026-07-03)

| repo | commit | content |
|---|---|---|
| zenrav1e (`master`) | `c4047cec` | `FrameHints { sb_q_scale }` + `FrameParameters.frame_hints` (queued per input_frameno like t35, keyframe-scoped), `CodedFrameData::apply_sb_q_scale_hints` (composes with Variance Boost, `first_ac_qi_at_or_above` re-quantization, dist-scale follow), delta-q activation in `set_quantizers`. Opt-in contract tested: hints=None / all-neutral / grid-mismatched maps are **byte-identical** to a plain encode. |
| ravif (`main`) | `13b1ca4b` | `expert::InternalParams.sb_q_scale` → `Encoder::override_sb_q_scale` → `Av1EncodeConfig.frame_hints_sb_q_scale` (color encode only; alpha never gets the map) → release-gated handoff in `encode_to_av1`. `pub const FRAME_HINTS_LIVE = false` until the zenrav1e dep bump (the hinted send is commented so the crate compiles against registry 0.1.4). |
| zenavif (`main`) | `2e8e9912` | feature `two-pass-butteraugli` (pulls `ravif/__expert` internally), `two_pass::encode_rgb8_two_pass` + `TwoPassOptions` + `TwoPassEncode`, the pooling port with unit tests, `FRAME_HINTS_LIVE` re-export, contract test `tests/two_pass.rs` that asserts the LIVE behavior when the passthrough is live and the **honest error** when release-gated (no silent double-encode, no silent skip). |

Measurement wiring (THROWAWAY dev-patches, never land, precedent `92dadd7`):
`ravif--diffmap` workspace commit `76005e7f` (zenrav1e path-dep →
`zenrav1e--diffmap`, `FRAME_HINTS_LIVE=true`, hinted send uncommented,
`ZENRAVIF_TUNE` env passthrough) + `zenavif--diffmap` throwaway (ravif
path-dep → `ravif--diffmap`). Harness: `examples/two_pass_cell.rs` (landable)
+ `scripts/rd_gap/zenavif_2p_cell.sh` + `run_2p_ab.sh` (landable; same pool +
cache discipline as `aom_only.sh`; cache keys hash the cell binary content +
mode/strength/tune, so arms and rebuilds never collide).

End-to-end validation at ship time: the live-path contract test drives
zenavif → ravif → zenrav1e → per-SB delta_q coded → pass-2 AVIF **differs
from single-pass and decodes cleanly through rav1d-safe**; 179/179 zenrav1e
lib tests, 37+5 ravif tests, 67 zenavif lib tests + 3 pooling unit tests
green.

## A/B protocol + interim verdict (2026-07-03, arms still running)

{train26, legacy} × {single, twopass}, s2 + `ZENRAVIF_TUNE=ssimulacra2` base
(the shipped composed config), 12-pt Q grid {30..95}, 8-bit, single-threaded
cells × 10-way pool. **butteraugli-BD is the target, ssim2 is the veto**
(roles inverted vs the tune-ss2 program). zensim third-opinion: NOT measured
(the harness cells score ssim2 + butteraugli only; noted honestly).
o_9051-class check pending the final legacy data — with the prior that
libaom's own loop cracks that class on *their* base (o_9051 −5.92% ba3n,
o_3003 −3.84, o_5004 −3.71, o_6632 −3.48, o_9077 −2.75; only o_6629 +2.38 —
from the evaluate-first per-image data).

**o_9051-class outcome (legacy complete, str1.0): INVERTED from aom — the
loop makes every outlier WORSE.** o_6632 +10.42% ba3n, o_9077 +9.92,
o_9051 +8.46, o_6629 +6.13, o_3008 +3.52, o_5004 +1.26, o_3003 +0.81 (7/7
regress; several are the corpus-worst cells). On aom's base the same
mechanism's biggest wins were these images (o_9051 −5.92%): their default
allocator under-serves smooth/gradient content, the map flags it, the boost
fixes it. zenrav1e's base (Variance Boost + activity masking + psy tune)
already over-serves exactly those superblocks — the butteraugli map flags
the same banding-prone flats and composes a second boost on top (o_6629's
known boost over-allocation class). Full legacy str1.0: ba3n **+3.65%**
median (5/22), ba-max +5.21%, ssim2 +1.70%, 1.99×.

**INTERIM VERDICT — the direct aom-formula port at λ-parity REGRESSES on the
tuned base.** train26 (24 imgs, complete pair): butteraugli-3n median
**+2.20%** (3/24 better), ba-max +3.25%, ssim2 +1.88% (5/24), 2.01× time.
Legacy partial (n=14 paired): ba3n +2.20% (3/14), same shape. Mechanism
autopsy (per-q paired deltas): pass-2 files are uniformly **~6% smaller**
with quality dropping — after geomean normalization the give-back side
(weight > 1) dominates on this base and its RD trade is negative; at q95
the shape approaches break-even (−5% bytes at ~0 quality delta), at low q it
over-steers. Strength 0.5 (partial n=17): ba3n **+0.93%** (6/17) — halving
strength halves the damage but does not cross zero; the knob is a shrinkage
factor on a correction that mis-fires directionally for enough superblocks.
Worst family: 9226 smooth gradients (+7.84% at str1.0) — exactly where the
tune's Variance Boost already acts, implicating base-correction conflict
(the same double-allocation class as the measured segmentation+boost
stacking regression).

Why the evaluate-first GO did not transfer: that measurement was aom's tune
vs **aom's untuned default** — a base with none of zenrav1e's landed
perceptual allocation (activity-masked cdef-dist, Variance Boost delta-q,
ss2 QM curves, chroma delta-q). Our base has already spent most of the
headroom the diffmap can see; correcting the residual at aom's strength
over-corrects. The mechanisms-not-constants lesson, again — this time the
*operating point* didn't transfer either.

Diagnosis arms in flight (each isolates one hypothesis):
- `tune-off base × {single, twopass}` — **MEASURED: still regresses.**
  ba3n +1.57% median (8/24 better), ssim2 +1.45%, 1.66× — smaller than the
  tuned-base loss (+2.20%) but same sign WITHOUT the Variance Boost / ss2
  tune in the base. Base-conflict is a contributor, not the cause: even
  zenrav1e's *default* allocation (Tune::Psychovisual + activity-masked
  cdef-dist) already carries most of the masking signal butteraugli would
  add — there is no aom-default-like "unmasked" base in zenrav1e to
  correct.
- `boost-only clamp (weight_hi=1.0)` — no give-back side (the bytes autopsy
  fingered it); the Variance-Boost-shaped one-directional variant.
- `probe_quality=40` — libaom's fixed-quality preliminary pass (their q96
  trick): a content-intrinsic degradation signal instead of the
  self-referential real-q residual, and a cheaper pass 1.
- Conformance (both samplings) at the shipped knobs.

Final tables land in `benchmarks/rd_gap_twopass_2026-07-03.tsv` + this
section when the arms complete.

## Dep-bump checklist additions (zenrav1e > 0.1.4)

1. ravif: flip `FRAME_HINTS_LIVE` to `true`, swap the plain send for the
   commented hinted send in `encode_to_av1`, bump the zenrav1e dep. (The
   THROWAWAY dev-patch in `ravif--diffmap` is the exact shape.)
2. zenavif: `two-pass-butteraugli` becomes functional automatically (the
   driver checks the const); `tests/two_pass.rs` flips to the live branch on
   its own. Re-run it.
3. Re-run the A/B (`scripts/rd_gap/run_2p_ab.sh`) on the bumped registry
   chain before advertising the feature.
4. Decide default strength from the fit (see benchmarks TSV when landed).

## Known limitations / follow-ups

- RGB8 driver only (v1). RGBA8 (alpha-aware pooling question: the map is
  color-only by design) and RGB16/10-bit follow the `target_quality.rs`
  pattern once the win is confirmed through this driver.
- The per-8×8 rdmult channel (aom's exact grain) is NOT wired: for stills,
  `distortion_scale()` reads only the per-SB channel; a finer
  `block_dist_scale` hint would need that function to consult the per-8×8
  maps for still frames too (measured decision deferred until the per-SB
  A/B says the mechanism pays).
- Animation: hints are keyframe-scoped by design; inter frames never inherit.
- `Cargo.lock` on zenavif main intentionally lacks the `butteraugli` entries
  until the dep bump (the landable commit adds the dep; the lock regenerates
  on first build/CI).
