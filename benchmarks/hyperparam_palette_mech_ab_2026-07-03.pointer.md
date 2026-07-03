# Palette-gate mechanism A/B raw data pointer — 2026-07-03

Committed files: `hyperparam_palette_mech_ab_2026-07-03.tsv` (per-file BDs of
auto/always vs off, both configs, veto columns, gate features) +
`hyperparam_palette_mech_timing_2026-07-03.tsv` (always/off enc_ms ratios,
RD_CACHE=off). Analysis: `scripts/hyperparam/analyze_palette_mech_ab.py`;
console dump with the three-way/confusion/refit tables:
`/mnt/v/output/rd-gap-palette-ab-2026-07-03/analysis_console.txt`.

## Canonical raw data (block storage)

- `/mnt/v/output/rd-gap-palette-ab-2026-07-03/` — run TSVs (`results/`: 6
  shipped-config cavif arms s2 12-pt + s6 6-pt, the 2700-cell isolated
  rav1e-CLI 3-arm × s{2,6} sweep, 2 timing arms), harness sample TSVs
  (`samples/`), all 2,700 isolated-config encoded IVF streams (`ivf/`,
  cell-named), `_MANIFEST.json` (runs, envs, conformance record, headline).
- `/mnt/v/output/rd-gap-palette-val-2026-07-03/` — the 14-origin VAL corpus
  (LSD {1,3,5} origins; picks + reasons in `picks_val14.json`, selected by
  `scripts/hyperparam/select_palette_val_picks.py`), materialized by
  `examples/wedge_corpus.rs` with the canonical feature-parquet conventions;
  join verification 108/108 exact (`verify_features.tsv`).
- Label store append: sweep_sources `palette-mech-ab-2026-07-03` +
  `palette-mech-iso-2026-07-03` (12 arms, 6,216 rows, 100% feature-join) in
  `/mnt/v/output/zenavif/hyperparam-labels-2026-07-03/labels.parquet`.

## Provenance

- Binary chain: `ravif--wedge@9d2b97c` → `zenrav1e--wedge@32477046` (the
  pinned WEDGE-FINDER clones; byte-continuity with the wedge-2026-07-03 zr
  arms verified bit-exact — 7052.full.native q60 auto = 2646 bytes).
- Box: `zenavif-sweep-2` (Hetzner ccx63, snapshot restore); runs
  20260703T0628*–0650* in `scripts/rd_gap/remote/results/`.
- Conformance: 0 failures in 6,216 cells. Isolated: aomdec decode per cell +
  raw-md5 agreement aomdec↔rav1d-safe on 1800/1800 palette-armed cells
  (`scripts/rd_gap/palette_iso_cell.sh` + `examples/ivf_raw.rs`). Shipped:
  PALCONF gate per always/auto cell (`scripts/rd_gap/zenrav1e_cell.sh`).
- Tower mirror: `/mnt/tower/output/rd-gap-palette-ab-2026-07-03/` +
  `/mnt/tower/output/rd-gap-palette-val-2026-07-03/` (sha-verified).
