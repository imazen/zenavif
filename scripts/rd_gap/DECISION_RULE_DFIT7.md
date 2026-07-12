# Pre-registered decision rule — Phase-1 D-diagnostic #7: the learned field (2026-07-12)

Registered BEFORE the multi-scale sbmaps finished generating and before any MLP was fit.

**Question:** does nonlinear capacity + multi-scale source context close the sensitivity
prediction gap that linear/64px could not (DFIT6: LOOCV 0.853 vs the 0.86/0.90 floors)?

**Data:** the existing 64px sbmaps + NEW 16px and 32px sbmap passes (decode-only from
persisted IVFs). Per 64px tile: {var, grad, luma} at 3 scales (16/32 aggregated up:
mean + max of the sub-tiles inside the 64px tile), + 3×3 neighborhood means at 64px, +
frame means — the multi-scale surround-masking feature set (~20 features).

**Model:** stage-1 mse-alone (as DFIT6), stage-2 residual via a SMALL MLP
(1 hidden layer, ≤16 units, tanh, L2, deterministic seed) — deliberately tiny per the
MLP-at-small-n lesson. LOOCV by origin; statistic = held-out per-encode |r| of the
combined prediction, as before.

**Decision rule:**
- "LEARNED FIELD FOUND"   iff LOOCV ≥ 0.90 → bake path (zenpredict) + FrameHints consumer.
- "PARTIAL"               iff 0.86 ≤ LOOCV < 0.90 → ship-relevant; corpus-widening pass
  (the imazen-26 145 train candidates) before bake.
- "TRANSFER-BOUND"        iff LOOCV < 0.86 AND the full-fit (train) |r| ≥ 0.90 —
  **the registered PREDICTION from the q0-head lesson: at n=24 origins, origin transfer
  binds before capacity.** Routing: widen the ORIGIN set (canonical corpora, 100+
  origins) and refit at the SAME tiny capacity — data before parameters.
- "CAPACITY-IRRELEVANT"   iff LOOCV < 0.86 AND full-fit < 0.90 — even in-sample the
  features don't carry it; route to decode-side (two-pass) or transform-domain features.
