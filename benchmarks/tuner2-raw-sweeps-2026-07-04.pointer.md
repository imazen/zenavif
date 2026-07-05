# TUNER2 raw sweeps (2026-07-04) — pointer

Raw per-cell TSVs + chain logs for the P3-residual TUNER2 program
(docs/RD_GAP_VS_LIBAOM.md "TUNER2"; committed summaries:
`tuner2_valstr_2026-07-04.tsv`, `hyperparam_boost_refit_2026-07-04.tsv`,
`hyperparam_boost_gate_2026-07-04.tsv`).

- Block storage: `/mnt/v/output/zenavif/tuner2-20260704/`
  (t2_cont8 / t2_valstr_{0.0,1.0,2.0,3.0,4.5} / t2_deep_{3.0_4,4.5_4} /
  t2_dz_{118,128} / t2_drift_{0.0,4.5} / t2_t26str0 + per-run logs)
- Canonical queryable form: label store
  `/mnt/v/output/zenavif/hyperparam-labels-2026-07-03/labels.parquet`
  (64,338 rows; sources `valstr-2026-07-04`, `tuner2-2026-07-04`)
- Box: zenavif-sweep-1 ccx63 (snapshot-restored; re-snapshotted at teardown)
- Binary chain: zenrav1e@6435e6f9 via ravif--tuner2 devpatch
  (box cavif sha256/16 80ff3fe2f8ce1810); byte-continuity 96/96 vs the
  store's speedladder/zr-s2-tune rows
- Grids: valstr 12q full; deep/dz 6q coarse (both measured negative — no
  full-grid escalation); drift 12q on 3 origins; t26str0 12q full
