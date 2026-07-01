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

## Confirmed findings (2026-07-01 audit — supersedes the "verify" framing below)

Source-read + measured, not hypothesized. See `scripts/rd_gap/palette_ablation.sh` +
`tool_ablation.sh` and `benchmarks/rd_gap_{palette,tool}_ablation_2026-07-01.tsv` for the harness
and raw numbers.

1. **Palette mode is 100% unimplemented in zenrav1e's encoder — not "early-terminated," never
   available at any speed.** `write_use_palette_mode` (`src/context/block_unit.rs:777`) is always
   called with `enable: false` (`src/encoder.rs:2392`); calling it with `true` hits `unreachable!()`
   with the comment "palette mode is not implemented." Tracked since 2026-04-12 in zenrav1e#2
   ("Filter intra + palette mode ... hits `unimplemented!()`"), still open. Default CDF tables for
   palette size/color-index exist in `entropymode.rs` (spec-ported, e.g.
   `default_palette_y_color_index_cdf`) but aren't wired into the live `CdfContext` struct — the
   entropy-coding scaffolding is partial, the encoder-side search (color quantization, RD-gated size
   2-8 selection, index-map coding) doesn't exist at all. `src/util/kmeans.rs` exists but is used
   only for segmentation clustering, not palette color quantization.
   - **Measured impact — libaom `--enable-palette=0` ablation, same corpus/settings as the baseline:**
     **~0% effect on photos** (17/18 photo images bit-identical bpp with palette on vs off; the
     18th, family 9, wobbles ±1-3% with no consistent sign — noise). **Median +51.8% (up to +103%)
     more bytes on synthetic screen content (family 7 plots)** at matched cq / ~matched ssim2 — this
     is the dominant explanation for the +130-470% plots gap. **Verdict: implementing palette would
     be a large, clear win for screen-content AVIF and is NOT a lever for the photo gap.** Priority
     depends on whether screen content is part of the target traffic mix.
2. **Tx-type search for intra blocks is spec-complete — ruled out, not a gap.** `RAV1E_TX_TYPES`
   (`src/transform/mod.rs:28`) contains exactly the 7 types of AV1's `TX_SET_INTRA_1` (DCT_DCT,
   ADST_DCT, DCT_ADST, ADST_ADST, IDTX, V_DCT, H_DCT). The commented-out FLIPADST variants
   ("TODO: Add a speed setting for FLIPADST") are real but **inter-only** — `get_tx_set`
   (`src/context/transform_unit.rs:137`) never returns an INTER tx-set for intra blocks, so this
   omission cannot affect AVIF (all-intra) encoding at all. No further work here for stills.
3. **CDEF and loop restoration ruled out for the photo gap.** `--enable-cdef=0` and
   `--enable-restoration=0` ablations on libaom (same photo corpus) both land under ±1% with no
   consistent sign (noise-level) — neither tool is where libaom's photo-bpp advantage comes from,
   at least not in a way a simple on/off toggle reveals. zenrav1e already runs `cdef=true` and
   `sgr_complexity=Full` at s2 (`speedsettings.rs`), consistent with this result.
4. **CfL bounds at ~1-2 points, real but not the dominant driver.** `--enable-cfl-intra=0` on
   libaom (photos, same corpus): median **+1.2 to +1.7%** more bytes (mean +1.9-2.8%), consistent
   sign across all four ssim2 targets — a real signal, unlike CDEF/restoration's noise. This is
   an **upper bound**: it's "CfL entirely off" vs libaom's full search, not a like-for-like
   search-depth comparison. zenrav1e already implements CfL (`rdo_cfl_alpha`, `src/rdo.rs:1786`,
   bounded/adaptive early-exit over α=1..16, breaking once `count < alpha`) rather than having
   none, so the recoverable fraction from tightening its search is *some fraction* of 1-2%, not
   the whole thing. See `benchmarks/rd_gap_cfl_ablation_2026-07-01.tsv`.
5. **The ~8-18% photo gap (the honest headline) remains mostly unexplained after five tools
   ruled out or bounded small.** Palette (~0%), CDEF (noise), loop restoration (noise), and
   tx-type completeness (spec-complete) are ruled out; CfL upper-bounds at ~1-2 points. That
   leaves roughly **6-16 percentage points unaccounted for** by any single named AV1 tool.
   Leading hypothesis: an aggregate of many small RDO/heuristic refinements accumulated in
   libaom over years (coefficient cost estimation precision, rate-control qindex mapping,
   mode-search ordering, partition/tx early-exit thresholds tuned finer than zenrav1e's, etc.)
   rather than one missing/incomplete feature — harder to close than a single lever, and harder
   to measure via simple flag ablation since there's no single flag to toggle.

## Credible narrowing levers (priority order, updated 2026-07-01)

1. **Implement palette mode in zenrav1e** (scoped: screen-content win, ~zero photo-gap effect —
   see confirmed findings above). Substantial encoder feature: color quantization (k-means per
   candidate block/plane), RD-gated size search (2-8), bitstream signaling (palette colors + index
   map), `CdfContext` wiring for the already-spec-ported default CDF tables. rav1d-safe (zenavif's
   decoder) already supports palette decode (`ipred.rs`, `recon.rs`, `safe_simd/pal.rs`), so no
   decoder-side blocker for round-trip validation. Cross-repo work (zenrav1e) — needs explicit
   scope sign-off before starting. **Only worth it if screen content is part of the target traffic
   — it will not move the photo number.**
2. **Widen/remove the `rdo_cfl_alpha` early-exit and re-measure.** Bounded upside (<=1-2 points on
   this corpus per the ablation above), but it's the cheapest remaining photo-gap lever — a
   same-repo, in-zenrav1e change (loosen the `count < alpha` break, or make it a speed setting)
   with a fast measure/revert cycle. Do this before investing in perceptual-tune or diffing.
3. **Perceptual-tune parity.** aomenc gains from `--tune=ssimulacra2` (not used in the baseline, to
   stay fair); zenrav1e's psy-tune "already covers VAQ." Verify psy-tune is competitive with
   libaom's default at matched ssim2. (Hypothesis — measure before building.)
4. **Broader photo-gap root-causing** if 2-3 don't move the number: per-block mode-decision diffing
   between aomenc and zenrav1e on shared test images (aomenc has stats/debug dump options), since no
   single-tool ablation so far explains the remaining 6-16 points — the gap may be diffuse
   (many small refinements) rather than localized to one tool.

**Explicitly NOT on the list:** deltaq/VAQ/trellis (rejected), EPT/NSDT (video-only gains, don't
transfer to stills — `AVIF_LEARNINGS §1`), learning-based intra / GPU search (out of scope for
zenrav1e's design), CDEF/restoration tuning (ruled out 2026-07-01, see confirmed findings).

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
