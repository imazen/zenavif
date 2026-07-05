# Revival manifest — every parked/gated/preserved ID, verified resolvable 2026-07-05

The composition inventory: what can be revived, alone or TOGETHER, with the exact IDs.
All commit IDs below were resolution-verified on 2026-07-05. Companion docs:
STATUS.md (state), FUTURE_DIRECTIONS_AND_FALSIFICATIONS.md (what not to revive),
COOPT_LOOP_PLAN.md (the program that composes most of this), RELEASE_TRAIN_2026-07-05.md.

## 1. THE FLIP — everything release-gated (revives together at the dep bump)
One composed solution; the §A gates verify it. Needs train cars 1+4.
- ravif consts (ravif/src/av1encoder.rs): `S1_DEEP_ARMS_LIVE`, `SMALL_PX_RDO_TX_LIVE`
  (user-signed-off, ravif@2a69a9dc), `S6_TX_SIZE_RDO_LIVE` (7baad5f9),
  `S6_PART_PRUNE_LIVE` (0191489b), `S6_INTRA7_LIVE` (4b98f0f8 — re-weigh vs top-5 per
  rd_gap_s4tier), `S10_RETIER_LIVE` (adb88ddc), `FRAME_HINTS_LIVE` (13b1ca4b) + the 16
  uncomment-at-bump sites (grep "UNCOMMENT"). Checklist: zenavif CLAUDE.md dep-bump
  entries; per-tier knob values recorded in each program TSV.
- zenavif forwards: palette-gate forward block in src/encoder.rs (marked UNCOMMENT),
  encode_plan mirror refresh, encode-mono fold-in (#6), identity-test tightening (#8),
  QM re-benchmark, alpha-tune=Psnr guard, tune-default decision.

## 2. Landed default-off knobs (armable NOW in-tree; the flip or any head can fire them)
zenrav1e master, all byte-identical off, all with verdict TSVs:
`topdown_prune{none_breakout,rect_margin,four_way_margin,homogeneity_gate}` (725f5f71+
767c8ff5) · `rdo_tx_size_override`/`rdo_tx_type_override`/`rdo_tx_size_depth` (d82c16ba)
· `num_modes_rdo_override` (071e9844) · `split_trial_depth` (2fac1af6) ·
`mixed_3way_partitions` (efbe0cf2 — IS Phase-2-v2, landed form) · `palette` PaletteMode
(68a8d81f..a3b72033 luma+UV) · `intrabc`+`intrabc_hash` (7a59e569, d655a6ee+184eb713) ·
`Tune::Ssimulacra2` (a37faea8 + boost d125713f/66733720/165e83b1 + qm-ratio 3710a573/
4279a673 + ramp b0098eb1 + lf-sharp 9a05d54a) · `FrameHints.sb_q_scale` (c4047cec).
**Probe knobs (measured-negative, kept for refits — do NOT arm without new evidence):**
`ssim_rdmult_strength` (57de2815) · `coeff_rd_stack` incl. tu_zero_out + rounding=0
Valin sentinel (3e5ff155+9bc2b71a) · `variance_boost_strength`/`variance_boost_deep` /
`quant_rounding_bias` (6435e6f9).

## 3. Truly-unlanded preserved commits (jj — need explicit revival)
- **zenrav1e ws `dfed8eda`** — Phase-2-v2 pre-knob form (superseded by efbe0cf2; archaeology).
- **zenrav1e ws `a7630aee`** — Phase-2 v1 + its 2 in-flight bug fixes (superseded; check
  whether both fixes reached master before deleting).
- **zenrav1e ws `1428ecdd`** — sizedecay leave-one-out + ramp dev arms (env-gated trial
  harness for all 5 tune mechanisms; revive for any size-conditional refit).
- **zenavif gainmap workspace, parked `76e4e034`** — the zencodec-trait gain-map impl
  (`with_gain_map_*` EncodeJob). Land order recorded in its description: caterr
  migration (`048e071f`, zenavif--caterr ws, ANOTHER SESSION'S — coordinate) →
  zencodec release (branch cancellation-classification-99, pin fde07d07) → rework
  (fix its silent non-AVIF drop) → land. Dev-pin `0460712d` NEVER lands.

## 4. Measurement fixtures (dev-patches, NEVER land, revive to re-measure)
ravif jj: `86de6714` (fastwins env-passthrough pattern) · `75f977ac` (S10: + ZR_ENC_MS
internal timing + SATD-decides passthroughs; EncoderConfig literals current as of
master@071e9844) · `37ec1ee8` (s4tier). Pattern doc: each program's TSV pointer +
the private-clone dev-patch pattern in the WEDGE MAP memory.

## 5. Beyond-budget operating points (recorded, revivable as per-image head targets)
All in TSVs with per-image rows: partition vargate@2.45× / max32+vg2@2.93× (88-104% of
the remaining s6→s4 step; rd_gap_p1part) · tx "min"@4.6× (92% of the tx step;
rd_gap_fastwins) · 5000-class oracle extras @10.1× (rd_gap_s4tier) · boost str2 for
smooth photos (5004 −15; TUNER2/deltaq records) · palette fire-always residual −0.19
(feature-capacity-blocked; hyperparam_palette_speed_ab).

## 6. Staged releases (the train; per-car user go)
rav1d-safe 0.6.0 @ `0579614`+ (main head; CI 18/18 green run 28717747678) ·
zenavif-parse 0.6.3 FROM `c36b822` (NOT main) · zenavif-serialize 0.2.0 @ main head
(incl. 0a48b468) · zenrav1e 0.2.0-window @ master head · zenravif FIRST publish ·
zencodec 0.1.26 (branch cancellation-classification-99; other session's program).

## 7. Data/model assets (all fits and refits draw from these)
Label store `/mnt/v/output/zenavif/hyperparam-labels-2026-07-03/labels.parquet`
(68,412 rows × 130+ arms; builder scripts/hyperparam/build_label_store.py; drift rule:
regenerate baselines per encoder rev) · box snapshot `zenavif-sweep-2-1783240231`
(cell cache incl. coeffrd/s10/s4tier sweeps) · `scripts/rd_gap/sample_images_train26.tsv`
+ `sample_doccharts.tsv` (+ renditions /mnt/v/output/rd-gap-{train26-2026-07-02,
doccharts-2026-07-05}/) · canonical picker datasets s3://zentrain/canonical/2026-06-27/
(local /mnt/v/output/canonical-picker-2026-06-27/) · `scripts/hyperparam/fit_q0_head.py`
(M0-M6 ladder + bake-JSON emitter for the MLP retry) · pre-registered decision-rule
pattern: scripts/rd_gap/DECISION_RULE_{SSIMRD,COEFFRD}.md · benchmarks/*.tsv (136
files, every verdict) · raws mirrored on Tower per program.

## The composition answer
Reviving "everything at once into a solution" = **the flip (§1) + the armable knobs
(§2) wired by heads firing the §5 points, verified by the gates, shipped by the train
(§6), fitted from §7** — which is precisely COOPT_LOOP_PLAN phases 3-4. The only
pieces needing pre-work first: §3's gain-map trait chain (zencodec release) and any
§2 probe knob (new evidence required per the falsification ledger).
