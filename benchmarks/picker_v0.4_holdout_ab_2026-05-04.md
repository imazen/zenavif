# zenavif v0.4 picker held-out A/B (table-lookup)

**Verdict: HOLD**

- Holdout: 117 of 587 images (frac=0.2, seed=7)
- Method: table-lookup over the v0.4 sweep TSV; picker chooses cell, default cell = speed6
- Cells in sweep: {'speed6': 5870, 'speed8': 5870}
- Picker cell preference (held-out): {'speed6': 547, 'speed8': 272}

## Per-band results

| band | n | mean Δbytes % | median Δbytes % | win rate (Δ<-0.1%) | mean Δzensim pp |
|---|---:|---:|---:|---:|---:|
| zq30..49 (low) | 234 | +0.35 | +0.00 | 3.0% | -0.04 |
| zq50..74 (mid) | 234 | +0.22 | +0.00 | 3.0% | -0.01 |
| zq75..95 (high) | 351 | +0.31 | +0.00 | 2.6% | -0.00 |
| overall | 819 | +0.30 | +0.00 | 2.8% | -0.02 |

## Reading
- A HOLD picker should beat the default cell on bytes at matched quality.
- Δbytes < 0 means picker is smaller. Δzensim_pp > 0 means picker is sharper.
- This is a TABLE-LOOKUP A/B: it does not measure closed-loop target_zensim convergence. The closed-loop SHIP gate is a separate harness.
- The v0.4 sweep grid is reduced (2 cells); the binary search space is small. Mean overhead is bounded above by the cell delta at any (img, q).
