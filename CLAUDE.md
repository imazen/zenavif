# zenavif

Pure Rust AVIF encoder/decoder wrapping rav1d-safe (pure Rust AV1 decoder) and zenavif-parse.

## Quick Commands

```bash
just check        # cargo check
just build        # cargo build --release
just test         # cargo test
just clippy       # cargo clippy with warnings as errors
just fmt          # cargo fmt
just build-encode # cargo build --features encode
just test-encode  # cargo test --features encode
```

## Architecture

### Decoding
- `src/lib.rs` - Public API, re-exports
- `src/error.rs` - Error types with whereat location tracking
- `src/config.rs` - DecoderConfig builder
- `src/image.rs` - DecodedImage enum, ImageInfo metadata
- `src/decoder_managed.rs` - Main decoder (100% safe Rust, rav1d-safe managed API)
- `src/decoder.rs` - Legacy FFI decoder (behind `unsafe-asm` feature gate)
- `src/convert.rs` - Alpha channel handling, unpremultiply

### YUV Conversion
- `src/yuv_convert.rs` - Float SIMD path (AVX2/FMA via archmage)
- `src/yuv_convert_libyuv.rs` - Exact libyuv integer math (BT.709, BT.601)
- `src/yuv_convert_libyuv_simd.rs` - AVX2 SIMD libyuv path
- `src/yuv_convert_libyuv_autovec.rs` - Auto-vectorized libyuv variant
- `src/yuv_convert_fast.rs` - Fast fixed-point integer path
- `src/chroma.rs` - YUV chroma upsampling iterators

### Encoding
- `src/encoder.rs` - AVIF encoding via zenravif (behind `encode` feature; currently disabled — zenravif not yet published)
- `src/encode_plan.rs` - `EncoderConfig::resolve_plan(PlanInput) -> EncodePlan` static resolution + provenance-tagged mirrors of zenravif's quality→quantizer curve and SpeedTweaks tables (`encode` feature)
- `src/sweep.rs` - Sweep planner + byte-identity fingerprint (`__expert` feature); see `docs/VARIANT_GENERATION.md` for the knob audit (dominance/trial/metric), the fingerprint exclusions, and the harness findings
- `examples/sweep_validate.rs` - Empirical axis validation; **re-run whenever bumping the zenravif dep or touching the sweep axes/fingerprint** (`benchmarks/sweep_validate_*.tsv`). Gotcha: encode+decode+score loops in rayon pools need `stack_size(32 MB)`. Per-task stack peaks at ~0.5 MB dominated by rav1d-safe's `rav1d_open` decoder-context construction (gdb-verified 2026-06-11; zenrav1e encode itself needs ≤128 KB even at speed 2/q10/noise — an earlier "partition RDO" attribution was wrong). The overflow mechanism is rayon work-stealing stacking whole task contexts on one worker stack at zensim's internal join points, so 2 MB defaults die probabilistically under load
- `tests/encode_contracts.rs` - Encode-level byte contracts (alpha quality fallback, quantizer mediation, subsampling liveness)

### Additional Source Files
- `src/decode_av1.rs` - AV1 bitstream decoding entry points
- `src/strip_convert.rs` - Strip-based pixel conversion utilities
- `src/detect.rs` - AVIF file detection / sniffing
- `src/codec.rs` - zencodec trait implementations
- `src/zennode_defs.rs` - zennode pipeline node definitions (behind `zennode` feature)
- `src/simd/` - SIMD acceleration modules

## Dependencies

- `rav1d-safe` - Pure Rust AV1 decoder (managed API, no C FFI)
- `zenavif-parse` - AVIF container parser (registry dep)
- `zenravif` / `ravif` - AVIF encoder (optional, `encode` feature)
- `zencodec` - Codec abstraction traits (registry dep)
- `zenpixels` - Pixel buffer types (registry dep)
- `archmage` / `magetypes` - Token-based safe SIMD
- `yuv` - YUV to RGB conversion (supplementary)
- `imgref` - Image buffer type
- `rgb` - RGB pixel types
- `enough` - Cooperative cancellation
- `whereat` - Error location tracking
- `thiserror` - Error derive macro

