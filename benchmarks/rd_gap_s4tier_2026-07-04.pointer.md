# S4TIER raw sweeps — 2026-07-04 (pointer)

Raw per-arm TSVs for `benchmarks/rd_gap_s4tier_2026-07-04.tsv` (block-storage
per repo policy):

- Block storage: `/mnt/v/output/zenavif/s4tier-20260704/` (SHA256SUMS;
  mirrored to `/mnt/tower/output/zenavif/s4tier-20260704/`)
- Contents: byte-continuity gate `s4_cont_{base,intra7}.tsv` (288/288
  byte-identical to the p2heads chain — the num_modes_rdo_override knob's
  env-off identity on the real pipeline); the top-5 knob axis
  `s4_s6_i5{,ship}.tsv` + `s4_s8_i5.tsv`; the hi-q filter probe
  `s4_s6_{cdef,lrf}.tsv`; the composed v3 classes 12q at BOTH intra arms
  `s4c_{none,size1,min}_{ship,m32}_{i7,i5}.tsv`; full-tx oracle extras
  `s4x_full_{ship,m32}_i7.tsv` (8414/6606/5048 + 9074/9868 — upper-bound
  factoring, no honest gate); solo timing `s4t_*.tsv` (JOBS=1 RD_CACHE=off
  — the ONLY wall-grade enc_ms; v3+i5 6.26× / v3+i7 7.61× / +fullx 10.12×
  plain-s6); val transfer `s4v_*.tsv` (13 scoreable VAL-LSD origins, both
  arms); `chain.log`, `ravif_devpatch_s4tier_2026-07-04.diff` (DEV-ONLY
  workspace, never lands), `README.txt` (full provenance), `SHA256SUMS`.
- Producer: `scripts/rd_gap/chain_s4tier.sh` on zenavif-sweep-1 (ccx63,
  FROM_SNAPSHOT=auto); cavif via ravif--s4tier (main d72304a1 + dev-patch)
  → zenrav1e--s4tier (master 0d392334 = 071e9844 num_modes_rdo_override
  knob + fmt); box cavif sha256/16 `26091145a8cdc388`.
- Design inputs (no fresh encodes): `scripts/hyperparam/fit_s4_tier.py`
  over the label store + the committed `hyperparam_{tx,partition}_budget`
  fit TSVs; samples `scripts/hyperparam/emit_s4tier_samples.py`.
- Label store: appended as `s4tier-2026-07-04` sources
  (`scripts/hyperparam/build_label_store.py`).
