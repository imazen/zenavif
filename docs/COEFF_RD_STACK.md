# aom's coefficient-level RD valuation stack vs zenrav1e — the mechanism study (COEFF_RD_STACK)

**Program**: the wall three residual hunts converged on. (1) The s1-deep close-out:
"partition levers exhausted, residual is coefficient-level RD" — 8 named photos still
lose to cpu0 (RD_GAP "s1 deep mode", o_6629 +25.3 worst pre-qmdist). (2) The 6096
no-skip diagnosis: aom codes coefficients on 100% of 4x4 cells at baseQ 64 where we
skip 57.5% at baseQ 54; the dead-zone CONSTANT alone was measured-rejected — "aom's
no-skip is a valuation STACK: FP-quant + neutered trellis + QM-PSNR tx-domain + iq
rdmult" (RD_GAP "Near-lossless rescans residual" + TUNER2 (2)). (3) The SSIMRD
close-out: every aom rdmult-side λ constant measured-and-rejected; "the remaining
candidates are outside the AQ/λ constants — aom's cq-mode rate loop / coefficient-level
RD stack" (RD_GAP "SSIMRD"). TUNER2's standing order, verbatim: *"if it is ever
re-attacked, port the valuation STACK (tx-domain QM-PSNR distortion under the tune +
rounding + trellis posture together), not constants one at a time."*

**Pins.** libaom `632172a468f5e91c5b40daaa0a91f4a291c63af4` at `~/work/aom` (the
benchmark reference build). zenrav1e master `57de2815` (workspace
`zenrav1e--coeffrd`). All file:line cites below are at these pins. Evaluation per the
93b83401 policy (RD_GAP "Corpus + split hygiene"): per-family first, cluster-mass
weights for aggregates, photos-only merit KEEPABLE.

---

## 1. aom's stack end-to-end (all-intra, `--end-usage=q`)

### 1.1 Quantizer construction — two postures, one table build

`av1_build_quantizer` (`av1/encoder/av1_quantize.c:604-655`) builds BOTH quantizer
postures for every qindex; `sharpness` (set to **7** by tune=iq AND tune=ssimulacra2 in
`handle_tuning`, `av1/av1_cx_iface.c:1954-1996`) reshapes them at
`av1_init_quantizer` time (`encoder.c:3122`, direct from `oxcf.algo_cfg.sharpness` —
frame-invariant; `enable_adaptive_sharpness` touches only the loop filter,
`picklpf.c:232`):

| table | zbin (dead zone) | rounding, sharpness=0 | rounding, sharpness=7 |
|---|---|---|---|
| B (`quantize_b`) | `84/128 ≈ 0.656·q` (`80/128` for q≥148, `get_qzbin_factor` :592-602) | `48/128 = 0.375` | `64/128 = 0.5` (`sharpness_adjustment = 16*(7-s)/7 = 0`, :609-621) |
| FP (`quantize_fp`) | **none** (`(void)zbin_ptr`, :84) | `64/128 = 0.5` **always** | `64/128 = 0.5` |

`quantize_fp_helper_c` (:72-124): hard gate `abs_coeff ≥ dequant/2` then round-half-up
— plain round-to-nearest, DC and AC alike. The QM variant scales both the compare and
the multiply by the forward weight `wt` and dequantizes with the inverse weight
(:98-121), so FP is QM-exact.

### 1.2 The FP↔trellis coupling — the two half-stacks are never run alone

Every quant call in the tx search picks its posture from whether the trellis will
follow (`tx_search.c:945-948, 1983-1985, 2181-2184`):

```c
skip_trellis ? (USE_B_QUANT_NO_TRELLIS ? AV1_XFORM_QUANT_B : AV1_XFORM_QUANT_FP)
             : AV1_XFORM_QUANT_FP
```

with `USE_B_QUANT_NO_TRELLIS = 1` (`av1/common/blockd.h:34`). **aom never quantizes
generously without the per-coefficient policer, and never runs the policer over
dead-zoned input.** When trellis is off it falls back to the zbin dead zone —
structurally the same design point as zenrav1e's Valin offsets (§2.1). This coupling
is exactly what our two isolated probes each violated: `quant_rounding_bias=128` alone
(TUNER2 (2): +2.67 med, 20/23 butteraugli vetoes — kept everything unconditionally)
and `enable_trellis` forced-on alone (+0.32/+0.55% — policed already-truncated input).
The composed cell has never been measured on zenrav1e.

