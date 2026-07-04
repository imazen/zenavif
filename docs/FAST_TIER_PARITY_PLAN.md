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

## Phase P3 — the remaining structural tools
- **intraBC chunk B** (hash search): −33% evidence on repeating content (zenrav1e#30);
  chunk A landed. Matters for screens at every tier.
- **TX_64X16/16X64 validation** (zenrav1e#28) — unlocks the sliver cap.
- 128×128 SB support (currently hardcoded off) — large-image fast tiers.

## Measurement rules for this program
Time-normalized pareto is the ONLY scoreboard (speed numbers don't compare across
encoders); aom-allintra {default, tune=iq} at cpu2/4/6/8/9 are the reference arms (cached);
both metrics with veto; per-family slices; conformance at every armed config (the 6-class
corruption list — fast paths just passed 5,520 cells clean, keep it that way); the tune
stays on in every zr arm (measured mandatory + nearly-free at fast tiers).

## Success criteria
- **Parity**: zr arm within ±1% BD of the aom-allintra pareto at matched wall-time for the
  s4/s6/s8-equivalent tiers, photos median, both metrics.
- **Beat**: below the curve at ≥2 tiers while keeping the quality-tip crown.
- Every landed lever byte-identical when off; no conformance regressions; honest per-family
  reporting (screens ride palette/intraBC, not the photo levers).
