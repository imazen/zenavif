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
| palette gate (wedge #6) | `patch_fraction > palette_gate_threshold(speed)` → `PaletteMode::Always` (0.197 at s≤5, 0.05 at s≥6) | **GRADUATED — mechanism A/B CONFIRMED on val 2026-07-03; SPEED-CONDITIONAL threshold measured + shipped same day (s6/s8 confirm τ=0.05, s2 keeps 0.197 — second status block in the rule-1 section); wiring landed release-gated (`src/palette_gate.rs` + `auto_tune`).** LOOCV-stable, val-firing sanity clean, fires on 15/16 small wedge cells where the ported detection is dead. MLP not warranted. |
| size-conditional tune (wedge #3) | qmdist ramp m=clamp((log2(longedge)−8)/2, 0.5, 1.0) | **A/B RAN 2026-07-03 — top suspect (ss2 QM curves) ACQUITTED; the QM-dist ratio convicted, its half-strength size ramp SHIPPED (zenrav1e@b0098eb1: train +1.03/+0.87 @256/512 vs full, VAL +1.12/+1.00, butteraugli agreeing). Most of the vs-cpu2 decay is the tune-OFF baseline's own (see the rule-2 STATUS block).** |
| variance-boost strength (wedge #2) | `luma_histogram_entropy > 2.61` → 2.0 else 1.0 (best found) | **NOT deployable**: LOOCV ≈ global-1.0 (mean −2.36 vs −2.24, median regresses). Oracle headroom +0.93 concentrated in one content class. MLP not warranted at n=24 — labels underfit, not the model. |

## Phase 1 — the label store

`/mnt/v/output/zenavif/hyperparam-labels-2026-07-03/labels.parquet` — **14,880 rows × 34
cols, 50 arms** at first cut; **28,008 rows / 75 arms** after the same-day palette-mech
(+6,216) and size-decay (+6,912, `sweep_source=sizedecay-2026-07-03`, 100% feature-join)
appends; **29,358 rows / 78 arms** after the s8 palette corroboration append
(`palette-mech-iso-s8-2026-07-03`, +1,350, 100% feature-join) (Tower mirror
sha-verified). Builder:
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

**STATUS 2026-07-03 (later) — mechanism A/B RUN, all three data needs met,
CONFIRMED on val; wiring LANDED release-gated.** Full record:
`benchmarks/hyperparam_palette_mech_ab_2026-07-03.tsv` (+ `_timing`),
analysis `scripts/hyperparam/analyze_palette_mech_ab.py`, raw + manifest
`/mnt/v/output/rd-gap-palette-ab-2026-07-03/`, val corpus (14 VAL-LSD origins,
join-verified 108/108) `/mnt/v/output/rd-gap-palette-val-2026-07-03/`; label
store gained `palette-mech-ab` (shipped cavif s2 12-pt + s6 6-pt) +
`palette-mech-iso` (rav1e CLI s2+s6) — 6,216 rows, 100% feature-join.
Headline findings:

- **Transfer to the shipped config holds, and the win concentrates where
  detection is dead.** Shipped s6 val, gate-fired classes: 6000 patents @1024
  auto +0.04 (dead) → rule **−39.5**; 8100 screenshots rule −28.5/−16.5/−15.6
  at 1024/512/256 (auto −12.6/−0.3/−0.0); 9000 clipart −12..−22.6 (auto ≈0);
  9226 products −4.1..−5.8. Shipped s2 val: 8100@1024 −9.94 vs auto −5.34,
  6000@1024 −3.97 vs −0.54, 7000@1024 −15.2 vs −12.9.
- **The ≤512 recovery is speed-dependent**: real at s6 (above), real-but-small
  at 512 s2 (−0.6..−3.2), ≈0 at 256 s2 — at 256 the RDO itself declines
  palette blocks on Lanczos-softened content (off==always byte-identical on
  many cells), so the gate neither wins nor costs bytes there.
- **Confusion (val)**: s2 17 fire&won / 12 fire&lost (losses tiny: median
  +0.27, max +3.46 — the one butteraugli-vetoed cell 5343@256), **0 miss&won**;
  s6 27 fire&won / 1 fire&lost (+0.46 max). The s6 miss&won cells (6621, 9165,
  1055, 9905@256: −0.7..−4.5 unclaimed) are quiet-class content where forced
  palette wins *only at s6* — conservative-gate upside, not harm.
- **False-fire cost**: photos never fire (0/12 val photo cells); fired-file
  encode time median **1.06×** p90 1.16× max 1.19× (RD_CACHE=off, idle box) —
  cheaper than the 1024-label estimate (1.07-1.8×).
- **Threshold**: train + pooled refits KEEP 0.197 (shipped pooled refit =
  IDENTICAL fire set at τ=0.1963); val-only refits move DOWN to 0.046-0.066
  (+0.165/+0.184 mean BD available) — **entirely an s6 phenomenon** (at s6
  palette wins even on quiet classes). Shipped threshold stays 0.197; a
  speed-conditional threshold (fire more at s≥6) is the documented follow-up.
- **Conformance: zero corruption anywhere.** 1800/1800 isolated palette-armed
  cells aomdec+rav1d-safe raw-md5 agree; every shipped always/auto cell passed
  the new PALCONF gate (`zenrav1e_cell.sh`).

**STATUS 2026-07-03 (later still) — the speed-conditional follow-up RAN and
SHIPPED.** Script `scripts/hyperparam/fit_palette_speed_threshold.py`, record
`benchmarks/hyperparam_palette_speed_ab_2026-07-03.tsv`. Arms τ {0.197, 0.10,
0.05, fire-always} × s{2,6,8}. **391 of the 481 evaluated BD cells came from the
store/TSV with zero fresh encodes** (the other 90 derive from the single
fresh s8 run below) — both palette outcomes (off/always/auto)
were already measured per (file, speed), so a threshold arm is a pure per-cell
selection (fire → veto-adjusted always, quiet → auto). The only fresh data:
the s8 corroboration run (90 files × 3 arms × 5q = 1,350 cells, 72 s on the
snapshot-restored box, binary byte-continuity sha-proven against the mech
run's kept 7052 IVF, 0 conformance failures, 900/900 armed cells md5-agree;
appended as `palette-mech-iso-s8-2026-07-03`, IVFs in `ivf_s8/`). Two
objectives reported, DEPLOY (fire→always_adj, quiet→auto — the honest
realized-BD view) decides; the fit view phantom-credits cells whose auto
detection already captured the identical win (e.g. 9905.256 s6:
bd_auto == bd_always, pf 0.1935).

| speed | verdict | deploy-mean vs 0.197 | evidence |
|---|---|---|---|
| s2 | **KEEP 0.197** | t0.05: +0.008 train / −0.028 val (one iso cell, contradicted by the shipped config: 6621@{512,1024} +0.41/+0.71; median +0.000; 1 vetoed flip) | refit plateau keeps 0.197 at all splits |
| s6 | **τ=0.05 CONFIRMED** | −0.047 train / −0.074 val; train median −2.69→−3.52 | flips 0-vetoed, butteraugli agrees on every winner (6600-class) |
| s8 | **τ=0.05 corroborated** | ±0.000 train (flip Δ=0) / −0.044 val (6621.1024 −1.97 clean) | same-direction; refits again want ≤0.07 |

fire-always is nominally best everywhere (s6 val −0.261, s8 val −0.137,
s8 train −0.227) **but** its extra value over t0.05 sits at pf ≤ 0.05 —
inside the photo patch_fraction mass (photo firing rates: 0.4% @0.197,
2.9% @0.05, 7.5% @0.0165, 9.3% @0.01, 100% always) — at a measured fired
encode cost of 1.80× (s6 idle-box labels) / 2.13× median (s8 within-cell,
contended) on content that gains ≈0. Rejected for the speed-oriented tier;
the residual ≈−0.19 val mean below pf 0.05 (9165/1055-class) is a
**feature-capacity limit** (patch_fraction cannot separate those from
photos), not threshold placement — a second gate feature would be the
follow-up if it ever matters.

**Wiring (landed, release-gated)**: `src/palette_gate.rs` — `PalettePreference`
+ `palette_gate(patch_fraction, speed)` (the pure rule, speed-tiered:
`palette_gate_threshold(speed)` = 0.197 at s≤5 — byte-identical to the
pre-change rule — and 0.05 at s≥6, measured at 6+8, s7/s9/s10 same-tier
assumption) + `palette_gate_for_rgb8(.., speed)` (auto-tune feature:
Offer-reuse per the auto_tune contract, else one-feature zenanalyze pass;
degrades to Auto on any failure); `EncoderConfig::with_palette_preference`
stores it (encode-imazen); `auto_tune` passes the speed it just picked. The
forward-to-encoder line in `build_ravif_encoder` is **commented until the
zenrav1e dep bump** (registry 0.1.4 has no palette tool) — see the CLAUDE.md
dep-bump checklist.

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

**STATUS 2026-07-03 (later) — the isolation A/B RAN (expanded: 7 leave-one-out arms ×
3 sizes × 16-q dense-high-q grid = 4,032 train cells + val + ramp trials); the
ranked-suspect list was WRONG in a useful way.** Full record:
`benchmarks/hyperparam_size_decay_ab_2026-07-03.tsv`, raw + the PRE-REGISTERED decision
rule at `/mnt/v/output/zenavif/sizedecay-2026-07-03/`, RD_GAP_VS_LIBAOM.md "Size-decay
isolation A/B". Headlines:

- **Suspect #1 (ss2 QM curves) ACQUITTED**: its leave-one-out contribution holds at every
  size (−8.81 @1024 → −7.23 @256, 12/12 better at 256) — scaling it down at small px would
  LOSE most of the tune's biggest win. Chroma delta-q GROWS toward small (−2.23 → −3.17);
  LF sharpness is flat below the conviction floor; boost is ≈0 at 512/1024 on the
  photo-like subset and helps only at 256 (−0.86) — the inverse of the decay hypothesis.
- **The un-suspected mechanism convicted**: the QM-dist ratio decays −3.48 → −2.13 → −0.96
  (8/12 origins, +2.52 ≥ the pre-registered +2.0 bar), its high-q band flips positive at
  ≤512, and at 256 it is butteraugli-adverse (ba3n +0.45 / bamax +1.33) while its ssim2 win
  is thinnest — exactly the high-q-band signature the wedge measured.
- **The proposal graduated — re-based to LONG EDGE and SHIPPED**: m(px) became
  m(longedge) = clamp((log2(maxdim)−8)/2, M256, 1.0) (the rendition classes are
  long-edge-defined; byte-identity at the shipped 1024 class must hold for non-square
  frames). M256 trials {0, 0.25, 0.5} on the convicted qmdist measured an **inverted-U**:
  half strength beats BOTH full and off (train +1.03 @256 / +0.87 @512 median vs full,
  11/12 and 9/12 better; M256=0 fails at −0.96 @256), **VAL-confirmed** (+1.12 / +1.00,
  butteraugli agreeing +1.1..+3.3 everywhere). Landed as `zenrav1e@b0098eb1`
  (`qm_dist_ratio_m`, exact u128 path at m=1.0; tune-off + 1024 byte-identity md5-gated;
  conformance 180/180 aomdec + rav1d-safe). Release-gated with the rest of the tune.
- **Most of the wedge decay is NOT the tune**: the tune-off baseline itself loses ~5.3
  (train) / ~12.2 (val) BD points vs cpu2 from 1024→256, while the tune's within-zr total
  holds at every size (train −14.38 → −10.42, 12/12 better; val −7.42 → −5.61) and its
  vs-cpu2 delta GROWS toward small on val (−1.98 → −4.83). The wedge #3 residual owner
  moves to non-tune small-px coding behavior (partition/coding defaults) + cpu2's own
  small-size strength.

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

1. **Palette-gate mechanism A/B** — ~~graduates first~~ **DONE 2026-07-03,
   confirmed + landed** (status block in the rule-1 section above).
   ~~Speed-conditional threshold follow-up~~ **DONE same day: s6/s8 confirm
   τ=0.05 for s≥6, s2 keeps 0.197; shipped as `palette_gate(pf, speed)`**
   (second rule-1 status block). Remaining: flipping the encoder forward at
   the zenrav1e dep bump.
2. **Size-decay isolation A/B** — ~~768 cells to convict the QM-curve suspect~~
   **DONE 2026-07-03**: QM curves acquitted, qmdist convicted, M256=0.5 long-edge ramp
   shipped (zenrav1e@b0098eb1); the residual decay is baseline-side (see the rule-2
   STATUS block). Follow-up candidate: small-px A/B of the NON-tune coding defaults.
3. Boost-strength head: parked until the val + dense-strength labels exist (fold
   into the next box sweep as extra arms; the store's append protocol takes them
   directly).
