# Pre-registered decision rule — Phase-1 D-diagnostic #3: per-tile (2026-07-12)

Registered BEFORE the sbmap dataset finished generating (the per-tile joins had
not been computed when this file was committed).

**Question:** at the BLOCK level — the granularity the D currency actually operates
at — does surviving-winner D predict local perceptual error (per-tile butteraugli)
better than local pixel error (per-tile RGB MSE) does?

**Data:** traces-scored-2026-07-12 (24 img × 5 q × s6): per 64-px tile, butteraugli
{mean, p3, max} + RGB MSE (butteraugli_sbmap.rs) joined to tile-aggregated surviving
winner-D from the traces (scope bo·4/64 → tile index). Per encode: correlations
ACROSS its tiles (~192 tiles at 1024px), Pearson on log10 scales; primary target =
tile butteraugli p3; primary predictors = log tile-ΣD vs log tile-MSE.

**Decision rule (mean over the 120 encodes of per-encode |r|):**
- "D-BETTER per-tile" iff mean|r_D| − mean|r_MSE| > 0.02 AND D wins on > 50% of encodes.
- "D-WORSE per-tile" symmetric.
- else "TIE".

**Registered interpretation matrix (with DFIT1's cross-image result, where D LOST):**
- D-BETTER here → the kernel is locally perceptual but mis-normalized ACROSS content;
  the refit targets per-image/per-content D normalization (cheap) before any new kernel.
- TIE/D-WORSE here → the kernel itself fails at its own granularity; the refit needs a
  new distortion kernel fit against per-tile metric targets (the full Phase-1 arc).

Secondary (reported, not gating): the DFIT2 bsize-weighted D as a third predictor;
tile-butteraugli mean and max as alternate targets.
