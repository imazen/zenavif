# Palette A/B raw data pointer — 2026-07-03

Full per-cell data for `benchmarks/palette_ab_train26_2026-07-03.tsv` (the
committed file is the per-(image,speed) BD summary; this pointer records the
raw layers).

## Canonical raw data (block storage)

- `/mnt/v/output/zenrav1e-palette/sweep-20260703-final2/` — **canonical**
  3-arm sweep (off/always/auto), 720 cells: train26 24-origin sample ×
  q{60,100,140,180,220} × s{2,6}. Encoder: zenrav1e master 49982460
  (`rav1e` binary), `--still-picture --threads 1 --lrf false
  --filter-intra false` (isolated config per zenrav1e#32/#33; skip-recon
  fix b30dd752 included). Scoring: color.py color-exact RGB<->YUV, aomdec
  decode, zen-metrics ssim2 + butteraugli (max + pnorm3). Per-cell
  conformance: aomdec success required; palette-armed cells additionally
  require aomdec-vs-rav1d-safe raw-I420 md5 byte agreement. `ivf/` holds
  every encoded stream (content-addressed by cell name).
- `/mnt/v/output/zenrav1e-palette/fam7-continuity-v3/` — **canonical**
  legacy-corpus family-7 continuity: o_7000/7001/7002 × arms {aomenc-cpu2
  (cq 8-63), zr-s2-{off,always,auto} (q 10-220; q10-30 added so the
  frontiers overlap aomenc's ssim2 range)}, same scoring path. Committed
  copy: `benchmarks/palette_fam7_continuity_2026-07-03.tsv` (frozen before
  the last redundant auto low-q cells; auto==always byte-identical on all
  three plots at every overlapping q).
- `/mnt/v/output/zenrav1e-palette/y4m-colorpy/` — color.py-converted
  420 y4m inputs (encoder inputs for all canonical runs).

## Superseded runs (kept for provenance; do NOT use for RD conclusions)

- `sweep-20260703/` + `sweep-20260703-auto/` — first pass; ffmpeg color
  conversion (poisons absolute ssim2) AND pre-skip-fix encoder with LRF +
  filter-intra desyncs (zenrav1e#32/#33) polluting photo cells. Their
  conformance columns (aomdec + md5-agreement, 480 palette-armed cells, 0
  fails) remain valid evidence.
- `sweep-20260703-colorpy/` — second pass; color-exact scoring but still
  pre-isolation (LRF/filter-intra on). Same status: RD superseded,
  conformance valid (480 more clean cells).
- `fam7-continuity/` — first fam7 attempt, partial (LRF/filter-intra on).

## Provenance

- git: zenrav1e @ 49982460 (master; palette 68a8d81f..df27117c + skip-fix b30dd752); zenavif commit = the one adding this file
- host: lilith workstation (WSL2), commands in
  the session scripts (palette_sweep_final2.sh, fam7_continuity_v3.sh)
