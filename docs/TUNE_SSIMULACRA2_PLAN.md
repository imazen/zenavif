# `Tune::Ssimulacra2` for zenrav1e — libaom mechanism study + implementation plan

**Status: implemented 2026-07-02 (items 1–5), per-step A/B measurement in flight.** Produced
by a source-level study of libaom's `--tune=ssimulacra2` at the exact rev we benchmark against
(`632172a468f5e91c5b40daaa0a91f4a291c63af4`, aomenc 3.14.1). Motivation: that tune alone
measures **−13.33% median BD-rate** (ssim2-scored) over aom cpu0-default on our photo harness
(`docs/RD_GAP_VS_LIBAOM.md` — the tier-2 "aom at its absolute best" target is aom cpu0+this
tune, vs which current zenrav1e master is +15.67% median). Implementing an equivalent tune is
the largest single known lever, and the general shape (a `Tune` variant driving allocation +
RDO reweighting) is the template for other metric tunes (butteraugli, etc.).

## Implementation status (2026-07-02)

Items 1–5 implemented in the `zenrav1e--tune` workspace as `Tune::Ssimulacra2`
(commit `ec6f3c89` and successors on top of master), each mechanism behind a
dev-only `ZENRAV1E_SS2_STAGE` env gate (1=chroma deltaq, 2=+frame λ, 3=+QM
curves, 4=+trellis λ, 5=+variance boost) for cumulative A/B sweeps; gates strip
at landing. Tune-off byte-identity vs master proven (cavif Q60/Q85, master
binary vs workspace binary, and stage-0 vs Psychovisual). Item 5 uses the plan's
segmentation channel but reproduces aom's full qindex-domain damping
(`(base+544)/1279`, cap 80, MINQ+1) rather than raw `qstep_ratio²`, converting
the boosted qindex back to a scale via `(ac_q(base)/ac_q(sb_qindex))²`.

**Two pre-existing zenrav1e QM bugs found and fixed on the way (zenrav1e#29,
both landed on master 2026-07-02):** the QM implementation this plan builds on
was silently diverging from conforming decoders —
1. `qm_v` was written only when the frame's u/v delta-qs differed; AV1 5.9.12
   gates it on the sequence `separate_uv_delta_q` (always 1 here). The tune's
   u==v chroma deltas made every QM frame corrupt to aomdec (fixed `9a8eaf61`).
2. Every rectangular TX quantized with **transposed QM weights**: rav1e stores
   coefficients transposed (like dav1d) and rav1d-safe's table mapping swaps
   w/h on purpose; zenrav1e's `qm_table()` didn't. Self-consistent inside the
   encoder, wrong on every decoder — invisible at near-flat levels 12–15,
   catastrophic at ss2-curve levels (decoded ssim2 85.7→55.7 at cavif Q85
   before the fix, 83.9 after; fixed `2310c7be` + transpose-pair tests).
   The historical "with_qm(true) ≈10% BD-rate win" predates this fix and
   deserves re-measurement at the dep bump.

Harness: `scripts/rd_gap` cells grew optional butteraugli 3-norm/max columns
(`BUTTER` env; zenavif `5e84d3f6`) implementing the metric-gaming protocol
below; `bd_metric.py` computes BD-rate on either metric (butteraugli quality
axis = −log distance).

## libaom mechanism (file:line at the pinned rev)

**Entry point.** `AOM_TUNE_SSIMULACRA2 = 11` (`aom/aomcx.h:1812`). `handle_tuning()`
(`av1/av1_cx_iface.c:1954-1996`) sets, for **both** TUNE_IQ and TUNE_SSIMULACRA2:
`enable_qm=1, qm_min=2, qm_max=10, sharpness=7, dist_metric=AOM_DIST_METRIC_QM_PSNR,
enable_cdef=CDEF_ADAPTIVE, enable_chroma_deltaq=1, deltaq_mode=DELTA_Q_VARIANCE_BOOST,
screen_detection_mode=ANTIALIASING_AWARE`. IQ-only extra: `enable_adaptive_sharpness=1`
(deliberately **off** for ss2 — costs ssim2 at low quality, cx_iface.c:1988-1991). No
ssimulacra2 is ever computed in-loop; it's all hand-tuned heuristics (aomcx.h:1798-1808).

### (a1) Per-SB deltaq — "Variance Boost"

