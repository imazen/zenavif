# Pre-registered decision rule — Phase-1 D-diagnostic #2: reweighting (2026-07-12)

Registered BEFORE any reweighted correlation was computed.

**Question:** can a per-blocksize reweighting of the EXISTING D currency (surviving
scopes only) close the cross-image ranking gap to raw MSE — i.e. is the DFIT1 failure
a CALIBRATION problem (fixable by weights on the current kernel) or a KERNEL problem
(the per-block distortion itself doesn't carry the perceptual signal)?

**Data:** the same scored corpus (traces-scored-2026-07-12, 24 img × 5 q × s6). Model:
log10(Σ_b w_bsize · D_b / px) per encode, weights shared across ALL images and
quantizers (one global weight per bsize class — deliberately tiny capacity, n=24
origins; the MLP-at-small-n lesson). Fit: maximize mean cross-image |Pearson r| vs
ssim2 over the 5 quantizers, LOOCV over origins (leave-one-origin-out; report
held-out r).

**Decision rule:**
- "CALIBRATION FIXES IT" iff the LOOCV held-out mean cross-image |r| of the
  reweighted D ≥ MSE's mean |r| − 0.05 (MSE from DFIT1: mean ≈ 0.807).
- "KERNEL PROBLEM" iff the reweighted D's held-out mean |r| improves on raw D by
  < 0.10 OR stays ≥ 0.15 below MSE's — then the Phase-1 D-refit needs per-block
  metric data (diffmap pooling), not weights.
- The in-between (improves ≥ 0.10 but stays > 0.05 below MSE) = "PARTIAL — weights
  help, kernel still binding"; proceed to diffmaps with the weights kept.

Failure-mode guard: weights are constrained non-negative; if the optimizer drives
most bsizes to ~0 (degenerate selection of one class), report that explicitly — it
means "which blocks" carries the signal, not "how much distortion," which is itself
a kernel finding.
