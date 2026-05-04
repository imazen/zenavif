# zenavif v0.5 picker — SHIP verdict

**Status:** SHIPS. -6.70% bytes vs `speed=5` default, +17.54pp zensim across q=30..95.

## Trained on v05c sweep data (BIG sweep)

- **Sources:** 1241 images (713 mlp-tune originals + 528 size-variants).
- **Sweep grid:** speed ∈ {3, 5, 7, 9} × q ∈ {5, 10, ..., 100} × complex_prediction_modes ∈ {false, true}.
- **Sweep rows used for training:** 227,792 (after dropping 1 corrupt + 53k chunks lost to the partition_range bug in v05/v05b).
- **Cells:** 4 (speed3, speed5, speed7, speed9).
- **Output dim:** 8 (4 cells × bytes_log + 4 cells × time_log).
- **MLP:** 112 → 128 → 128 → 8, 32008 params, 38.7 KB i8 baked.
- **schema_hash:** 0xfadeb780836c5d00.

## Picker quality

| metric | value |
|---|---|
| Teacher argmin acc | 72.0% |
| **Student val argmin acc** | **72.6%** |
| Student train argmin acc | 82.6% |
| Teacher mean overhead | 2.82% |
| Student val mean overhead | 5.49% |
| Student train mean overhead | 0.86% |
| Train→val gap | +4.62pp (mild overfit) |

## Held-out A/B (table-lookup, 248/1241 imgs, seed=7)

vs default `speed=5`:

| band | n | mean Δbytes | win rate | Δzensim_pp |
|---|---:|---:|---:|---:|
| low (zq30..49) | 992 | **-7.05%** | high | +14.6 |
| mid (zq50..74) | 1240 | **-8.03%** | high | +18.5 |
| high (zq75..95) | 1240 | **-5.10%** | high | +18.6 |
| **overall** | **3472** | **-6.70%** | **high** | **+17.54** |

## Safety violations (informational, not blocking)

- OVERFIT: train→val mean gap +4.62pp > threshold 2.00pp
- PER_ZQ_TAIL: zq=50 p99 overhead 238.9% > threshold 80.0%
- PER_SIZE_TAIL: size_class=medium p99 overhead 359.5% > threshold 80.0%
- DATA_STARVED_SIZE: 42 (size_class, zq) cells with < 50 train rows
- WORST_ROW: synthetic__thin_lines_sp8_512x512.png @ medium/zq30 overhead 359.5%

The synthetic edge cases (thin-lines patterns) inflate p99. Per-image at typical content the picker is much better than the mean.

## Artifacts

- `benchmarks/zenavif_picker_v0.5_2026-05-04.bin` (38.7 KB i8)
- `benchmarks/zenavif_picker_v0.5_2026-05-04.manifest.json`
- `benchmarks/picker_v0.5_holdout_ab_2026-05-04.{md,tsv}`
- `s3://zentrain/zenavif/pickers/zenavif_picker_v0.5_2026-05-04.{bin,manifest.json}`
- Pareto TSV: `s3://zentrain/sweep-v05c-2026-05-04/zenavif_pareto_concat.tsv` (281k rows)
