# SSIMRD decision rule — PRE-REGISTERED 2026-07-05 before any arm data

Program: the per-16×16 ssim-rdmult λ scaling port (aom
`av1_set_mb_ssim_rdmult_scaling` / `av1_set_ssim_rdmult` at rev 632172a4;
TUNE_SSIMULACRA2_PLAN.md §(a2), the TUNER2-named remaining iq-AQ owner).
Knob: `zenrav1e EncoderConfig::ssim_rdmult_strength` (exponent blend on the
frame-geomean-normalized factor; 1.0 = aom curve verbatim; None/0 = off,
byte-identical). Registered BEFORE the coarse arms ran (sizedecay
precedent). AMENDED (still pre-arm-data) per the 2026-07-05 user
correction on train26 evaluation policy — see "Aggregation policy".

## Aggregation policy (USER CORRECTION 2026-07-05, applied to all verdicts)

train26's k-means subset is DIVERSE, not REPRESENTATIVE: one pick per
cluster regardless of cluster mass, so an unweighted all-24 median
over-weights rare classes and dilutes photo-dominant effects. Therefore:

1. **Per-family verdicts FIRST.** Families (train26 origins, cluster mass
   from `/mnt/v/output/rd-gap-train26-2026-07-02/_MANIFEST.json`):
   photos {1236:56, 1438:43, 1614:100, 2000:21, 5004:14, 5048:45},
   products/gen {9958:117, 9868:109, 9678:84, 9228:50, 9074:50},
   screenshots {8268:115, 8196:78, 8302:50, 8414:8},
   illustrations-9094 {9100:38, 9118:17},
   scans {6096:32, 6018:12, 6606:2},
   plots {7028:25, 7058:14, 7052:1, 7050:1}. Total mass 1082.
2. **Aggregates are cluster-mass-weighted** (weighted median over
   per-image BD, weights = cluster_size). Never keep/drop on the
   unweighted all-24 median.
3. **Photos-only merit is KEEPABLE**: a lever that wins on the photos
   family with other families neutral is a shippable result (global
   default if others are neutral; otherwise a content-gated /
   feature-hints verdict) — do not reject photo wins because
   diverse-subset dilution drowns them.

## Winner selection (coarse phase, t26 6q, arms {0.25, 0.5, 1.0, 2.0})

The strength with the best cluster-mass-weighted train26 median ssim2 BD
vs the same-binary env-off base, restricted to arms where NO family's
butteraugli medians (3n AND max) exceed +0.50 and the mass-weighted
butteraugli medians are ≤ +0.30. Tie within 0.1 BD → the LOWER strength.
If no arm clears the mass-weighted bar but some arm wins the photos
family cleanly (photos median ssim2 ≤ −0.30, photos butteraugli ≤ +0.30,
other families ≤ +0.30 ssim2), that arm advances as a PHOTOS-MERIT
candidate.

## Ship bar (ALL must hold at WINNER, full 12q)

1. TRAIN (t26) mass-weighted median ssim2 BD ≤ −0.30% — OR the
   photos-merit form: photos-family median ≤ −0.30 with every other
   family median ≤ +0.30.
2. TRAIN butteraugli: mass-weighted median ba3n ≤ +0.30 AND bamax
   ≤ +0.30; no family's ba3n/bamax median > +0.50. Per-cell
   fire-conservative veto: an image with ssim2 BD < 0 but ba3n BD > +1.0
   is vetoed — banked as 0 in win counts.
3. VAL confirm, same per-family shape (val families: photos {1055, 2021},
   charts {5343}, scans {6091, 6621}, plots {7053, 7071}, screenshots
   {8015, 8103, 8363}, gen {9021, 9165, 9631, 9905}): the winning
   famil(ies) reproduce direction on val members; no val family regresses
   beyond +0.50 with butteraugli agreeing.
4. s6 spot check: per-family BD(s6+winner vs s6 base) — no family beyond
   +0.50; mass-weighted median ≤ +0.30 (must not regress the composed
   fast mode).
5. Conformance: 0 CELLFAIL / 0 CONFFAIL across every armed cell
   (PALCONF: aomdec decode + aomdec/rav1d-safe raw md5 agreement).
6. Timing: median solo-wall overhead ≤ 1.10× vs base.

Global pass → land default-ON under `Tune::Ssimulacra2` (fitted const,
knob stays as override), release-gated, standard byte gates (tune-off
identical to master; landing binary reproduces the measured winner-arm
md5s). Photos-merit pass → land the knob default-off + record the
photos-family verdict as a content-gate / feature-hints candidate (the
per-image head channel). Fail → land the default-off knob + the
honest-negative record (TUNER2 pattern). Either way the TSVs, docs
(TUNE_SSIMULACRA2_PLAN (a2) status + RD_GAP_VS_LIBAOM), and label-store
append land.

## Reported either way (NOT ship-gating)

Class movement vs aom-cpu2iq-ai (the TUNER2 residual owners): per-image
BD for 1236, 6018, 9100, 9118 (train) and 9165, 6091 (val), winner vs
base. Also BD vs cpu2def-ai for the def→iq attribution.

FYI noted from the correction: the TUNER2 "no document-chart class" gap
is a subset artifact (imazen-26 has document charts; a supplemental
sample is being built) — unaffected here, recorded for the next
train-corpus revision.
