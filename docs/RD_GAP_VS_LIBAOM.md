# zenrav1e RD gap vs libaom-at-slow — measurement + narrowing plan

**Status:** open. zenrav1e (our AV1 still-image encoder, the engine behind zenavif) is a genuinely
weaker RD encoder than the latest libaom at slow settings. This doc records the measured gap, the
levers already tried-and-rejected (so we don't repeat them), the credible remaining levers, and the
repeatable harness (`scripts/rd_gap/`) for tracking progress as we close it.

## The gap (measured 2026-06-30)

Paired per-image bpp, our AVIF **larger** than libaom-slow at matched SSIMULACRA2 (cpu-used=2, 28
images from clean-picker-corpus-2026-06-26):

| ssim2 target | PHOTOS (n=16–18) | ALL 28 | synthetic plots (fam7) |
|-------------:|-----------------:|-------:|-----------------------:|
| 82 | **+16.3%** | +22.8% | +130.7% |
| 85 | **+11.8%** | +15.9% | +248.9% |
| 88 | **+7.8%**  | +21.8% | +413.9% |
| 90 | **+18.5%** | +23.8% | +468.8% |

- **BD-rate +25% median across all 28; 28/28 images need more bits.**
- The honest headline is the **photo number, ~8–18%**. The extreme +130–470% is entirely synthetic
  screen-content (`7000-lilith-plots`) where AV1/AVIF is weak regardless — content mismatch, not pure
  encoder weakness (but see the palette-RDO note below).

### It is not a speed handicap — zenrav1e was given its best

cavif/zenrav1e speed scale is **1 (best) → 10 (fast); there is no s0**, and **s1 == s2 byte-identical**
(reconfirmed 433 034 B both on a 1 MP photo). So s2 is already zenrav1e's max-effort operating point.
At **matched speed** (aomenc cpu-used=2 ≈ zenrav1e s2, both ≈0.088 Mpx/s) the gap is the ~10–18% above;
and it *widens* as libaom is given more time (cpu-used=0), while zenrav1e cannot search deeper than s2.
**Conclusion: zenrav1e's search ceiling sits below libaom's — this is an RD-completeness gap, not a
speed/effort tradeoff.**

### Why this matters beyond the encoder

Our canonical picker/Pareto data showed JXL winning the ssim2→bpp Pareto at HQ by −39–63% vs
"best-other" — *more* than the published Cloudinary figure (−28% vs libavif). This AVIF gap is a large
part of that inflation: with a libaom-quality AVIF as the reference, JXL's photo margin is ~15–25%
(≈ Cloudinary), and a picker trained on our-AVIF data **over-picks JXL**. Closing this gap tightens
both the codec and the picker. See `~/work/zen/zenmetrics/benchmarks/avif_vs_libaom_2026-06-30.md`.

### Provenance

- **libaom:** git HEAD `632172a468f5e91c5b40daaa0a91f4a291c63af4` (aomedia), aomenc 3.14.1, cmake Release;
  `--cpu-used={2,1,0} --end-usage=q --cq-level=<sweep> --passes=1`, default tune, color-exact
  BT.601-full / identity-RGB, formats {420, 444, 444-RGB}, 8-bit.
- **our-AVIF:** canonical `zenavif_lossy` (zenavif `a5697e0a8b0d` / zenrav1e `22a58d58db1d`, `modes_full`
  best-of over speeds {2,4,6,8}×3 formats×8/10-bit); cross-checked vs a fresh `cavif` encode (bpp <1%).
- **Scorer:** `fast-ssim2-cli v0.6.0` / core 0.8.2 — the same crate that produced the canonical
  `score_ssim2` column (no scorer mixing). A color-conversion confound (ffmpeg, ~MAE 0.07) was found and
  replaced with a color-exact converter.
- **Full data:** `/mnt/v/output/avif-vs-libaom-2026-06-30/` (RD graph `rd_curves.png`, CSVs, raw
  `aom_results.tsv`), mirrored to `/mnt/tower/output/avif-vs-libaom-2026-06-30/`.

## Already tried and REJECTED — do not re-run

From `EXPERIMENTS-SURVEY-2026-05-17.md` + `AVIF_LEARNINGS.md §1`:

| Lever | Result | Status |
|---|---|---|
| VAQ (variance-adaptive quantization) | +2.8% BD-rate *worse* (psy-tune already covers it) | rejected |
| Trellis quantization | +34% encode time for 0.3% bytes | rejected |
| Per-SB delta-q / complex segmentation | shifts operating point, not efficiency | rejected |
| SGR full complexity at speed 6 | zero effect | rejected |
| LRU on skip | zero effect | rejected |
| Bottom-up partition search | zero effect | rejected |
| SVT-AV1 integration | maintenance burden; zenrav1e proven | rejected |

**A naive "improve AVIF RD" plan proposes exactly deltaq/VAQ/trellis. Those are dead ends here** — the
plateau is documented across multiple prior experiments. New work must target the search-completeness
ceiling instead.

## Credible narrowing levers (priority order)

The gap is a search-completeness ceiling (s1==s2, gap widens as libaom deepens). The levers are about
finding RD the current best-speed search leaves on the table — measurement-first, per `AVIF_LEARNINGS §1`.

1. **Best-speed RDO completeness audit (highest priority, measurement not new code).** At zenrav1e's
   slowest (s2), verify that **palette search, CfL parameter scan, and transform-kernel selection** are
   not early-terminated where a fuller search would find bytes. libaom at cpu-used=0 does more of this;
   zenrav1e may be shortcutting it even at s2. Probe with the harness's per-knob forced-on mode.
2. **Investigate the s1==s2 clamp.** zenrav1e refuses to search deeper than s2. If that ceiling is an
   artificial clamp rather than true convergence, exposing a deeper mode (fuller partition-RD /
   tx-type / CfL search) could recover RD. Note bottom-up partition search was already rejected (zero
   effect), so the lever is tx-kernel / CfL / palette *thoroughness*, not partition *order*.
3. **Perceptual-tune parity.** aomenc gains from `--tune=ssimulacra2` (not used here, to stay fair);
   zenrav1e's psy-tune "already covers VAQ." Verify psy-tune is competitive with libaom's default at
   matched ssim2, and evaluate an ssimulacra2-aware tune. (Hypothesis — measure before building.)
4. **Palette-RDO thoroughness for screen content.** The synthetic-content blowup (+130–470%) is partly
   AVIF being a poor screen-content codec, but `AVIF_LEARNINGS` flags verifying palette RDO is not
   early-terminated at speed≥6. Low priority for the photo gap; relevant if screen content matters.

**Explicitly NOT on the list:** deltaq/VAQ/trellis (rejected), EPT/NSDT (video-only gains, don't
transfer to stills — `AVIF_LEARNINGS §1`), learning-based intra / GPU search (out of scope for
zenrav1e's design).

## The harness — `scripts/rd_gap/`

Repeatable RD-gap measurement so every zenrav1e change can be scored against the current baseline.

**What it does:** for each corpus image, encodes with **zenrav1e (cavif, best speed s2)** and, if
present, **libaom (aomenc, cpu-used 2)**, decodes both, scores decoded-vs-source with the same
`fast-ssim2-cli` the canonical data used, and computes the paired bpp gap at ssim2 {82,85,88,90} split
by content class. Emits a TSV under `benchmarks/`.

**Prerequisites** (external, not vendored):
- `cavif` — build `~/work/zen/ravif` (`cargo build --release`, → `target/release/cavif`).
- `aomdec`/`aomenc` — build libaom at a pinned rev (see Provenance). Optional; without them the harness
  emits only the zenrav1e RD curve and diffs it against the committed baseline.
- `fast-ssim2-cli` — build `~/work/zen/fast-ssim2` (`target/release/fast-ssim2-cli`).

**Run:**
```bash
cd scripts/rd_gap
CAVIF=~/work/zen/ravif/target/release/cavif \
AOMENC=~/work/aom/build_slow/aomenc AOMDEC=~/work/aom/build_slow/aomdec \
SCORER=~/work/zen/fast-ssim2/target/release/fast-ssim2-cli \
  ./run_gap.sh                       # runs under nice; writes rd_gap_results.tsv
python3 analyze.py rd_gap_results.tsv # prints the per-ssim2-bin gap + content split
```

**Regression tracking:** the committed baseline is `benchmarks/rd_gap_baseline_2026-06-30.tsv` (the
2026-06-30 gap). After any zenrav1e RD change, re-run and diff — the photo-gap column is the number to
drive down. Target: photo gap < 5% at ssim2 82–90 (from today's ~10–18%).

## References
- `AVIF_LEARNINGS.md` §1 — full research synthesis (tried/rejected + actionable), gitignored, mirrored
  to `~/work/zen/zenpapers/AVIF_LEARNINGS.md`.
- `~/work/zen/EXPERIMENTS-SURVEY-2026-05-17.md` — the tried/rejected source of record.
- `docs/RAV1E_PICKER_PLAN.md` — the zenrav1e picker (separate: picks per-image settings; does not change
  the encoder's RD ceiling).
- `~/work/zen/zenmetrics/benchmarks/avif_vs_libaom_2026-06-30.md` — the measurement this doc summarizes.
