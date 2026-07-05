# SSIMRD raw sweeps (2026-07-05) — pointer

Raw per-cell TSVs for the per-16×16 ssim-rdmult scaling program
(`docs/TUNE_SSIMULACRA2_PLAN.md` §(a2) port; verdict tables:
`benchmarks/rd_gap_ssimrd_2026-07-05.tsv` + `rd_gap_ssimrd_val_2026-07-05.tsv`).

- Block storage: `/mnt/v/output/zenavif/ssimrd-20260705/`
  (`sr_base_t26.tsv` 288 rows, `sr_base_val.tsv` 168, `sr_str_{0.25,0.5,1.0,2.0}.tsv`
  144 each, `sr_val_0.5.tsv` 84, `aom_t26_cpu2{iq,def}.tsv` 192 each,
  `aom_val_cpu2iq.tsv` 112, `DECISION_RULE.md` — the pre-registered rule,
  amended pre-arm-data per the 93b83401 evaluation policy)
- Box raw dir (also inside the `zenavif-sweep-1` teardown snapshot):
  `/home/lilith/sweep_out/ssimrd_20260705/` incl. per-run `.log`s
- Binary chain: cavif from `ravif--ssimrd` devpatch (DEV-ONLY commit
  29ae48b on ravif d72304a; box cavif sha256/16 `909857ad43f9c227`) →
  `zenrav1e@57de2815` (`ssim_rdmult_strength` knob, default-off).
  Reproduce: ravif @ d72304a + the 29ae48b-shape devpatch
  ([patch.crates-io] path → zenrav1e @ 57de2815, ZENRAVIF_SSIMRD env →
  `ssim_rdmult_strength`), `chain_ssimrd.sh`.
- Gates: knob-off + Some(0.0) 36/36 byte-identical vs master-built cavif;
  armed 36/36 byte-live; env-off box rows byte-equal to the label store's
  `speedladder/zr-s2-tune` train26 rows 288/288; PALCONF (aomdec +
  rav1d-safe raw-md5 agree) 0 CONFFAIL / 0 CELLFAIL on all 1,116 zr cells.
