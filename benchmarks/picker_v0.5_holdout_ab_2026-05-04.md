# zenavif v0.4 picker held-out A/B (table-lookup)

**Verdict: SHIP**

- Holdout: 248 of 1241 images (frac=0.2, seed=7)
- Method: table-lookup over the v0.4 sweep TSV; picker chooses cell, default cell = speed5
- Cells in sweep: {'speed3': 56534, 'speed5': 57086, 'speed7': 57086, 'speed9': 57086}
- Picker cell preference (held-out): {'speed3': 2885, 'speed5': 254, 'speed7': 118, 'speed9': 215}

## Per-band results

| band | n | mean Δbytes % | median Δbytes % | win rate (Δ<-0.1%) | mean Δzensim pp |
|---|---:|---:|---:|---:|---:|
| zq30..49 (low) | 992 | -7.05 | -6.06 | 85.2% | +21.98 |
| zq50..74 (mid) | 1240 | -8.03 | -6.42 | 87.4% | +23.46 |
| zq75..95 (high) | 1240 | -5.10 | -2.67 | 74.8% | +8.06 |
| overall | 3472 | -6.70 | -5.13 | 82.3% | +17.54 |

## Reading
- A SHIP picker should beat the default cell on bytes at matched quality.
- Δbytes < 0 means picker is smaller. Δzensim_pp > 0 means picker is sharper.
- This is a TABLE-LOOKUP A/B: it does not measure closed-loop target_zensim convergence. The closed-loop SHIP gate is a separate harness.
- The v0.4 sweep grid is reduced (2 cells); the binary search space is small. Mean overhead is bounded above by the cell delta at any (img, q).