### 1.3 `av1_optimize_txb` — the per-coefficient RD descent (`txb_rdopt.c:400-561`)

Runs **inside every tx-type trial** (`search_tx_type`, `tx_search.c:2078`:
`skip_trellis = !is_trellis_used(...)`; optimized result feeds the trial's rate/dist,
so tx-type/size/mode/partition decisions all see post-optimization coefficients).
Mechanics, back-to-front over scan positions:

- **λ posture.** `rdmult' = x->rdmult · (8−sharpness) · plane_rd_mult[intra][plane] >> rshift`
  (:449-460), `plane_rd_mult` intra = {17 luma, 20 chroma} (`encodetxb.h:270-273`),
  `rshift` = 5 default, **7 under tune=iq/ss2** (:449-452). Relative to the block-RDO
  λ: **default = ×4.25 rate-averse** (17·8/32 — an aggressive dropper), **iq/ss2 =
  ×0.1328** (17/128 — near-keep-everything; the "÷32" in TUNE_SSIMULACRA2_PLAN item 4).
- **Candidates.** eob coefficient + DC: `update_coeff_general` ({qc, qc−1, 0}).
  Near-eob zone: `update_coeff_eob` (:186-315) — additionally evaluates **moving the
  eob** to each earlier nonzero (full new-eob-position rate + context-switch cost,
  :247-273). Interior: `update_coeff_simple` (:88-184) — {qc, qc−1} single step.
- **Round-up-only rule.** Interior/eob-zone lowering is only *considered* when the
  coefficient rounded UP (`if (abs_dqc < abs_tqc) keep` — :113, :146): the descent
  undoes unprofitable round-ups; it never squeezes a coefficient that already sits
  below its true value. FP's 0.5-rounding is what creates the round-up population.
- **Preserve guards under sharpness≠0 (iq/ss2).** Interior `abs_qc==1` never zeroed
  (:118); eob-zone lowering requires `abs_qc > 2` at scan pos ≤5, `>1` after (:275-276);
  **eob may only move to position ≥5** (:287); **whole-block zeroing (`update_skip`,
  :316-334) requires `sharpness==0`** — disabled under the tunes (:518-521). Net under
  ss2: keep nearly everything, still price everything exactly.
- **Rate model.** Per-frame CDF-derived cost tables (`CoeffCosts`/`LV_MAP_COEFF_COST`
  + `LV_MAP_EOB_COST`, :422-441) — table lookup, NOT adapted during the search.
- **Distortion.** Tx-domain squared error vs the ORIGINAL transform coefficient,
  QM-forward-weighted iff `dist_metric == AOM_DIST_METRIC_QM_PSNR` (`get_coeff_dist`,
  `txb_rdopt_utils.h:48-64`; qmatrix selection `txb_rdopt.c:413-417`). `RDCOST(RM,R,D)
  = (R·RM >> 9) + D·128` (`rd.h:32-34`).
- **Live contexts.** `levels[]` is updated as coefficients are lowered (:136, :176,
  :288-307) — downstream (lower-scan) decisions see the modified neighborhood.
- Also touches **DC** (si==0, :539-546).

### 1.4 Speed/tune gating in all-intra — trellis is speed-INVARIANT

- `optimize_coefficients` is set ONLY by `init_rd_sf` from `disable_trellis_quant`
  (default 3, `av1_cx_iface.c:291,450`) → `NO_ESTIMATE_YRD_TRELLIS_OPT`
  (`speed_features.c:2488-2509`), which `is_trellis_used` (`encodemb.h:157-163`)
  treats as TRUE for everything except the inter-only `estimate_yrd_for_sb`
  (`compound_type.c:486`). The allintra speed functions never touch it. **For
  all-intra non-lossless at every speed 0-8, trellis runs in BOTH the RDO search and
  the final encode.** The real per-block speed knob is `perform_coeff_opt` (allintra:
  sp0=1, sp1=2, sp2=3, sp4=5, sp6=6) indexing `coeff_opt_thresholds[9][..][2]`
  (`speed_features.c:88-98`): an MSE/qstep² gate (`tx_search.c:2146-2149`) and a SATD
  gate (`tx_search.c:1958-1988`) that route high-energy blocks to B-quant-no-trellis.
  Index 0 = `{UINT_MAX, UINT_MAX}` = trellis always (cpu0).
