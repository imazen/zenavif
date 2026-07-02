# Pointer: Tune::Ssimulacra2 raw sweep TSVs (2026-07-02)

Raw per-cell sweep outputs for the `Tune::Ssimulacra2` measurement program
(committed summary: `rd_gap_tune_ss2_2026-07-02.tsv`; program record:
`docs/RD_GAP_VS_LIBAOM.md`; design + verdicts: `docs/TUNE_SSIMULACRA2_PLAN.md`).
Each file is an rd_gap TSV (`image w h family encoder fmt q bytes bpp ssim2
enc_ms butteraugli_3n butteraugli_max`), 264 data rows (22 images x 12 Q),
cavif `--depth 8` 4:4:4 on zenavif-sweep-1 (Hetzner ccx63), zenrav1e workspace
tree at the commits noted in the summary header.

**Block-storage path:** `/mnt/v/output/zenavif/tune-ss2-2026-07-02/`

| file | config |
|---|---|
| tune_base.tsv | s2, tune off (== master; +0.0000% vs committed splitcost baseline) |
| tune_s1chroma.tsv | s2, stage 1 (chroma delta-q) |
| tune_s2lambda.tsv | s2, stages 1+2 (+frame lambda weight — DROPPED) |
| tune_s3qm.tsv | s2, stages 1+2+3 (+ss2 QM curves) |
| tune_s4trellis.tsv | s2, stages 1..4 (trellis lambda x0.25 — DROPPED) |
| tune_s5varboost.tsv | s2, stages 1..5 (+Variance Boost — DROPPED) |
| tune_composed13.tsv | s2, final composition (mechanisms 1+3; lambda removed from build) |
| tune_s4t100.tsv | s2, composition + trellis lambda x1.0 (DROPPED) |
| tune_s1speed.tsv | s1 deep (dev-flipped ravif) + final composition |
| tune_conformance_s2.tsv | 110-cell conformance, s2 + tune (aomdec + rav1d-safe columns) |
| tune_conformance_s1.tsv | 110-cell conformance, s1 deep + tune |

sha256 (first 16 hex chars) at copy time:

```
See /mnt/v/output/zenavif/tune-ss2-2026-07-02/SHA256SUMS
```

Provenance: produced 2026-07-02 by the tune-ss2 session; sweeps driven via
`scripts/rd_gap/remote/run_remote.sh` (per-run dirs also fetched under
`scripts/rd_gap/remote/results/`, gitignored). The ravif dev-patch state used
for the sweeps ([patch.crates-io] zenrav1e -> zenrav1e--tune workspace,
ZENRAVIF_TUNE env passthrough, S1_DEEP_ARMS_LIVE=true) is documented in
`docs/TUNE_SSIMULACRA2_PLAN.md` and was reverted after the program.
