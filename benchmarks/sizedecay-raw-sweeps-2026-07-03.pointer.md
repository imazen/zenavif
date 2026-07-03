# Pointer: size-decay isolation A/B raw sweeps (2026-07-03)

Committed summary: `hyperparam_size_decay_ab_2026-07-03.tsv` (this dir).
Analysis: `scripts/hyperparam/analyze_size_decay_ab.py`; arms driver:
`scripts/rd_gap/sizedecay_arms.sh`.

Raw per-arm TSVs (7 train arms + 2 val arms + val cpu2 refs + qmdist ramp
trials, run logs, and the PRE-REGISTERED decision rule):

- Block storage: `/mnt/v/output/zenavif/sizedecay-2026-07-03/`
  (`train_arms/`, `val_arms/`, `val_cpu2/`, `ramp_arms/`, `DECISION_RULE.md`,
  `README.md` with box/encoder provenance)
- Tower mirror: `/mnt/tower/output/zenavif/sizedecay-2026-07-03/`

Encoder: zenrav1e--sizedecay workspace commit `1428ecdd` on master `c9c2d5f7`
via ravif--wedge@9d2b97c (dev env passthroughs). `ZENRAV1E_SD_DISABLE`
leave-one-out gates + `ZENRAV1E_SD_RAMP` long-edge ramp trials; env-unset is
byte-identical to the master binary (md5-gated locally and on-box).
Box: zenavif-sweep-1 (Hetzner ccx63, snapshot restore), runs
20260703T075827Z (train), 20260703T082246Z (val cpu2), 20260703T083533Z
(val arms), ramp run id in the raw README.

Label store: `sweep_source=sizedecay-2026-07-03` rows in
`/mnt/v/output/zenavif/hyperparam-labels-2026-07-03/labels.parquet`.
