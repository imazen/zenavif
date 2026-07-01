# zenrav1e RD gap vs libaom-at-slow — measurement + narrowing plan

**Status:** partially closed, 2026-07-01. The dominant driver of the photo-gap search-completeness
ceiling — `encode_partition_topdown` never offering `PARTITION_HORZ`/`VERT` — is root-caused, fixed,
measured, and pushed to zenrav1e's `master` (unreleased; see "Fixed 2026-07-01" below). A second,
larger, **still-open** structural gap was found in the same investigation: 6 of AV1's 10 partition
types are never attempted by the RDO search at any speed. This doc records the measured gap, the
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
speed/effort tradeoff.** *(2026-07-01: root cause found — see "Fixed 2026-07-01" below. s1==s2 wasn't
"true convergence," it was a dead-code setting; the actual ceiling was a hardcoded 2-candidate list
that no speed value ever affected.)*

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

## Fixed 2026-07-01: `encode_partition_topdown` never offered PARTITION_HORZ/VERT

**Root cause.** `encode_partition_topdown` (the ONLY partition-search path any current consumer
uses — `encode_bottomup` is unconditionally forced `false` by ravif's `SpeedTweaks`, citing a
QM/RDO-cost-model interaction) called `rdo_partition_decision` with a **hardcoded 2-element
candidate list**: `&[PartitionType::PARTITION_SPLIT, PartitionType::PARTITION_NONE]`
(`zenrav1e/src/encoder.rs:3306`, pre-fix). `PARTITION_HORZ`/`PARTITION_VERT` were never candidates,
**at any speed, on any block, ever** — not gated by `non_square_partition_max_threshold` or
anything else; that setting only existed inside the unused `encode_partition_bottomup` path. This
is why cavif's `-s1` and `-s2` were byte-identical (md5-verified): neither speed's value for that
setting ever reached code that used it. `rdo_partition_decision`'s internal dispatch already
handled `PARTITION_HORZ`/`VERT` (grouped with `PARTITION_SPLIT` via the same generic
`rdo_partition_simple` helper, shared with `encode_partition_bottomup`) — the RD-cost machinery
was fully wired and tested, just never invoked with those two variants from the topdown call site.

**Found via:** per-block AV1 decode-decision diffing against libaom, using aom's own bitstream
inspector (`examples/inspect`, built with `-DCONFIG_INSPECTION=1 -DCONFIG_ACCOUNTING=1` —
decodes ANY valid AV1 bitstream, not just aom's own). At matched byte size, zenrav1e showed
**0% usage of BLOCK_8X16/16X8/8X4/4X8** on a perfectly SB-aligned image (1024×768, so it isn't a
frame-border artifact) where libaom used them for **10–20% of the image area each**. See
`scripts/rd_gap/inspect_diff.sh` + `analyze_inspect_diff.py`.

**Fix:** `zenrav1e@665e58e4` (pushed to `master`) builds the candidate list conditionally —
`PARTITION_HORZ` gated on `has_cols`, `PARTITION_VERT` on `has_rows && chroma_sampling !=
Cs422` — both further gated by `bsize <= non_square_partition_max_threshold`, mirroring
`encode_partition_bottomup`'s existing pattern and finally making that setting meaningful for the
path that's actually used. Companion change `ravif@b4853c68` widens speed-2's threshold to
`BLOCK_64X64` (from `BLOCK_32X32`) — RDO-safe by construction (more candidates can only tie or
improve the result); spot-checked byte-identical on its own before the topdown fix existed, so it's
free, not the source of the measured gain. **Verified**: 131 zenrav1e lib tests + doctests +
`trellis_roundtrip` pass; `cargo fmt`/`clippy` clean; round-tripped through zenavif's own decoder
(rav1d-safe) with no corruption (pixel diff vs the unfixed encode: mean abs 2.4, max 63 — normal
lossy-reencode magnitude, not corruption).

**Not yet consumable:** the fix is on zenrav1e's `master`, unreleased (crates.io still serves
0.1.4). Same situation as the lossless ±2 fix in zenavif's own `CLAUDE.md` Known Bugs — needs a
zenrav1e release + the ravif/zenavif dependency bump before registry builds pick it up.

**Measured impact** (`scripts/rd_gap/rd_gap_{fixed,unfixed}_results.tsv`, 22-image photo corpus,
cavif `-s2`, identical settings before/after — **narrower methodology than the 2026-06-30 baseline
table above**: single cavif config/format, not the best-of-{speeds×formats×bit-depth} frontier, so
absolute numbers aren't directly comparable to the +16.3/11.8/7.8/18.5% headline; the *relative*
before/after delta is the valid signal here):

