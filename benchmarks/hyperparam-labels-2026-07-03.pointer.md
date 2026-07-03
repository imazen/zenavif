# Hyperparameter-expert label store — 2026-07-03 (pointer)

The FEATURE_HINTS_PLAN §E label store: every mechanism fit sweep + the wedge
dataset aggregated into one queryable per-(image, arm, q) parquet so threshold
rules / future heads fit against a single store instead of one-shot TSVs.

- **Block storage (canonical):**
  `/mnt/v/output/zenavif/hyperparam-labels-2026-07-03/labels.parquet` (629 KB,
  35,118 rows × 34 cols, 89 arms as of 2026-07-03 late — incl. the sizedecay + sizedecay-nontune sources) + `_MANIFEST.json` (build commit, per-source
  row counts, join coverage, honesty contract: encoder_rev validity domains,
  q_kind semantics, enc_ms reliability, palette-pipeline caveat, exclusions)
- **Tower mirror (sha256-verified):**
  `/mnt/tower/output/zenavif/hyperparam-labels-2026-07-03/`
  labels.parquet sha256 `6ad341be721389a2` (first 16)
- **Builder (deterministic, asserts row counts + join integrity):**
  `scripts/hyperparam/build_label_store.py` — future fit sweeps APPEND by adding
  a SOURCES entry and re-running.
- **Sources aggregated:** tune-ss2-2026-07-02, deltaq-2026-07-02,
  qmdist-2026-07-03, lfsharp-2026-07-03, desyncfix-2026-07-03, wedge-2026-07-03
  (parquet + paletteoff TSV), palette-ab-final2-2026-07-03 — each with per-file
  arm_id / knob_json / encoder_rev / q_kind.
- **Feature join:** `origin_path|crop_label|size_class` into
  `imazen26_features_2026-06-23.parquet`; wedge rows pixel-exact (123/123),
  train26 rows derived (WxH-verified; vips-vs-Lanczos rendition caveat),
  legacy22 rows NULL. Split = canonical LSD origin rule (the features parquet's
  own `split` column is an older convention — disagrees on 1,148/2,157 origins;
  do not use it).
- **First consumers:** `scripts/hyperparam/fit_palette_gate.py`,
  `fit_size_decay.py`, `fit_boost_strength.py` → committed rule evals
  `benchmarks/hyperparam_*_2026-07-03.tsv`; report
  `docs/HYPERPARAM_FIRST_CUT_2026-07-03.md`.
