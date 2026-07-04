# FASTWINS P0 raw sweeps — 2026-07-04 (pointer)

Raw per-arm TSVs for `benchmarks/rd_gap_fastwins_2026-07-04.tsv` (block-storage
per repo policy):

- Block storage: `/mnt/v/output/zenavif/fastwins-20260704/` (SHA256SUMS;
  mirrored to `/mnt/tower/output/zenavif/fastwins-20260704/`)
- Contents: `w2_s6_{base,size1,size2,type,typred,min,full,red}.tsv` +
  `w2_s8_{base,size1,min,red}.tsv` (the s4→s6 rdo_tx cliff decomposition, every
  cell PALCONF-clean), `w1_s6_thr{2,4,8,16,48}.tsv` + `w1_s4_thr{1,4,8,48}.tsv`
  (the tile-count RD curve; the pre-policy binary's `--threads N` arms measure
  the OLD default formula), `confirm_s{6,8}_{base,size1}.tsv` (full 12-q landing
  grids), `timing_*.tsv` (19 solo RD_CACHE=off arms — the ONLY wall-time-grade
  enc_ms, including the `timing_default_{old,new}_s{6,4}` no-flag 48-core
  before/after pair), `chain.log` + per-run logs,
  `ravif_devpatch_fastwins_2026-07-04.diff` (DEV-ONLY commit 86de671466d1,
  never lands), `README.txt` (full provenance), `SHA256SUMS`.
- Producer: `scripts/rd_gap/chain_fastwins.sh` on zenavif-sweep-1 (ccx63,
  restored from snapshot lineage 404626301); cavif via ravif--fastwins
  (main 55f8c935+7baad5f9 + dev-patch) → zenrav1e--fastwins (master d82c16ba);
  analysis `scripts/rd_gap/bd_arm.py`; report `docs/SPEED_LADDER.md`
  ("Wrapper-level threading/tiling hazard" + cheap-win queue updates) and
  `docs/FAST_TIER_PARITY_PLAN.md` (P0 status).
- Label store: appended as `fastwins-2026-07-04` (see
  `benchmarks/hyperparam-labels-2026-07-03.pointer.md`).