## Features

- `(default)` - Pure Rust decode only, safe SIMD via archmage
- `encode` - AVIF encoding via zenravif
- `encode-imazen` - Encoding with zenrav1e fork extras (QM, VAQ, still-image, lossless)
- `encode-asm` - Encoding with hand-written assembly (fastest, unsafe)
- `encode-threading` - Encoding with multi-threading
- `unsafe-asm` - Decoding with hand-written assembly via C FFI (fastest, unsafe)
- `zencodec` - zencodec trait integration
- `_dev` - Expose internal YUV modules for profiling (not public API)

## Known Bugs

### zenrav1e lossless ±2 — FIXED upstream, release-gated
Root cause found and fixed 2026-06-11 (zenrav1e c3567081, zenrav1e#9
closed): "lossless" never reached qindex 0 — the rate path floored it
at 1, so every lossless encode was qi=1 lossy. The same fix resolves
the zenavif#8 size-vs-speed inversion (validated bit-exact + monotonic
via path-patch; `benchmarks/lossless_speed_sweep_fixed_2026-06-11.tsv`).
**Until zenrav1e 0.1.5 releases and the zenravif → zenavif dep chain
bumps**, registry builds still ship the broken behavior: the identity
roundtrip tests keep their documented ≤2 tolerance and zenavif#8 stays
open. At the dep bump: tighten `tests/identity_roundtrip.rs` to exact,
re-run `examples/lossless_speed_sweep.rs`, close #8.


### zenrav1e topdown partition search missing HORZ/VERT — FIXED upstream, release-gated
Root cause found and fixed 2026-07-01 (zenrav1e@665e58e4, pushed to `master`):
`encode_partition_topdown` (the primary partition-search path cavif/zenavif use; ravif's
own `encode_bottomup` speed setting is off by default, but `encode_partition_bottomup`
is ALSO forced regardless of that setting for any superblock straddling the frame's
right/bottom edge — corrected 2026-07-01, see below) hardcoded its RDO candidate list to
`[SPLIT, NONE]`, so `PARTITION_HORZ`/`VERT` were never offered at any speed. This is also
why cavif's `-s1` and `-s2` were byte-identical (`non_square_partition_max_threshold`
only mattered in the bottom-up path). Companion `ravif@b4853c68` widens speed-2's
threshold to `BLOCK_64X64`. Measured: median bpp −1.8% to −2.8% at ssim2 70-85; BD-rate
gap vs libaom +5.7%→+3.6% median (same-day, narrower methodology — see
`docs/RD_GAP_VS_LIBAOM.md`). **Until zenrav1e releases past 0.1.4 and the zenravif →
zenavif dep chain bumps**, registry builds still ship the pre-fix behavior.

