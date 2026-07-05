# P2 per-image hyperparameter heads — tx budget, partition budget, intra axis, the composed fast mode (2026-07-04)

**Program**: FAST_TIER_PARITY_PLAN Phase P2 ("prediction replaces search") =
FEATURE_HINTS §E heads 2-4, built with the method that shipped the palette
gate (threshold rules on ≤3 features, TRAIN fit + LOOCV, butteraugli-veto
objectives, VAL confirm, per-family honesty). Fit inputs: the label store's
fastwins + p1part per-image response surfaces (zero fresh encodes for the
fits); measurement: `scripts/rd_gap/chain_p2heads.sh` on the sweep box
(zenrav1e master 39f0ecdd via the ravif--p2heads dev patch; every cell
PALCONF, 0 CELLFAIL / 0 CONFFAIL). Records:
`benchmarks/hyperparam_tx_budget_2026-07-04.tsv`,
`benchmarks/hyperparam_partition_budget_2026-07-04.tsv`,
`benchmarks/rd_gap_p2heads_2026-07-04.tsv` (+ pointer / raw dir / Tower).

**Headline verdicts:**

| head | rule (v2, deployed in `src/fast_heads.rs`) | verdict |
|---|---|---|
| 1: tx budget (s6-s8) | `pf>0.8505 && dcty>100` → Largest; `pf≤0.8505 && dcty<8.352` → Min; else Size1 | **SHIPPED (release-gated).** Oracle dominates global-min on both axes (s6 −7.46 mean @2.23× vs −6.28 @4.57×); LOOCV −5.84 @2.04× vs global-size1 −5.03 @1.67×; the withhold-only form dominates size1 on BOTH axes (−5.42 @1.56×). The v1 pf-only withhold was CONVICTED on val (below) → conjunctive v2. |
| 2: partition budget (s6) | `gradient_fraction_smooth<0.4105` → Max32 (r16m32_bkvg2); else Ship | **SHIPPED (release-gated).** The only LOOCV-stable per-image partition rule: −5.46 mean @2.41× — beats global-vg2 (−5.20 @2.46×) on both axes. Withhold side has NO stable win (liveness pays 24/24 — unlike tx); s8/s4 heads ≈ null over the right global rung → s6-only. |
| 3: intra-mode budget | — | **NOT a per-image head.** Top-7 keyframe intra (ComplexKeyframes + `filter_intra=Some(false)`, zenrav1e#5-safe) is a small BROAD win: s6 −0.56 / s8 −1.17 med, 17/24 & 16/24 better, composition-stable on the ship point (−0.51 on-ship), one +1.4 regressor (8268) — no honest per-image structure at n=24. → ravif SpeedTweaks GLOBAL arm candidate. **No top-5 knob exists** (`num_modes_rdo` hardcoded 7\|3, zenrav1e rdo.rs:1623) — a mid-point budget would need a new encoder knob (reported, not built). |

## Head 1 — per-image tx budget (fit: `fit_tx_budget.py`)

Labels: fastwins-2026-07-04 per-image BD of {size1, size2, min, full} vs the
same-speed base (train26, coarse 6q), veto-adjusted (`bd:=max(bd,0)` when the
arm's ba3n>+1.0 or bamax>+1.5); degenerate hulls (near-lossless razor-edge
plots where ssim2 saturates) fall back to same-q byte % — never 0. Solo costs
from the fastwins timing section (s6: size1 1.67×, min 4.57×; s8: 1.43×/3.37×).

The per-image response is wildly heterogeneous — the whole point:

- razor-edge tiled plots PAY under size-RDO: 7050 +19.3, 7052 +8.2, 7028 +3.8
  (the RD-model misfire at the near-lossless floor, the P3-owned class);
- screens get ~nothing (8100 family recovery 3%); 8302 +3.5 regressor;
- smooth products/people leave −2.6..−4.5 extra on the table unless "min"
  (size1 + type-RDO over the reduced set) runs: 2000/9228/9958/9868-class —
  these sit at LOW dct_compressibility (libwebp α 3.2-7.9 vs photo median
  ~16): sparse-AC residuals are where tx-TYPE choice pays;
- λ-frontier: the per-image ORACLE dominates global-min on BOTH axes
  (λ=0: −8.25 mean at 3.46× vs global-min −6.28 at 4.57×).

s8 mirrors s6 (LOOCV −6.21 @1.92× vs global-size1 −4.94 @1.43×).

## Head 2 — per-image partition budget (fit: `fit_partition_budget.py`)

Labels: p1part-2026-07-04 {pr1, ship=r16no4_bkvg2, vg2=r16_bkvg2,
m32=r16m32_bkvg2} vs the same-tier base. The response is far more UNIFORM
than tx (every liveness rung improves 24/24) — so the honest oracle headroom
is small (−6.70 @λ=0 vs global-m32 −6.00) and the only stable rule is the
m32 UPGRADE gate: `gradient_fraction_smooth < 0.4105` = flat/synthetic
content (plots 7028 m32 −18.2, clipart, products, 1-bit scans) vs
smooth-gradient photos (gfs 0.58-0.84) where the prange re-test already
showed global widening loses. pr1 (the cheap margin rung) is never picked by
the oracle — margins stay dead. At s8/s4 per-image selection adds ~nothing
over picking the right global rung (s8: rule ≈ global-vg2 exactly).

## The VAL attribution revision (v1 → v2) — the important honesty step

The composed VAL leg (14 held-out VAL-LSD origins, 12q) exposed a v1 W-gate
false fire: 8103 (bls chart, pf 0.936) lost **+12.6** as (none,m32). The
factoring cells (`p2vx_*`) attributed it decisively:

| cell (8103, vs global-ship) | ssim2 | ba3n |
|---|---|---|
| (none,ship) — withhold alone | **+18.1** | +15.3 |
| (size1,m32) — m32 alone | **−1.9** | −1.1 |

Same direction on 5343 (hurricane chart). The razor-edge class that size-RDO
genuinely harms is `pf>0.85 AND dcty>100` (7050/7052 at α 163/202); pf-high
CHART content sits at α ≈ 8-12 and still wants size-RDO. v2 makes both gates
conjunctive (W: +dcty>100; D: +pf≤0.8505), fires strictly FEWER images
(harm-avoiding), and remaps exactly 3 images (7028, 5343, 8103) onto classes
whose cells were ALSO measured (`p2rx_*`/`p2vx_*`). No val-refit of
thresholds — the bounds sit in the empty band between measured clusters
(12.1 vs 162.9 for the W bound; support n=5 in the pf>0.85 band, stated).

## The composed s6 fast mode (12q, both corpora)

| comparison | med | mean | better |
|---|---|---|---|
| composed-v2 vs s6+size1 base (train26) | **−4.38** | −7.07 | 23/24 |
| global-ship vs s6+size1 base (train26) | −2.89 | −4.80 | 23/24 |
| composed-v2 vs global-ship, 11 deviating images (train26) | −3.87 | **−5.13** | 10/11 (1 bamax veto banked 0) |
| VAL composed-v2 vs base | **−3.98** | −5.19 | 12/13 |
| VAL global-ship vs base | −3.41 | −4.61 | 13/14 |
| VAL composed-v2 vs ship, 8 deviating images | −1.08 | **−2.41** | 6/8, worst real loss +0.32 |
| composed-v2+i7 vs base (train26) | −4.77 | −7.38 | 22/24 |

Family wins vs ship (train26 medians): 7000 plots −9.61→−21.46, 9226
products −3.94→−8.20, 2000 people −2.12→−5.90, 9000 clipart −5.20→−7.57.
The one family regression: 6000 scans +0.85 (6096's m32 at 12q under-delivered
its coarse promise; bamax-vetoed, banked 0).

**Parity scoreboard** (photos = t26 minus fam-7000, n=20, per-image BD vs the
CACHED speedladder aom-allintra refs, medians ssim2 / ba3n):

| vs aom arm (arm wall vs plain-s6-tune) | global-ship | composed-v2 | composed-v2+i7 |
|---|---|---|---|
| cpu4iq-ai (2.71×) | +2.88 / +0.91 | **+0.57 / −0.94** (±1% band) | **−0.35 / −1.70 — BELOW** |
| cpu4def-ai (1.93×) | −4.56 / −6.29 | −5.99 / −6.29 | −6.92 / −6.48 |
| cpu6iq-ai (0.46×) | −5.89 / −6.73 | −7.21 / −7.56 | −8.42 / −8.92 |
| cpu6def-ai (0.35×) | −12.45 / −13.56 | −13.97 / −14.38 | −14.92 / −14.61 |
| cpu2def-ai (4.59×) | −0.16 / −1.82 | −3.23 / −1.82 | −3.43 / −3.09 |
| cpu2iq-ai (6.47×) | +8.47 / +5.65 | +5.72 / +4.66 | +4.40 / +4.04 |

The cpu4iq column was the last fast-tier pairing above the curve after P1
(+7.1 → +2.9 → +0.57 heads-only → **−0.35/−1.70 with the intra arm — below
zero on both metrics**).

**Measured wall (solo JOBS=1 RD_CACHE=off, 24 train26 images, vs plain
s6-tune 1026 ms/MP):** global size1+ship **3.00×** (~3.08 s/MP), composed-v2
**3.45×** (~3.54 s/MP; per-image 1.17-8.21×, med 3.43), composed-v2+i7
**5.11×** (~5.24 s/MP; the i7 marginal is **1.49×** on the composed mix —
NOT cheap; 1.38× on plain size1). Honest time-normalized readings:

- **composed-v2 (3.54 s/MP): strictly DOMINATES cpu2def-ai** (4.71 s/MP =
  1.33× slower, and composed-v2 is better on both metrics −3.23/−1.82);
  reaches the ±1% band vs cpu4iq-ai, an arm 0.79× its wall.
- **composed-v2+i7 (5.24 s/MP) sits at cpu2def-class time** (cpu2def 0.90×
  its wall): below that curve −3.43/−3.09 at ~matched time; vs cpu2iq-ai
  (1.27× slower) still +4.40/+4.04 — the s4-tier bracket stays open, owned
  by the s4-tier program, not this mode.
- The intra arm's flip decision at the dep bump should weigh its 1.49×
  composed marginal against −0.39 med (train) / −1.34 med (VAL, 13/13
  clean): it buys robustness and the below-zero cpu4iq medians, at real
  time cost — it is a mode ingredient, not a free default.

## Data gaps + follow-ups (honest list)

1. **Intra top-7 global arm** — measured worth −0.5..−1.2 med at s6/s8;
   **LANDED ravif-side same day** (`S6_INTRA7_LIVE=false` +
   `SpeedTweaks.intra_top7`, ravif@9e413ac0 message / 4b98f0f8 content —
   the first commit is empty from a recovered jj-abandon slip; byte gate
   9/9 identical off). Flips with the other arms at the dep bump.
   **S4TIER UPDATE 2026-07-04: re-weigh the flip against top-5** — at the
   s4-tier MODE level top-5 dominated top-7 (same parity column at 6.26×
   vs 7.61× plain-s6; `benchmarks/rd_gap_s4tier_2026-07-04.tsv`); a
   composed+i5 s6-tier mode is the obvious cheaper variant (unmeasured at
   mode level for v2 classes).
2. ~~**No top-5 intra knob**~~ **BUILT + MEASURED (S4TIER 2026-07-04)**:
   `PredictionSpeedSettings::num_modes_rdo_override` (zenrav1e@071e9844,
   default `None` byte-identical — local 6/6 md5 + 288/288 chain
   continuity). Top-5 keeps ≈78% of top-7's aggregate value (s6 −0.51 vs
   −0.56 med; s8 −1.09 vs −1.17; on-ship −0.57 vs −0.83) at a 1.22×-cheaper
   composed wall — the s4-tier operating point ships it. A SATD-margin
   adaptive budget (P1 lever 3's original shape) remains unbuilt.
3. The pf∈(0.85,0.99)+dcty<100 band has n=5 labels — the v2 W bound sits in
   a wide empty band; more razor-edge-adjacent labels would tighten it.
4. min-class butteraugli fragility: 2021 (val) min fire vetoed (bamax
   +2.16); the D-gate's val record is 2 clean big wins + 1 veto-neutralized.
5. s7/s9/s10 are same-tier assumptions (heads no-op outside measured tiers:
   tx 6-8, partition 6).
6. The composed mode at s8: only the tx head applies today (partition m32
   rung unmeasured at s8 — a cheap future arm if the s8 tier matters).
7. 7053 (val razor-edge plot, (none,ship)) has a degenerate BD hull vs ship —
   its per-cell bytes are in the store for byte-level checks; excluded from
   BD means (n=13 vs 14).
8. **The cpu2iq-ai +4.4 column (the s4-tier bracket): CLOSED-with-residual
   2026-07-04** — v3 rules (D bound 23.69) + top-5 intra landed the column at
   **+2.80/+4.14 at 0.97× cpu2iq's wall**; the rest is measured structural
   (intraBC screens / iq-AQ interiors / near-lossless rescans / gated-less
   full-tx headroom). Full record: FAST_TIER_PARITY_PLAN §s4-tier +
   `benchmarks/rd_gap_s4tier_2026-07-04.tsv`.

## Runtime wiring (landed, release-gated)

`src/fast_heads.rs` — `TxBudget`/`PartitionBudget`/`FastTierBudgets` +
`tx_budget_gate`/`partition_budget_gate` (pure rules, no model file) +
`fast_tier_budgets_for_rgb8` (Offer-reuse per the auto_tune contract; any
failure → Size1+Ship defaults). `EncoderConfig::with_fast_tier_budgets`
stores; `auto_tune` populates. Forward-to-encoder needs the zenrav1e release
past 0.1.4 + zenravif expert passthroughs (`rdo_tx_type_override`,
`reduced_tx_set`, `topdown_prune`, `non_square_partition_max_threshold`) —
the CLAUDE.md "P2 per-image budget heads" dep-bump checklist.
