# P1PART raw sweeps — 2026-07-04 (pointer)

Raw per-arm TSVs for `benchmarks/rd_gap_p1part_2026-07-04.tsv` (block-storage
per repo policy):

- Block storage: `/mnt/v/output/zenavif/p1part-20260704/` (SHA256SUMS;
  mirrored to `/mnt/tower/output/zenavif/p1part-20260704/`)
- Contents: `p1_s6_{base,base2,r16,r16no4,r16m32,r32m32,r16_bk,r16_pr1,
  r16_pr2,r16_pr3,r16_pr4,r16_vg2,r16_vg3,r16no4_pr3,r16m32_pr1,r16m32_pr2,
  r16m32_pr3,r16_bkvg2,r16_bkvg3,r16_bk4vg2,r16no4_bkvg2,r16m32_bkvg2}.tsv` +
  `p1_s8_*` + `p1_s4_*` shortlists (coarse 6-q; wave-1 `_pr1/_pr2/_bk/no4`
  arms measured the SYMMETRIC margin semantics on zenrav1e 725f5f71, waves
  2-4 the one-sided semantics on 767c8ff5 — `base2` is the 144/144
  byte-identity sentinel across the change), `confirm_s{6,8,4}_{base,r16no4}
  .tsv` (full 12-q landing grids), `timing_*.tsv` (solo JOBS=1 RD_CACHE=off
  — the ONLY wall-time-grade enc_ms), `chain*.log` + per-run logs,
  `ravif_devpatch_p1part_2026-07-04.diff` (DEV-ONLY commit ad89cfbb559e,
  never lands), `README.txt` (full provenance), `SHA256SUMS`.
- Producer: `scripts/rd_gap/chain_p1part.sh` on zenavif-sweep-1 (ccx63,
  FROM_SNAPSHOT=auto); cavif via ravif--p1part (main 4f2caa93 + dev-patch) →
  zenrav1e--p1part (master 725f5f71 → 767c8ff5); analysis
  `scripts/rd_gap/analyze_p1part.py` + `bd_arm.py`; report
  `docs/FAST_TIER_PARITY_PLAN.md` (P1 lever 1 record) +
  `docs/SPEED_LADDER.md` (cheap-win queue).
- Label store: appended as `p1part-2026-07-04` (see
  `benchmarks/hyperparam-labels-2026-07-03.pointer.md`).
