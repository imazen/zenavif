# zenavif v0.4 picker — summary

**Status:** trained, baked, table-lookup A/B run. **Production verdict: HOLD pending v0.4-extension sweep.**

## Trained model

| metric | value |
|---|---|
| val argmin_acc | **62.4%** (vs v0.3's 23%) |
| val mean overhead | **1.10%** |
| train→val gap | +0.47pp (NOT overfit) |
| n_inputs / n_outputs / n_layers | 112 / 6 / 3 |
| .bin size | 38 KB (i8) |
| schema_hash | 0xfadeb780836c5d00 |

## Cell taxonomy (REDUCED vs v0.4 spec)

The v0.4 spec asked for 4 cells × 2 tune = 8 configs. The actual sweep
ran **2 cells × no-tune-axis** (`speed ∈ {6, 8}`, `tune` always 0)
because the fan-out agent used a stripped-down KNOB_GRID to fit within
the 4hr budget.

| picker dimension | v0.4 spec | actual collected | gap |
|---|---|---|---|
| speed values | {3, 5, 7, 9} | {6, 8} | missing extremes |
| tune values | {0, 1} | {0} | no signal at all |
| q values per image | 16 (step 5) | 10 (step 10) | coarser |

## Held-out A/B (table-lookup, 117/587 images, seed=7)

Picker compared against two fixed-cell baselines:

| baseline | mean Δbytes | win rate (Δ<-0.1%) | Δzensim_pp | n |
|---|---:|---:|---:|---:|
| `speed=6` (more compressive) | **+0.30%** | low | -0.02 | 819 |
| `speed=8` (faster)            | **-1.14%** | mod | +0.23 | 819 |

**Reading:** speed=6 dominates the bytes-at-quality trade-off. The picker
beats speed=8 (faster baseline) but cannot beat speed=6 (slower, more
compressive baseline). The 2-cell grid leaves no room for the picker to
add bytes-savings on top of the dominant static choice.

## Recommendation

1. **Do not ship as `Preset::Auto` knob picker.** With this sweep grid the
   "always pick speed=6" static heuristic is a strict-better default.
2. **Rerun sweep with full v0.4 grid:** `speed ∈ {3, 5, 7, 9}` + `tune ∈ {0, 1}`.
   The picker's value is in choosing across 4-8 cells where no single
   cell dominates — not in arbitrating between two cells where one wins
   the byte-at-quality contest.
3. **Picker quality itself is good** (62.4% val argmin_acc, 1.10% mean
   overhead) — the limitation is data shape, not model capacity.

## Artifacts
- `benchmarks/zenavif_picker_v0.4_2026-05-04.bin` (38 KB)
- `benchmarks/zenavif_picker_v0.4_2026-05-04.manifest.json`
- `benchmarks/picker_v0.4_holdout_ab_2026-05-04.md` (per-band table-lookup A/B)
- `s3://zentrain/zenavif/pickers/zenavif_picker_v0.4_2026-05-04.bin`
- Sweep TSV at `s3://zentrain/sweep-v04-2026-05-04/zenavif_pareto_concat.tsv` (11,740 rows, 587 imgs × 20 configs)