**2026-07-01: extended partition types (HORZ_4/VERT_4) — FIXED, root cause was a
`BlockSize` ordinal-vs-dimension mismatch, upstream, release-gated.** The 6-of-10-types
gap referenced above (measured 10-13% area share on libaom's side) turned out to be
blocked on a real bitstream-conformance bug, not a missing feature. Root cause: libaom's
`av1_use_angle_delta`/`av1_allow_palette` gate several per-block syntax elements on
`bsize >= BLOCK_8X8`, which is a genuinely *ordinal* comparison in libaom's C
`BLOCK_SIZE` enum (the six "extended" 4:1-aspect sizes are numbered after every classic
size, so the comparison is always `true` for them regardless of actual dimensions).
zenrav1e's textually-identical `bsize >= BlockSize::BLOCK_8X8` is NOT ordinal — `BlockSize`
has a custom width/height-based `PartialOrd`, under which `BLOCK_4X16`/`BLOCK_16X4` are
*incomparable* with `BLOCK_8X8` (one dimension smaller, one larger), so `>=` silently
evaluated `false` where it should have been `true`. The encoder skipped writing a
required `angle_delta` syntax element for directional-mode blocks of those two sizes —
a missing-symbol bitstream desync that `aomdec` correctly rejected as corrupt (the exact
symptom the previous attempt hit and reverted, per zenrav1e#26). Fixed via
`BlockSize::ge_8x8_ordinal()` (zenrav1e@2866397e) + Phase 1 of `PARTITION_HORZ_4`/`VERT_4`
re-implemented on top (zenrav1e@7d254289). Verified clean on 110 cells (22-image photo
corpus x 5 quality levels): 0 `aomdec` corruption (was 100% corrupt before the ordinal
fix, direct A/B confirmed causality), extended block-size area share 1.8-56% per cell,
rav1d-safe round-trip pixel diff scaling normally with quality. Measured RD impact:
`docs/RD_GAP_VS_LIBAOM.md` "Fixed 2026-07-01 (4)".

**2026-07-01 (later): `PARTITION_HORZ_A/B`/`VERT_A/B` (Phase 2, the other 4 of 6) —
implemented, verified conformance-clean, measured as a NET RD REGRESSION, reverted.**
Same 110-cell aomdec-clean bar as Phase 1, all 4 types genuinely chosen by the search —
but direct-isolation BD-rate median +0.60% (worse on 14/22 images), median BD-rate vs
libaom-slow +0.1%→+0.6%, ~1.46x encode time. Root cause of the regression is a
SPLIT-cost-estimation bias in zenrav1e's one-level topdown trial (SPLIT evaluated
pessimistically as 4 NONE-leaves while the mixed 3-way types are evaluated exactly),
not a defect in the new types. Not on zenrav1e master; the full implementation (plus 2
real bug fixes it required) is preserved as zenrav1e workspace commit `a7630aee`.
zenrav1e#27 stays open re-scoped to the trial-SPLIT-cost-accuracy dependency. See
`docs/RD_GAP_VS_LIBAOM.md` "TRIED AND REVERTED 2026-07-01" +
`benchmarks/rd_gap_extended_partitions_phase2_2026-07-01.tsv`.

**2026-07-02: trial-SPLIT-cost accuracy — FIXED, TRUE RD PARITY REACHED, upstream,
release-gated.** The dependency above is resolved: `zenrav1e@b073182c` (master) refines each
SPLIT child's trial cost to min(NONE-leaf, tell-metered child-SPLIT symbol + 4 quarter
NONE-leaves), eliminating the pessimistic bias. Measured: **BD-rate vs libaom-slow median
+0.0695% → −0.6487% (crosses the ≤0% parity target), mean +2.1734% → +0.2373%**, improved
16/19 images, 1.057x median encode time; 110/110 aomdec-clean + 110/110 rav1d-safe roundtrip.
This also unblocks a Phase 2 re-attempt from `a7630aee` (the types regressed against the
then-underestimated SPLIT). Registry builds ship pre-fix behavior until zenrav1e releases past
0.1.4 + the dep chain bumps. See `docs/RD_GAP_VS_LIBAOM.md` "Fixed 2026-07-02" +
`benchmarks/rd_gap_splitcost_2026-07-02.tsv`.

**2026-07-02 (later): Phase 2 v2 re-measured on the fixed estimate — the regression FLIPPED
TO A WIN** (direct isolation −0.5759% median, better 17/22; vs cpu2 −0.65%→−1.87% median) at
1.461× encode time → preserved as zenrav1e workspace commit `dfed8eda`, the prime `-s1`
deep-mode ingredient (above the 1.2× matched-speed gate for s2). See
`benchmarks/rd_gap_phase2v2_2026-07-02.tsv`.