- **`av1_dropout_qcoeff` is DEAD CODE at this rev** — defined (`encodemb.c:269`) but
  never called; the `DROPOUT_OPT`/`TRELLIS_DROPOUT_OPT` enum is unused. No coefficient
  dropout runs at any speed/tune. (Earlier drafts of our notes assumed it ran —
  corrected here.)
- tx-type PRUNING helpers always use B-quant + Laplacian cost
  (`av1_cost_coeffs_txb_laplacian`, `tx_search.c:1184,1313`); only the surviving
  candidates get FP+trellis+exact cost.

### 1.5 Block-level λ and the skip side

`av1_compute_rd_mult_based_on_qindex` (`rd.c:389-444`): `rdmult = qDC² ·
(3.3 + 0.0015·qDC)` for keyframes, then the iq/ss2 weight `(clamp((255−qindex)·3/4,
0, 72)+128)/128` — that weight is the **measured-rejected** frame-λ transplant
(TUNE_SS2 step 2, +4.41%). Per-SB: rdmult recomputed from the boosted SB qindex
(`set_rdmult`, `encodeframe_utils.h:298-322` — the deltaq follow we ported with
Variance Boost) plus an `intra_sb_rdmult_modifier` from SB variance
(`partition_search.c:652`, set :5726-5731 — λ-side, untried here, low prior given
the 3× λ-transplant failure pattern; table row 13).

**Per-TU zero-out (an aom mechanism we do NOT have, not sharpness-gated):** after the
tx search, each intra TU is force-zeroed when
`RDCOST(x->rdmult, rate, dist) >= RDCOST(x->rdmult, zero_blk_rate, sse)`
(`tx_search.c:3294-3311`; inter analog :2425-2439; `zero_blk_rate` = the txb_skip
flag cost). This runs at the BLOCK rdmult even under ss2 — the counterweight that
keeps the keep-everything trellis posture honest on flat blocks. zenrav1e has no
per-TU zero test (only the all-TUs-zero → skip force-flip; §2.3). Also
`predict_dc_only_block` (`tx_search.c:1992-2053`) forces eob=0 on near-flat residual
at speed≥6 (`dc_blk_pred_level`).

### 1.6 Closed by inspection: the "cq-mode rate loop"

**No recode ever runs in `--end-usage=q --passes=1`:** good-quality single-pass sets
`recode_loop = DISALLOW_RECODE` (`speed_features.c:2795-2797`) →
`encode_without_recode` (`encoder.c:3706-3729`); even the dummy-pack size probe is
`rc_cfg.mode != AOM_Q`-gated (`encoder.c:3562-3565`). q is chosen once, the frame
encodes once. The SSIMRD close-out's "cq-mode rate loop" suspect therefore reduces
to machinery already measured (frame-λ curve — rejected; deltaq rdmult follow —
ported) **plus the coefficient stack of §1.1-1.3. The stack is the only remaining
unported valuation machinery.** (Also verified: the TUNER2 record's "qindex ≤112"
rounding gate was a conflation — the 112 threshold is the tune-iq adaptive
LOOP-FILTER sharpness gate, `picklpf.c:232-246`; the quantizer rounding gate is just
`sharpness != 0 && q != 0`, frame-invariant.)

---

## 2. zenrav1e's stack end-to-end (master 57de2815)

### 2.1 The quantizer is a mean-field trellis — Valin static offsets

`QuantizationContext::update` (`src/quantize/mod.rs:274-336`): rounding offsets are
**statically fitted averages of exactly the RD trade aom computes per-instance** —
derivation comment cites Valin's `threshold = 0.5 + λ·avg_rate_diff/2`, λ = ln2/6,
iterated to convergence over clips (:301-327):

