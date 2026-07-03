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

## A/B protocol (in progress)

{train26, legacy} × {single, twopass}, s2 + `ZENRAVIF_TUNE=ssimulacra2` base
(the shipped composed config), 12-pt Q grid {30..95}, 8-bit, single-threaded
cells × 6-way pool. **butteraugli-BD is the target, ssim2 is the veto**
(roles inverted vs the tune-ss2 program): a step that wins butteraugli but
regresses ssim2 beyond noise gets investigated, both metrics reported
per-family. zensim as a third opinion where cheap. o_9051-class check: does
the spatial loop crack the per-image outliers the scalar loop can't
(o_3003/o_3008/o_5004/o_6629/o_6632/o_9051/o_9077)? Results land in
`benchmarks/` + this doc when the sweep completes.

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
