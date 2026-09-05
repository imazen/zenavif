# zenav1-svt pin bump 2ca060f42 -> 2d75a105f — the measured cost delta (2026-09-05)

**Why:** the zensim ladder-instrument rebuild needs two AVIF ladders (zenravif +
svt-rs) across ~2,600 cells each, so the svt backend's per-cell cost is a budget
input. The user's description of their own encoder work was "faster"; this file
records what the AVIF **still-encode path in this crate** actually does across the
two pins. **Nothing here is inherited or estimated.**

## Pins

| | rev | note |
|---|---|---|
| old | `2ca060f421760cff4b278fb4baf47ba86afcb5f2` | the pin before this bump |
| new | `2d75a105fe0b310bf586110951315f014e274fff` | `imazen/zenav1-svt` origin/main, 140 commits, pushed 2026-09-05 05:48 MDT |

Both arms are `zenmetrics` built `--features sweep,avif-svt` against this crate as a
path dep, into **separate `CARGO_TARGET_DIR`s**. Which rev each binary links was
**verified from the build fingerprints**, not assumed:
`target-svtold/release/.fingerprint/zenav1-svt-encoder-*` references only
`checkouts/zenav1-svt-.../2ca060f`, and `target/…` only `…/2d75a10`. (This check
exists because the old-pin build was launched moments before the manifests were
restored to the new pin; cargo resolves at build start, and the fingerprint is what
proves it resolved to the old one.)

## Method

3 references from the zensim dial-grid source set (1022x818, 818x1022, 512sq),
`--codec zenavif --knob-grid '{"backend":["svt-rs"]}'`, `--q-grid 30,60,90`,
`--no-score --jobs 1`, arms **interleaved** (old,new,old,new…) over **7 rounds**,
**min** per arm. Box otherwise quiet.

## Result — 1.50x, and the output is byte-identical

**Bitstream identity: 9 of 9 cells have identical `encoded_bytes` across the pins.**
The 140-commit range is port-vs-C cost reduction landed byte-identical, and this
measurement independently confirms that on this crate's own encode path.

| | old | new | speedup |
|---|--:|--:|--:|
| wall clock, whole 9-cell sweep (min of 7) | 7.067 s | 4.768 s | **1.482x** |
| summed per-cell `encode_ms` (min of 7) | 6932.8 ms | 4627.9 ms | **1.498x** |

Per-cell, min of 7 rounds:

| image | q | old ms | new ms | speedup |
|---|--:|--:|--:|--:|
| 00b13be94a4867dd_1022x818 | 30 | 998.4 | 682.2 | 1.463x |
| 00b13be94a4867dd_1022x818 | 60 | 1156.7 | 765.5 | 1.511x |
| 00b13be94a4867dd_1022x818 | 90 | 1265.1 | 803.4 | 1.575x |
| 037aa5751d88b97f_818x1022 | 30 | 734.3 | 545.1 | 1.347x |
| 037aa5751d88b97f_818x1022 | 60 | 938.5 | 651.7 | 1.440x |
| 037aa5751d88b97f_818x1022 | 90 | 1321.4 | 823.3 | 1.605x |
| 090d19695a8b43c2_512sq | 30 | 125.3 | 96.2 | 1.303x |
| 090d19695a8b43c2_512sq | 60 | 171.1 | 122.6 | 1.396x |
| 090d19695a8b43c2_512sq | 90 | 222.0 | 137.9 | 1.609x |

**The gain rises with quality** (1.30-1.46x at q30, 1.58-1.61x at q90) on all three
images — consistent with the range's heaviest single item being Wiener
`compute_stats` (`2d9262178`, 127x C per call -> 1.35x), whose share of the frame
grows as more of the image survives quantization.

## Honest caveats

* **This is a two-BUILD comparison**, which `zensim/CLAUDE.md`'s perf discipline warns
  cannot be trusted below ~10% (a rebuild reshuffles binary layout by about that
  much). The separation here is **48%**, and the per-round spreads are tight
  (old 7.067-7.237 s, new 4.768-4.804 s, non-overlapping by 2.26 s), so the result
  is far outside that band. A sub-10% claim from this setup would not be reportable;
  this one is.
* **1.50x is not 2x.** "2x faster" was a description of work in the encoder repo, not
  a measurement of this crate's path, and this file does not repeat it.
* Scope: SDR 4:2:0 still encode at zenavif's default preset, 3 images x 3 qualities.
  It is a budget input for the ladder grid, not a general encoder benchmark.

Raw data: `/mnt/v/output/zensim/ladder-2026-09-05/svtpin/` (`ab_wall.tsv`,
`ab_{old,new}_r{1..7}.tsv`).