| offset | intra value | ≈ rounding point |
|---|---|---|
| DC | 109/256 | 0.426 |
| AC bulk (`ac_offset1`, after a >1 coeff) | 109/256 | 0.426 |
| AC tail (`ac_offset0`, in the zeros/ones tail) | 98/256 | 0.383 |
| EOB dead zone (`ac_offset_eob`) | 88/256 | tail-zeroing threshold `1 − 88/256 = 0.656·q` |

The eob dead zone is **numerically the same 0.656·q as aom's B-path zbin (84/128)** —
independent fits landing on the same average confirms both are the "no policer"
design point. `level_mode` (:436-483) switches ac_offset0/1 by local run state (a
2-state context). The flat override `quant_rounding_bias = Some(k)` (TUNER2 knob,
:286-299) sets ALL FOUR offsets to k/256 — `Some(128)` is byte-parity with aom's
sharpness≠0 posture on the rounding side only. QM: forward quant divides by the
QM-weighted step with proportionally scaled offsets (:371-393, :442-467) — QM-exact
like aom FP. ONE quantization path serves RDO trials and final encode
(`encode_tx_block`, `src/encoder.rs:2663`; `qc.update` sites :3687/:3800/:3894/:3994).

### 2.2 The fork trellis exists but is structurally sidelined

