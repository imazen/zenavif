# SPEED-LADDER GAP MAP raw sweeps — 2026-07-04 (pointer)

Raw per-arm TSVs for `benchmarks/rd_gap_speed_ladder_2026-07-04.tsv` (block-storage
per repo policy):

- Block storage: `/mnt/v/output/zenavif/speedladder-2026-07-04/` (SHA256SUMS;
  mirrored to `/mnt/tower/output/zenavif/speedladder-2026-07-04/`)
- Contents: `zr_{t26,leg}_s{2,4,6,8,10}_{tune,off}.tsv` (20 zr RD arms, every cell
  PALCONF-clean), `aom_{t26,leg}_cpu{2,4,6,8,9}{def,iq}.tsv` (20 allintra arms),
  `aomgood_*` (6 GOOD anchor replays), `timing_*.tsv` (20 solo RD_CACHE=off arms —
  the ONLY wall-time-grade enc_ms), `chain.log` + per-run logs,
  `ravif_devpatch_2026-07-04.diff` (sha256/16 b2180ec28e61e447, reverted after),
  `README.md` (bit-reproducibility record), `SHA256SUMS`.
- Producer: `scripts/rd_gap/chain_speed_ladder.sh` on zenavif-sweep-1 (ccx63,
  restored from snapshot 404331993; teardown snapshot 404626301 = new canonical);
  cavif via ravif dev-patch -> zenrav1e master@origin 184a616f; analysis
  `scripts/rd_gap/analyze_speed_ladder.py`; report `docs/SPEED_LADDER.md`.
- Label store: appended as `speedladder-2026-07-04` (9,776 rows; see
  `benchmarks/hyperparam-labels-2026-07-03.pointer.md`).