Direct fixed-vs-unfixed (isolates the fix, no libaom involved):

| ssim2 target | median bpp change | note |
|---:|---:|---|
| 70 | **−2.24%** | |
| 75 | **−1.76%** | |
| 80 | **−2.04%** | |
| 82 | **−1.86%** | |
| 85 | **−1.77%** | |
| 88 | −0.08% | gain fades at very high quality |
| 90 | +0.68% | small reversal, noisy (n=16, high-ssim2 rare in this q-grid) |
| 92 | +4.56% | n=11, treat as noise |

The win concentrates in **ssim2 70–85** — exactly the aggressive-compression range this workspace
weights most heavily (see "Codec quality sweeps MUST cover q5-q60" in the global rules). At very
high quality the marginal benefit of rectangular shapes shrinks (content is already using fine
granularity) and the extra partition-type signaling bit may cost slightly more than it saves.

Against the same-day libaom baseline (single format/config both sides):

| | median BD-rate | mean BD-rate |
|---|---:|---:|
| unfixed | +5.7% | +7.5% |
| fixed | **+3.6%** | **+4.5%** |

A ~35–40% relative reduction in the measured BD-rate gap under this methodology. A full
canonical-methodology re-measurement (best-of-many-configs frontier, matching the 2026-06-30
table exactly) is a good follow-up once the fix ships in a real zenrav1e release.

## STILL OPEN — 6 of AV1's 10 partition types never attempted at any speed

Found in the same inspect-diff investigation, **not fixed by the above**. AV1 defines 10 partition
types: `NONE, HORZ, VERT, SPLIT` (the ones just fixed above) plus `HORZ_A, HORZ_B, VERT_A, VERT_B,
HORZ_4, VERT_4` (the "extended" set). zenrav1e's `PartitionType` enum and CDF/entropy-context
plumbing (`context/partition_unit.rs`) know about all 10, but the RDO search
(`encoder.rs`'s `partition_types` `ArrayVec<PartitionType, 3>` in `encode_partition_bottomup`, and
now the topdown candidate list) only ever constructs `{HORZ, VERT, SPLIT}` — the extended 6 are
never candidates anywhere, at any speed. `recon_intra.rs:155` has an explicit, long-standing TODO:
`// TODO: Enable the case for PARTITION_VERT_A/B once they can be encoded by rav1e.` This is a
bigger, structural gap than the topdown fix (which just unblocked already-implemented HORZ/VERT) —
implementing extended partitions means new recursive encode patterns for 3-way (HORZ_A/B, VERT_A/B:
one half full-size, the other split again) and 4-way 1:4 splits (HORZ_4/VERT_4), plus verifying the
entropy-context wiring that's currently unused.

**Measured area share** (post-topdown-fix inspect data, 3 photos): libaom uses `BLOCK_4X16 +
BLOCK_16X4 + BLOCK_8X32 + BLOCK_32X8 + BLOCK_16X64 + BLOCK_64X16` (the sizes only reachable via
`HORZ_4`/`VERT_4`) for **10–13% of image area**; zenrav1e: **0%**, always. `HORZ_A/B`/`VERT_A/B`
sizing overlaps with plain HORZ/VERT block sizes so isn't separately visible in a block-size
histogram — would need mode-decision-level instrumentation to isolate, not yet done.

