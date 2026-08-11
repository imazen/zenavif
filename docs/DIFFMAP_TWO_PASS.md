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
- `boost-only clamp (weight_hi=1.0)` — **MEASURED: BREAK-EVEN, the
  give-back side WAS the poison.** ba3n +0.10% median (10/24 better),
  ba-max **−0.04% (12/24 — the first zero-crossing of the program)**,
  ssim2 +0.19%, 1.94×. Removing the coarsen-the-over-served half (the bytes
  autopsy's suspect) recovers the entire +2.20% regression. Follow-up arms
  queued: boost-only at strength 1.5 / 2.0 (is there a win past
  break-even?).
- `probe_quality=40` — **MEASURED: still regresses** (ba3n +0.75% median,
  3/24 better, ba-max +0.96%, ssim2 +0.38%, 2.17×). libaom's fixed-quality
  probe signal does NOT rescue the symmetric formula — consistent with the
  give-back side, not the signal shape, being the poison.
- Conformance (both samplings) at the shipped knobs — **MEASURED
  (partial): 180/180 OK, exactly 90/90 per sampling (420 + 444), 18/22
  legacy images** — zero aomdec rejections, zero aomdec/rav1d-safe raw
  mismatches. The remaining 4 images' cells never ran: their workers hit
  the futex hang below. Re-run queued with the per-cell timeout guard.

### Known bug: rare in-process futex hang (v2 cell binary) — ROOT-CAUSED + FIXED upstream 2026-07-03, release-gated

4/220 conformance cells hung 76-90 min in `futex_` (kernel wait, no
progress) under 10-way process parallelism. Root cause (zenavif#30, closed;
full forensics there): NOT butteraugli/rayon — a **rav1d-safe tile-worker
panic** (`overlapping DisjointMut`, `picture.rs` via CDEF padding vs the
loop filter's tile-threading compact-COW guards) killed one worker, whose
claimed task could then never complete, so `rav1d_decode_frame`'s
completion wait blocked forever (all threads parked → 0 CPU, `futex_`).
The four hung cells were exactly the first-420 cells of the four largest
images; the panic messages sat unread in their `enc.log`s the whole time
(`/tmp/tp_conf.3354053`, preserved in the issue). Both halves are fixed in
rav1d-safe@49df1fc0 (guards narrowed to dav1d's exact read/write sets +
worker panics now fail the decode with an error in ms instead of wedging),
with regression tests + the committed trigger vector upstream. zenavif's
`examples/hang_stress.rs` is the product-path stress loop that found it
(613/613 full-stack cells clean on the patched chain, 14-way, hot-cell
weighted; plus 9,600 clean decode-stress iterations upstream). **Release-gated:**
registry builds ship rav1d-safe 0.5.7 (panic+wedge behavior) until the
rav1d-safe release past 0.5.7 and the zenavif dep bump; the harness
`timeout 600` per cell stays as belt-and-suspenders until then.

Final tables land in `benchmarks/rd_gap_twopass_2026-07-03.tsv` + this
section when the arms complete.

### Measurement integrity — incident log + reconciliation

Three transient failure bursts hit the arms; every one is recorded, root-
caused, and the final data is complete (failures TSVs preserved alongside
the raw arm TSVs in the /mnt/v archive):

1. **drvfs EIO burst** (17:14Z, legacy-single, 10 cells): the documented
   `/mnt/v` WSL-memory-reclaim stall class (see `run_gap.sh` WORK note) —
   source-image opens EIO'd. Gap-filled via RESUME; final 264/264.
2. **Harness reaped the backgrounded driver** (~17:41Z, mid legacy-twopass,
   arm died at 169/264) and the failover chain deadlocked on a
   self-matching `pgrep -f` (the watch pattern appeared in the monitor's own
   cmdline) — ~1h stall until a sentinel broke it. Re-run via RESUME; final
   264/264. Hardening landed: stall watchdog keyed on actual cell-process
   count + TSV growth, RESUME in the runner (`322ece36`).
3. **Live-edit offset corruption** (18:53Z, str0.5, 10 cells): an in-place
   rewrite of the cell script corrupted in-flight bash readers (bash reads
   scripts incrementally by offset). Gap-filled; final 288/288. Discipline
   adopted: live-harness files are only replaced atomically (tmp + `mv` —
   running readers keep the old inode); the cache-disabled key-derivation
   noise that buried the first burst's diagnosis is also fixed
   (`de765183`).

Timing note: enc_ms/time_ratio for gap-filled arms mixes fresh and
re-encoded cells (same binary, same box, no row-cache replays) — ratios
remain paired and honest.

## THE DEP BUMP HAPPENED — 2026-08-06, ravif 619d81a

Items 1 and 2 below are **done**: ravif moved its zenrav1e dep to sibling
master and `FRAME_HINTS_LIVE` is `true`, so `zenavif::SPATIAL_HINTS_LIVE`
flipped with no zenavif edit and a supplied per-superblock map genuinely
changes the bitstream. Item 4 (choose a default strength) was then answered
by measurement, and the answer was **not the shipped default**.

**Read `src/two_pass_zensim.rs`'s two-shot section and
`benchmarks/zensim_hint_probe_2026-08-06.{tsv.zst,summary.txt,meta}` before
touching this channel.** 54 cells / 1,578 encodes on one pinned build.
Headlines:

* **Activating delta-q is not a small perturbation.** It also disables
  segmentation. At an *unchanged* quantizer, merely switching the channel on
  (every superblock's delta quantizing to zero) moves zensim by a median
  **+1.10** (p90 |Δ| 4.49, range −2.34..+7.76) and bytes by +2.9% median /
  +21% max. That exceeds the median achievable-score gap (0.72) on **68.5%**
  of cells.
* **Per-superblock deltas are coded at a RESOLUTION of 1/2/4/8 quantizer
  indices**, keyed on the frame base quantizer (zenrav1e
  `variance_boost_delta_q_res_log2`). A scale implying a smaller move
  quantizes to zero: the map still activates delta-q but nothing moves, so
  every such map produces an identical encode and the channel reads as if it
  ignores map content. A 1.5% scale is below the resolution at ~76% of
  indices. **This cost a whole sweep.** Size scales against the resolution,
  and since a wrong-length map is *silently ignored*, always confirm bytes
  actually changed before believing a null here.
* **RD at matched bytes**, diffmap-derived map vs the un-hinted curve:
  strength 0.5 → **+0.48** zensim median (wins 8/12), 1.0 → **−0.28**
  (4/11), 2.0 → **−3.72** (0/7). The derived `strength = 1.0`, chosen while
  the map was inert and unfittable, is measurably too strong.
* Consequently `ZensimLoopOptions::spatial_strength` now defaults to **0.0**
  (zenavif `fix(loop)`, 2026-08-06). That restores the behaviour every prior
  measurement of that loop was actually taken under — the map was inert when
  they were taken. It also fixed a red in-tree test
  (`low_target_lands_no_worse_than_the_secant_baseline`), which the live map
  had been failing by 24 zensim points.
* **Rejected for two-shot precision.** A mixed map *can* place the score
  between two achievable points and its granularity is genuinely
  sub-lattice (median step 0.19 zensim at 256 px, 0.012 at 1024 px vs gaps
  0.72 / 0.65) — but only 67–75% of adjacent-k steps move in the sweep's own
  direction, with reversals up to 0.40–0.78 zensim. Interpolation is
  possible; it is not *aimable*. And the lattice was never the binding term
  anyway (see `src/two_pass_zensim.rs`: the earlier "±0.5 is unreachable
  half the time" was measured on an integer-quality grid addressing only 100
  of the codec's 256 quantizers).

Remaining from the original checklist: item 3 (re-run the butteraugli A/B on
the bumped chain) and a wider RD sweep with a byte window broad enough to
judge the 78 rows this probe honestly could not.

## Dep-bump checklist additions (zenrav1e > 0.1.4) — HISTORICAL

1. ~~ravif: flip `FRAME_HINTS_LIVE` to `true`, swap the plain send for the
   commented hinted send in `encode_to_av1`, bump the zenrav1e dep.~~ DONE
   2026-08-06 (ravif 619d81a).
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

## FINAL VERDICT (2026-07-04, coordinator close-out): DROP as default/recommendation

The boost-only strength push completed the response curve (coordinator-run
after the session died mid-arm; single baseline regenerated + two arms fresh,
288 rows each, `/mnt/v/output/zenavif/twopass-2026-07-03/`):

| arm (vs single, train26) | ba3n med (better) | ba-max med | ssim2 med | time |
|---|---|---|---|---|
| aom-formula str1.0 | +2.20% (3/24) | +3.25% | +1.88% | 2.01× |
| boost-only str1.0 | +0.10% (10/24) | −0.04% | +0.19% | 1.94× |
| boost-only str1.5 | +0.38% all / **+0.0015% photos** (10/24) | +1.06% | +0.43% | 2.03× |
| boost-only str2.0 | +1.00% (6/24) | +3.11% | +0.95% | 2.03× |

An inverted response with its optimum AT break-even: no strength crosses
positive on any norm. The mechanism's premise — that a butteraugli map finds
misallocation the encoder can't see — is FALSE on this base: zenrav1e's
open-loop perceptual allocation (activity-masked psy distortion + Variance
Boost + ss2 QM curves + chroma delta-q) already spends the headroom the map
detects; the o_9051-class inversion (7/7 outliers worse under the loop) is
the sharpest form of the finding. libaom's tune=butteraugli pays BECAUSE
their default base is unmasked; ours is not.

**What survives (landed, keep):** the `FrameHints` per-SB AC-q-scale API
(zenrav1e c4047cec — trial-rollback-safe, decoder-followed), the
metric-pluggable two-pass driver (butteraugli | ssim2 maps; zensim
profile-B slots in when it ships), the harness (`run_2p_ab.sh` /
`two_pass_cell` / `twopass_report.py`), and the measured record here. The
promising future consumer is NOT a trial-encode loop but **analysis-derived
hints at zero extra encodes** (zenanalyze features → FrameHints — the
feature-hints program's P3/P4), and external callers with per-region
priorities (saliency, faces) who can now express them.

**Blocking follow-up if the two-pass feature is ever shipped live:** the
futex hang (4/220 cells, ~2% incidence, tracked in the issue filed at
close-out) must be root-caused; the per-cell timeout guard is a harness
mitigation, not a fix.
