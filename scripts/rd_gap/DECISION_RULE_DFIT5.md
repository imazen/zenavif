# Pre-registered decision rule — Phase-1 D-diagnostic #5: committed-block level (2026-07-12)

Registered BEFORE any per-block correlation or model fit was computed (the blockmap
driver had produced only the single smoke cell when this was committed).

**Question (two parts), at COMMITTED-block granularity — the D currency's exact unit:**
1. Confirmation: does winner-D predict per-block butteraugli-p3 better or worse than
   per-block MSE? (DFIT3 said worse at 64px tiles; blocks are the native unit.)
2. Kernel candidate: does the block-level feature model
     log10(ba_p3) ~ a·log10(mse) + b·log10(1+src_var) + c·log10(1+src_grad)
                   + d·(src_luma/255) + per-BSIZE-class intercepts
   reach INGREDIENTS-FOUND? (bsize intercepts encode the DFIT2 block-class signal.)

**Data:** traces-scored-2026-07-12 + blockmap_*.tsv (butteraugli_blockmap.rs over the
kept IVFs: per committed block ba{mean,p3,max}, mse, src_var/grad/luma; joined to
winner-D from the trace by (bo, bsize); 1947-6000 blocks per encode × 120 encodes).

**Statistics:** per-encode Pearson |r| across its committed blocks, averaged over
encodes; model fits global, LOOCV by origin.

**Decision rules:**
- Part 1: same D-BETTER / D-WORSE / TIE margins as DFIT3 (0.02, majority).
- Part 2: "INGREDIENTS FOUND" iff LOOCV held-out mean per-encode |r| ≥ 0.90;
  "PARTIAL" 0.86–0.90; "INSUFFICIENT" < 0.86 → the offline-feature path is
  exhausted; route to in-encoder/transform-domain features or the learned per-block
  predictor (plan item 7), designed in a fresh registration.
