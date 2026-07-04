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

## Phase P0 — cheap wins (IN FLIGHT, fastwins agent)
- **cavif default-threading byte hazard**: default tiling costs +3.6% bytes at s6 on 48
  cores. Fix the default policy (bytes must not degrade silently with core count);
  measured pareto of tiles-vs-bytes-vs-time.
- **The s4→s6 rdo_tx cliff** (single cleanest wedge: s6×interiors +103 BD): decompose
  size-RDO vs type-RDO (the boolean couples them), find a depth-limited/reduced-set
  operating point that recovers most of the wedge inside an s6 time budget; new default-off
  zenrav1e knob if the boolean is too blunt.

## Phase P1 — the pruning-schedule rebuild (mechanism work, the core of parity)
Replace ravif's per-speed amputation table with graduated search budgets, one lever at a
time, each A/B'd at its tier's time budget (the discipline that built the quality tip):
1. **Partition search**: keep HORZ/VERT (+16-block 4-ways) live at s4-s8 with aom-style
   early-exit pruning (their `partition_search_breakout`/none-vs-split thresholds and ML
   gates at `speed_features.c:711`-family, pinned rev) instead of candidate deletion.
   The SPLIT-trial estimate (b073182c) is exactly the cost model a breakout needs — reuse it.
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