**2026-07-01 attempt: implemented HORZ_4/VERT_4, found 2 real bugs (both fixed and landed),
blocked by a 3rd (unresolved), reverted.** Wiring HORZ_4/VERT_4 into the dispatch/geometry/
candidate-list surfaced two independent, genuine pre-existing bugs, both fixed regardless of
this feature's fate:
- A second hardcoded dispatch gap in `rdo_partition_decision` (same shape as the topdown fix).
- **`dc0a1165`** (landed on `master`): `sse_h_edge` passed the wrong axis to `deblock_size`
  (mismatching `filter_h_edge`'s own convention) — harmless for every previously-reachable tx
  shape (masked by a `min(14, ...)` cap), but produces an out-of-bounds filter reach for
  extreme-aspect tx like `TX_16X4`. Verified safe standalone on the full 22-image corpus.

But even after both fixes, **encodes using HORZ_4/VERT_4 produce a bitstream libaom's own
reference decoder (`aomdec`) rejects as corrupt** (`Corrupted segment_ids` / `Failed to decode
tile data`) — rav1d-safe silently decodes it anyway (wrong per spec, no error), which is the
worst case for this project's zero-corruption-tolerance bar. Root cause not found despite ruling
out segmentation (any level, or fully disabled — "segment_ids" in the error is a red herring),
CDEF/restoration (inactive at the tested quality regardless), and the angle-delta search
(disabling it didn't help; cross-checked the `bsize >= BLOCK_8X8` angle-delta gate against
libaom's `av1_use_angle_delta` — identical logic in both codebases, likely spec-conformant).
**Reverted** (not on master) rather than ship a known-corrupt path. Full writeup + what's not
yet tried: [zenrav1e#26 comment](https://github.com/imazen/zenrav1e/issues/26#issuecomment-4854718037).

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
   the whole thing. See `benchmarks/rd_gap_cfl_ablation_2026-07-01.tsv`. **Update: tried widening
   the search (see "Credible narrowing levers" #3) — the recoverable fraction turned out to be
   ~0/slightly negative, not a fraction of 1-2%.** The 1-2% upper bound was real, but zenrav1e's
   search was already capturing most of it; the gap between "CfL entirely off" and "CfL as
   currently searched" was smaller than between "as currently searched" and "exhaustive."
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

1. ~~Fix `encode_partition_topdown`'s hardcoded HORZ/VERT exclusion~~ **DONE** — see "Fixed
   2026-07-01" above. −1.8 to −2.8% median bpp at ssim2 70-85, BD-rate gap +5.7%→+3.6% median
   (narrower methodology; see caveat above). Unreleased.
2. **Implement the 6 extended partition types** (`HORZ_A/B`, `VERT_A/B`, `HORZ_4`/`VERT_4` — see
   "STILL OPEN" above). **BLOCKED, 2026-07-01**: a HORZ_4/VERT_4 prototype hit a bitstream-
   conformance bug (libaom's `aomdec` rejects the output as corrupt) that resisted diagnosis;
   reverted rather than ship it. Two other real bugs found in the attempt ARE fixed and landed
   (see "Fixed 2026-07-01" section above). Still the highest-*value* remaining lever (10-13% area
   share) if the conformance bug gets resolved — see
   [zenrav1e#26](https://github.com/imazen/zenrav1e/issues/26) for the full writeup and what's
   not yet tried (CfL/tx-type eligibility checks, or a full syntax-element trace — filter_intra
   is separately ruled out, see lever below).
   Do not re-attempt without a new diagnostic angle; re-treading the same bisection will waste
   time the comment already covers.
3. ~~Widen/remove the `rdo_cfl_alpha` early-exit~~ **RULED OUT, 2026-07-01.** Tried (removed the
   `count < alpha` break, fully exhaustive +/-16 search); measured noise-level, slightly negative
   in aggregate (BD-rate +3.6%→+3.8%, direct isolation -0.2% to +0.5% with no consistent sign —
   see `benchmarks/rd_gap_cfl_widening_2026-07-01.tsv`). Root cause: `rdo_cfl_alpha` optimizes SSE
   only (not RD) — a wider search sometimes finds a lower-distortion alpha that costs more bits to
   *signal*, net-negative for the outer RD comparison. **Reverted, not on master.** A real fix
   would need to make the search RD-aware (weigh signaling cost, not just SSE) — larger, not
   attempted.
3b. ~~Enable `filter_intra`~~ **RULED OUT, re-confirmed 2026-07-01.** This is why
   `read_filter_intra_mode_info` is 0% for zenrav1e in the inspect-diff bit-cost breakdown:
   `enable_filter_intra` requires `prediction_modes >= ComplexKeyframes`, and ravif forces
   `Simple` — [zenrav1e#5](https://github.com/imazen/zenrav1e/issues/5) (closed) documents a
   severe (12 dB) PSNR regression when that's relaxed. Re-tested today: **still just as broken**
   (ssim2 80→18.7, encode time 0.x s→11.4s on the same repro shape) — the CDF fixes referenced in
   that issue (`d696f4d`/`2d0ae25`/`04129b4`, already on master) did not resolve it. Also newly
   confirmed: **not specific to `ComplexAll`** — `ComplexKeyframes` alone (which the issue's own
   analysis says is equivalent to `ComplexAll` for all-keyframe stills) produces byte-identical,
   equally-broken output, so there's no simple "use the narrower complex tier" workaround. A real
   fix needs `enable_filter_intra` decoupled from `prediction_modes` entirely, then bisecting the
   filter_intra RDO cost path itself — not attempted (time-boxed). See the reconfirmation comment
   on zenrav1e#5.
4. **Implement palette mode in zenrav1e** (scoped: screen-content win, ~zero photo-gap effect —
   see confirmed findings above). Substantial encoder feature: color quantization (k-means per
   candidate block/plane), RD-gated size search (2-8), bitstream signaling (palette colors + index
   map), `CdfContext` wiring for the already-spec-ported default CDF tables. rav1d-safe (zenavif's
   decoder) already supports palette decode (`ipred.rs`, `recon.rs`, `safe_simd/pal.rs`), so no
   decoder-side blocker for round-trip validation. Cross-repo work (zenrav1e) — needs explicit
   scope sign-off before starting. **Only worth it if screen content is part of the target traffic
   — it will not move the photo number.**
5. **Perceptual-tune parity.** aomenc gains from `--tune=ssimulacra2` (not used in the baseline, to
   stay fair); zenrav1e's psy-tune "already covers VAQ." Verify psy-tune is competitive with
   libaom's default at matched ssim2. (Hypothesis — measure before building.)
6. **Broader photo-gap root-causing** if 2-5 don't close it: per-block mode-decision diffing
   between aomenc and zenrav1e on shared test images (now tooled via `inspect_diff.sh` — extend to
   mode-level/RD-level instrumentation, not just block-size histograms), since the remaining
   residual after all of the above may be diffuse (many small refinements) rather than localized
   to one tool.

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

**Tool-ablation sub-harnesses** (`palette_ablation.sh`, `tool_ablation.sh` +
`analyze_{palette,tool}_ablation.py`): isolate how much of the gap a specific libaom flag accounts
for, by running aomenc twice per (image, cq) — default vs the flag disabled — on the same corpus.
`tool_ablation.sh` takes a `VARIANTS="label=flag ..."` env var so any future libaom knob (tune,
deltaq mode, etc.) can be probed the same way. See `benchmarks/rd_gap_{palette,tool,cfl}_ablation_
2026-07-01.tsv` for the palette/CDEF/restoration/CfL results.

**Per-block decode-decision diffing** (`inspect_diff.sh` + `analyze_inspect_diff.py` +
`obu_to_ivf.py`): the tool that found the `encode_partition_topdown` gap. Decodes matched-byte-size
zenrav1e and libaom encodes with aom's own bitstream inspector and diffs per-4x4-cell
partition/mode/txType/skip/palette/cfl plus per-syntax-element bit cost (libaom's Accounting API).
Requires a **separately built** aom tree with introspection enabled (not the `AOMENC`/`AOMDEC` used
above):
```bash
mkdir ~/work/aom/build_inspect && cd ~/work/aom/build_inspect
cmake -B . -S .. -DCMAKE_BUILD_TYPE=Release -DCONFIG_INSPECTION=1 -DCONFIG_ACCOUNTING=1 \
  $(other flags matching build_slow, see CMakeCache.txt)
cmake --build . --target inspect aomdec aomenc -j 8
```
Then:
```bash
cd scripts/rd_gap
CAVIF=~/work/zen/ravif/target/release/cavif AOMENC=~/work/aom/build_slow/aomenc \
INSPECT=~/work/aom/build_inspect/examples/inspect \
EXTRACT_AV1=$(git rev-parse --show-toplevel)/target/release/examples/extract_av1 \
  bash inspect_diff.sh IMG W H Q_ZENRAV1E CQ_LIBAOM /tmp/outdir
python3 analyze_inspect_diff.py /tmp/outdir/zenrav1e.json /tmp/outdir/libaom.json
```
Pick `Q_ZENRAV1E`/`CQ_LIBAOM` to land close in encoded bytes first (re-run once, check the printed
byte ratio). `extract_av1` (zenavif's own example, no `encode` feature needed) pulls the raw AV1
payload out of cavif's AVIF container; libaom's `--obu` flag does the same on its side natively.

## References
- `AVIF_LEARNINGS.md` §1 — full research synthesis (tried/rejected + actionable), gitignored, mirrored
  to `~/work/zen/zenpapers/AVIF_LEARNINGS.md`.
- `~/work/zen/EXPERIMENTS-SURVEY-2026-05-17.md` — the tried/rejected source of record.
- `docs/RAV1E_PICKER_PLAN.md` — the zenrav1e picker (separate: picks per-image settings; does not change
  the encoder's RD ceiling).
- `~/work/zen/zenmetrics/benchmarks/avif_vs_libaom_2026-06-30.md` — the measurement this doc summarizes.
