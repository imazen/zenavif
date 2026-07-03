# CONSOLIDATED RE-STATEMENT raw sweeps — 2026-07-03 (pointer)

Raw per-cell TSVs for `benchmarks/rd_gap_final_2026-07-03.tsv` (block-storage
per repo policy):

- Block storage: `/mnt/v/output/zenavif/final-2026-07-03/` (mirrored to
  `/mnt/tower/output/zenavif/final-2026-07-03/`, sha256-verified 19/19)
- Contents: `final_{legacy,t26}_{s2,s1}.tsv` (composed-config zr sweeps, full
  12-q grid, butteraugli columns), `aom_{cpu2,cpu0def,cpu0ss2}_timing.tsv`
  (legacy refs, RD_CACHE=off fresh), `aom_t26_{cpu2,cpu0def,cpu0ss2}.tsv`
  (first train26 refs), `cacheverify_*.tsv` (the 176/176 determinism passes vs
  the saved deltaq-2026-07-02 baselines), `conf_{s2,s1}.tsv` (110/110 + 110/110
  aomdec+rav1d clean at the composed config), `ravif_devpatch_2026-07-03.diff`
  (the exact dev-patch, sha256 d8a40c47…, reverted after the sweep),
  `final_report.txt` (full analysis output), `README.md` (bit-reproducibility
  record), `SHA256SUMS`.
- Producer: zenavif scripts/rd_gap on zenavif-sweep-1 (ccx63, restored from
  snapshot 404271314); cavif via ravif dev-patch -> zenrav1e master@origin
  `c9c2d5f7`; env `ZENRAVIF_TUNE=ssimulacra2 ZENRAVIF_PALETTE=auto`;
  `S1_DEEP_ARMS_LIVE=true`.
- enc_ms: zr sweeps + timing refs ran sequential solo (fresh cells; honest);
  cacheverify TSVs' enc_ms are contended — do not use those for speed claims.
