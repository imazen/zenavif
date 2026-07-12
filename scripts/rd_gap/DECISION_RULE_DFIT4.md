# Pre-registered decision rule — Phase-1 D-diagnostic #4: kernel ingredients (2026-07-12)

Registered BEFORE the feature-extended sbmap dataset finished generating.

**Question:** do cheap per-tile SOURCE features close the MSE→butteraugli prediction
gap — i.e. what does the NEW kernel (DFIT3's routing) need to contain? If
{pixel error + activity/masking features} predicts local butteraugli well, the kernel
is an activity-normalized error computable in-encoder without running the metric.

**Data:** traces-scored-2026-07-12 regenerated with per-tile {mse, src_var (luma
variance), src_grad (mean |∇luma|), src_luma} alongside butteraugli {mean,p3,max};
IVFs now persisted (feature re-passes = decode-only).

**Model:** global linear least squares on
  log10(p3) ~ a·log10(mse) + b·log10(1+src_var) + c·log10(1+src_grad)
             + d·(src_luma/255) + e
fit on all tiles of all encodes; LOOCV over the 24 origins. Statistic (comparable to
DFIT3): held-out per-encode |Pearson r| between model prediction and actual log-p3,
averaged over the 120 encodes.

**Decision rule (MSE-alone baseline from DFIT3 = 0.8355):**
- "INGREDIENTS FOUND"  iff held-out mean per-encode |r| ≥ 0.90.
- "PARTIAL"            iff 0.86 ≤ |r| < 0.90 — features help; per-block (sub-tile)
  or frequency-domain features are the next axis before encoder work.
- "INSUFFICIENT"       iff |r| < 0.86 — tile-level source stats don't carry the
  signal; the kernel needs per-block pixel/transform features extracted in-encoder.
Coefficient signs are reported (masking predicts b<0: high-variance tiles tolerate
error). No ship decision rides on this; it shapes the kernel the fit-then-port step
builds.