`trellis::optimize` (`src/quantize/trellis.rs:56-349`), called inside
`encode_tx_block` (`src/encoder.rs:2674-2691`) → **decision-visible in RDO trials,
same placement as aom**. Machinery: Phase 1 = global-best explicit eob cut (rate
includes eob-position delta + EOB-context switch, :134-240); Phase 2 = per-coefficient
**full multi-level descent to 0** (interior; floor 1 at the eob coefficient) with
exact monotonic early-break (:242-341). Rate = live-CDF float approximation
(`cdf_rate` LUT, ~0.1 bit accuracy, frozen snapshot of the CURRENT adapted CDFs,
:414-543 — fresher than aom's per-frame tables). Distortion = tx-domain `sq_err`
with QM-exact reconstruction and forward-QM weighting under the tune
(`qm_weighted_trellis`, :581-609). DC untouched (loop from scan index 1, :268).

But its **posture** is the opposite of aom's ss2 stack at every point:

1. **Opt-in and OFF everywhere**: `enable_trellis` default false
   (`src/api/config/encoder.rs:163, :286`), armed by no speed preset, no tune.
2. **Hard-dead below ~Q80**: `if ac_quant >= 200 { return eob; }` (:87-89) — the
   entire web-quality midband (where the 6096/iq-AQ residuals live) gets NO
   optimization even when the knob is on.
3. **Dampened above**: `λ_trellis = λ · 2^(6−2·lts) · min(1, 80/ac_quant)` (:83-95)
   — fades with quantizer coarseness.
4. **λ-scale 1.0** (the 2026-06-18 sweep optimum — fitted over the deadzone-quantized
   input the trellis then received, at the high qualities where it ran at all).
5. **Downstream of the dead zone**: it polices coefficients that already lost their
   0.34-0.5·q mass — descent-only machinery cannot recover what quantization dropped.

### 2.3 Where zenrav1e is already at parity or better (do not port these)

- **RDO rate**: exact `tell_frac` from real symbol writes with **in-trial CDF
  adaptation** (`WriterCounter` + `symbol_with_update!`, `src/rdo.rs:1051-1100`,
  `src/context/cdf_context.rs:649-661`, rollback :1136) — strictly better than aom's
  static per-frame cost tables. `tx_domain_rate` is false at every shipped speed
  (`speedsettings.rs:100`).
- **RDO distortion**: pixel-domain `cdef_dist` + `apply_ssim_boost` activity masking
  (`src/rdo.rs:273-396`, `src/activity.rs:167-194`) + the tune's `qm_dist_ratio`
  luma scaling (`src/rdo.rs:330-357`, accumulators `src/encoder.rs:2749-2786`).
  Measured: the literal aom tx-domain routing LOSES here (+6.07% domain switch,
  +4.47% aom-literal QM_PSNR; ratio composition ships at −1.78/−1.71% — RD_GAP
  "QM-weighted RD distortion"). `use_tx_domain_distortion` is Psnr-tune-only
  (`src/encoder.rs:1724-1726`).
- **Block λ**: Daala `λ = (ln2/6)·q²` (`src/rate.rs:571-574`; `compute_rd_cost` =
  λ·bits + scaled dist, `src/rdo.rs:833-838`). Both aom λ-curve transplants measured
  worse (frame-λ +4.41%, per-16×16 curve +2.11..+6.85 — RD_GAP "SSIMRD").
- **Skip**: intra never RD-evaluates skip; a winning mode whose TUs all quantize to
  zero is force-flipped to skip (`src/encoder.rs:4086-4093`). So on intra content the
  quantizer posture IS the skip lever — aom's "0% cell skip" on 6096 is its FP
  posture, our "57.5%" is the 0.656·q eob dead zone. Same lever, different setting.

---

## 3. The mechanism table

| # | mechanism | aom @ ss2 tune (cpu0/2) | zenrav1e composed (s1/s2+tune) | difference | transplantable? |
|---|---|---|---|---|---|
| 1 | quant rounding | FP round-to-nearest 0.5, no dead zone, DC+AC (§1.1) | Valin 0.383-0.426 + eob dead zone 0.656·q (§2.1) | **structural** | YES — knob exists (`quant_rounding_bias=128`); alone measured-rejected, must compose with #2 |
| 2 | per-coeff RD optimizer | ALWAYS on, every trial, all q; λ ×0.1328 (ss2) / ×4.25 (default); preserve guards; eob-move; live contexts; DC included (§1.3) | opt-in OFF; hard-dead ac_quant≥200; dampened; λ ×1.0; no DC (§2.2) | **structural — THE GAP** | YES — machinery exists; port = un-gate + λ-posture + guards |
| 3 | optimizer distortion | tx-domain, QM-weighted under ss2 | same (`qm_weighted_trellis` rides the tune) | parity | done |
| 4 | optimizer rate model | static per-frame CDF tables | live-CDF float approx (~0.1 bit) | parity (ours fresher) | no action |
| 5 | trial rate model | LV_MAP cost tables | exact tell, in-trial CDF adaptation | **ours better** | no action |
| 6 | trial distortion | tx-domain QM-SSE forced under ss2 | pixel psy `cdef_dist` × qm-ratio | ours better (measured: literal port +4.47%) | REJECTED 2026-07-03 |
| 7 | frame λ from qindex | q²·(3.3+.0015q)·tune-weight | Daala (ln2/6)q² | different lineage | REJECTED (+4.41%) |
| 8 | per-16×16 rate-λ masking | `av1_set_mb_ssim_rdmult_scaling` | `ssim_boost` dist-side + boost alloc-side | different placement | REJECTED (+2.11..+6.85, SSIMRD) |
| 9 | whole-block zero in optimizer | `update_skip` — disabled under tunes | absent | parity-by-absence under ss2 | no action |
| 10 | dropout (isolated-coeff pruning) | **dead code at the pin** — `av1_dropout_qcoeff` never called | absent | none | no action (myth dispelled) |
| 11 | per-SB λ follow (deltaq) | `av1_get_deltaq_offset` rdmult follow | `sb_dist_scales` (Variance Boost) | ported 2026-07-02 | done |
| 12 | cq "rate loop" | no recode in end-usage=q allintra single-pass (§1.6) | n/a | — | closed by inspection |
| 13 | per-TU zero-out RD test | `RDCOST(rate,dist) ≥ RDCOST(zero_blk_rate, sse)` → eob=0, every TU, block λ, NOT sharpness-gated (§1.5) | absent (all-TUs-zero force-flip only) | real, small | YES — optional `tu_zero_out` field of the #1 knob (counterweight arm) |
| 14 | intra SB rdmult modifier (variance) | `partition_search.c:652` | absent | λ-side | low prior (3× λ-transplant failures); not in round 1 |

## 4. Ranked differences (evidence-weighted)

**#1 — the composed posture: FP-parity rounding + always-on conservative trellis
(rows 1+2 as ONE knob).** This IS the "valuation stack" of the TUNER2 verdict minus
the parts already shipped (QM curves, QM-dist ratio, boost) or measured-rejected
(tx-domain switch, λ curves). Evidence it owns residual classes:
- **6096 / near-lossless band**: aom's 0%-skip-at-higher-baseQ posture is literally
  rows 1+2 (§2.3 skip note). The two half-stack rejections bracket the composition:
  rounding-alone kept noise (butteraugli veto 20/23), trellis-alone had nothing to
  keep. Between them sits aom's actual pipeline: keep 0.5-rounded mass, police it
  per-instance with exact rates at tune-λ.
- **the 8-photo s1 class** (o_6629 +25.3→+7.6 already recovered by the QM-dist ratio;
  o_5004 +11.1, o_3008 +9.9, o_3003 +6.7, o_9051 +3.1, o_6632 +2.2, o_2202 +1.1,
  o_9077 +0.6 vs cpu0-DEFAULT): the s1 postmortem names "trellis-class optimization,
  cost-model precision" as the residual. Note cpu0-default runs the ×4.25 posture
  (round-nearest + aggressive exact-rate pruning + dropout, no QM) — sparse-but-
  precise placement. The λ-scale parameter must span both postures ({~0.13 …
  ~4.25}); the winning point is a FIT, not an assumption (iron lesson).
- **1236/9094 iq-AQ class**: partial-coverage candidate only — def reaches no-skip
  without the sharpness rounding (TUNER2), and the SSIMRD close-out leaves "aom's
  better coefficient-level RD in the low-mid band" as the surviving hypothesis for
  this class. Watch, don't promise.
- Cost expectation: forced-trellis measured 1.66× at s2 WITH the ≥200 gate returning
  early on most cells; always-on will cost more before optimization. Phase-2-skip
  heuristics for tiny eob and armed-only gating are the mitigations; measure first.

**#2 — the cheap decomposition fallback (if #1's time cost is prohibitive at fast
tiers): phase-1-only (eob RD placement) + DC inclusion, always-on.** The eob cut is
O(eob) with no descent loop; it converts the static 0.656·q eob dead zone into an
instance-exact eob decision — the single biggest structural asymmetry of row 1 at
low/mid q where blocks are mostly tail. Only pursued if #1 wins RD but fails the
tier time budgets.

