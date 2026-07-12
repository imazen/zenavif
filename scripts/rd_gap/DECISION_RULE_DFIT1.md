# Pre-registered decision rule — Phase-1 D-diagnostic #1 (2026-07-12)

Registered BEFORE the scored corpus finished generating (traces-scored-2026-07-12;
the correlations had not been computed when this file was written).

**Question:** does the encoder's own distortion currency — surviving Σ D per frame,
from the decision trace — track the perceptual metric (ssim2) better than raw pixel
error (RGB MSE, same decode, same owned color transform)?

**Data:** train26 24 images × zenrav1e quantizers {40,60,100,160,220} × s6,
tune-ss2, threads=1. Per encode: d_surviving (fit_trace_d), ssim2 + mse (manifest).

**Tests:**
1. *Cross-image, per quantizer* (the interesting axis — within-image monotonicity
   across q is trivially expected): Pearson r of ssim2 vs log10(D_surv/pixel) and of
   ssim2 vs log10(MSE), across the 24 images at each fixed q.
2. *Within-image, across q* (reported, not gating): per-image Pearson r on the same
   log scales, 5 points each.

**Decision rule:** "the D currency beats raw MSE cross-image" iff
|r(log D_surv/px)| > |r(log MSE)| at **≥ 4 of the 5 quantizers** (test 1).
- If TRUE: the psy-weighted D already embeds perceptual structure beyond pixel
  error; Phase-1's D-refit quantifies and closes the remaining gap to the metric.
- If FALSE: the D currency is no better than (or worse than) plain pixel error at
  explaining perceptual quality across content — large Phase-1 headroom, and the
  D-refit becomes the top-priority lever.

No ship decision rides on this diagnostic; it directs Phase-1 effort. Either
outcome is recorded in benchmarks/ with this rule referenced.
