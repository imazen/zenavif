# deltaq variance-boost raw sweeps — 2026-07-02 (pointer)

Raw per-cell TSVs for `benchmarks/rd_gap_deltaq_2026-07-02.tsv` (too large /
data-shaped for git; block-storage per repo policy):

- Block storage: `/mnt/v/output/zenavif/deltaq-2026-07-02/` (mirrored to
  `/mnt/tower/output/zenavif/deltaq-2026-07-02/`, md5-verified)
- Contents:
  - `deltaq_conf_s2.tsv`, `deltaq_conf_s1.tsv` — 110-cell conformance runs
    at strength 3.0 (aomdec + rav1d-safe columns, both ALL CLEAN)
  - `deltaq_conf_str1_s2.tsv`, `deltaq_conf_str1_s1.tsv` — same at the
    shipped strength 1.0 (both ALL CLEAN)
  - `t26_s2_dq0.tsv` — train26 baseline: s2 + Tune::Ssimulacra2, deltaq off
  - `t26_s2_dq_str{1,2,3,4p5,6}.tsv` — strength arms (aom-units strength)
  - `t26_s2_dq_str3_segB.tsv` — keep-segmentation arm
  - `legacy_s2_deltaq.tsv`, `legacy_s1_deltaq.tsv` — 22-image legacy-corpus
    confirm at the winning strength
  - `aom_cpu2.tsv`, `aom_cpu0_default.tsv`, `aom_cpu0_ss2.tsv` — fresh
    legacy-corpus libaom baselines (the tune session's raws were lost with
    its scratchpad; same pinned rev 632172a4, aomenc 3.14.1, 420, cq 8-63)
- Producer: zenavif scripts/rd_gap on zenavif-sweep-1 (ccx63), cavif via
  ravif dev-patch -> zenrav1e--deltaq workspace (master chain d125713f ->
  66733720 -> 9d14e662 -> 165e83b1; the baked 165e83b1 binary was verified
  byte-identical to the env-swept strength-1.0 arm)
- enc_ms columns are contended (concurrent runs) — not usable for speed
  claims.
- sha256: see SHA256SUMS in the directory.
