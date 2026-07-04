# P2HEADS raw sweeps — 2026-07-04 (pointer)

Raw per-arm TSVs for `benchmarks/rd_gap_p2heads_2026-07-04.tsv` (block-storage
per repo policy):

- Block storage: `/mnt/v/output/zenavif/p2heads-20260704/` (SHA256SUMS;
  mirrored to `/mnt/tower/output/zenavif/p2heads-20260704/`)
- Contents: head-3 intra-axis arms `p2_s{6,8}_{base,intra7}.tsv` +
  `p2_s6_{ship,intra7ship}.tsv` (coarse 6-q, t26); 12-q global refs
  `p2_conf_s6_{base,ship}.tsv`; composed per-class cells
  `p2c[i7]_{none,size1,min}_{ship,m32}.tsv` (t26) and
  `p2v_{base,ship}.tsv` + `p2vc/p2vi7_*.tsv` (14 VAL-LSD origins); the
  W-gate attribution-factoring cells `p2vx_{size1_m32,none_ship}.tsv`
  (the 8103 conviction) + rules-v2 reassignment cells `p2rx_*.tsv`;
  `p2t_*.tsv` solo timing (JOBS=1 RD_CACHE=off — the ONLY wall-grade
  enc_ms); `chain*.log`, `ravif_devpatch_p2heads_2026-07-04.diff`
  (DEV-ONLY workspace commit b21b35b9f23c, never lands), `README.txt`
  (full provenance incl. the stale-workspace incident), `SHA256SUMS`.
- Producer: `scripts/rd_gap/chain_p2heads.sh` on zenavif-sweep-1 (ccx63,
  FROM_SNAPSHOT=auto); cavif via ravif--p2heads (main 0191489b + dev-patch)
  → zenrav1e--p2heads (master 39f0ecdd, includes the one-sided margin fix);
  box cavif sha256/16 `bd0b33d2ec5ef156`.
- Fit inputs (no fresh encodes): `benchmarks/hyperparam_tx_budget_2026-07-04
  .tsv` + `hyperparam_partition_budget_2026-07-04.tsv` fit from the label
  store's fastwins/p1part per-image surfaces.
- Label store: appended as `p2heads-2026-07-04` sources
  (`scripts/hyperparam/build_label_store.py`).