(`allintra_vis.c:1075-1127`; stat in `aq_variance.c:184-248`; provenance comment cites
SVT-AV1's Appendix-Variance-Boost.md, i.e. SVT-AV1-PSY lineage). Requires 64×64 SBs
(`encoder_utils.c:979-982`). Per SB: compute 64 source 8×8 variances, each `vf(...)/64`
(per-pixel, truncated); qsort ascending; sample octile 5 with 1:2:1 smoothing:
`var = (v[31] + 2*v[39] + v[47] + 2) / 4`. Then with `strength = deltaq_strength/100*3.0`
(default 3.0, clamp ≤6), `var==0 → 1`:

```
qstep_ratio = clamp(0.15*strength*(10 - log2(var)) + 1.0, 1.0, 8.0)   // crossover at var=1024
target_q    = base_q / qstep_ratio
boost       = min(80, round((base_qindex + 544)*(base_qindex - target_qindex)/1279))
sb_qindex   = max(base_qindex - boost, MINQ+1)
```

Boost is one-directional (flat/dark SBs get lower q; busy SBs stay at base). Signaled via
`delta_q` with resolution 1/2/4/8 for base qindex <80/<120/<160/≥160
(`encodeframe.c:1955-1988`). Per-SB rdmult follows the boosted qindex
(`encodeframe.c:357-379`).

### (a2) Per-block rdmult scaling — shared verbatim with `--tune=ssim`

(`encoder.c:4307-4311` → `av1_set_mb_ssim_rdmult_scaling`, `encoder_utils.c:1483-1551`;
applied per coding block `partition_search.c:626-631` → `encodeframe_utils.c:35-70`). Per
16×16 block: mean per-pixel 8×8 variance `var`, then
`factor = 67.035434*(1-exp(-0.0021489*var)) + 17.492222`, normalized by frame geometric mean
(range ~[0.207, 4.832]); block rdmult ×= geomean(factors over block). RDCOST =
`R*rdmult>>9 + D<<4` (`rd.h:32-34`) so high-variance blocks pay more per bit (masking).

### (a3) Frame rdmult weight

(`rd.c:406-434`): all-intra `weight = clamp((255-qindex)*3/4, 0, 72) + 128;
rdmult *= weight/128` — 1.5625× for qindex≤159 ramping to 1.0 at 255. Biases toward larger
TX sizes.

### (b) Chroma deltaq

(`av1_quantize.c:886-975`): 4:2:0 `dc=ac=-clamp(base/2-14, 0, 20)` (**20 is ss2-specific**;
IQ=16); 4:2:2 `ac=+clamp(base/2,0,6)`; 4:4:4 `ac=+clamp(base/2,0,24)` (chroma *worse*, luma
fed).

### (c) QM + QM-weighted distortion

Luma curve `aom_get_qmlevel_luma_ssimulacra2` (**ss2-specific**, `quant_common.h:111-136`):
qindex ≤40/60/90/120/130/140/160/200/else → level 10/9/8/7/6/5/4/3/2. Chroma 4:4:4:
`aom_get_qmlevel_444_chroma` (h:151-175, ≤12/24/32/36/44/48/56/88/else → 10..2); other
subsampling: `aom_get_qmlevel_allintra` (h:77-98); chroma curve input is
`base_qindex + chroma_ac_delta_q` (`av1_quantize.c:1024-1035`). `DIST_METRIC_QM_PSNR` makes
RDO's transform-domain SSE QM-weighted: `err = (diff*qm[i])²>>2*AOM_QM_BITS`
(`tx_search.c:1078`, trellis `txb_rdopt_utils.h:48-64`) — HF errors cost less, matching what
dequant does.

### (d) Trellis keeps coefficients

(`txb_rdopt.c:443-461`): `rshift` 5→7 **and** `sharpness=7` → trellis rdmult
`= x->rdmult*(8-7)*plane_rd_mult>>7` vs default `*8>>5`: **÷32**. Comment: preserves
repeating patterns/noise, raises ssim2.

### (e) Filters

LF `sharpness_level=7` written to frame header (`picklpf.c:220-231`). CDEF_ADAPTIVE
(`pickcdef.c:837-851,927-940,1046-1096`, AOM_Q/CQ only): off cq≤32; halve pri/sec strengths
cq≤220; zero SB strengths with pri≤4&&sec≤1 when qindex≤140 (`speed_features.c:2899-2903`).

### (f) Misc

Speed features `skip_intra_in_interframe=0, inter_mode_rd_model_estimation=0, use_intrabc=1,
intra_pruning_with_hog clamped ≤3` (`speed_features.c:1516-1541`); 1.125× inter-candidate RD
penalty (`rdopt.c:796-817`) — inter-only, no-op for still AVIF.

## zenrav1e mapping (files verified at HEAD b073182c)

Add `Tune::Ssimulacra2` to `src/encoder.rs:109-117`; extend the `matches!` gates at
`src/rdo.rs:282,473`, `src/api/internal.rs:1368` (activity on), and the StillImage sites
`encoder.rs:1118` (segmentation on), `1436`, `3679`.

1. **Chroma deltaq** — replace Daala `chroma_offset` (`src/rate.rs:510-522`, CIEDE2000-tuned)
   for this tune: compute `base_q_idx`, set `ac_qi/dc_qi[1,2]` by the (b) formulas (infra
   exists: `QuantizerParameters` `rate.rs:598-607`, signaling `encoder.rs:1455-1461`);
   `dist_scale = (target_q/plane_q)²` (`rate.rs:581-582`) auto-adjusts chroma RDO weight.
   Smallest change; aom claims 1.5-3% BD from this alone.
2. **Frame λ weight** — `fi.lambda` (set `encoder.rs:1464`; used `rdo.rs:760-765`, λ
   multiplies rate exactly like rdmult): `λ *= (clamp((255-qi)*3/4,0,72)+128)/128`.
3. **QM curves** — swap `qm_level_for_qindex` (`encoder.rs:135-143`, linear 15→4) for the ss2
   luma + 444-chroma piecewise curves under the tune; keep the fork's qindex-0→15 lossless
   guard (zenavif CLAUDE.md cliff).
4. **Trellis λ** — `src/quantize/trellis.rs:85-89` `lambda_trellis`: sweep ×{1/4,1/8,1/16,1/32}
   (aom net ÷32).
5. **Variance Boost** — zenrav1e codes no delta_q; the equivalent channel is segmentation
   `SEG_LVL_ALT_Q` (`src/segmentation.rs:74-166`: k-means(3..8) over log2 scores →
   per-segment qi deltas, ±63 covers most of aom's 80 cap; per-block `select_segment`). 8×8
   variances already exist (`ActivityMask`, `src/activity.rs:23-55`; divide by 64 to match
   aom's per-pixel stat). For the tune, override `segmentation_scores`
   (`api/internal.rs:1381-1383`) with `scale = qstep_ratio²` per (a1) so `compute_delta`'s
   `Q' = Q/sqrt(scale)` reproduces the boost. Note zenrav1e's existing `ssim_boost`
   (`activity.rs:159-186`, `(svar+dvar+C2)/sqrt(C1²+svar·dvar)` ≈ var^(-1/3)) is the *same
   idea* as (a2) but gentler and distortion-side; the aom curve is much stronger for
   var<1024.
6. **QM-weighted tx distortion** — weight `RawDistortion` (`encoder.rs:1918-1932`) and trellis
   coeff dist by `qm²`. Lower priority: Psychovisual RDO is pixel-domain `cdef_dist`
   (`rdo.rs:282-295`), so this lever is smaller here than in aom.
7. **Filters** — align StillImage's existing CDEF scaling (`encoder.rs:1436-1444`) and deblock
   sharpness (`encoder.rs:3679-3687`) to aom's thresholds (constant sharpness 7; CDEF off ≤32
   / halved ≤220 / zeroed ≤140).

## Effect split & implementation order

**Both allocation and reweighting, weighted to allocation**: (a1)+(b)+(a3) are bit
redistribution; (c)+(d) are in-RDO reweighting; (a2) sits between. Implement in the order
above — 1-4 are frame-level and nearly free; 5 is the marquee item with a ready insertion
point; 6-7 last. Measure each step A/B on the rd_gap harness (the fixes compose; don't land a
step that regresses the running total).

## Metric-gaming caveats (mandatory protocol)

Our judge IS ssim2, so this tune partially optimizes the judge. Genuinely perceptual
(subjective validation cited in aom source — keep): variance boost, 4:2:0 chroma boost,
trellis coefficient retention, LF sharpness. Metric-leaning (ss2-only deltas vs TUNE_IQ —
aomcx.h:1806-1808 *explicitly says* these lack subjective backing): chroma 20-vs-16, luma QM
down to level 2, 4:4:4 chroma +24 (ssim2 mis-scores subsampling, av1_quantize.c:925),
adaptive-sharpness off. **Protocol**: benchmark every mechanism A/B with fast-ssim2 *and*
butteraugli 3-norm/max (zenmetrics); where an ss2-vs-IQ divergent knob regresses butteraugli
beyond noise, ship the IQ value; record both metrics in `benchmarks/*.tsv`.