**2026-07-02 (final): the `-s1` deep mode SHIPPED, release-gated.** zenrav1e master gained
two default-off knobs — `mixed_3way_partitions` (`efbe0cf2`, the gated Phase 2 v2) and
`split_trial_depth` (`2fac1af6`, recursive SPLIT-trial refinement) — both byte-identical
off (9/9-cell md5 each). ravif main (`9d2b97c`) arms s1 with mixed-3way + unconditional
rdo_tx_decision + partition_range (4,32) (winner of a 16/32/64 × depth{1,2} ablation)
behind `S1_DEEP_ARMS_LIVE = false` until the zenrav1e dep bump (byte-identical until then,
6/6-cell md5 vs b4853c68 on registry deps; flip the const + uncomment 2 apply lines at the
bump). Measured at the live config: **median BD-rate −0.97% vs libaom cpu-used=0
(slowest-best; s2 was +1.47%), −3.01% vs cpu2, 11/19 photos win per-image; 8 named photos
still lose (o_6629 +25.3 worst) — partition levers exhausted, residual is
coefficient-level RD.** 110/110 aomdec+rav1d conformance at the shipped config (+3 more
110-cell configs). ~3.7× cpu0 wall per cell (RD-first mode by design). Full record:
`docs/RD_GAP_VS_LIBAOM.md` "s1 deep mode" + `benchmarks/rd_gap_s1_2026-07-02.tsv`.

