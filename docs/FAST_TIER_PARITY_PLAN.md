# Pruning & Parity: the fast-tier program (2026-07-04)

**Goal (user directive): beat libaom at EVERY speed setting.** The speed-ladder gap map
(`docs/SPEED_LADDER.md`, `benchmarks/rd_gap_speed_ladder_2026-07-04.tsv`) measured where we
actually stand: **aom's `--allintra` ladder pareto-dominates every zenrav1e arm at matched
wall-time, and the gap widens with speed** — except the extreme-quality tip, where s2-tune
beats aom's best allintra arm (−1.4% at 2.66× time). All prior GOOD-mode wins (cpu0/cpu2)
stand as measured, but **GOOD mode is off aom's own still-image pareto** (cpu2-allintra ≈
4× faster than cpu2-GOOD at higher quality with tune=iq): allintra is the frontier to beat.

**Root cause is structural, not mysterious**: aom's fast tiers keep the whole toolset and
**prune the search** (early-exit thresholds, model-guided candidate pruning,
resolution-keyed feature schedules — `speed_features.c`); ravif's SpeedTweaks table
**amputates tools outright** (tx RDO → `TX_MODE_LARGEST` + DCT-only at s6+ via the coupled
gate at rdo.rs:909; rect partitions dead at s4+; block cap 16; intra RDO top-3 always;
CDEF/LRF only ≲Q50). Amputation saves the *whole* cost of a tool and loses the *whole*
value; pruning keeps most value for a fraction of cost. Parity = replace amputation with
pruning, then let prediction replace search where it can.

## Phase P0 — cheap wins (**DONE 2026-07-04**; record `benchmarks/rd_gap_fastwins_2026-07-04.tsv`)
- **cavif default-threading byte hazard — FIXED, LIVE on ravif main (55f8c935).**
  The +3.6% single-cell number under-sold it: full BD curve vs 1 tile at s6 is
  +0.96/+1.90/+3.42/+5.37/+7.40% median at 2/4/8/16/64 tiles with **0/24 images
  better at any level** (pool size bitstream-inert; tiles are zenrav1e's only
  intra-frame parallelism; 48c wall speedup saturates at 5.9×/6.8× s6/s4). New
  default: tiles capped to ≥1 MP each (`TILE_RD_MIN_AREA`) — ≤1 MP never tiles,
  bytes identical 1..48 cores (18/18 md5), `--threads 1` byte-identical (18/18).
  Give-back reported honestly: 48c 1 MP defaults 170→1005 (s6) / 871→5911 (s4)
  ms/MP; explicit `-s`/`--threads` keep the speed available.
- **The s4→s6 rdo_tx cliff — DECOMPOSED + LANDED (release-gated).** New zenrav1e
  default-off knobs (d82c16ba: `rdo_tx_size_override`/`rdo_tx_type_override`/
  `rdo_tx_size_depth`, byte-identical off 27/27 md5) split the boolean. Verdict:
  the SIZE half depth-limited to 1 (DCT-only) is the efficient point — **51% of
  the whole s6→s4 RD step (−8.26% median at 4.49×) for 1.67× solo**; full-grid
  confirm s6 −2.78/−3.95/−6.01 (ssim2/ba3n/bamax, 18-20/24 better), s8
  −2.89/−3.52/−5.49 at 1.43×. Landed on ravif main 7baad5f9 as s6-s8 arms behind
  `S6_TX_SIZE_RDO_LIVE=false` (flip at the zenrav1e dep bump). TYPE half alone:
  2.4× + butteraugli-max veto (+0.29) — rejected standalone. size1+reduced-types
  ("min") = 92% of the step at 4.6× solo — **the P1 tx-search seed point** (item 2
  below): per-family recovery fractions in the TSV (photo/scan/clipart wedges
  75-89% recovered by size alone; interiors/food/nature only 12-46% → their
  remainder is partition-owned, P1 item 1). reduced_tx_set standalone: measured
  null at s6/s8. 4,176/4,176 armed cells aomdec+rav1d-safe clean. Wedge caveat:
  fam-7000 near-lossless plots pay +2..18% bytes on ~3 KB files under size-RDO
  (7050 q30 quality crater: RD model misfires on razor-edge palette content) —
  owned by the intraBC/near-lossless program (P3).

