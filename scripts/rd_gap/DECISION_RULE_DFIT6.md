# Pre-registered decision rule — Phase-1 D-diagnostic #6: the sensitivity field (2026-07-12)

Registered BEFORE any neighborhood-feature correlation was computed.

**Question:** is the per-tile perceptual-sensitivity structure — the part of local
butteraugli that pixel error does NOT explain — predictable from SOURCE-ONLY features
with NEIGHBORHOOD context? If yes, the in-loop kernel is D = SSE × field(SB), where the
field is computed once per frame from the source (zero extra encodes, the FrameHints
channel), and the D-refit merges with Phase 3's allocation model.

**Data:** the existing sbmap_*.tsv (24 img × 5 q × s6; 64-px tiles with butteraugli
{p3} + mse + src_var/grad/luma). No regeneration.

**Model (two stages, both global):**
1. MSE-alone stage: log10(p3) ~ â·log10(mse) + b̂ (per DFIT4's mse-alone form).
2. Field stage on the stage-1 RESIDUAL, from SOURCE-ONLY features: tile
   {log(1+var), log(1+grad), luma} + their 3×3 neighborhood means + the frame-level
   means of the three (context normalization) — 9 features + intercept, linear LSQ.
Prediction = stage1 + stage2; statistic = per-encode |Pearson r| vs actual log10(p3)
across tiles, averaged over the 120 encodes; LOOCV by origin.

**Decision rule (tile MSE-alone baseline from DFIT4 = 0.8280):**
- "FIELD FOUND"     iff LOOCV held-out mean per-encode |r| ≥ 0.90.
- "PARTIAL FIELD"   iff 0.86 ≤ |r| < 0.90 — ship-relevant signal exists; a richer
  learned field (small MLP, multi-scale source features, zenpredict shape) is the
  registered escalation.
- "NO FIELD"        iff |r| < 0.86 — source-side prediction of sensitivity fails at
  this feature capacity; escalate directly to the learned predictor OR conclude the
  metric's non-locality requires decode-side information (two-pass territory).
Also reported (not gating): the field-alone contribution (stage-2 Δ|r| over stage 1).