### zenrav1e 64×64-parent HORZ_4/VERT_4 sliver corruption — FIXED upstream, release-gated
Found 2026-07-02 by the partition_range re-test: BLOCK_64X16/16X64 slivers (only reachable
with `partition_range` max=64, e.g. via the public `override_partition_range`) coded their
never-validated TX_64X16/TX_16X64 max transforms and desynced BOTH `aomdec` ("Corrupted
segment_ids") and rav1d-safe. Initial attribution to `b073182c`'s deeper SPLIT estimate was
WRONG — exonerated by a six-variant bisect. Fixed `zenrav1e@3fa735dc`: intra slivers cap to
TX_32X16/16X32, the tx-size RDO walk shrinks by the consumed level (else an out-of-alphabet
depth-3 symbol — a second latent corruption caught in the same fix; that bound is now a hard
assert in all builds), inter frames without `enable_inter_txfm_split` don't offer 64-parent
4-way candidates. Byte-identical at shipped configs; 264/264-cell clean sweep at (4,64).
zenrav1e#28 tracks validating the real 64-dim sliver transforms. Registry builds can hit the
corruption via `override_partition_range` until zenrav1e releases past 0.1.4. The bisect
lesson (silent-corruption bug classes + the s.replace() trap) is in the project memory.

The prange (4,64) s2 widening itself REMAINS RULED OUT on clean data (+0.48% median direct,
worse 15/22; `benchmarks/rd_gap_prange_retest_2026-07-02.tsv`) — but now with 7/22 winners
(three at −1.8..−2.5%), making a content-adaptive large-block gate a feature-hints candidate.

### Tune::Ssimulacra2 — SHIPPED upstream 2026-07-02, release-gated
`zenrav1e@a37faea8` adds `Tune::Ssimulacra2` (aom-parity chroma delta-q + ss2
QM curves; the other three aom mechanisms measured as regressions and were
dropped — see docs/TUNE_SSIMULACRA2_PLAN.md). Measured: s2 −4.28% / s1-deep
−3.57% median ssim2 BD vs tune-off, beats aom cpu0-default at both speeds,
tier-2 gap vs cpu0-ss2tune +11.08% → +8.71%. 220/220 conformance cells clean.
Since extended by per-SB delta_q Variance Boost (tier-2 → +5.63%/+5.02%) and
the QM-weighted RD distortion ratio (2026-07-03, tier-2 s2 +5.63% → +2.12%,
s1 +5.02% → **−1.94% — tier-2 median crossed**;
docs/RD_GAP_VS_LIBAOM.md "QM-weighted RD distortion").
**At the zenrav1e dep bump:** wire tune selection through zenravif/zenavif
(the sweep used a dev-only ZENRAVIF_TUNE env passthrough, reverted) and decide
the default for still images. From the libavif v1.4.0 study
(`docs/LIBAVIF_1_4_STUDY.md`, mechanisms c3/c5): (1) **never apply a
perceptual tune to the ALPHA channel** — libavif measured ringing from
perceptual tunes on alpha and pins alpha to tune=psnr; A/B our alpha path
(currently Tune::Psychovisual) and pin accordingly; (2) if the ss2 tune
becomes the still default, **re-fit the quality→quantizer curve** — libavif's
precedent (their new piecewise LUT compensates tune=iq's higher spend at
matched QP); our curve diverges from their mapping above q70.

### zenrav1e per-SB delta_q + Variance Boost — SHIPPED upstream 2026-07-02 (later), release-gated
zenrav1e gained true per-SB delta_q coding (`d125713f`, inert syntax; the
encoder previously coded none) and `Tune::Ssimulacra2` now drives libaom's
DELTA_Q_VARIANCE_BOOST through it (`66733720` + `165e83b1`: strength 1.0
offline-fit on train26 — aom's 3.0 default over-boosts on top of zenrav1e's
activity masking; 4.5/6/keep-segmentation arms butteraugli-vetoed).
Segmentation is disabled while the boost is active. Measured (legacy-corpus
confirm, photos n=19): s2 tier-2 gap +10.10% → +5.63% median, vs cpu0-default
−3.43% → −5.07%, direct −1.81% on top of the tune; s1 numbers in
docs/RD_GAP_VS_LIBAOM.md "Per-SB delta_q + Variance Boost". 4×110-cell
conformance clean (both speeds × strength 3.0 + shipped 1.0). Known residual:
o_6629 (ultra-flat gradient TEST-split origin) regresses further with the
boost (+14.2 → +32.7 vs cpu0-default; q30-40 misallocation) — per-image
strength/gating via the picker is the tracked follow-up, alongside strength-2
for smooth photos (train26 5004_nps −15.0% at str2). **At the zenrav1e dep
bump:** nothing extra to wire beyond the tune itself (the boost rides
`Tune::Ssimulacra2`); re-run the QM benchmark note below still applies.

### zenrav1e QM encodes diverged from conforming decoders — FIXED upstream, release-gated
Two composing bugs (zenrav1e#29, both fixed on master 2026-07-02) made
`enable_qm=true` encodes decode differently than the encoder's own
reconstruction: (1) `qm_v` was gated on frame-level diff_uv_delta instead of
the sequence's separate_uv_delta_q (u==v chroma delta-qs → aomdec-rejected
frames; fixed zenrav1e@9a8eaf61), and (2) every rectangular TX quantized with
transposed QM weights (rav1e stores coefficients transposed; the table lookup
didn't swap w/h like rav1d-safe's does; fixed zenrav1e@2310c7be). zenavif's
`with_qm(true)` default ships both until the zenrav1e release past 0.1.4 +
dep bump; effect at the shipped near-flat levels 12-15 is small but real.
**At the dep bump: re-run the QM benchmark** — the "~10% BD-rate win" for
with_qm(true) was measured with transposed rect weights.

### zenrav1e palette mode — IMPLEMENTED upstream 2026-07-03, release-gated
The screen-content palette tool (RD_GAP item 4) is fully implemented on
zenrav1e master (68a8d81f, 5f82e2d4, cda831e7, df27117c + changelog), default
OFF behind `SpeedSettings.prediction.palette = PaletteMode::{Off, Auto,
Always}` (`--palette` on the rav1e CLI). Luma-only search (UV flag coded
"off" — conformant); `Auto` ports libaom's AA-aware screen-content detection.
Conformance: every palette-on cell measured (720+ cells across sweeps)
decodes aomdec-clean AND byte-agrees (raw I420 md5) with rav1d-safe; the
in-repo roundtrip encodes synthetic screen content to LOSSLESS luma at ~1/8
the palette-off bytes. RD numbers: `docs/RD_GAP_VS_LIBAOM.md` item 4 status +
`benchmarks/palette_*_2026-07-03.tsv`. **At the zenrav1e dep bump:** wire
`PaletteMode` through zenravif/zenavif (picker owns Off/Auto/Always per
image) and re-measure the screen-content tier gap.

### zenrav1e LRF + filter-intra desync encoder recon from decoders (OPEN upstream)
Found 2026-07-03 while measuring palette (zenrav1e#32, #33): at s<=7 (LRF)
and s<=6 (filter-intra, enabled when prediction_modes >= ComplexKeyframes),
zenrav1e's own recon diverges from what aomdec AND rav1d-safe (byte-agreeing
with each other) decode — running drift up to ~50 luma RMSE on smooth
content; a third composing bug (forced-skip intra blocks never wrote their
prediction into the recon) is FIXED upstream (zenrav1e@b30dd752).
**Measurement impact:** decoded-quality scores of zenrav1e/cavif output are
systematically depressed wherever these tools fire — ravif disables LRF at
normal quality (low-q cells affected only), but filter-intra is ON in cavif
at s1/s2, so rd_gap/tune-sweep photo numbers carry unintended error. Isolate
with `--lrf false --filter-intra false` (overrides added upstream). Re-check
tier measurements after the upstream fixes land.

### rav1d-safe Threading Race Condition (RESOLVED)
DisjointMut overlap panic was caused by frame threading. Fix: `max_frame_delay=1`
gives tile parallelism without frame threading. Default threads now 0 (auto-detect).

## TODO: Encoding Enhancements

### Target-Quality Convergence — IMPLEMENTED for RGB8 (2026-07-02)
`encode_rgb8_with_target` (`target-quality` feature, `src/target_quality.rs`):
bracketed secant search over quality converging on `TargetMetric::Ssim2` or
`::Zensim` (per the 2026-07-02 user directive: quality/ssim2/zensim, not
Butteraugli). Selection policy: smallest file inside the target band;
honest `converged=false` when unreachable. RGBA8 covered
(`encode_rgba8_with_target`: zensim scores RGBA natively; ssim2 composites
on mid-gray); RGB16 covered (`encode_rgb16_with_target`, 10-bit AV1: ssim2
native 16-bit, zensim via identical 8-bit views). Contract tests:
`tests/target_quality.rs` (6 tests, ~0.6 s). NOT YET COVERED: RGBA16,
animation.

### Encoding Features (`encode-imazen` feature gate)
All wired through to zenrav1e fork. Benchmarked results (ravif 7265eea):
- `with_qm(true)` - only measurable win (~10% BD-rate). Default enabled.
- `with_vaq()` - hurts quality; psychovisual tune already includes SSIM boost.
- `tune_still_image` - no effect; ravif disables CDEF at high quality levels.
- `with_lossless` - implemented, works.

## Canonical training data + indexes (added 2026-05-20)

**The canonical index for all ML data lives at `~/work/zen/DATA_PROVENANCE.md`.**

Quick paths:
- Trainer input: `/mnt/v/zen/zensim-training/canonical-2026-05-21/`
- Master inventory: `~/work/zen/_ml-inventory-2026-05-20/00-MASTER-SYNTHESIS.md`
- Per-codec picker audit: `~/work/zen/_ml-inventory-2026-05-20/05-per-codec-pickers.md`

## ML/auto-tune status (2026-05-20)

zenavif is the **only zen codec with a production picker.** `EncoderConfig::auto_tune()` (feature `auto-tune`) loads `src/models/rav1e_picker_v0_1_1.bin` (ZNPR v2, ~217 KB) + the two LUT JSONs and returns optimal (speed, quality) knobs for a `QualityTarget`. Reference implementation for any future codec picker wiring.

Training intermediates from earlier 2026-05-04 sweep (`benchmarks/zenavif_picker_v0.{3,4,5}_2026-05-04.bin`) are kept for reproducibility; production wires `v0.1.1`.

The training pipeline that produces a new `rav1e_picker_v*.bin` lives in `~/work/zen/zenanalyze/zentrain/` (Python). See `~/work/zen/_ml-inventory-2026-05-20/05-per-codec-pickers.md` for cross-codec picker design.
