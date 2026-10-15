# AV1 backend probe, 2026-10-15

An exploratory comparison of zenravif's zenrav1e, zenav1-svt and zenav1-aom
on stills. It was run to choose the router's capability-fallback order:
which backend should take a request the preferred one refuses, such as
12-bit for zenav1-svt. It is not a sweep, and nothing beyond that order is
calibrated from it.

**Revisions** (each repository's head, built by `dev/backend-probe`):
- zenrav1e master `fa0e25bc02125c5cc496b6af8983f37b1a83ec68`
- zenav1-svt main `224c6bbb9a3dc577c64f6e0531bda5c3f60c6b0b`
- zenav1-aom main `93783de612062707501932d0a4d9d1c22630fb32`

**Host:** dev (Ryzen 9 9950X3D), one thread, under `run-heavy`. Wall time
was 104 s; peak RSS 0.05 GiB.

**Cells:** 4 photos × 3 depths × 4 quantizer points × 3 backends.
- Photos: CLIC 2025 final-test, files 1, 8, 15 and 22 by name, cover-scaled
  to 768×512.
- Depths: 8, 10 and 12-bit 4:2:0, all converted by FFmpeg from the 8-bit
  PNGs.
- Quantizer points: qindex 60, 100, 140 and 180.
  - zenrav1e takes `quantizer` directly.
  - zenav1-svt takes qp = qindex / 4.
  - zenav1-aom takes cq_level = qindex / 4.
- Speed: zenrav1e speed 6, zenav1-svt preset 6, zenav1-aom cpu-used 6
  (all-intra, loop restoration on).

Per-cell data is in `backend_probe_2026-10-15.csv`. PSNR is 6:1:1 over Y,
Cb and Cr, on code values at the coded depth, after a libdav1d decode
(FFmpeg 8.0.1). All 128 encodes decoded. zenav1-svt refused all 16 12-bit
cells: "12-bit is outside C's shipping envelope".

```text
bits backend   qindex  bytes(mean)  psnr(mean)  ms(mean)
   8 zenav1-aom     60       77100      43.96     154.3
   8 zenav1-aom    100       53529      40.53     165.5
   8 zenav1-aom    140       32158      36.38     141.1
   8 zenav1-aom    180       17084      32.34     124.6
   8 zenav1-svt     60       74684      43.51      83.1
   8 zenav1-svt    100       49831      39.82      79.0
   8 zenav1-svt    140       28777      35.66      69.2
   8 zenav1-svt    180       14341      31.51      50.5
   8 zenrav1e       60       89808      44.59    2216.8
   8 zenrav1e      100       62930      41.00    1997.0
   8 zenrav1e      140       37006      36.42    1742.9
   8 zenrav1e      180       18054      31.85    1460.7
  10 zenav1-aom     60       80305      44.75     203.7
  10 zenav1-aom    100       53620      40.83     226.4
  10 zenav1-aom    140       31652      36.49     192.0
  10 zenav1-aom    180       16246      32.27     163.4
  10 zenav1-svt     60       78672      44.32     245.9
  10 zenav1-svt    100       50487      40.09     237.1
  10 zenav1-svt    140       28489      35.67     224.3
  10 zenav1-svt    180       13692      31.32     190.3
  10 zenrav1e       60       93308      45.34    2269.2
  10 zenrav1e      100       63456      41.26    2046.7
  10 zenrav1e      140       37168      36.54    1769.4
  10 zenrav1e      180       18031      31.89    1492.2
  12 zenav1-aom     60       79118      44.68     216.7
  12 zenav1-aom    100       50828      40.46     236.9
  12 zenav1-aom    140       28147      35.74     199.0
  12 zenav1-aom    180       12867      31.10     159.7
  12 zenrav1e       60       94419      45.50    2270.0
  12 zenrav1e      100       63735      41.32    2060.8
  12 zenrav1e      140       37343      36.57    1779.1
  12 zenrav1e      180       18041      31.88    1489.0

BD-rate vs zenav1-aom at the same PSNR (negative = fewer bytes), per image:
   8 zenrav1e    mean  +13.7%  images ['+9.5%', '+15.8%', '+14.9%', '+14.8%']
   8 zenav1-svt  mean   -0.8%  images ['-1.5%', '+0.1%', '+2.3%', '-3.9%']
  10 zenrav1e    mean  +15.7%  images ['+11.4%', '+17.4%', '+16.6%', '+17.3%']
  10 zenav1-svt  mean   +1.0%  images ['+0.1%', '+1.8%', '+4.0%', '-2.1%']
  12 zenrav1e    mean  +17.6%  images ['+13.2%', '+19.4%', '+18.9%', '+19.0%']
```

**Reading.**
- zenrav1e needs 14–18% more bytes than zenav1-aom at equal PSNR, at every
  depth and on every image, and takes about ten times as long at these
  speed settings.
- zenav1-svt and zenav1-aom are within about 1% of each other on bytes.
  zenav1-svt is about twice as fast at 8-bit and about the same at 10-bit.
- At 12-bit only zenav1-aom and zenrav1e encode, and zenav1-aom is better
  by 17.6%.

**Limits.**
- Four photos at one size.
- PSNR only: no perceptual metric.
- One speed point per backend, and the speed scales are not equivalent.
- The 10- and 12-bit sources are converted from 8-bit PNGs.
- Treat the zenav1-svt vs zenav1-aom ordering as a tie. Only zenrav1e's
  deficit is consistent across every cell.