**#3 — nothing else qualifies.** Rows 6-8 are measured rejections (do NOT re-run:
frame-λ +4.41%, tx-domain +6.07%/+4.47%, ssim-rdmult +2.11..+6.85, trellis-λ÷32-alone
+0.01% [over deadzone input — superseded by the #1 composition which changes its
input], rounding-alone +0.94/+2.67, VAQ +2.8%, old trellis +34%-time-for-0.3%).
Rows 4-5 favor us. Row 10 (dropout) is only meaningful for a default-tune program.

## 5. Phase B port spec (the knob)

zenrav1e `EncoderConfig::coeff_rd_stack: Option<CoeffRdStack>`, default `None` =
byte-identical (md5 gate). Fields and semantics when armed:

```rust
pub struct CoeffRdStack {
  /// Flat forward-quant rounding, k/256 for DC+AC+eob dead zone (128 = aom FP parity).
  pub rounding_bias: u8,              // default 128
  /// Trellis λ relative to fi.lambda (aom ss2 posture 0.133; aom default-tune 4.25).
  pub trellis_lambda_scale: f64,      // sweep axis
  /// aom sharpness≠0 guards: no zeroing of level-1s in phase 2 (floor 1),
  /// phase-1 eob cut restricted to >= 5 coefficients kept.
  pub preserve_guards: bool,          // sweep axis
  /// aom's per-TU zero-out counterweight (table row 13): after the trellis,
  /// zero the whole TU when RDCOST(coded) >= RDCOST(zero) at BLOCK lambda,
  /// rates from the trellis's CDF model (estimate_block_coeff_rate exists,
  /// validated ±0.3% BD), dist = tx-domain sq_err sums.
  pub tu_zero_out: bool,              // one arm; default false in round 1
}
```

- Quant: effective rounding bias = `Some(rounding_bias)` at the four `qc.update`
  sites (overrides `quant_rounding_bias`).
