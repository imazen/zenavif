# Hyperparameter expert — label store + first threshold-rule cuts (2026-07-03)

**Program**: FEATURE_HINTS_PLAN.md §E — per-image hyperparameter prediction where no
global optimum exists. Discipline: interpretable threshold rules on 2-3 features
FIRST; MLP heads only where thresholds demonstrably underfit. This document reports
Phase 1 (the label-store aggregator) and the three wedge-armed first cuts
(palette gate, size-conditional tune strength, per-image variance-boost strength),
fitted on TRAIN-LSD origins only.

**Headline verdicts** (detail below):

| head | rule | verdict |
|---|---|---|
| palette gate (wedge #6) | `patch_fraction > 0.197` → `PaletteMode::Always` | **STRONGEST — graduate to mechanism A/B.** LOOCV-stable, val-firing sanity clean, fires on 15/16 small wedge cells where the ported detection is dead. MLP not warranted. |
| size-conditional tune (wedge #3) | attribution, not yet a rule | **Decay narrowed, not convicted**: the 1024→512 step is entirely a high-quality-band loss on photo-like content; top suspect = ss2 QM curves. Needs the per-size isolation A/B (768 cells). |
| variance-boost strength (wedge #2) | `luma_histogram_entropy > 2.61` → 2.0 else 1.0 (best found) | **NOT deployable**: LOOCV ≈ global-1.0 (mean −2.36 vs −2.24, median regresses). Oracle headroom +0.93 concentrated in one content class. MLP not warranted at n=24 — labels underfit, not the model. |

## Phase 1 — the label store

`/mnt/v/output/zenavif/hyperparam-labels-2026-07-03/labels.parquet` — **14,880 rows × 34
cols, 50 arms** (Tower mirror sha-verified). Builder:
`scripts/hyperparam/build_label_store.py` (append protocol in its docstring +
`_MANIFEST.json`); shared fit helpers: `scripts/hyperparam/hp_common.py` (imports
`bd_arm.py` BD conventions and the canonical `origin_split.py` — never re-implements
either).

Rows per source / feature-join coverage:

| sweep_source | rows | corpus | feature_join |
|---|--:|---|---|
| tune-ss2-2026-07-02 (9 arms) | 2,376 | legacy22 | NULL (not in imazen-26) |
| deltaq-2026-07-02 (12 arms incl 3 aom refs) | 3,072 | train26 + legacy22 | train26 rows 100% |
| qmdist-2026-07-03 (12 arms) | 2,400 | train26 + legacy22 | train26 rows 100% |
| lfsharp-2026-07-03 (12 arms) | 2,784 | train26 + legacy22 | train26 rows 100% |
| desyncfix-2026-07-03 (2 arms) | 576 | train26 | 100% |
| wedge-2026-07-03 (4 arms) | 2,952 | wedge26 | 100%, pixel-exact |
| palette-ab-final2-2026-07-03 (6 arms) | 720 | train26 | 100% |

Coverage totals: train26 6,912/6,912 joined (`feature_join_exact=false` — the sweep
corpus is vipsthumbnail-linear, features are Lanczos3-sRGB; same origin + size class,
WxH verified exact on all 24 origins); wedge26 2,952/2,952 joined exact (the wedge
program's 123/123 pixel verification); legacy22 5,016 rows NULL (documented).

Honesty contract (full text in `_MANIFEST.json` + builder docstring):
- **encoder_rev** per row — arm deltas valid within one `sweep_source`; verified
  cross-sweep byte-continuities listed in the manifest.
- **q_kind** per row (`cavif_q` 0-100 ↑ / `aom_cq` 0-63 ↓ / `rav1e_quantizer` 0-255 ↓)
  — never pool q across kinds.
- **split** from the canonical LSD origin rule. The features parquet's own `split`
  column is an OLDER convention that disagrees on **1,148/2,157 origins — do not use
  it** (measured during the build).
- palette-ab rows were scored through a different pipeline (rav1e CLI, color.py 420
  y4m, aomdec): absolute scores not comparable to cavif rows; within-source deltas
  valid.
- `enc_ms` reliability varies per source (contention, different hosts) — per-source
  notes in the manifest.

## Rule 1 — zenanalyze palette gate (wedge #6): **the graduating head**

**Fit script**: `scripts/hyperparam/fit_palette_gate.py`; eval TSVs:
`benchmarks/hyperparam_palette_gate_2026-07-03.tsv` (+ `_wedge.tsv`).

**Labels.** palette-ab-final2 train26 @1024-rendition, s2+s6: per-(origin,speed)
direct BD of always-vs-off and auto-vs-off. Palette wins (bd_always ≤ −0.5) on
**15/24 origins at s2 and 19/24 at s6**; the ported AA-detection captured only 10 and
8 of those respectively. Missed-win magnitude is speed-dependent: s2 mean −0.74
(max 9074 −4.55), **s6 mean −4.90** (7058 −28.6, 6096 −21.9, 5048 −14.0 available
while auto captured ~0). Wedge zr-vs-paletteoff supplies the size axis: the ported
detection fires on 17/40 native cells but only 5/16 at ≤512 (byte ratio ~1.000 — dead).

**Butteraugli discipline.** 6 winner cells are per-cell butteraugli-VETOED (5004_s2,
6018_s2, 6096_s2, 9678_s2, 6606_s6, 9228_s6 — palette banding on gray scans/products
games ssim2). The fit objective refuses to bank vetoed wins (`max(bd,0)` when fired on
a vetoed cell).

**Objective** (deployment-honest): mean veto-adjusted BD of FIRED cells with **no
Auto rescue** — the regime this gate exists for is downscaled screen content where
detection is dead. Specificity tiebreak: within 0.1 BD of best, fewest fires (every
fire costs palette-search time: measured always/off enc_ms **s2 med 1.07×, s6 med
1.80×**).

**RULE: fire `PaletteMode::Always` iff `patch_fraction > 0.197`.**

- fit −11.00 vs fire-everything −11.05 (mean BD, n=48 cells) at far fewer fires;
  **LOOCV (leave-one-origin-out) −10.91** — stable.
- Confusion vs "palette actually won": 30 fire&won, 2 fire&lost (both ≈0 cost:
  5048_s2 −0.17, 8196_s2 −0.00), 4 miss&won (5004 s2/s6, 6606_s6, 9118_s6 — modest,
  and 5004_s2/6606's wins are the butteraugli-gamed kind), 12 quiet&lost.
- **Size transfer measured**: the gate's features keep their screen-vs-photo
  separation at every size (entropy medians 2.05→2.75 screen vs ~4.15 photo;
  patch_fraction ≥0.44 screen vs ≤0.008 photo at 256) — it sees "this WAS screen
  content" through the resample. On the wedge paletteoff subset the gate fires
  40/40 native (17/17 agreement where detection fired) and **15/16 at ≤512 where
  detection fires 5/16**.
- **Val-split sanity** (feature rows only — no val RD labels exist): firing rates
  0.00-0.03 on every photo class (1000/1200/1400/1600/2000/2400/3000/3300), 0.83-1.00
  on screens/plots/patents/docs (5300/6000/7000/8000/8100/9000), and 0.00 on the two
  classes where forced palette LOSES (9094 AI-illustrations, 6600 scan-illustrations)
  — the single threshold rejects exactly the right content.

**MLP verdict: NOT warranted.** The single threshold is within 0.05 BD of the
fire-everything ceiling on these labels; residual error is missing labels (forced
palette at ≤512; shipped-config cavif arms), not model capacity.

**Data needs before landing** (the mechanism A/B this graduates to):
1. palette {off, always} × {256, 512} on the wedge fired-class subset (the ≤512
   recovery is currently *bounded by 1024 measurements, unmeasured directly*);
2. the same A/B under the SHIPPED cavif config (the labels come from the isolated
   rav1e CLI config — transfer untested);
3. a val-origin palette A/B for honest held-out numbers.
Wiring shape: zenavif `auto_tune`/expert plumbs `PaletteMode::Always` when the gate
fires (ravif already exposes the palette knob; feature cost is a Tier-1 zenanalyze
call, ≤14 ms at 4 MP per the P0 cost grid).

## Rule 2 — size-conditional tune strength (wedge #3): attribution

**Script**: `scripts/hyperparam/fit_size_decay.py`; TSV:
`benchmarks/hyperparam_size_decay_2026-07-03.tsv`.

**Stated up front**: no per-mechanism arms exist below 1024 (every mechanism sweep ran
at the 1024 rendition scale), so existing data yields a measured NARROWING + an A/B
spec, not a conviction.

**q-band decomposition** (bpp gap vs cpu2 at the 25%/75% points of the overlapping
ssim2 window, medians, full crops, fam-7000 plots excluded):

| slot | n | BD med | low-q band | high-q band |
|---|--:|--:|--:|--:|
| top | 12 | −13.82 | −21.51 | −7.87 |
| 1024 | 11 | −13.04 | −15.71 | −5.11 |
| 512 | 12 | −7.89 | **−15.26** | **+0.55** |
| 256 | 12 | −0.79 | −1.88 | −2.61 |

**The 1024→512 decay is ENTIRELY a high-quality-band (low qindex) loss** — the low-q
band holds. 512→256 then collapses both bands. Decay is content-selective:
per-origin decay slopes concentrate on photo-like content (spearman vs slope:
luma_histogram_entropy **+0.63**, patch_fraction −0.55, grayscale_score −0.49;
steepest: 8196 screenshot +3.24 — compounded by palette-detection death — then
1480 nature +3.03, 9098 ai-illustr +2.98, 9446 products +2.27).

**Ranked suspects** (fingerprints = each mechanism's per-family 1024 win from the
store): **(1) ss2 QM level curves** — the dominant 1024 mechanism on exactly the
decaying photo families (legacy fams 1/3/5/6 medians −9.9..−12.6) and QM acts where
quantization is fine, i.e. the band that collapses first; (2) variance-boost/activity
scaling (8×8 activity stats see a compressed spectrum after downscale; −2.0..−5.6
fingerprints); (3) chroma delta-q clamp (qindex-proportional — small in the
collapsing band); (4) LF sharpness (right band, but −0.5..−0.8 fingerprint cannot
explain a 5-point swing). Confound stated: cpu2's own small-size behavior; the
tune-off A/B arm separates it. At 256 partition/coding defaults join the suspects.

**Measured proposal (NOT landed)**: `m(px) = clamp((log2(px) − 16) / 4, M256, 1.0)` —
full tune strength ≥2^20 px, log-px linear ramp down to M256 at 2^16 px, applied to
whichever constants the A/B convicts; M256 candidates {0, 0.25, 0.5}.

**Data need**: {tune-off, +chroma-dq, +QM-curves, +boost1.0} × {256, 512} on the
wedge full-crop corpus — 16 origins × 6q × 4 arms × 2 sizes = **768 cells**,
cell-cache cheap; the same isolation ladder the 1024 program used with one size axis
added.

## Rule 3 — per-image variance-boost strength (wedge #2): not deployable yet

**Script**: `scripts/hyperparam/fit_boost_strength.py`; TSV:
`benchmarks/hyperparam_boost_rule_2026-07-03.tsv`.

**Labels**: deltaq-2026-07-02 strength arms {0,1,2,3,4.5,6} on train26 (24 origins,
s2+tune, 12-pt Q), per-image direct BD vs strength-0. Label structure:

- global 1.0 (shipped): mean −2.24 / median −2.34 (butteraugli-clean);
- oracle within {0,1,2}: mean −3.18 (**headroom +0.93**, dominated by 5004: str1
  −2.62 → str2 −15.04); oracle all-strengths −3.51;
- "always 2.0" beats "always 1.0" on the mean (−2.50 vs −2.24) — matching the
  original sweep, which tie-broke to 1.0 on median+butteraugli;
- 8 origins prefer OFF, 8 keep 1.0, ~6 want ≥2 — but classes are NOT feature-monotone
  (best single-feature class correlation |ρ|=0.47); butteraugli BD is undefined
  (degenerate frontier) on 5004 + parts of 8302 — nanmedian veto, counted in the TSV.

**Best rules found** (pre-registered 6-feature shortlist; policy-level butteraugli
veto): 1f `luma_histogram_entropy > 2.61 → 2.0 else 1.0` (resub mean −2.92);
2f `aq_map_std > 2.51 → 0 elif entropy > 2.61 → 2 else 1` (−2.96). **LOOCV: 1f −2.36
/ 2f −2.24 vs global-1.0 −2.24 — within noise, and the LOOCV median REGRESSES
(−1.75 vs −2.34).** Verdict: the threshold rule does not robustly beat the shipped
global 1.0 on n=24.

**fam-9226 does NOT improve**: 9228/9868 stay flat or regress under LOOCV (+1.05,
+1.33), 9958 gets mispredicted to 0 (loses its −3.35). Cross-head insight: the 9226/
clipart losses that motivated wedge #2 are palette-shaped, not boost-shaped — 9074
had −4.55 (s2) / −9.58 (s6) available from FORCED PALETTE while its boost oracle was
0; the palette gate fires on 84-100% of 9226 val rows. The remaining 9226
smooth-gradient residual (9908-class +78 c50_br) needs the QM-discount /per-SB
delta_q mechanisms, not a global strength head.

**MLP verdict: NOT warranted — the LABELS underfit, not the model.** n=24 with one
dominant winner is memorization bait. Data needs before revisiting: (a) strength
arms on val origins (honest eval), (b) wider train coverage (wedge K=16 origins at
1024), (c) densified low range {0.5, 1.0, 1.5, 2.0, 2.5} — the response is
inverted-U with instability above 3 (5048: str2 vetoed, str3 clean).

## Recommended next dispatch

1. **Palette-gate mechanism A/B** (graduates first): palette {off, always} ×
   {256, 512, 1024} × {isolated, shipped-cavif} on the wedge fired-class subset +
   val-origin cells. Cheapest measured-upside head: s6-class speeds have −4.9 mean
   BD sitting behind a dead detector, and the gate's false-fire cost is ≈0 BD +
   1.07-1.8× encode time on fired cells only.
2. **Size-decay isolation A/B** (768 cells) to convict the QM-curve suspect, then
   calibrate M256 for the proposed log-px ramp.
3. Boost-strength head: parked until the val + dense-strength labels exist (fold
   into the next box sweep as extra arms; the store's append protocol takes them
   directly).