## Phase P1 — the pruning-schedule rebuild (mechanism work, the core of parity)
Replace ravif's per-speed amputation table with graduated search budgets, one lever at a
time, each A/B'd at its tier's time budget (the discipline that built the quality tip):
1. **Partition search**: keep HORZ/VERT (+16-block 4-ways) live at s4-s8 with aom-style
   early-exit pruning (their `partition_search_breakout`/none-vs-split thresholds and ML
   gates at `speed_features.c:711`-family, pinned rev) instead of candidate deletion.
   The SPLIT-trial estimate (b073182c) is exactly the cost model a breakout needs — reuse it.
   **DONE 2026-07-04 (P1PART; record `benchmarks/rd_gap_p1part_2026-07-04.tsv`).**
   Mechanism landed: zenrav1e `PartitionSpeedSettings::topdown_prune` (725f5f71 +
   one-sided fix 767c8ff5; default-off, 27/27 md5 + 144/144 sentinel byte-identical
   off) — NONE-first candidate walk (the existing per-child early exit then bounds
   every SPLIT/rect/4-way trial against the NONE incumbent) + none_breakout
   (skip-gated τ·λ·pels) + rect/4-way NONE-dominance margins + the 4×4-log-var
   homogeneity gate (aom allintra `prune_rect_part_using_4x4_var_deviation` port).
   Gate decomposition on train26 s6 (tune-ss2, vs the s6+size1 base): liveness
   ceiling r16 = −3.47/−3.28/−3.52 med (ssim2/ba3n/bamax, 24/24) at 2.91× solo =
   **90% of the whole remaining (s6+size1)→s4 step**; +max32 −4.11 (107%) at 2.93×;
   32-rects DEAD (+0.16 over max32 for +1.0×). **Margins are a measured dead end in
   BOTH semantics** (symmetric kept 26% — killed exactly the SPLIT-dominant
   razor-edge content; one-sided 46-48%, and 0.10-vs-0.25 margins not differing
   shows the lost rect wins sit where NONE dominates the split estimate — the
   contested-band premise is false on our cost model). **Skip-gated breakout is a
   null at every τ** (≡ vargate's shadow). **The homogeneity vargate is the gate
   that pays**: vg2.0 keeps 94% (−3.28) at 2.45× solo — and it is a shape prior,
   not just a cost gate (no4+vg2 beats no4-alone on RD: skipped rect leaves on
   smooth blocks redirect into deeper SPLIT recursion). Shipped (release-gated
   ravif `S6_PART_PRUNE_LIVE=false` @ 0191489b, byte-identical off 18/18):
   rect threshold 8×8→16×16 at s4-s8 + the gate triple {none_breakout 1.0,
   four_way_margin 0.0 (rects always live, 4-ways only on SPLIT-dominant
   blocks), homogeneity_gate 2.0} — cheaper than ungated liveness at every
   tier (solo 2.16/2.08/1.75× vs 2.33/2.23/1.91×, s6/s8/s4). Full-grid 12q
   confirms: s6 −2.89/−2.51/−2.45 (24/24 both primaries), s8
   −3.00/−2.49/−2.86 (24/24), s4 −1.94/−2.32/−2.74 (22/23); no bamax veto.
   **Ladder-column movement (photos, vs the cached aom-allintra refs): s6 vs
   cpu4def-ai +1.4→−4.6/−6.3 CROSSED both metrics, vs cpu4iq-ai +7.1→+2.9
   ssim2 / +0.9 ba3n (near-parity at an arm ~0.77× our wall); s8 vs
   cpu6iq-ai +0.3→−3.6/−5.1 CROSSED; s4 vs cpu2def-ai +2.8→−0.9/−5.6
   CROSSED, vs cpu4iq-ai +1.3→−0.5/−1.1 CROSSED. The s6 column is
   converging: two of its three reference pairings now sit below the curve,
   and the third (cpu4iq) gap fell from +11.4 (plain s6-tune) → +7.1
   (size1) → +2.9 (composed).** s6 wedge recovery of the remaining step:
   interiors 60%, food 68%, 1600 50%, nps 63%, scans 183%, screens 175%,
   ALL 77%. Honest budget note: the ~1.3-1.7× per-lever aspiration is NOT
   met — the cheapest measured liveness point is 1.75-2.2×; rects-only
   (four_way_margin −1.0) at ~1.8×/−2.40 is the documented fallback. Beyond-budget arms (vg2 at
   2.45×, m32+vg2 −3.89 at 2.93× = the pareto tip recovering 104% of the remaining
   step) recorded as P2 per-image-hint targets: the partition budget is now a
   measured per-image dial, exactly what FEATURE_HINTS §E needs.
2. **Tx search**: depth-limited size RDO + reduced (not DCT-only) type sets per tier (P0's
   decomposition seeds this); aom's tx-type pruning-by-model is the reference.
3. **Intra mode budget**: replace the hard top-3 with SATD-margin adaptive budgets
   (the prescreen already ranks; prune by score gap, not fixed count).
4. **Filter schedule**: CDEF/LRF gates keyed to (tier × qindex × content) rather than the
   blanket ≲Q50 rule; the tune's CDEF-adaptive thresholds (plan item 7 leftovers) fold in.
5. Re-run the ladder after each lever; the target is monotone pareto convergence toward the
   aom-allintra curve, tier by tier (s6 first — biggest traffic relevance, worst wedges).

## Phase P2 — prediction replaces search (the hyperparameter fast mode)
`FEATURE_HINTS_PLAN.md` §E, now armed with everything it needs: 44,894 labeled rows × 111
arms (label store), the wedge maps (size/crop + fast-tier), the P0 cost grid (analysis ≈
free vs encode), and the shipped ZNPR/auto_tune runtime. Per-image heads pick
{partition budgets, tx budgets, mode budgets, tune knobs, palette gate} so an s5-class
encode approaches s2 decisions without s2 search. Threshold rules first (the palette gate
proved the pattern: one feature, LOOCV-validated, shipped), MLP heads only where thresholds
demonstrably underfit. Per-SB hints (FrameHints is live: c4047cec) follow per-image heads.

**DONE 2026-07-04 (per-image heads; record `benchmarks/rd_gap_p2heads_2026-07-04.tsv`,
full report `docs/HYPERPARAM_P2_HEADS_2026-07-04.md`).** Two heads shipped release-gated
in `src/fast_heads.rs` (pure descriptor rules, auto_tune-wired, palette-gate pattern;
MLP nowhere warranted — thresholds never demonstrably underfit at n=24):
**tx budget** {Largest|Size1|Min}: withhold size-RDO on razor-edge line tilings
(`pf>0.8505 && dcty>100` — the conjunctive dcty bound is a VAL attribution revision:
the pf-only fit false-fired on pf-high CHARTS, factoring cells put +18.1 of 8103's
+12.6 composed loss on the withhold alone), deepen to size1+types+reduced on smooth
low-α content (`pf≤0.8505 && dcty<8.352`; the type-RDO-on-sparse-AC win), s6-s8;
**partition budget** {Ship|Max32}: 32-blocks on flat/synthetic (`gfs<0.4105`), s6 —
the only LOOCV-stable partition rule (liveness pays 24/24, withhold has no stable win;
s8/s4 per-image ≈ null over the right global rung). **Head 3 (intra) is NOT a per-image
head**: top-7 keyframe intra (ComplexKeyframes+filter_intra=off) = small broad GLOBAL
win (s6 −0.56/s8 −1.17 med, composition-stable, one +1.4 regressor) → ravif SpeedTweaks
arm candidate; no top-5 knob exists (`num_modes_rdo` hardcoded 7|3) — the one missing
encoder knob P2 found. **Composed s6 mode (12q, PALCONF-clean)**: train26 −4.38 med vs
s6+size1 base (global-ship −2.89); deviating images −5.13 mean vs ship (10/11); VAL
(14 held-out origins) −3.98 med vs base, deviators −2.41 mean (6/8, worst +0.32).
**Parity: the last open fast-tier column CROSSED the band — photos vs cpu4iq-ai
+2.88/+0.91 (ship) → +0.57 ssim2 / −0.94 ba3n median, inside ±1% on both metrics
(−0.35/−1.70 with the intra arm); below the curve vs cpu4def/cpu6iq/cpu6def, and
composed-v2 (measured wall 3.45× plain-s6 ≈ 3.54 s/MP) STRICTLY DOMINATES
cpu2def-ai (better both metrics at 0.75× its time).** The intra global arm landed
ravif-side release-gated (S6_INTRA7_LIVE @4b98f0f8) but measured 1.49× marginal on
the composed mix for −0.39/−1.34 med (train/val) — a flip-decision ingredient, not
a free default. Follow-ups in the report §data-gaps (s8 m32 rung unmeasured; pf
0.85-0.99 band n=5; the remaining cpu2iq-ai +4.4 column is the s4-tier bracket at
1.27× the composed+i7 wall).

## The s4-equivalent tier — the LAST COLUMN (**CLOSED-with-residual 2026-07-04**; record `benchmarks/rd_gap_s4tier_2026-07-04.tsv`)

The one pairing above the band after P2 was aom cpu2iq-allintra: +4.40/+4.04 vs
composed-v2+i7 at 1.27× its wall (~27% budget to spend). Designed fully OFFLINE first
(`scripts/hyperparam/fit_s4_tier.py` over the 60k-row label store — zero fresh encodes
for the design): the residual map put the gap on 8414 (+22.5, lkml text = the intraBC
class), 1236/9100/9118 (the aom-iq AQ-machinery class — cpu2def trails cpu2iq by
+100/+133 BD points on exactly these images), 6096/6018 (1-bit rescans; EVERY tx lever
measured harmful on 6018), 5048/5004/1614; the knapsack over the measured per-image
surfaces bounded the closable share at ~1.5-2 BD points inside the budget (projected
+2.8 rules / +2.1 oracle) — the ±1% band was NOT projected reachable, and the box
measurement confirmed the projection almost exactly.

Measured (chain_s4tier.sh; byte-continuity gate 288/288 vs the p2heads chain; every
cell PALCONF, 0 CELLFAIL/CONFFAIL):

- **v3 rules** = v2 with ONE bound moved: the tx D gate refit at the tier budget
  (`dcty < 8.352 → 23.69`, LOOCV 22/24-stable at λ=0.5 AND 0.25; Min fires 11/24, was
  3; W and the partition gate unchanged — the λ=0.25 partition alternative gfs@0.6474
  is LOOCV-flat and fires m32 onto measured-harm 6018). 12q composed:
  **v3+i5 = THE OPERATING POINT at 6.26× plain-s6 solo (~6.42 s/MP = 0.97× cpu2iq-ai's
  wall): +2.80 ssim2 / +4.14 ba3n photos median vs cpu2iq-ai** (v2+i7 was +4.40/+4.04
  at 5.11×); vs s6+size1 base −6.64 med (v2+i7 −4.77); the cpu4iq column deepens to
  −1.53/−2.62 and cpu4def to −7.66/−7.23; vs cpu2def-ai −2.80/−2.96 both metrics.
- **The top-5 intra knob** (zenrav1e@071e9844 `num_modes_rdo_override` — the P2 report's
  one missing encoder knob, built for this column; default None byte-identical: local
  6/6 md5 + the 288/288 chain continuity gate): axis value ≈ 78% of top-7's aggregate
  (s6 −0.51 vs −0.56 med, s8 −1.09 vs −1.17, on-ship −0.57 vs −0.83) — and at MODE
  level **top-5 dominates top-7**: v3+i7 measured 7.61× plain-s6 (OVER the 6.47×
  budget) for the same column (+2.84/+4.04). The composed i7 marginal (1.22× over i5)
  buys ~nothing. The s4-tier intra arm is top-5; the s6/s8 i7 global-arm flip decision
  at the dep bump should re-weigh against this (a composed-v2+i5 s6-tier mode is the
  obvious cheaper variant — unmeasured at mode level, data-gap).
- **Hi-q filter probe** (P1 lever 4 axis, the one unswept surface): CDEF forced-on at
  every q = global null (−0.09 med; best single image −1.4) — aom-iq's CDEF edge is
  strength ADAPTATION + AQ, not enablement; LRF-on = adverse (+0.28 med, 6/24; only the
  gray-scan pair wants it, and that axis is clouded by the OPEN zenrav1e#32 LRF
  recon-desync). Neither is a tier arm.
- **full-tx oracle extras** (the labels' clean full winners 8414/6606/5048/9074/9868;
  NO honest gate at n=24): swapping their cells in reaches +2.36/+2.04 — at 10.12×
  plain-s6. Real headroom, not affordable and not deployable as a rule.
- **VAL transfer (13 scoreable of 14 origins, 12q)**: v3+i7 −7.26 med vs val-base
  (12/13, 1 veto) — BEATS v2+i7's −5.32 (13/13): the refit D bound transfers.
  v3+i5 −3.89 med with **4 bamax vetoes, all min_ship class** (2021/6621/1055/8363:
  bamax +3.8..+7.4 while ssim2 AND ba3n are −3.2..−7.3 on every one; the ba3n leg is
  13/13 better at −4.26 med) — top-5 on min-class content amplifies the type-RDO
  worst-case signature. Deployment reading: **i5 is the matched-wall operating point;
  i7 is the val-robust variant at 1.18× the reference wall**; a bamax-safe Min
  (typred/size2 shape) is the open de-risking arm.
- **s4-native ruled out**: p1part confirm-s4+prune vs cpu2iq-ai +4.22/+2.90 at ~10×
  plain-s6 (worse ssim2 than the composed mode at ~1.6× the time); zr-s4-tune
  +6.40/+6.50. The s6-mechanics composed architecture owns this tier.
- Honesty notes: six min-class 12q cells bamax-veto vs ship (the P0 type-RDO worst-case
  signature; ssim2 AND ba3n agree on the wins; 9958's veto pre-exists in v2) — adjusted
  rows bank 0 per the convention; a bamax-safe Min variant (typred/size2 shapes) is an
  open arm. Runtime: the v3 D bound ships in `src/fast_heads.rs` as the
  requested-speed-4..=5 tier (release-gated recommend-only, same as the v2 heads).

**Column verdict: ±1% NOT met — the residual is measured structural, by family:**
8100-text screens (8414 +22.5) = intraBC absence (P3 chunk B); 1200 interiors (+17.2)
and 9094 illustrations (+7.4/+2.5) = iq's AQ/deltaq machinery (tune-program-owned, not
search-budget-owned); 6000 rescans (+15.8/+7.2) = the near-lossless floor (P3); 5000
brochures (+6.8/+6.2) = partial full-tx headroom with no stable gate.

## Phase P3 — the remaining structural tools
- **intraBC chunk B (hash search) — DONE 2026-07-04** (zenrav1e@d655a6ee +
  @184eb713, release-gated; record `benchmarks/ibc_hash_ab_2026-07-04.tsv` +
  `docs/RD_GAP_VS_LIBAOM.md` "intraBC chunk B"): libaom `av1_hash_table` port
  (source-luma CRC-32C pyramid, 8..64 squares, dispersal-capped buckets, ≤64
  exact-match DVs into chunk A's SAD/RD machinery; `intrabc_hash` knob default
  true, hash-off byte-identical 81/81). Measured hash-on vs chunk A: **the
  legacy fam-7 trio where aom's hash took −33% moved −22..−29% ssim2-BD; 7058
  −36.6/−40.1; 8414 (the s4-tier #1 residual) −4.6/−5.4; photos byte-identical;
  enc median 1.00×/1.06×; 400/400 armed cells aomdec+rav1d-safe clean, no bamax
  veto.** Residual columns vs the cached cpu2iq refs (sc10 pass, intraBC-armed
  isolated config): **the 8414 +22.5 column CROSSES — −8.1/−15.1 (chunk A) →
  −13.6/−18.2 (A+B)**; 7028 +6.6→+2.2 / +9.3→+3.6; 7050 +40.8→+37.8 (odd-parity
  + near-lossless-floor remainder); 6018/6096 hash-inert (the rescan residual is
  not intraBC's). Remaining intraBC headroom: 4:1 slivers, sub-8x8, odd-DV
  chroma subpel, SB128; composed-column confirm at the dep bump.
- **Near-lossless rescans (6096/6018 +15.8/+7.2) — DIAGNOSED 2026-07-04,
  handed off per the fix-or-document rule** (full record `docs/RD_GAP_VS_LIBAOM.md`
  "Near-lossless rescans residual"): 6018 = the iq-AQ class (composed already
  beats cpu2def; aom's boost reaches deeper — {36,64} vs our {42,61}; strength
  ladder −4.14 at str4.5 vs shipped 1.0's −2.84 → tune program + per-image
  strength hint). 6096 = coefficient-level RD valuation in the 90–93 band
  (+28–30% bytes; **cpu2def ≈ cpu2iq there**; inspect: aom 0% skip at baseQ 64
  both tunes, us 57.5% skip at baseQ 54; deltaq acquitted — parity maps, and
  boost hurts 6096 at every strength) → the coefficient-level program; first
  probe = the sharpness-rounding/dead-zone A/B (`av1_build_quantizer` 48→64).
  **PROSECUTED 2026-07-04 (TUNER2, `docs/RD_GAP_VS_LIBAOM.md` "TUNER2"): four
  honest negatives.** The strength-head refit fails on train-LOOCV, on label
  drift (the 2026-07-02 strength labels are STALE — qmdist+lfsharp subsumed
  2-4 BD of the boost's marginal; 6018's deep headroom is now 0.40, not 1.3),
  and on val transfer (frozen rule regresses val). The deeper-curve ramp
  never fires on the deep-AQ class (its content is NOT low-8×8-variance).
  The 6096 dead-zone/rounding probe is rejected at both settings (QROUND=128:
  med +2.67, 20/23 vetoes; the constant does not transplant without aom's
  whole valuation stack; zenrav1e#30 item-1 rounding surface closed). Boost
  default 1.0 STANDS (18/23 train wins on current binary). Remaining owners:
  the UNPORTED iq machinery — per-16×16 ssim-rdmult curve, CDEF_ADAPTIVE
  strength schedule — plus the release-gated FrameHints/diffmap closed loop;
  and a named corpus gap (document-charts absent from train26) blocks the
  one live derivative (the anti-boost OFF-gate).
- **TX_64X16/16X64 validation** (zenrav1e#28) — unlocks the sliver cap.
- 128×128 SB support (currently hardcoded off) — large-image fast tiers.
- iq-class AQ/deltaq machinery (the 1236/9100 residual class, now + 6018) —
  tune-program follow-on to TUNE_SSIMULACRA2 (the dropped aom mechanisms are
  exactly where cpu2iq's +100-BD per-image edge on interiors lives).

## Measurement rules for this program
Time-normalized pareto is the ONLY scoreboard (speed numbers don't compare across
encoders); aom-allintra {default, tune=iq} at cpu2/4/6/8/9 are the reference arms (cached);
both metrics with veto; per-family slices; conformance at every armed config (the 6-class
corruption list — fast paths just passed 5,520 cells clean, keep it that way); the tune
stays on in every zr arm (measured mandatory + nearly-free at fast tiers).

## Success criteria — FINAL STATUS (2026-07-04, program measurement complete)
- **Parity**: zr arm within ±1% BD of the aom-allintra pareto at matched wall-time for the
  s4/s6/s8-equivalent tiers, photos median, both metrics.
  **s6 CROSSED** (cpu4iq +0.57/−0.94 composed-v2; −0.35/−1.70 with i7); **s8 CROSSED**
  (cpu6iq −3.6/−5.1); **s4-tier NOT MET** — v3+i5 +2.80/+4.14 vs cpu2iq-ai at 0.97× its
  wall; the residual is measured structural (intraBC screens / iq-AQ interiors+illustrations
  / near-lossless rescans — quantified per-family in the s4-tier section; owners are P3 +
  the tune program, not search budgets).
- **Beat**: below the curve at ≥2 tiers while keeping the quality-tip crown.
  **MET** — s6 + s8 below their curves; composed-v2 strictly dominates cpu2def-ai; v3+i5
  below the cpu4iq/cpu4def/cpu2def curves on both metrics. Quality tip **KEPT** (s1-deep
  −0.97% vs cpu0-slowest stands, untouched).
- Every landed lever byte-identical when off; no conformance regressions; honest per-family
  reporting (screens ride palette/intraBC, not the photo levers).
  **HELD** throughout: every knob None-off byte-identical (md5-gated), 288/288 s4tier
  continuity, 0 CELLFAIL/CONFFAIL across the program's chains.