- Trellis: runs regardless of `enable_trellis`; the `ac_quant >= 200` return and the
  `80/ac_quant` dampening are BYPASSED (the λ posture replaces them);
  `λ_trellis = fi.lambda · 2^(6−2·lts) · trellis_lambda_scale`. Guards per above.
  QM-weighted dist continues to ride the tune (`qm_weighted_trellis`).
- Coarse arms (s2, train26 + `sample_doccharts.tsv`, 6q, BD vs same-binary base,
  both metrics + bamax veto): A `{128, 0.133, guards on}` (aom-ss2-verbatim posture),
  B `{128, 0.35, on}`, C `{128, 1.0, on}`, D `{128, 4.25, on}` (aom-default-tune
  posture under our tune), E `{128, 1.0, off}` (control ≈ historical flat+trellis
  compose, brackets vs the two rejections), F = best-of-A..E `+ tu_zero_out` (the
  row-13 counterweight, targeted at any fam-7/flat over-keep the per-family slices
  show). Winner → full 12q grid + s6 + s1 probes + legacy continuity photos
  (o_6629/o_5004/o_3008/o_9051 etc. REPORT-ONLY — val/test origins) + named-class
  probes (6096/6018/6091/1236/9100/9118 where present in train26/doccharts).
- Conformance: quantization/eob semantics change ⇒ full battery at armed configs,
  both samplings (420+444), aomdec + rav1d-safe byte-agree (PALCONF).
- Release-gating: same shape as every 2026-07 knob — lands default-off on zenrav1e
  master; ravif/zenavif forwards wait for the dep bump.

Follow-ups noted, not in round 1: DC descent in phase 2 (aom polices DC; ours never
touches it — matters for ultra-flat gradients, o_6629-class); eob-move-with-lowering
(aom's `update_coeff_eob` considers both simultaneously); the per-block MSE/SATD
trellis bypass for the fast tiers (aom's actual speed lever, §1.4); row-14's
variance rdmult modifier (λ-side, low prior).

---

## 6. VERDICT (measured 2026-07-05, same day): HONEST NEGATIVE at every posture — the wall is refuted as a transplant

The knob landed (`zenrav1e@3e5ff155` + `@9bc2b71a`, incl. the `rounding_bias = 0`
fitted-Valin sentinel added mid-program for the un-gate-only decomposition) and the
full arm ladder ran per DECISION_RULE_COEFFRD.md (pre-registered at zenavif
`bcc02310` before any arm data; byte-continuity 288/288; PALCONF clean on every
armed cell). **No arm advanced**: t26 mass-weighted median ssim2 BD +0.97 (G2,
Valin+λ0.35 descent) … +14.02 (D, aom default-tune posture), butteraugli vetoes
in every posture arm, strict Pareto wins ≤ 5/144 cells anywhere, doccharts
in-distribution replication, named residual classes flat-to-harmed (single
exception 6018, the known val-refutable 1-bit-scan pattern).

The measured close: **row 1 and row 2 of §3 are not "differences to port" — they
are two encoders' correctly-fitted optima for two different valuation loops.**
Flat-0.5 rounding is aom-FP-optimal because av1_optimize_txb + LV_MAP table costs
+ tx-domain dist re-price every coefficient after it; zenrav1e's Valin offsets are
optimal because nothing downstream re-prices (mean-field IS the policer), and its
psy-pixel/exact-tell RDO already prices whole-block alternatives better than a
per-coefficient tx-domain pass can (G/G2: the descent has NO beneficial λ over
Valin input at s2 mid/low q — the 2026-06-18 quality gate is vindicated, not
timid). Full record: RD_GAP_VS_LIBAOM.md "COEFF_RD_STACK" +
`benchmarks/rd_gap_coeffrd_2026-07-05.tsv`. The ranked-differences list in §4 is
therefore closed: #1 measured-and-rejected (this program); #2's premise — that
the composed stack's RD value exists and only its COST needs decomposing — died
with #1 (a pure phase-1-only arm was not isolated, but G/G2 ran phase-1-inclusive
descents at two λ and both lost monotonically; there is no measured RD value left
to slice for); #3 was already the no-action row.
Arm F (`tu_zero_out`) was never triggered (no winner) — the field stays landed
as infrastructure.
