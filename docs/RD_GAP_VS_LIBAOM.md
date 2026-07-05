# zenrav1e RD gap vs libaom-at-slow — measurement + narrowing plan

**Status: see "CURRENT POSITION (consolidated 2026-07-03)" immediately below** — one
fresh measurement of the composed shipped config: legacy photos median **−12.3% vs
libaom cpu2 / −11.6% vs cpu0-default / tier-2 (cpu0+ss2tune) dead even** at s2, with
butteraugli agreeing and zr-s2 *cheaper* than the tier-2 reference it ties.

## CURRENT POSITION (consolidated 2026-07-03)

**One fresh measurement of the composed shipped configuration, superseding the day's
stacked deltas as the current-position statement** (the per-mechanism sections below
remain the isolation evidence; this section is what the composed config measures TODAY).
~12 mechanisms landed on zenrav1e master over 2026-07-02/03: the partition-search fixes,
the `-s1` deep mode, `Tune::Ssimulacra2` (chroma delta-q + ss2 QM curves + variance-boost
1.0 + QM-dist ratio + LF schedule {7,5,3}@{80,160}), palette mode + AA detection, and the
recon fixes (`b30dd752`/`32477046`/`17cff82f`/`c1fab5b3`) — each was measured in
isolation as it landed; this is the ONE authoritative composed statement.

**Config under test (bit-reproducible):** cavif `-s2` / `-s1` `--depth 8` from
**zenrav1e master@origin `c9c2d5f7`** (code tip `9a05d54a`) via the ravif dev-patch
(ravif main `9d2b97cc` + path dep + `ZENRAVIF_TUNE`/`ZENRAVIF_PALETTE` env passthroughs +
`S1_DEEP_ARMS_LIVE=true`; patch sha256 `d8a40c47…`, full diff in the raw dir, reverted
after) — **Tune::Ssimulacra2 armed + palette=Auto armed**. References: pinned libaom
`632172a4` (aomenc 3.14.1), 420, cq 8–63 (the deltaq-2026-07-02 tier-table convention);
all three legacy refs re-encoded fresh and verified **176/176 cells value-identical** to
the saved deltaq baselines (three independent passes). Scoring: fast-ssim2 + butteraugli
3-norm/max, decoded-vs-source; zr decoded by zenavif's own save_png (rav1d-safe), aom by
aomdec+color.py. Box: zenavif-sweep-1 (ccx63). Full tables:
`benchmarks/rd_gap_final_2026-07-03.tsv`; raws
`/mnt/v/output/zenavif/final-2026-07-03/` (+ pointer, SHA256SUMS, Tower mirror).

**Conformance at the composed config: s2 110/110 + s1 110/110** (aomdec AND rav1d-safe,
legacy 22 × Q{30,50,60,75,90}); 0 FAILED_CELLS across all 12 measurement runs.

### The ladder (legacy photos n=19, ssim2 BD-rate; negative = zenrav1e needs fewer bits)

| ref | s2 med / mean / wins | s2 ba3n / bamax med | s2 time | s1 med / mean / wins | s1 ba3n / bamax med | s1 time |
|---|---|---|---|---|---|---|
| cpu2 (matched-speed "slow") | **−12.29 / −11.94 / 17/19** | −6.77 / −4.84 | 3.04× | **−12.06 / −12.16 / 17/19** | −7.76 / −3.45 | 8.01× |
| cpu0 default (slowest-best) | **−11.58 / −10.03 / 16/19** | −4.16 / −4.39 | 1.55× | **−11.31 / −10.27 / 17/19** | −3.38 / −1.89 | 4.08× |
| cpu0 --tune=ssimulacra2 (tier 2) | **+0.05 / −1.17 / 9/19** | −2.50 / −4.91 | 1.01× | **+0.37 / −1.38 / 9/19** | −1.60 / −5.07 | 2.67× |

Both butteraugli norms are **negative against every reference at both speeds** — no
metric gaming; where the tier-2 ssim2 median rides ~0, butteraugli says zenrav1e wins
outright. The tier-2 knife-edge (±0.4 around 0 across recent measurements) is o_9051
sitting at the median. Legacy all-22 (plots included): s2 −11.84 / −10.47 / +0.44,
s1 −11.93 / −10.56 / +0.75, **zero reach failures** vs every ref (the historic
o_7001-class "aom floor unreachable" failure is eliminated — palette + the 12-pt grid
give frontier overlap everywhere on the legacy corpus).

### train26 (fitting corpus, 24 TRAIN origins — FIRST aom-referenced statement)

| ref | scope | s2 med / mean / wins | s1 med / mean / wins |
|---|---|---|---|
| cpu2 | all (n=22†) | **−13.33 / −11.30 / 18/22** | **−13.66 / −12.51 / 19/22** |
| cpu2 | photos (n=20) | −13.33 / −13.54 / 17/20 | −13.66 / −13.65 / 18/20 |
| cpu0-default | all (n=23†) | −10.91 / −5.80 / 15/23 | −11.15 / −7.43 / 16/23 |
| cpu0-ss2tune (tier 2) | all (n=21†) | **−0.34 / −3.24 / 11/21** | **−0.11 / −3.27 / 11/21** |
| cpu0-ss2tune | photos (n=20) | −0.15 / −1.66 / 10/20 | −0.02 / −1.70 / 10/20 |

**The tier-2 median crosses on the fitting corpus at both speeds** (means clearly
negative). † n<24: aom's 420 frontier collapses to <4 unique points on saturated screen
content — NOT silently dropped: `7028_plots` = aom saturates at ssim2 64.0@0.095 bpp
while zr's *floor* (64.9@0.075 bpp) beats aom's best on both axes → zr dominates aom's
entire achievable range (inverse reach-failure, categorical win); `7052_plots` = aom's
near-lossless top 89.4@0.014 bpp vs zr 1.35× bytes (s2) / 1.27× (s1) at matched quality
— the real remaining saturated-screen-tail loss. Per-family s2 medians vs cpu2:
interiors −23.1, screenshots −19.3 (4/4), gen-illustrations −19.4, nps −12.1, people
−11.4, nature −11.2, food −9.0, products −3.9, scans/patents +0.8 (1-bit rescans),
plots split (7058 −37.8 / 7050 +59.9 — 7050 is the worst remaining cell).

### Historic holdouts (TEST/VAL-split origins — illustration only)

| img | s2 cpu2 / cpu0def / tier2 | s1 cpu2 / cpu0def / tier2 | verdict |
|---|---|---|---|
| o_5004 | −23.3 / −21.1 / +0.05 | −23.0 / −20.7 / +0.90 | **RESCUED** (was +3.3/+5.9/+29.9 pre-qmdist) |
| o_6629 | −0.3 / +0.7 / +4.2 | −3.7 / −2.7 / +0.4 | moved to ~even; s1 now WINS vs cpu2+cpu0def (was +31.6/+32.7/+24.9) |
| o_9051 | +11.8 / +11.7 / +0.1 | +12.9 / +12.3 / +1.6 | the remaining material loser vs cpu2/cpu0def; ~even at tier-2 |

### Encode time (fresh cells, RD_CACHE=off refs, sequential solo runs, Σ enc_ms)

Legacy all-22: zr-s2 = **2.65× cpu2, 1.15× cpu0-default, 0.86× cpu0-ss2tune** (the full
tune at s2 is *cheaper* than the tier-2 reference it ties); zr-s1 = 6.91× / 3.00× /
2.24×. train26: s2 = 3.50× / 1.38× / 0.98×; s1 = 9.18× / 3.62× / 2.56×. Photos-only
ladder ratios in the table above (plots shift the mix: palette makes zr fast on plots
while aom cpu0 crawls on them).

### What this supersedes / keeps

This section is the current position; the dated sections below (deltaq, qmdist, lfsharp,
s1 deep mode, Tune::Ssimulacra2, palette, partition fixes) remain the per-mechanism
isolation evidence and are NOT invalidated. Everything above is release-gated the same
way: registry builds ship pre-fix behavior until zenrav1e releases past 0.1.4 and the
zenravif → zenavif dep chain bumps.

**Historical status (2026-07-02, superseded by the section above): TRUE RD PARITY
REACHED** — median BD-rate vs libaom-slow improved
**+5.7% → −0.65%** (mean +7.5%→+0.24%), crossing the ≤0% parity target at matched speed
(1.057× median encode time). The final lever was the trial-SPLIT-cost accuracy fix ("Fixed
2026-07-02" below), on top of the four 2026-07-01 bug fixes. **Same day, the `-s1` deep
mode shipped (release-gated): median −0.97% vs libaom cpu-used=0 (its slowest-best) and
−3.01% vs cpu2, winning 11/19 photos per-image — see "s1 deep mode" below.** Re-run
`scripts/rd_gap/` after any future zenrav1e change to check for regressions.

**2026-07-04 — the SPEED-LADDER GAP MAP extends the program over the SPEED axis**
(zr s2-s10 × tune/off vs libaom `--allintra` cpu2-9 × default/`--tune=iq` — the first
fast-tier measurement; everything above is s1/s2 vs GOOD-mode cpu0/cpu2): **the aom
allintra ladder pareto-dominates every zr arm at matched wall-time on photos** (no
crossover; gap +2..+8% at the slow end widening to +33..+49% at s10; the tune is
mandatory and nearly free at fast tiers; GOOD-mode references are themselves off the
aom pareto). 5,520 fast-tier cells PALCONF-clean (no new conformance bugs). Ranked
fast-tier wedge list + mechanism-liveness audit + the s2-off/s2-tune time inversion:
`docs/SPEED_LADDER.md` + `benchmarks/rd_gap_speed_ladder_2026-07-04.tsv`.

**2026-07-03 — the WEDGE MAP extends the program over size (256/512/1024/2048) and
c50 quadrant crops** (everything above was measured at 1024-full only): the 1024 win
reproduces (continuity PASS) but decays monotonically to **parity at 256px**, family
9226 (AI product shots, ~32% of imazen-26) is a systemic loss at every size, aom's
near-lossless screen floor is unreachable on grid plots (3.4–3.9× bpp "reach
failures" invisible to BD), and palette-Auto detection stops firing on ANY downscaled
screen content. Ranked wedge list + feature correlations + the labeled per-cell
dataset: `docs/WEDGE_MAP_2026-07-03.md`.

Five real defects found and fixed on zenrav1e's `master` (unreleased): `encode_partition_topdown`
never offering `PARTITION_HORZ`/`VERT`, `sse_h_edge`'s wrong-axis `deblock_size` call,
`rdo_tx_type_decision`'s overly-aggressive first-iteration early-exit, a `BlockSize`
ordinal-vs-dimension mismatch in the `angle_delta`/palette gates that was blocking
`PARTITION_HORZ_4`/`VERT_4`, and the topdown trial's systematically pessimistic SPLIT cost
estimate — see "Fixed 2026-07-01", "Fixed 2026-07-01 (4)", and "Fixed 2026-07-02" below. The
`HORZ_4`/`VERT_4` fix closed the single largest structural gap: 2 of AV1's 6 "extended"
partition types (of 10 total) are attempted by the RDO search. The other 4 (`HORZ_A/B`,
`VERT_A/B`, Phase 2) were **implemented, verified conformance-clean, measured as a net RD
regression under the then-biased SPLIT estimate (+0.1%→+0.6% median, ~1.46x encode time), and
reverted** — see "TRIED AND REVERTED 2026-07-01" below; with the SPLIT estimate now fixed, a
re-attempt from the preserved implementation (`a7630aee`) is unblocked. CfL search-widening,
filter_intra, tx-depth widening, and widening ravif's `partition_range` speed heuristic to unlock
`BLOCK_32X32`/`64X64` were all tried and ruled out; perceptual-tune parity was verified as
already-real and already-active (no further headroom there); the `rdo_tx_decision` high-quality
gate was found to be a real, large win that was deliberately not adopted (breaks the matched-
speed comparison basis) — see "Credible narrowing levers". This doc records the measured gap,
the levers tried (fixed, rejected, verified, blocked, or found-but-declined), and the repeatable
harness (`scripts/rd_gap/`) for tracking progress.

## Corpus + split hygiene (added 2026-07-02, per user directive)

**The fitting corpus is now `scripts/rd_gap/sample_images_train26.tsv`** — 24 origins picked
by k-means K=24 (centroid-nearest per cluster) over 94 zenanalyze features on imazen-26,
**TRAIN origins only** under the canonical LSD split (`zenmetrics/scripts/picker/
origin_split.py::split_of`, locked in `zensim/docs/DATA_SPLITS.md` §2a: last digit
{0,2,4,6,8}=train, {1,3,5}=val, {7,9}=test). Renditions: 1024-long-edge linear-light Lanczos
downscale-only at `/mnt/v/output/rd-gap-train26-2026-07-02/` (+`_MANIFEST.json` provenance;
selector adapted from `zenmetrics/scripts/sweep/knobablation_firstcut_select.py`). 12 content
classes — this reflects imazen's real web workload (screens, AI products, scans, plots — not
photo-pure), so report **per-family slices** alongside the medians; photo-parity claims
remain per-family. Usage: `SAMPLE=$HERE/sample_images_train26.tsv ./run_gap.sh` (or via
run_remote.sh `SAMPLE=` passthrough); aom baselines for it need one `aom_only.sh` run per
config.

**Why:** the legacy 22-image corpus (`sample_images.tsv`, clean-picker-corpus-2026-06-26)
MIXES splits — o_1029/o_6629/o_8107/o_9067/o_9077 are TEST origins and
o_1015/o_1023/o_3003/o_9051/o_7001 are VAL under the LSD rule. Every constant landed up to
2026-07-02 (tune mechanisms, s1 config, partition decisions) was A/B'd on that mixed corpus;
the *mechanisms* are sound (butteraugli-gated, mostly aom ports) but per-image claims about
val/test origins (e.g. the o_6629 holdout) must not feed later model-evaluation claims on
those buckets. **New encoder-constant fits happen on train26; the legacy corpus stays for
continuity comparisons against the 2026-07-02 baselines** until aom baselines are rebased
onto train26.

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
| Widen `rdo_tx_depth` past 2 (chasing `read_selected_tx_size` 0.3% vs libaom 1.0%) | `MAX_TX_DEPTH=2` is a normative AV1 spec limit (`tx_depth` syntax element), not a tunable heuristic — caught immediately by the existing `encode_8bit_in_u16_does_not_trip_cdef_range_assert` test (2026-07-01) | rejected |

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
| fixed (topdown + deblock only) | +3.6% | +4.5% |
| **current master** (+ tx-type fix below) | **+2.1%** | **+2.9%** |

A ~63% relative reduction in the measured BD-rate gap under this methodology, from all fixes
combined. A full canonical-methodology re-measurement (best-of-many-configs frontier, matching
the 2026-06-30 table exactly) is a good follow-up once the fixes ship in a real zenrav1e release.

## Fixed 2026-07-01 (3): `rdo_tx_type_decision`'s overly-aggressive first-iteration early-exit

**Root cause.** The tx-type RDO search (`rdo_tx_type_decision`) tried `RAV1E_TX_TYPES` in order
(`DCT_DCT` always first, since it's index 0) and **abandoned the entire tx-type search for a
given tx size** if `DCT_DCT` alone already cost more than `cur_best_rd` — the best RD found
across *other tx sizes* so far. This assumes DCT_DCT's cost is representative of what any tx
type could achieve at this size; it isn't always — content where a non-DCT type (ADST/IDTX/
V_DCT/H_DCT/etc.) wins by a wide margin at that specific size never gets evaluated, since the
search bails before trying it.

**Found via:** the inspect-diff per-syntax-element bit-cost breakdown (built for the topdown-fix
investigation, `scripts/rd_gap/analyze_inspect_diff.py`) showed `av1_read_tx_type` at 2.5% of
total bits for zenrav1e vs 3.6% for libaom — libaom spending *more* bits choosing non-DCT types
suggested zenrav1e's search wasn't finding them as often as it plausibly could.

**Fix:** `zenrav1e@6b3b0493` (pushed to `master`) removes the early-exit — every candidate tx
type in the set is now evaluated regardless of how the first one scored. Also drops the
`cur_best_rd` parameter, left fully unused once the early-exit using it was removed.
**Verified**: 131 lib tests + doctests + `trellis_roundtrip` pass; fmt/clippy clean; 22-image
encode+`aomdec`-decode correctness sweep clean (no regressions).

**Measured impact** (`benchmarks/rd_gap_txtype_fix_2026-07-01.tsv`, same methodology/caveats as
the topdown fix): direct isolation shows median bpp **−0.3% to −1.2%** at ssim2 70-82 (same
quality-range profile as the topdown fix, fading to exactly 0 at 85+). BD-rate gap vs libaom
improved **+3.6% → +2.1%** median (+4.5% → +2.9% mean) on top of the topdown+deblock fixes.

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

**Update 2026-07-01: `HORZ_4`/`VERT_4` (2 of the 6) are now fixed** — the blocker was a real
conformance bug, not a missing feature. See "Fixed 2026-07-01 (4)" below.
**Update 2026-07-01 (later): `HORZ_A/B`/`VERT_A/B` (the other 4) were implemented and verified
conformance-clean, but measured as a net RD regression and reverted** — see "TRIED AND REVERTED
2026-07-01: extended partition types Phase 2" below. All 6 extended types have now been
genuinely attempted; this line item is closed either way.

## Fixed 2026-07-01 (4): extended partition types (`HORZ_4`/`VERT_4`) — the zenrav1e#26 conformance bug

**Root cause.** `BlockSize` has a **custom `PartialOrd`** (`src/partition.rs:165-179`) based
on `width()`/`height()`, not the enum's declaration order. libaom's
`av1_use_angle_delta(bsize) { return bsize >= BLOCK_8X8; }` (`av1/common/reconintra.h:59`)
is a genuinely *ordinal* C-enum comparison — `BLOCK_4X16`/`BLOCK_16X4` have discriminants
16/17 (appended after all 16 classic sizes), so libaom's check is `true` for them
regardless of their 4:16/16:4 aspect ratio. zenrav1e's textually-identical
`bsize >= BlockSize::BLOCK_8X8` is NOT ordinal: for `BLOCK_4X16` (4,16) vs `BLOCK_8X8`
(8,8), width 4<8 but height 16>8 -- an *incomparable* pair under the width/height
`PartialOrd`, so `>=` silently evaluates `false`. The encoder skipped writing
`angle_delta` for directional-mode `BLOCK_4X16`/`BLOCK_16X4` blocks -- exactly the two
most common `HORZ_4`/`VERT_4` sub-block sizes -- while any spec-conformant decoder
expects to read one. Missing symbol -> bitstream desync -> the "Corrupted segment_ids" /
"Failed to decode tile data" `aomdec` errors from the previous attempt (zenrav1e#26). The
same divergence hits `av1_allow_palette`'s ordinal `sb_type >= BLOCK_8X8`
(`av1/common/blockd.h:1503-1509`), config-gated behind `allow_screen_content_tools` so a
secondary, not the repro trigger, but fixed for the same reason.

**Ruled out as the same bug class** (verified byte-identical or dimension-correct against
libaom source, not assumed): `cfl_allowed()`, `av1_filter_intra_allowed_bsize`,
`partition_gather_horz_alike`/`vert_alike` (identical except an inconsequential
`bsize != BLOCK_128X128` guard -- 128x128 superblocks are unsupported in this codebase),
`max_txsize_rect_lookup`, `partition_context_lookup`, `size_group_lookup`,
`num_pels_log2_lookup`, `has_tr_tables`, `has_bl_tables` (all byte-identical to libaom's
tables for all 22 block sizes). Also corrected in passing: the "bottom-up is forced off"
framing in `CLAUDE.md`'s Known Bugs was imprecise -- `encode_partition_bottomup` is
additionally forced (regardless of the `encode_bottomup` speed setting) for any
superblock straddling the frame's right/bottom edge; harmless here since Phase 1 only
wires `HORZ_4`/`VERT_4` into the topdown candidate list, never bottomup's.

**Verified causally, not just correlated:** reverting only the new `ge_8x8_ordinal()`
helper (keeping the `HORZ_4`/`VERT_4` wiring) reproduces the exact `aomdec` error
verbatim on the same repro; restoring it makes every cell decode cleanly again.

**Fix:** `zenrav1e@2866397e` adds `BlockSize::ge_8x8_ordinal()` (an explicit
`!matches!(self, BLOCK_4X4 | BLOCK_4X8 | BLOCK_8X4)`) and swaps it in at the four call
sites needing libaom's ordinal semantics: the luma/chroma `write_angle_delta` gates, the
RDO angle-delta-search gate, and the palette gate.

**Re-implemented Phase 1** (`zenrav1e@7d254289`) per the design already documented in
zenrav1e#26: `PARTITION_HORZ_4`/`VERT_4` in `encode_partition_topdown`'s candidate list
(gated on `non_square_partition_max_threshold`, an exact `{16X16,32X32,64X64}` size
match, and **strict full containment** of the parent block -- not just the half-point
`has_cols`/`has_rows` check `HORZ`/`VERT` use -- sidestepping the spec's
conditional-4th-sub-block case for this first pass), `rdo_partition_decision` +
`get_sub_partitions` dispatch, and `rdo_partition_simple`'s quarter-sliver offset
geometry (a different formula from the 2x2 quadrant grid `HORZ`/`VERT`/`SPLIT` share,
since `subsize` for these two types already IS the sliver, not a half-sized child).

**Verified** (22 images x 5 quality levels = 110 cells): `aomdec` clean on 100% of cells
(0 corrupt, was 100% corrupt before the fix on the same repro). Extended block-size
(`BLOCK_4X16`/`16X4`/`8X32`/`32X8`/`16X64`/`64X16`) area share 1.8-56% per cell across
the corpus (vs 0% before) -- confirms broad, not one-lucky-case, usage. 32x32-parent
slivers (`BLOCK_8X32`/`32X8`) additionally appear at low quality once `partition_range`
(ravif's pre-existing speed-2 heuristic, unrelated to this fix) permits 32x32 blocks to
reach the candidate list at all; `BLOCK_64X16`/`16X64` not reached at speed 2, consistent
with (not a new instance of) the "RULED OUT... `BLOCK_32X32`/`64X64` at 0% usage" finding
below -- not reachable at default speed-2 settings regardless of partition type.
rav1d-safe round-trip pixel diff scales monotonically with quality (0.16-9.3 mean abs,
normal lossy-reencode behavior, no corruption signature). 131 lib tests + doctests pass;
fmt/clippy clean.

**Measured impact** (`benchmarks/rd_gap_extended_partitions_2026-07-01.tsv`, same
methodology as the topdown/tx-type fixes -- direct before/after isolation, libaom
baseline reused from the tx-type fix's same-corpus/same-settings measurement since
libaom itself is unaffected by this change):

Direct after-vs-before (isolates this change; no libaom involved), bpp change at matched
ssim2, `-` = after needs FEWER bits (better):

| ssim2 target | median bpp change | mean | photos median |
|---:|---:|---:|---:|
| 70 | **−0.44%** | −1.10% | −0.47% |
| 75 | **−0.72%** | −0.83% | −0.93% |
| 80 | **−0.61%** | −0.09% | −0.63% |
| 82 | **−1.51%** | −0.42% | −1.59% |
| 85 | **−0.87%** | +0.16% | −0.91% |
| 88 | **−1.09%** | −3.00% | −1.88% |
| 90 | **−0.47%** | +0.65% | −0.59% |
| 92 | **−0.17%** | +2.71% | −0.57% |

Every median AND every photos-median is negative across the full 70-92 range -- a real,
consistently-signed win, smaller in magnitude than the topdown fix (−1.8% to −2.8%) but
real. A few individual targets show a positive *mean* despite a negative median (85/90/92,
small n at the high-quality tail) — the same noise shape the topdown/tx-type fixes' own
high-quality tail showed.

Against the same-day libaom baseline (single format/config both sides):

| | median BD-rate | mean BD-rate |
|---|---:|---:|
| before (current master, pre-this-session) | +2.1% | +2.9% |
| **after** (this fix, current master) | **+0.1%** | **+2.2%** |

**Median BD-rate gap vs libaom-slow closes from +2.1% to +0.1%** — a ~95% relative
reduction from this session's starting point, ~98% relative reduction from the +5.7%
baseline at the start of the whole 2026-07-01 investigation. Effectively at parity on
the median. Mean improves less (+2.9%→+2.2%), consistent with the mean-vs-median
divergence already visible in the direct-isolation table above.

This closes zenrav1e#26.

## TRIED AND REVERTED 2026-07-01: extended partition types Phase 2 (`HORZ_A/B`, `VERT_A/B`)

The remaining 4 of AV1's 6 extended partition types (mixed-granularity 3-way splits: one half
kept whole, the other split into two quarters) were **fully implemented, verified
conformance-clean at the same bar as Phase 1, measured as a net RD regression, and reverted**
(zenrav1e#27). Full numbers: `benchmarks/rd_gap_extended_partitions_phase2_2026-07-01.tsv`.

**Two real bugs were found and fixed during the attempt** (both preserved in the experiment
commit, neither applicable to master since the feature they gate is reverted with them):
1. **Bitstream desync** (the same `aomdec` "Corrupted segment_ids" symptom as the Phase 1
   blocker, different root cause): every child of `HORZ_A/B`/`VERT_A/B` is an unconditional
   LEAF per spec — libaom's `decode_partition` decodes all 3 sub-blocks directly, none get a
   fresh `read_partition`. The square "split again" quarters — unlike SPLIT's children, which
   DO carry their own partition decision — were re-entering the partition search and writing an
   extra, illegal partition symbol. Fixed via `is_forced_leaf_child` threading in
   `encode_partition_topdown`.
2. **Stale side-state panic**: `encode_tx_block` read the block's parent-partition tag from
   `cw.bc.blocks[bo].partition`, a deliberately rollback-unprotected side array — RDO trial
   paths calling `write_tx_blocks`/`write_tx_tree` directly saw whatever a previous,
   rolled-back trial at *different geometry* left there (repro: impossible `(VERT_B,
   BLOCK_16X8)` pair indexing an empty `has_tr_vert_tables` slot → index-out-of-bounds panic).
   Fixed by threading `partition` as an explicit parameter (libaom's `mbmi->partition`
   semantics).

**Verification (PASSED — the regression is an RD-search outcome, not a correctness bug):**
110/110 cells (22 images × Q 30/50/60/75/90) `aomdec`-clean; rav1d-safe roundtrip pixel diffs
scaling normally with quality; all 4 types genuinely chosen by the search (1,282–1,399
final-encode instances each across sampled cells, parents ~93% `16X16` / ~7% `32X32`); 131 lib
tests + clippy + fmt clean.

**Measured (precise, 22-image corpus, 12-point Q grid, same methodology as every other fix):**
- Direct isolation (after vs before, no libaom): integrated BD-rate **median +0.60%, mean
  +0.56%, worse on 14/22 images**. Small wins only at ssim2 70/75 (−0.39%/−0.31% median);
  everything from 80 up regresses, growing with quality (+1.63% at 92).
- Vs libaom-slow: median BD-rate **+0.1% → +0.6%** (mean +2.2% → +2.5%), worse on 12/19
  images — broad, not outlier-driven.
- Encode time: **~1.46× median** (paired per-cell, n=264).

**Mechanism (consistent with the quality-dependence):** zenrav1e's one-level topdown trial
evaluates SPLIT *pessimistically* as 4 NONE-leaves (the final encode then recurses deeper and
does better), while `HORZ_A/B`/`VERT_A/B` trials are evaluated *exactly* (their children ARE
unconditional leaves — no deeper recursion exists for them per spec). The mixed types therefore
win trials against an underestimated SPLIT and lock out profitable deeper splits — worst where
deep splitting pays (high quality). libaom's `rd_pick_partition` evaluates SPLIT recursively,
so it doesn't have this bias. A fair re-attempt needs recursive SPLIT evaluation in the trial
(what `encode_partition_bottomup` does; large speed cost) or a corrected SPLIT cost estimate —
a separate, larger project in the same family as the CfL-widening and `partition_range`-widening
rulings: **RD-cost-model accuracy, not search completeness.**

**Reverted, not on master.** The complete implementation (both bug fixes + feature + tests
passing) is preserved as zenrav1e workspace commit `a7630aee` (anonymous, not on any branch)
for a future attempt. See zenrav1e#27 for the tracking issue.

**UPDATE 2026-07-02 (Phase 2 v2): re-measured on the fixed SPLIT estimate — the regression
FLIPPED TO A WIN, confirming the root cause.** `a7630aee` semantically re-integrated onto
`b073182c` (deeper estimate guarded to genuine SPLIT children; A/B quarters keep symbol-free
trials; rollback discipline re-derived): direct isolation **−0.5759% median / −0.4917% mean,
better on 17/22** (v1: +0.60%, worse on 14/22); vs libaom-cpu2 **−0.65% → −1.87% median with
the mean also negative** (+0.24% → −0.39%); vs cpu0-default +1.47% → +0.92%; vs
cpu0-ssim2tune +15.67% → +11.21%. 110/110 aomdec-clean + roundtrip, tests/clippy/fmt green.
**Encode time 1.461× median (n=32 dedicated paired cells)** — above the 1.2× matched-speed
gate, so NOT the s2 default: preserved as zenrav1e workspace commit `dfed8eda`, the prime
ingredient for the `-s1` deep mode. Full numbers:
`benchmarks/rd_gap_phase2v2_2026-07-02.tsv`.

## Fixed 2026-07-02: pessimistic SPLIT cost estimate in the topdown partition trial — PARITY REACHED

**The fix that crossed the ≤0% parity line** — and the direct product of the Phase 2 postmortem
above: its regression's root cause turned out to be the last systematic defect in the search.

**Root cause.** The topdown partition trial (`rdo_partition_simple`, `src/rdo.rs`) scored each
SPLIT child as a single NONE-leaf via `rdo_mode_decision`, while the final encode re-searches
every SPLIT child recursively (seeded with that NONE leaf as the incumbent) and usually does
better. So SPLIT's trial cost was **systematically pessimistic** relative to the
exactly-evaluated NONE/HORZ/VERT/HORZ_4/VERT_4 candidates — mis-ranking partitions everywhere,
not just for Phase 2's types. libaom's `rd_pick_partition` evaluates SPLIT recursively and has
no such bias.

**Fix** (`zenrav1e@b073182c`, pushed to `master`): refine each SPLIT child's trial cost to
`min(NONE-leaf cost, tell-metered child-SPLIT symbol + 4 quarter NONE-leaf costs)` — exactly the
first comparison the child's own future search will make. The winning deeper state is kept for
sibling estimation; losing deeper state is fully rolled back (ContextWriter + both writers).
`child_modes` still carry the NONE incumbents, so the final-encode machinery is unchanged — only
the parent-level ranking sharpens. The deeper estimate only fires for SPLIT-candidate children
that can split further, with an early break once the running deeper cost exceeds the NONE leaf
it must beat — which is why the speed cost stays negligible.

**Verified:** 110/110 cells (22 images × Q 30/50/60/75/90) `aomdec`-clean, 110/110 rav1d-safe
roundtrip OK, pixel diffs scaling normally with quality (median mean-abs 0.97@Q90 → 4.77@Q30);
131 lib tests + clippy `-D warnings` + fmt clean.

**Measured** (`benchmarks/rd_gap_splitcost_2026-07-02.tsv`, same 22-image/12-Q methodology as
every other fix):
- **BD-rate vs libaom-slow: median +0.0695% → −0.6487% — crosses parity.** Mean +2.1734% →
  +0.2373%. Improved on 16/19 images (worst single regression +1.63pp) — broad, not
  outlier-driven.
- Direct isolation: median bpp **−0.55% to −4.93%** at matched ssim2 across all 8 targets
  (70–92), gain **growing with quality** — the exact inverse of the Phase 2 regression profile,
  confirming the bias mechanism.
- Encode time: **1.057× median** (1.068× mean, n=264 paired cells) — essentially matched speed.

This also unblocks a Phase 2 re-attempt: `HORZ_A/B`/`VERT_A/B` (preserved at `a7630aee`)
regressed specifically because they competed against the underestimated SPLIT.

## s1 deep mode — SHIPPED 2026-07-02 (release-gated): beats libaom cpu-used=0 on the median, 11/19 photos per-image

**cavif `-s1` is now a real maximum-RD mode** (it had been byte-identical to `-s2` — a
dead-code speed arm — until the 2026-07-01 topdown fix). Goal (user directive): beat
libaom's slowest-best operating point (`aomenc --cpu-used=0`, default tune) **per-image**
across the quality range on the photo harness, with speed explicitly subordinate to RD
("s1 can be slower — push for best RD").

### The s1 bundle (ravif `SpeedTweaks::from_my_preset`, speed == 1)

- `mixed_3way_partitions: true` — `PARTITION_HORZ_A/B`/`VERT_A/B` in the topdown search
  (zenrav1e `efbe0cf2`, new default-off knob; the Phase 2 v2 integration from #27).
- `rdo_tx_decision: true` at EVERY quality — drops the `!high_quality` gate (the measured
  −5.7%-bytes-AND-better-ssim2 lever at -Q80-95 that was declined for s2's matched-speed
  basis, §6b above).
- `partition_range: (4, 32.min(max_block_size))` at every quality — the winner of a
  16/32/64 ablation (below).
- `split_trial_depth: 1` — depth 2 (zenrav1e `2fac1af6`, new default-off knob: recursive
  SPLIT-trial refinement) was measured and does NOT ship; see the ablation.
- Pre-existing (previously dead) s1-vs-s2 arm differences also became live: `lru_on_skip:
  true`, `min_tile_size: 2048`.

### partition_range × trial-depth ablation

Decision rule pre-registered before any data: most per-image wins vs cpu0-default →
tiebreak median → tiebreak median vs cpu2. 22-image corpus × 12-Q grid, zenavif-sweep-1
(Hetzner ccx63), run-ids `20260702T{125426,130629,131031,135710,135725}Z`; full per-image
tables in `benchmarks/rd_gap_s1_2026-07-02.tsv`.

| arm | vs cpu0-default med / mean | wins/19 | vs cpu2 med | vs s2 direct med |
|---|---|---|---|---|
| s2 (reference) | +1.471 / +2.235 | 9/19 | −0.649 | 0 |
| s1 (4,16) | −0.263 / +1.539 | 10/19 | −1.785 | −1.111 |
| **s1 (4,32) — ships** | **−0.968 / +0.390** | **11/19** | **−3.013** | **−2.059** |
| s1 (4,64) | −0.056 / +0.314 | 10/19 | −2.206 | −1.236 |
| s1 (4,32)+depth2 | −0.674 / −0.065 | 10/19 | −2.726 | −2.049 |
| s1 (4,64)+depth2 | −0.118 / −0.082 | 10/19 | −2.258 | −2.097 |

Mechanism: the (4,16) arm's big losers are SMOOTH images (o_6629/o_6632/o_9051: mean|grad|
≈3 vs ≈11 for typical winners) — large-block starvation. (4,32) rescues them (o_6632
+14.0→+2.2, o_9051 +9.5→+3.1) at small cost on textured content; (4,64) helps only o_6629
further (+25→+13) while bleeding elsewhere — the large-block NONE cost estimate is still
the limiter (same family as the s2 prange-widen ruling). `split_trial_depth=2` sharpens
exactly that ranking and rescues the worst outliers (o_5004 +12.3→+3.6 at 64; o_3008
+9.9→+6.9 at 32; mean flips negative), but costs the median and one marginal win — under
the pre-registered rule depth 1 ships. Depth 2's content-dependent flip (helps textured,
hurts smooth: the NONE-leaf-only deeper estimate over-credits SPLIT where big flat blocks
are right) is the follow-up lever if the per-image picker ever selects s1 knobs.

### Verdict vs "beats libaom's slowest-best on everything always"

**Median: yes — s1 needs 0.97% fewer bits than aomenc cpu-used=0 at matched ssim2 (s2:
+1.47% more), and beats cpu-used=2 by −3.01% median.** Banded medians vs cpu0-default are
negative at every ssim2 target 70–90. **Per-image: 11/19 — NOT everything.** The 8
still-losing photos, worst first (BD-rate vs cpu0-default at the shipped config): o_6629
+25.3, o_5004 +11.1, o_3008 +9.9, o_3003 +6.7, o_9051 +3.1, o_6632 +2.2, o_2202 +1.1,
o_9077 +0.6. No tested config wins them all simultaneously — even an oracle picking the
best arm per image would still lose 7 (o_3003/o_3008/o_5004/o_6629/o_6632/o_9051/o_9077
are positive under every arm). These are cpu0-only gaps (all but o_6629/o_6632 BEAT
cpu-used=2 at s1): libaom's slowest mode pulls ahead through search depth zenrav1e does
not yet have — partition-type/depth is now exhausted as a lever (all 10 types + deeper
trials measured); the residual is coefficient-level RD (trellis-class optimization,
cost-model precision at large blocks) and remains open. Honest summary: **s1 beats
libaom-slowest-best broadly and on the median, decisively beats it at matched speed, but
"everything always" is not yet reached — 8 named photos still lose, tracked with measured
per-image data.**

### Conformance + speed

- 110/110 cells (22 img × Q30/50/60/75/90) `aomdec`-clean + rav1d-safe roundtrip at the
  shipped config (`s1_conformance_p32d1.tsv`), and independently at (4,16)+d1,
  (4,32)+d2, (4,64)+d2 (440 verified cells total this session); ssim2 monotone with
  quality throughout.
- Speed: box-native per-cell median, s1 ≈ 3.7× aomenc cpu-used=0 wall (n=4 images × 3 cq;
  upper bound — the s1 side ran under 22-wide sweep contention, cpu0 under 4-wide). The
  box runs cpu0 at 1.38× the local workstation's wall (median, n=12 matched cells).
  Cross-TSV enc_ms comparisons under different contention are invalid (a known-1.06×
  paired case reads as 3.2× cross-TSV), so only the box-native sample is load-bearing.
  s1 is deliberately slower than cpu0 — RD-first per the user directive.

### Release gating

zenrav1e knobs are landed (master `efbe0cf2` + `2fac1af6`, both default-off,
byte-identical off — 9/9-cell md5 each) but unreleased; registry serves 0.1.4. ravif's s1
arms landed on `main` behind `SpeedTweaks::S1_DEEP_ARMS_LIVE = false` with the two knob
apply-lines commented — `from_my_preset` output verified byte-identical to b4853c68 at
both speeds on registry deps (6/6 cells md5). At the zenrav1e ≥0.2 dep bump: flip the
const, uncomment the two apply lines, drop two `allow(dead_code)`. Until then registry
`-s1` behaves exactly as before.

## Tune::Ssimulacra2 — SHIPPED 2026-07-02 (release-gated): the metric-tune lever lands, cpu0-default beaten at BOTH speeds

**The tier-2 program's named lever** (docs/TUNE_SSIMULACRA2_PLAN.md): port libaom's
`--tune=ssimulacra2` mechanisms, A/B-measure each with ssim2 AND butteraugli, keep only
what wins on THIS encoder. Landed as `zenrav1e@a37faea8` (`Tune::Ssimulacra2`), preceded
by two pre-existing QM bugs the work exposed and fixed (zenrav1e#29, both on master):

- **`qm_v` header gating** (`9a8eaf61`): written only when the frame's u/v delta-qs
  differed; AV1 5.9.12 gates it on the sequence `separate_uv_delta_q` (always 1 here).
  Any QM frame with u==v delta-qs was corrupt to aomdec and silently mis-parsed by
  dav1d-lineage decoders. Masked by the Daala chroma offsets (u≠v almost always).
- **Transposed rectangular QM tables** (`2310c7be`): rav1e stores coefficients
  transposed (like dav1d) but `qm_table()` didn't swap w/h the way rav1d-safe's mapping
  deliberately does — every rect TX quantized with transposed weights, self-consistent
  in the encoder but wrong on every decoder. Invisible at the near-flat levels 12–15 the
  old curve picks; catastrophic at ss2-curve levels (decoded ssim2 85.7→55.7 at Q85
  before the fix). The historical "with_qm(true) ≈10% BD-rate win" predates this fix
  and needs re-measurement at the dep bump.

### Per-step verdicts (each stage A/B'd cumulatively, 22-image corpus, s2)

| mechanism | ssim2 med | butteraugli 3n / max | verdict |
|---|---|---|---|
| chroma delta-q (4:4:4 ac +clamp(qi/2,0,24)) | **−2.79%** (20/22) | −0.39% / −1.38% | ships |
| frame rdmult weight ×(200..128)/128 | +4.41% (0/22) | +3.36% / +2.86% | dropped — aom-calibrated rdmult doesn't transfer to the Daala λ |
| ss2 QM level curves (+QM always on) | **−7.79%** (21/21) | −6.00% / −8.16% | ships — biggest single lever of the whole program |
| trellis λ×0.25 / ×1.0 | +0.01% / +0.21% | ~0 / +0.8% | dropped |
| Variance Boost via segmentation | +1.92% (7/22) | +2.21% / +3.24% | dropped — double-boosts flats vs the existing activity masking; helps only family-7 screen content (o_7002 −10.5%). Full staged impl preserved as zenrav1e workspace commit `6257b65f` |

Every keep/drop is metric-consistent (ssim2 and both butteraugli norms agree in sign), so
no ss2-vs-IQ divergent knob needed the TUNE_IQ fallback. Tune-off is byte-identical to
master (proven per-encode + 22-image sweep continuity +0.0000%).

### Composed results (mechanisms 1+3; benchmarks/rd_gap_tune_ss2_2026-07-02.tsv)

**s2 + tune** (direct vs tune-off: ssim2 −4.28% med / −4.72% mean, better 20/22;
butteraugli 3n −2.53%, max −3.71%):

| ref | s2 master | s2 + tune |
|---|---|---|
| cpu0-default | +1.47% | **−3.43%** med / −2.62% mean |
| cpu2 (libaom-slow) | −0.65% | **−4.77%** |
| cpu0-ss2tune (tier 2) | +15.67% | **+10.10%** (improved 19/19) |

**s1 deep + tune** (direct vs shipped s1: **−3.57% med / −4.46% mean, better on 22/22**):

| ref | s1 shipped | s1 + tune |
|---|---|---|
| cpu0-default | −0.97% med, 11/19 wins | **−3.63% med / −4.00% mean, 16/19 wins** |
| cpu2 (libaom-slow) | −3.01% | **−6.22%** |
| cpu0-ss2tune (tier 2) | +11.08% | **+8.71%** med / +4.87% mean (improved 18/19) |

**The 8 s1-loser images: 5 flip to per-image wins vs cpu0-default** (o_2202 +1.1→−3.1,
o_3003 +6.7→−1.4, o_3008 +9.9→−3.4, o_6632 +2.2→−1.1, o_9077 +0.6→−1.1); the other 3
improve (o_6629 +25.3→+14.2, o_5004 +11.1→+7.4, o_9051 +3.1→+2.8). 7/19 photos now beat
cpu0+tune=ssimulacra2 itself. The remaining tier-2 gap concentrates in the same smooth-
content coefficient-level-RD images the s1 postmortem identified.

**Conformance:** 110/110 aomdec-clean + 110/110 rav1d-safe at BOTH s2+tune and
s1(deep)+tune configs. **Availability:** `Tune::Ssimulacra2` selects it; wiring into
zenavif/zenravif defaults is a release-gated follow-up at the zenrav1e dep bump (raw
sweeps: /mnt/v/output/zenavif/tune-ss2-2026-07-02/, see the benchmarks pointer file).

## Per-SB delta_q + Variance Boost — SHIPPED 2026-07-02 (release-gated): tier-2 gap +10.10% → +5.63% (s2)

**Mechanism #1 for the residual tier-2 gap** (user directive: algorithmic solutions with
offline-fit constants). zenrav1e coded NO delta_q syntax at all — the `//write_q_deltas()`
stub sat unused since the rav1e import, so libaom's fine-grained per-SB q allocation had no
zenrav1e counterpart and the tune's step-5 attempt had to route through segmentation (and
double-boosted flats, +1.92%). Landed on zenrav1e master:

- `d125713f` **per-SB delta_q syntax** (inert): frame-header `delta_q_present`/`delta_q_res`
  (5.9.17), per-SB `delta_q_index` symbol at the first block of each SB with the spec's
  SB-sized-skip omission, per-tile qindex predictor mirroring the decoder, `delta_q_cdf`
  (dav1d-identical default), qindex plumbed through quantize/dequant/rate via `get_qidx`
  composing with segmentation exactly like dav1d `init_quant_tables`. Includes a
  `BlockContextCheckpoint` fix for the rollback-unprotected `code_deltas` flag (same
  side-state class as zenrav1e#27) that any delta coding would have silently desynced on.
- `66733720` **Variance Boost through real delta_q** for `Tune::Ssimulacra2` (libaom
  `DELTA_Q_VARIANCE_BOOST`, allintra_vis.c rev 632172a4, SVT-AV1-PSY lineage): activity-mask
  8×8 variances → octile-5 1:2:1 sample → aom's still-picture boost curve (exact
  `av1_convert_q_to_qindex` scan semantics, qindex damping `(base+544)/1279`, cap 80,
  floor MINQ+1) → deadzone-rounded delta at aom's res 1/2/4/8-by-base-qindex; per-SB RDO
  distortion follow `(ac_q(base)/ac_q(sb))²` (the λ-side of libaom's per-SB rdmult
  tracking); segmentation disabled while active.
- `165e83b1` **strength 1.0 baked** from the fit below; dev sweep gates stripped.

**Strength fit** (train26 corpus per Corpus hygiene above, 24×12q, s2+tune, direct BD vs
boost-off, decision rule pre-registered before data: median-ssim2 rank, butteraugli veto
ba3n>+1.0%/bamax>+1.5%, ties ≤0.3% → better ba3n → lower strength):

| arm | ssim2 med / mean / better | ba3n med | bamax med | verdict |
|---|---|---|---|---|
| **1.0** | **−2.34% / −2.24% / 19/24** | **−1.13%** | **−0.76%** | **SHIPS** |
| 2.0 | −2.09% / −2.50% / 17/24 | −1.09% | −0.32% | tie→1.0 |
| 3.0 (aom default) | −2.20% / −1.82% / 18/24 | −0.46% | +0.75% | tie→1.0 |
| 4.5 | −1.45% / −0.15% / 14/24 | +1.07% | +5.51% | VETOED |
| 6.0 | −0.74% / −0.02% / 14/24 | +1.74% | +4.54% | VETOED |
| 3.0 + segmentation kept | −0.37% / −0.70% / 13/24 | +0.63% | +5.41% | VETOED |

libaom's default 3.0 does not transfer: zenrav1e's Psychovisual pipeline already
activity-masks distortion, so the optimal *allocation* boost on top is gentler (inverted-U
on ssim2, monotone butteraugli decay with strength). Strength 1.0 is non-regressing on
effectively every train26 family (worst +0.34%, one photo); smooth-gradient photos peak at
strength 2 (5004_nps −15.0% — per-image strength is a picker-knob candidate). The
keep-segmentation arm re-confirms the double-boost diagnosis with real syntax.

**Legacy-corpus confirm at strength 1.0** (continuity with the committed baselines; fresh
same-day aom baselines — the tune session's raws were lost with its scratchpad, and the
fresh ones reproduce the committed tune positions to 3 decimals):

| ref (photos n=19, ssim2 BD) | s2+tune (committed) | s2+tune+deltaq | s1+tune (committed) | s1+tune+deltaq |
|---|---|---|---|---|
| direct vs tune-only | — | **−1.81%** med (13/19) | — | **−1.19%** med (14/19; ba3n −0.72, bamax −1.59 — all norms agree) |
| cpu0-default | −3.43% | **−5.07%** (13/19) | −3.63% | **−5.38%** (16/19) |
| cpu2 (libaom-slow) | −4.78% | **−6.86%** (15/19) | −6.22% | **−7.18%** (17/19) |
| cpu0-ss2tune (tier 2) | +10.10% | **+5.63%** (6/19 win) | +8.71% | **+5.02%** med / +3.49% mean (7/19 win) |

Butteraugli on the legacy confirm: 3n −0.39% med / max +0.42% med (neutral; the fit corpus
had both clearly negative). **The 3 named holdouts** (TEST/VAL-split origins — illustration
only): **o_5004 is largely rescued at s1** — direct −8.96% ssim2 with butteraugli agreeing
(−13.7/−14.4), flipping to a WIN vs cpu0-default (+7.38→−1.09); at s2 it improves directly
(−1.11) though its vs-cpu0 number wobbles +4.45→+5.94. o_9051: s2 +8.51→+2.72 vs
cpu0-default and now BEATS cpu0-ss2tune at both speeds (−7.40 s2 / −6.73 s1); s1 vs
cpu0-default wobbles +2.81→+3.63. **o_6629 regresses (+14.15→+32.66 s2, +14.19→+26.42
s1)** — at q30-40 the one-directional boost + segmentation-off misallocates on this
ultra-flat gradient (+30% bytes for worse ssim2 at matched -Q; from q50 up the deltaq
curve dominates). o_6629 stays THE residual coefficient-RD outlier; the boost is not its
fix and per-image gating (picker) is the tracked follow-up.

**Conformance:** 110/110 cells (aomdec + rav1d-safe) × {s2, s1-deep} × {strength 3.0,
shipped 1.0} = 4 clean sweeps; local cross-decoder byte-agreement on 2/4-tile,
1000×700 straddle, 10-bit, keep-seg arm, and 20 tiny-frame cells spanning every
`delta_q_res` tier. The syntax fixed one latent trap on the way in: `code_deltas` was
rollback-unprotected in `BlockContextCheckpoint` (LF-delta scaffolding shared it,
inertly). Full data: `benchmarks/rd_gap_deltaq_2026-07-02.tsv` + pointer file
(`/mnt/v/output/zenavif/deltaq-2026-07-02/`).

## QM-weighted RD distortion (dist_metric=QM_PSNR analog) — SHIPPED 2026-07-03 (release-gated): the s1 TIER-2 MEDIAN CROSSES (+5.02% → −1.94%)

**Mechanism #2 for the residual tier-2 gap** (TUNE_SSIMULACRA2_PLAN item 6). aom's ss2
tune sets `dist_metric=AOM_DIST_METRIC_QM_PSNR`: coefficient-domain RD error is scaled by
the forward QM weight before squaring (`av1_block_error_qm` tx_search.c, `get_coeff_dist`
txb_rdopt_utils.h, rev 632172a4), and aom **forcibly enables tx-domain distortion**
whenever that metric is selected (rdopt_utils.h `set_tx_domain_dist_params`) — at
cpu0+ss2tune both surfaces are live in the reference. zenrav1e's RDO was QM-blind: the
tune's QM curves reshape dequant error per frequency, but decisions still priced every
frequency's error equally.

**Forward weights from the ported inverse tables.** zenrav1e stores only the spec's
dequant-side weights; libaom's stored forward table satisfies `wt == round(1024/iwt)`
exactly (verified numerically vs quant_common.c), so `QM_FWD_WEIGHT[iwt]` derives it. The
weight lookup uses the same storage-order indexing as `dequantize_with_qm`, so the
zenrav1e#29 rect-orientation fix carries over by construction.

**Round 1 — the literal aom routing loses; the isolated mechanism wins** (coarse grid,
train26, s2+tune, direct BD vs tune baseline):

| arm | ssim2 med | better | verdict |
|---|---|---|---|
| tx-domain switch, unweighted (control) | +6.07% | 1/23 | the switch alone forfeits cdef_dist activity masking |
| QM-weighted tx-domain (aom-literal) | +4.47% | 3/23 | REJECTED — recovers under half the handicap |
| *isolation: weighted vs unweighted tx-domain* | **−2.57%** | 16/23 | the weighting itself is real |
| trellis forced on, unweighted / QM-weighted | +0.32% / +0.55% | 4/23, 3/23 | REJECTED (1.66× time; re-confirms item-4) |

aom can route RD through weighted tx-domain SSE because it has no perceptual pixel metric
to lose; zenrav1e's `cdef_dist` (measured ~9.5% vs plain SSE) is worth more than the
frequency discount. **Round 2 — ratio composition:** keep the psy pixel metric and scale
its luma term by the per-trial QM-weighted/unweighted tx-error ratio (`Σw/Σu`,
accumulated per TX in `write_tx_block`, applied in `compute_distortion`) — exactly the
frequency-dependent forgiveness QM dequant applies to that block's error spectrum,
composed with activity masking instead of replacing it. Skip trials stay undiscounted
(mirrors tx-domain skip pricing).

**Direct isolation, full 12-pt grid, train26** (all three metrics agree at both speeds;
~1.01-1.02× encode time — the accumulation loops are noise vs transforms):

| config | ssim2 med / mean / better | ba3n med | bamax med |
|---|---|---|---|
| s2+tune+ratio vs s2+tune | **−1.78% / −1.45% / 15/24** | −1.46% | −0.37% |
| s1+tune+ratio vs s1+tune | **−1.71% / −1.52% / 15/24** | −1.51% | −2.49% |

Per-family: flat-gradient photos are the big winners (5004_nps −18.8%; fam 5000 −11.0%
median), screenshots −3.4%, interiors/nature/illustrations −2..−6; synthetic line plots
(fam 7000) lose +5.6% — the discount misprices sharp synthetic HF edges (palette-mode
content anyway; photos-first program accepts).

**Legacy-corpus confirm (photos n=19, ssim2 BD, deltaq-2026-07-02 baselines + same-day
aom refs):**

| ref | s2+tune+deltaq | s2 +ratio | s1+tune+deltaq | s1 +ratio |
|---|---|---|---|---|
| direct vs shipped | — | **−1.60%** med / −2.30% mean (16/19; ba3n −1.35, bamax −4.66) | — | **−2.51%** med / −2.61% mean (15/19; ba3n −1.24, bamax −2.11) |
| cpu0-default | −5.07% | **−7.90%** (14/19) | −5.38% | **−7.85%** (16/19) |
| cpu2 (libaom-slow) | −6.86% | **−8.49%** (14/19) | −7.18% | **−9.75%** (16/19) |
| cpu0-ss2tune (tier 2) | +5.63% | **+2.12%** med / +3.65% mean (9/19 win) | +5.02% | **−1.94%** med / +1.30% mean (10/19 win) |

**THE TIER-2 MEDIAN IS CROSSED at s1: −1.94%.** zenrav1e s1+tune now beats libaom
cpu-used=0 *with its own --tune=ssimulacra2* — the "aom at its absolute best" reference
this tier was defined against — on the median, with 10/19 per-image wins. The mean stays
positive (+1.30%), pulled by the two remaining per-image losers (o_5004 +21.9, o_6629
+7.9).

**o_6629 — THE residual coefficient-RD outlier — is finally moved:** s2 direct −13.5%
ssim2 / −12.6% ba3n / −22.3% bamax; s1 direct −15.3%/−11.4%/−10.7%. vs cpu0-default:
s2 +32.7 → +13.6, s1 +26.4 → **+7.6**; tier-2 s1 → +7.9. The item-6 hypothesis (the QM
discount fixes its q30-40 misallocation on the ultra-flat gradient) is confirmed — the
first lever to touch it since the s1 postmortem named it. o_9051 stays a per-image
tier-2 win (−1.9 at both speeds) though it's a direct-isolation loser at s1 (+5.19).
o_5004 improves everywhere that matters at s1 (direct −3.39; vs cpu0-default flips to
a −2.78 win) but its butteraugli 3-norm disagrees on this image (+12.7) and it remains
the largest tier-2 outlier (+21.9) — with o_6629 largely fixed, o_5004 is now the top
per-image target (picker-gating candidate).

**Conformance:** 110/110 s2 + 110/110 s1 cells (aomdec + rav1d-safe, legacy 22-image
corpus × Q{30,50,60,75,90}) at the shipped config — encoder-internal decision change,
no new syntax, zero corruption. **Landed:** zenrav1e `3710a573` (mechanism + measured
arms) + `4279a673` (landing shape, dev gates stripped), rebased over the same-day
palette + skip-recon landings (byte-gates re-verified on each new base)
(`qm_dist_ratio` + `qm_weighted_trellis` ride `Tune::Ssimulacra2`; dev gates stripped;
tune-off byte-identity vs master binary verified, stripped envs inert). Raw sweeps:
`/mnt/v/output/zenavif/qmdist-2026-07-03/` + `benchmarks/rd_gap_qmdist_2026-07-03.tsv`.

## LF sharpness schedule — SHIPPED 2026-07-03 (release-gated): zenrav1e#30 item 1, the last un-A/B'd tune-IQ ingredient

From the libavif-1.4 study (`LIBAVIF_1_4_STUDY.md` §c4). aom writes LF `sharpness=7` to
the frame header for allintra/IQ/SS2 (picklpf.c:220-231 @ 632172a4); tune-IQ additionally
clamps by qindex {≤112→7, ≤160→1, else 0} (picklpf.c:232-249, costs a little ss2 at low q
— why aom's SS2 tune omits it). zenrav1e wrote the field only under `Tune::StillImage` —
and **the encoder's own filter ignored it**: recon diverged from every conforming decoder
for any nonzero sharpness, and `deblock_filter_optimize` priced levels for thresholds the
decoder would not use.

**Groundwork (`zenrav1e@c1fab5b3`, measured as aba01be7):** the deblock filter + level
optimizer honor header sharpness — exact const-built inverse tables for the AV1 7.14.4
threshold derivation (rav1e's inverted minimal-level formulation), verified exhaustively
against the forward map (3 new unit tests); the schedule is decided once per frame BEFORE
tile encoding (delayed-LF RDO included); lossless forced 0. Sharpness-0 output is
byte-identical (18/18-cell md5 gate).

**4-arm A/B** (train26 coarse ranking → winner full 12-pt grid + legacy confirm;
pre-registered rule in the raw-dir README: median ssim2 rank, butteraugli veto at
ba3n>+1.0%/bamax>+1.5%, ties ≤0.3% → better ba3n, ship bar −0.3%):

| arm | ssim2 med (coarse) | ba3n med | bamax med | verdict |
|---|---|---|---|---|
| const 7 (aom SS2/IQ/allintra) | −0.50% (18/24) | +0.18% | +0.17% | ties `still`, loses ba3n tiebreak |
| adaptive {7,1,0}@{112,160} (tune-IQ) | −0.23% (17/24) | +0.07% | +0.01% | misses the −0.3% ship bar |
| **still {7,5,3}@{80,160}** (zenrav1e's dormant StillImage schedule) | **−0.50% (18/24)** | +0.11% | +0.14% | **WINNER** |

**Winner, full grids (direct isolation, ~1.00× encode time):**

| config | ssim2 med / mean / better | ba3n med | bamax med |
|---|---|---|---|
| s2 still vs lf0, train26 | **−0.43% / −0.47% / 19/24** | +0.11% | +0.29% |
| s1 still vs lf0, train26 | **−0.42% / −0.44% / 19/24** | +0.08% | +0.04% |
| s2 still vs lf0, legacy photos | **−0.67% / −0.43% / 16/19** | +0.00% | −0.12% |
| s1 still vs lf0, legacy photos | **−0.66% / −0.26% / 14/19** | +0.12% | +0.04% |

**Metric-divergence note (first of its kind in the tune):** butteraugli's *sign* diverges
from ssim2 on train26 (+0.1..0.3% med, far under the veto; flat-to-negative on legacy
photos). Sharpness trades a small blocking cost for a larger edge-retention win — aom
ships it for SS2/IQ on subjective-sharpness grounds. Losers concentrate in bi-level
content (1-bit patent scans +1.2/+1.4): sharpness suppresses the deblocking that content
relies on at low q. Per-image sharpness is a plausible picker knob later (text
screenshots love it: LKML page −4.8%).

**Tier movement (photos n=19, fresh lf0 baselines at master 9b79b442):** master's s2
tier-2 median had already CROSSED between the qmdist measurement and this baseline
(+2.12 → −1.54; the same-day palette + skip-recon + filter-intra/LRF desync fixes, not
this change). With the schedule: tier-2 medians ride the o_9051 knife edge (s2 −1.54 →
+0.18, s1 −2.10 → −0.71) — o_9051 is the *only* material tier-2 loser (s1 −4.05 → −0.71)
and lands on/near the median at both speeds, while 14-16/19 images' tier-2 BD improve or
stay flat, tier-2 MEANS improve (s2 −0.68 → −0.96, s1 −1.47 → −1.52), the s1 tier-2
median stays crossed, and the cpu0-default / cpu2 tiers improve at both speeds (s2
−10.10 → −10.82 / −11.18 → −12.06; s1 −10.43 → −11.27 / −11.88 → −12.19).

**Conformance:** 110/110 s2 + 110/110 s1-deep (aomdec + rav1d-safe), schedule armed —
decoder-visible header field + filter-behavior change. **Landed:** `zenrav1e@c1fab5b3` +
`zenrav1e@9a05d54a` (schedule: `lf_sharpness()` returns {7,5,3}@{80,160} for
StillImage | Ssimulacra2; dev gates stripped), rebased over the same-day #33 + #32
desync landings with the tune-off byte-gate re-verified at each base; the landed default
byte-reproduces the measured `still` arm (3-cell md5). aom's `sharpness` knob also
carries a quantizer-rounding bias (av1_quantize.c:607-620, qrounding 48→64 when
sharpness≠0) — NOT ported; closest prior art is the measured-rejected sharpness-7
trellis surface. Raw sweeps: `/mnt/v/output/zenavif/lfsharp-2026-07-03/` +
`benchmarks/rd_gap_lfsharp_2026-07-03.tsv`.

## Fixed 2026-07-03: zenrav1e#32/#33 encoder-recon desyncs — shipped-config NEUTRALITY verified, filter-intra measured and REJECTED

**The fixes** (zenrav1e `32477046` #33 filter-intra, `17cff82f` #32 LRF, changelog
`c9c2d5f7`): filter-intra blocks got DC_PRED's edge preparation (top-left corner fed 128
instead of the recon pixel; empty left column at x==0) AND read the bottom-to-top left
edge buffer upside-down — every filter-intra block's prediction diverged from conforming
decoders, compounding to 17-25 luma RMSE at rav1e speeds ≤ 6. The issue-#32 LRF report
was this same bug misattributed (its speed bisect jumped s6→s8; the missing s7 rung —
LRF on, filter-intra off — measures 0.000, and LRF application is byte-exact on 27
isolation cells). One real latent LRF desync existed and is fixed: an inherited 2019
skip left *signaled* sgrproj units unapplied in the recon for cdef-off+lrf-on configs
(API-only; RMSE 0.387-0.580 → 0.000, measured via the new zenrav1e
`examples/recon_probe.rs`). Desync metric at the shipped fixes: **0.000 luma RMSE
recon-vs-aomdec-vs-rav1d-safe** on the repro corpus (s2/s4/s6/s0) and 120/120 train26
conformance cells (rav1e s2 defaults = filter-intra ON, Q{25,64,102,127,178}).

**The headline for THIS program: cavif was never exposed — every rd_gap number stands.**
ravif pins `complex_prediction_modes: Some(false)` → `PredictionModesSetting::Simple` →
`enable_filter_intra = false` at every speed (`ravif/src/av1encoder.rs:1590`, f07d552),
and its lrf-on configs keep cdef on. cavif s2+tune encodes are **byte-identical between
pre-fix and post-fix zenrav1e** (8/8 spot cells + sweep-level). Issue #33's exposure
claim ("ravif does NOT disable filter intra — rd_gap photo measurements systematically
pessimistic on smooth content") was **wrong**; the free win for the shipped config is
**0.00%, and the tier-2 residual recovered by these fixes is 0.00%**. (In the LF-sharpness
section's baseline-lineage note above, the +2.12 → −1.54 tier-2 move belongs to the
same-day **palette + skip-recon (b30dd752)** landings — measured here as −0.72% median
ssim2 on train26 via lineage continuity (1/288 cells byte-exact vs the qmdist committed
arm) — NOT to the desync fixes, which are byte-neutral for cavif.)

**Filter-intra, measured honestly for the first time and REJECTED** (train26, s2+tune,
full 12-q grid, direct isolation, `ZENRAVIF_FILTER_INTRA=1` dev-arm forcing
`prediction.filter_intra = Some(true)`):

| arm | ssim2 med / mean / better | ba3n med | bamax med | time |
|---|---|---|---|---|
| fi-on vs shipped | **+1.82% / +1.97% / 2/24** | +1.33% (veto: >+1.0%) | −0.54% | 1.70× |

Worse on 22/24 images including the smooth-photo families the desync theory hoped to
rescue (fam 5000 +1.51% med; 5004 +1.72, 5048 +1.31); only 7052 (line plot) clearly wins
(−2.53). ravif's historical "ComplexAll causes 12 dB PSNR regression" (zenrav1e#5) **was
the desync** — spot: 5048 q60 fi-on ssim2 59.50 pre-fix → 75.11 post-fix (shipped
75.00) — but with correct predictions the tool is still a net RD loss at s2+tune and
**stays off by measured policy, not by workaround**. The `complex_prediction_modes`
pin's justification in ravif should cite this measurement going forward. Who the fixes
DO help: `rav1e`-binary/API users at s ≤ 6 (recon and RDO no longer diverge from what
decoders produce) and any future config that arms cdef-off+lrf-on.

Data: `benchmarks/rd_gap_desyncfix_2026-07-03.tsv` +
`/mnt/v/output/zenavif/desyncfix-2026-07-03/` (both sweep arms, conformance TSVs,
SHA256SUMS). zenrav1e#5 can close against the fi-on measurement; `tests/
trellis_roundtrip.rs`'s loose `PSNR > 10` gate ("rav1d-safe is not bit-exact...") can
now be tightened — the inexactness was these encoder bugs, not rav1d-safe.

## RULED OUT, 2026-07-01: `BLOCK_32X32`/`BLOCK_64X64` at 0% usage — explained, not a bug, widening regresses

Same inspect-diff methodology surfaced a second block-size anomaly: `BLOCK_32X32` and
`BLOCK_64X64` sit at **exactly 0.00%** for zenrav1e on **3/3** tested photos (o_1015, o_2012,
o_5004), vs libaom's 4–19% each — a large, consistent gap that looked like a third
search-completeness bug in the same family as the topdown fix. **It isn't.**

**Root cause (not a bug):** `encode_partition_topdown` and `rdo_partition_decision` handle
`PARTITION_NONE` at large block sizes correctly when reached — the candidate list construction
and RD-cost computation (`rdo_partition_none`, `rdo.rs:2034-2047`) are both complete. The actual
gate is `must_split = is_square && (bsize > fi.partition_range.max || !has_cols || !has_rows)`
(`encoder.rs:3272-3273`): when true, `PARTITION_SPLIT` is forced with **no candidate list built
at all**. `fi.partition_range.max` isn't zenrav1e's own default — it's set by the *consuming*
crate `ravif`'s `SpeedTweaks::from_my_preset` (`ravif/ravif/src/av1encoder.rs:1546-1553`), a
deliberate 2021/2022 upstream tuning table:
```rust
partition_range: Some(match speed {
    0 => (4, 64.min(max_block_size)),
    1 if low_quality => (4, 64.min(max_block_size)),
    2 if low_quality => (4, 32.min(max_block_size)),
    1..=4 => (4, 16),           // <- speed 2, non-low-quality: this is the branch our
    5..=8 => (8, 16),              sweep range (ssim2 70-92) mostly falls into
    _ => (16, 16),
}),
```
`low_quality = quantizer > 150`, `high_quality = quantizer < 80` (`max_block_size = 16` if
`high_quality` else `64`). At speed 2 with anything but an aggressive/low quality target, this
falls through to `(4, 16)` — structurally excluding 32x32/64x64 `NONE` regardless of content.
Confirmed via `git log -p` on ravif: long-standing, deliberate, not an oversight. `zenavif`
already mirrors and exposes this as an overridable expert knob (`override_partition_range`,
`src/expert.rs:110`).

**Tested anyway** (per this project's measure-before-deciding discipline): widened the
speed-2/non-low-quality branch to `(4, 64.min(max_block_size))` (matching the low-quality
branch), rebuilt, verified no corruption (5 photos aomdec-clean + zenavif roundtrip clean,
pixel diff in normal lossy range), then measured direct isolation vs the unwidened build on the
same 22-image corpus/Q-grid (`benchmarks/rd_gap_partitionrange_widen_2026-07-01.tsv`):

**RESULT: regression.** At matched quantizer (not matched quality), the affected quality band
(-Q 50–75, where `high_quality`'s clamp doesn't neutralize the change) costs **+1.4% to +1.8%
more bytes** (median) for **~0 ssim2 change** (mean −0.08, i.e. not even a quality trade) — and
**24–36% slower encode** in that band. 17/19 photos at -Q 60 got strictly worse; only 2 improved
marginally. Root cause: unlike the topdown HORZ/VERT fix (pure addition, safe by construction),
enabling large-block `NONE` isn't free — `rdo_partition_none`'s RD-cost estimate at 32x32/64x64
apparently doesn't reliably reflect true bit cost on this corpus, so the search sometimes picks
a large block its cost model likes but that actually costs more to encode. Same root-cause shape
as the CfL-widening finding (RD-cost-model accuracy, not search completeness). **Reverted**
(`ravif` restored to `b4853c68`, not landed). A real fix would need to improve the RD-cost
estimate itself at large block sizes, not just permit more candidates for the existing one —
larger, not attempted.

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

## IMPLEMENTED 2026-07-03: palette mode (item 4) — release-gated

zenrav1e master now carries the full AV1 luma palette tool (zenrav1e@68a8d81f syntax
writers + CDF wiring, @5f82e2d4 search/recon/RDO, @cda831e7 AA-aware detection +
`PaletteMode`, @df27117c 10-bit roundtrip; default **OFF** behind
`SpeedSettings.prediction.palette = PaletteMode::{Off, Auto, Always}` and the rav1e
binary's `--palette`). Search is the libaom av1_rd_pick_palette_intra_sby shape
(top-color + k-means families, neighbor-cache snapping) trialed through the real
bitstream writers — full flag+size+colors+index-map rate, no `discount_color_cost`
(their overuse bug b:421196988). `Auto` ports
`estimate_screen_content_antialiasing_aware` (their all-intra default since 3.14.0) as
a per-key-frame gate that also drops `allow_screen_content_tools` signaling on photos.
UV palette search and palette-in-inter-frames are not implemented (conformant
omissions; the UV flag is coded "off").

**Conformance:** every palette-armed cell across all measurement sweeps (720+ cells,
both partition paths, q 60-220, 8-bit; plus a 10-bit in-repo roundtrip) decodes
aomdec-clean AND byte-agrees (raw I420 md5) with rav1d-safe. Synthetic screen content
round-trips with LOSSLESS luma at ~1/8 the palette-off bytes
(zenrav1e tests/palette_roundtrip.rs).

**Measured RD (train26 24-origin sample, q{60,100,140,180,220} × s{2,6}, rav1e IVF
layer, color-exact color.py scoring, `--lrf false --filter-intra false` to isolate
zenrav1e#32/#33; 720 cells, 0 conformance failures; per-image table in
`benchmarks/palette_ab_train26_2026-07-03.tsv`, raw pointer alongside):**

Per-family median BD-rate, bytes at matched quality (negative = palette wins),
`always` vs `off`, with butteraugli-pnorm3 BD as the metric-gaming guard:

| family | n | s2 ssim2-BD | s2 bp3-BD | s6 ssim2-BD | s6 bp3-BD |
|---|--:|--:|--:|--:|--:|
| 7000 plots | 4 | **−20.6%** | −21.9% | **−65.9%** | −65.4% |
| 8100 web screenshots | 4 | −3.4% | −2.3% | **−22.0%** | −25.0% |
| 6000 scans/patents | 3 | −1.4% | +0.0% | **−21.8%** | −27.6% |
| 5000 nps maps | 2 | −0.7% | −2.6% | −8.9% | −11.2% |
| 9000 gen illust/products | 7 | −0.9% | −1.2% | −0.9% | −2.2% |
| 1000 photos | 3 | +0.7% | −0.1% | −0.1% | −0.4% |
| 2000 people | 1 | +0.1% | −0.3% | −0.2% | +0.7% |

Same-q view: photos are a true **no-op** (dssim2 median +0.00, dbytes −0.0%); the
worst always-arm cell anywhere is dssim2 −1.45 (butteraugli neutral) — the
b:421196988 overuse shape does **not** reproduce with full-rate RD trials. The big
wins concentrate exactly where the ablation predicted: plots −62.7% bytes at same-q
median (s6) with +4.2 ssim2 on top. Encode cost: +4-12% at s2, +58-189% relative at
s6 on photos (fast encodes, wasted search — `Auto` removes it); plots encode 13-16%
FASTER with palette (palette blocks skip transform/RDO work).

`Auto` (detection) vs `off`: identical to `always` on the fired images —
plots 7028/7050/7052, text screenshots 8268/8302/8414, 1-bit scan 6018, product
shot 9868 — and a no-op (±flag-bit noise) on every rejected image. Detection
faithfully reproduces libaom (verified: aomenc default vs `--enable-palette=0` is
byte-identical on 7058), which means it also inherits libaom's conservatism: it
rejects the gradient-heavy plot 7058 (always-BD −28.6% at s6) and the mixed
photo/text screenshot 8196 (−4.5%), leaving real wins on the table. Detection
threshold tuning / a zenanalyze picker head is the follow-up for that remainder.

**Family-7 continuity (the +130-470% headline):** same legacy plots
(o_7000/7001/7002), same-day 3-arm re-measurement against aomenc `--cpu-used=2`
(the doc's matched-speed reference; rav1e IVF layer, 420, color-exact scoring,
zr q-grid extended to q10 so the frontiers overlap; raw rows in
`benchmarks/palette_fam7_continuity_2026-07-03.tsv`). In 420 these plots
saturate at ssim2 ≈67-70, so the gap is evaluated at the midpoint of the
frontier overlap per image — at the top of the achievable quality range,
where the original headline numbers lived:

| image | matched ssim2 | aom bpp | off gap | always gap | auto gap |
|---|--:|--:|--:|--:|--:|
| o_7000 | 66.9 | 0.349 | +163% | **+61%** | +61% |
| o_7001 | 70.0 | 0.105 | +297% | **+99%** | +99% |
| o_7002 | 69.6 | 0.411 | +176% | **+72%** | +72% |
| **median** | | | **+176%** | **+72%** | +72% |

The legacy band reproduces on the off arm (+163..297%, same shape as the
original +130-470% multi-format table), and palette cuts it to +61-99% —
roughly **60% of the excess bits over libaom removed**. Auto equals Always
byte-for-byte on all three (detection fires). Notably, palette-off zenrav1e
could not even *reach* libaom-cq63's quality floor on o_7001 within the
original q40-220 grid (aomenc's worst point beat zenrav1e's best at 3.6×
fewer bits); the palette arm crosses it at q60. The residual +70-99% is
non-palette headroom (intraBC and coefficient-level RD are the known
candidates).

**Measurement-config caveat discovered en route:** three separate encoder-recon
divergences were found while scoring this work — forced-skip intra blocks never
predicting into the recon (FIXED, zenrav1e@b30dd752), LRF (zenrav1e#32, open) and
filter-intra (zenrav1e#33, open) desyncing the encoder recon from what aomdec+rav1d
(byte-agreeing) decode. All three depress decoded-quality scores of s≤6 zenrav1e
output on smooth content; the palette tables above use the isolated config. Existing
photo-gap numbers in this doc predate that isolation and are systematically
pessimistic about zenrav1e wherever filter-intra fired (ravif keeps LRF off at normal
quality but filter-intra ON).

**At the zenrav1e dep bump:** wire `PaletteMode` through zenravif/zenavif (encoder
options; the zenanalyze picker is the natural owner of Off/Auto/Always per image —
detection features are cheap), then re-measure the screen tier gap.

**2026-07-03 (later still): UV (chroma) palette landed upstream (zenrav1e@a3b72033,
release-gated) — plus a latent CDF-undo-log corruption fixed en route
(zenrav1e@e86235b5).** The previously-"off"-coded UV flag now carries a real joint
(U,V) palette: libaom `av1_rd_pick_palette_intra_sbuv`'s per-size 2-D k-means
candidates PLUS a dominant-pairs family (2-D analogue of the luma top-colors family;
k-means-only search measurably misses exact palettes on palette-exact content),
U colors coded min-step-0 against the U neighbor cache, V raw-vs-wraparound-delta by
libaom's exact rate arithmetic, one shared chroma index map — all trialed through the
real writers on top of the winning luma side (libaom's decoupled sby-then-sbuv shape).
Same `PaletteMode` knob, default Off, byte-identity off re-verified 48/48 vs 17e67842.

Measured vs the shipped luma-palette base (`--palette always` both arms, isolated
config, q{60,100,140,180,220} × s{2,6}; `benchmarks/uvpal_ab_2026-07-03.tsv`):
fam-7000 plots ssim2-BD median **−1.95% (s2) / −2.59% (s6)** with butteraugli-pnorm3
BD agreeing (−4.5%/−8.3% median; 7050 −9.0%/−7.2% ssim2 at q60 byte ratios 0.83/0.79);
legacy fam-7 anchors (o_7000/7001/7002) −2.55%/−5.73% ssim2 median (butteraugli mixed
at s6: +1.9% median on those three — chroma palettization trades smooth-chroma DCT for
quantized flats; noted, not gating); 8100 screenshots −0.5%/−1.1%; photos ±0.0-0.65%
(noise, both signs). Conformance: 200/200 corpus cells (420) + 84/84 at 444 aomdec-clean
AND rav1d-safe raw-md5-agreeing, plus in-repo exact-chroma roundtrips at both samplings,
odd-dims/tiny frames, and 10-bit.

The conformance sweep also exposed a LATENT ENCODER BUG shipped since the CDF undo log
existed: the log captured/restored fixed 16-word snapshots regardless of CDF length, and
its small/large partitions roll back sequentially rather than globally LIFO — so a luma
n=8 color-index update at the last `palette_y_color_index_cdf` row spilled its snapshot
over `palette_uv_color_index_cdf[0][0]`, and rollbacks resurrected stale UV CDF state
(encoder-only state no decoder reaches ⇒ content-dependent desync; silent for luma-only
palettes whose adjacent bytes never changed). Fixed with exact-length snapshots + a
compile-time bound (zenrav1e@e86235b5, regression test
`cdf_log_rollback_is_exact_length_across_field_boundaries`); bug-class 6 in the project
memory (fixed-width undo logs over adjacent adaptive state).

**2026-07-03 (later still, 2): intraBC chunk A landed upstream (zenrav1e@7a59e569,
release-gated) — the wedge-#1 owner engaged.** Evaluate-first verdict on aomenc's own
delta (`--enable-intrabc=0` vs auto, cpu2-default + cpu6-ss2tune, 18 screen/plot/doc
files; raw rows in the scratch A/B feeding this section): far above the 2% defer bar —
7052 −55.6% bytes at cpu2 same-quality (min ratio 0.269), 7050 −33.5%/−52.5%,
fam-7 legacy −33% both speeds, screens 0-20%, photos ~0 → IMPLEMENT. Chunk A ships DV
prediction (rav1d `decode_b` dual), the `av1_is_dv_valid` port (256-px delay +
wavefront), fullpel all-plane copy MC, seeded diamond SAD search + top-2 full-rate RD
trials, per-block flag + DV coding via the inter tx/coef path; default off behind
`SpeedSettings.prediction.intrabc` / `--intrabc` (byte-identity off 80/80). With
`PaletteMode::Auto` the AA-aware detection's stricter intraBC criterion gates it —
VERIFIED byte-identical on photos and firing on plots (7052 q100 s6 3503→2026 B).

Measured on the same corpus vs the palette+UV base (blanket `--palette always
--intrabc`, isolated config; rows in `benchmarks/uvpal_ab_2026-07-03.tsv`):
**7052 −34.9%/−39.4% ssim2-BD (s2/s6; q60 byte ratios 0.39/0.42), 7050
−17.6%/−23.6%**, 8414 −4.3%, fam-7000 median −2.3%/−1.8%; blanket-on regresses photos
+3..+8% (in-loop filters hard-off frame-wide per spec) — which is precisely why the
detection gate is the production path, and why `Auto` was verified photo-byte-identical.
Conformance: 200/200 armed corpus cells aomdec-clean + rav1d-safe raw-md5 agree;
in-repo roundtrips at BOTH samplings shrink exactly-repeating non-palettizable content
to ~0.52x. Encode cost ~1.15-1.25x (7052 FASTER: copies skip transform work).

**fam-7 legacy continuity (the +130-470% → +61-99% headline), extended:** at the same
matched top-of-range ssim2 vs aomenc cpu2 (420): median gap **+169% (off) → +75%
(+palette) → +55% (+UV palette) → +57% (+intraBC)**. The UV palette removes another
~20 points of the legacy residual; chunk-A intraBC is neutral THERE (fullpel/even-DV
diamond search misses the legacy plots' repeats that aom's hash search finds — its −33%
on those files is the chunk-B headroom, zenrav1e#30 item 3) while owning the wedge
anchors above. The remaining ~+55% on legacy plots is coefficient-level RD +
intraBC-search headroom.

**2026-07-03 (later): the zenanalyze palette gate landed (release-gated) after a
val-confirmed mechanism A/B.** The detection-conservatism remainder above plus the
wedge finding that the AA-aware detection dies on ANY downscaled screen content are
now handled zenavif-side: `patch_fraction > 0.197` → `PaletteMode::Always`
(`zenavif src/palette_gate.rs`, wired into `auto_tune`, encoder-forward commented
until the dep bump). Measured on 14 held-out VAL origins × sizes {256,512,1024} ×
both configs: where the gate fires and detection is dead the rule recovers
−10..−39% BD at s6 (6091 patents @1024: auto +0.04 vs always −39.5) and
−3.3..−15.2% at s2 @1024; photos never fire; false fires cost ≈0 bytes +
1.06× median encode time; 0 conformance failures in 6,216 cells (aomdec +
rav1d-safe raw-md5 agreement on every palette-armed cell). Full record:
`docs/HYPERPARAM_FIRST_CUT_2026-07-03.md` rule-1 status +
`benchmarks/hyperparam_palette_mech_ab_2026-07-03.tsv` + raw at
`/mnt/v/output/rd-gap-palette-ab-2026-07-03/`.

**2026-07-04: intraBC chunk B landed upstream (zenrav1e@d655a6ee infrastructure +
@184eb713 integration, release-gated) — the hash search, the legacy-plot residual
owner engaged.** Port of libaom's `av1_hash_table` (hash_motion.c + CRC-32C, pinned
rev 632172a4), scoped to chunk A's domain: the tile's SOURCE luma block-hashed once
per tile encode (2x2 identity/xor-fold base layer, hierarchical CRC-32C combine for
8/16/32/64 squares, 2^16 buckets per size, 256-entry caps filled in libaom's
hierarchical dispersal order), then square intraBC blocks add up to 64 exact-match
DV candidates — nearest-first, through chunk A's unchanged validity + recon-SAD
ranking + top-2 full-rate RD trial machinery. New
`PredictionSpeedSettings.intrabc_hash` / `--intrabc-hash` (default true, inert
without `intrabc`); hash-off byte-identical to pre-chunk-B (81/81 gate cells vs
master 0d392334 — the SAD-0 diamond skip is hash-gated precisely so the off-path
keeps the diamond's incidental second-candidate updates bit-exact).

Measured (same uvpal 20-file screen corpus + isolated config as the chunk-A table,
hash-on vs hash-off, `benchmarks/ibc_hash_ab_2026-07-04.tsv`; 200/200 armed cells
per arm aomdec-clean + rav1d-safe raw-md5 agree, no butteraugli-max veto):
**the legacy fam-7 trio — where chunk A stayed neutral while aomenc's hash search
took −33% — moved: o_7000 −26.2/−22.3 ssim2-BD (s2/s6), o_7001 −29.2/−28.4, o_7002
−26.3/−21.5 (q60 bytes 0.75–0.82×); 7058 line-tiling −36.6/−40.1 (bytes
0.60/0.56×); 7028 −4.6/−4.7; 8414 lkml screens (the s4-tier residual #1) −4.6/−5.4;
8302 −2.4/−2.6; 7080 −1.4/−2.4. Photos and the gray rescan are byte-identical
(ratio 1.000 — natural content produces no exact hash matches), encode wall median
1.00× (s2) / 1.06× (s6), worst 1.27×.** In-repo, `tests/intrabc_roundtrip.rs`
gains a long-range-repeat liveness gate (noise field + one distant 64×64 stamp,
both samplings) and the existing armed roundtrips + tiny-frame gates run the hash
path by default. Remaining intraBC scope (post-B): extended block sizes (4:1
slivers, sub-8x8), odd-DV chroma subpel (the 4:2:0 chroma-fullpel restriction
rejects odd-parity exact matches — visible as 7052's ~0.0 delta here: its chunk-A
diamond already found the even-parity repeats), 128-px superblocks.

fam-7 legacy continuity (within this A/B's own config + log-interp matched
top-of-range ssim2 vs the cached aomenc-cpu2 GOOD refs, s2): the trio's median
matched-quality byte gap falls **+36% (chunk A) → +7% (chunk B)**, with o_7002
CROSSING to −15%; per-image o_7000 +44%→+17%, o_7001 +36%→+7%. (Not directly
splice-able into the earlier +169→+57 ladder — that ran the shipped-path config
and a different match — but the trio's chunk-B share of the legacy residual is
now measured: most of what aom's hash search was taking on these files, ours
takes too.)

**s4-tier residual columns (same-day sc10 pass, train26 sc set, tune-ss2 +
palette auto + intrabc, BD vs the CACHED speedladder cpu2iq-ai rows; TSV
sections in `benchmarks/ibc_hash_ab_2026-07-04.tsv`): the 8414 column — the
s4-tier verdict's quantified #1 residual at +22.5 (composed-v3i5, which had NO
intraBC) — sits at −8.1/−15.1 (chunk A) → −13.6/−18.2 (chunks A+B) vs the same
reference rows in the intraBC-armed isolated config**: the mechanism owns the
column, with chunk B adding −5.5/−3.1 on top of chunk A's move (config caveat:
isolated rav1e-CLI vs the composed cavif cells — the definitive composed-column
re-measure happens at the dep bump when intraBC is exposed through zenravif).
7028 +6.6→+2.2 (s2) / +9.3→+3.6 (s6); 7050 +40.8→+37.8 / +26.6→+24.2 (mostly
NOT intraBC-reachable: odd-parity repeats + the near-lossless floor); 6018/6096
hash-inert (iso +0.00 EXACT — confirming the rescan diagnosis below: that
residual is not intraBC's); 8196/8268 ≈0. One adverse pair: 7052-s6 iso +3.65
ssim2 / bamax +2.32 (a veto by the standing rule) in the sc10 auto-gated config
while the same image is neutral-to-positive in the always-armed AB grid —
equal-SAD hash ties displacing diamond picks under the tune; banked as 0 per
the veto convention and worth a tie-break refinement if it recurs.

## Near-lossless rescans residual (6096/6018, s4-tier +15.8/+7.2) — DIAGNOSED 2026-07-04: two different owners, neither a missing screen tool; documented + handed off per P3

**Program**: FAST_TIER_PARITY_PLAN P3 item 2 — the s4-tier column verdict charged the
6000-family rescans (+15.8 on 6096, +7.2 on 6018 vs aom-cpu2iq-ai at the composed
point) to "the near-lossless floor" and asked for diagnose-then-fix-or-document.
Instruments: the 61,922-row label store (per-arm per-image BD across every measured
lever), per-q ladders, and aom's own `inspect` bitstream tool (accounting +
per-4x4-cell decisions) at byte-matched near-lossless cells (aom cq16 tune={iq,def}
allintra cpu2 vs rav1e s2 tune-ss2 + palette auto at matched bytes).

**6018 (1-bit patent scan): the iq-AQ machinery owns it — handed to the tune
program.** The composed fast mode ALREADY beats aom-cpu2def-ai on this image
(+6.93 vs +10.30 BD against the shared cpu2iq reference); the residual vs cpu2iq
is ~the def→iq delta itself, which sits in the low-mid band (def's ladder is
dominated there while its top-of-range matches iq). Both encoders palettize it
(inspect: 20.7% of our cells vs 15.1% of aom's carry palette) and both run per-SB
deltaq boost (bimodal maps: ours {42,61}, aom-iq {36,64} at cq16) — aom just
boosts DEEPER, and the in-repo boost-strength ladder responds exactly as that
predicts: str0→str4.5 = −4.14% BD monotone on 6018 (shipped tune strength 1.0
= −2.84, so ~1.3 points sit above the shipped constant on this image; strength
is a global tune fit — per-image boost strength is the FEATURE_HINTS candidate).
Every tx lever is measured harmful on it (s6-base +19.9 → size1 +15.7 → min
+18.8 → type +22.4), the v3 tx-D withhold is already the right call, and intraBC
chunk B leaves it byte-identical (1-bit glyphs sit on palette, not exact copies
at even parity). **Verdict: not search-budget, not a missing tool — the
tune-program's iq-AQ residual class (same owner as 1236/9100/9118), with the
boost-depth data attached. Do not force a fix here.**

**6096 (gray rescan): a coefficient-level RD valuation gap in the near-lossless
band — handed to the coefficient-level program (NOT iq-AQ).** The pain
concentrates at ssim2 90→93 where we pay +28–30% bytes vs cpu2iq — and
**cpu2def ≈ cpu2iq in exactly that band** (91.5@1.13 vs 91.4@1.09 bpp; def's
whole-curve BD vs iq is +5.25 ssim2 / −0.18 ba3n, all of it low-band), so the
iq-AQ machinery is acquitted for the near-lossless residual. Stream inspection
at byte-matched cq16/q70 shows the mechanism: **aom codes coefficients on 100%
of 4x4 cells (0% skip, both def and iq) at baseQ 64 while our encode skips
57.5% of cells at baseQ 54** — on scan grain, "higher quantizer + residual
everywhere" beats "lower quantizer + half the plane skipped" on both metrics.
Deltaq is not the lever (our variance-boost map has the same shape and range as
aom-iq's on this image; def is FLAT-64 and still never skips); deltaq strength
sweeps HURT 6096 (+0.35..+1.8 at every strength); every partition/tx lever
measured leaves ≥ +16 (full-tx +16.7, min +17.4, size1 +21.1, base +61.3);
even zr-s2-tune (max effort) leaves +8.55 whole-curve and +29% at the top band.
The candidate aom mechanisms are all coefficient-valuation constants: the
dead-zone/rounding pair (`av1_build_quantizer`: qzbin/qrounding 48/128, and
tune-iq's sharpness≠0 path lifting rounding to 64/128 = dead-zone removal at
qindex ≤112 — the un-A/B'd rounding surface already flagged in zenrav1e#30's
item-1 verdict), the zbin factor, and the skip-vs-code RD pricing itself. A
rounding-bias probe is mechanically small in zenrav1e's quantizer but is a
global coefficient-level constant under the tune — it needs the full
tune-program A/B discipline (train26 both metrics + veto + conformance), and
def reaches no-skip WITHOUT the sharpness rounding, so rounding alone cannot be
the whole mechanism. **Verdict: documented, handed to the coefficient-level RD
program (the s1/s2 program's own named residual), first probe = the
sharpness-rounding/dead-zone A/B; not chunk-fixable inside P3 without forcing.**

Record: label-store mining + ladders in this section; inspect JSONs + byte-matched
streams at `/tmp`-scratch (reproducible: `scripts/rd_gap/inspect_diff.sh`
methodology, aom build_slow cq16 {iq,def} vs rav1e_p3bc q70 tune-ss2). The plan's
P3 bullet and zenrav1e#30 carry the handoff.

## Size-decay isolation A/B — MEASURED 2026-07-03: four of five tune mechanisms ACQUITTED for the small-size decay; the QM-dist ratio convicted and its size ramp SHIPPED

**Program**: WEDGE_MAP wedge #3 / HYPERPARAM_FIRST_CUT rule 2's specified follow-up. The wedge
measured the shipped config's advantage vs libaom cpu2 decaying monotonically below 1024
(−13.0 med @1024 → −6.5 @512 → −1.15 @256, entirely high-q-band at the first step), and the
ranked suspects were the Tune::Ssimulacra2 constants — all fit at 1024 only, led by the ss2 QM
level curves. This A/B isolates each mechanism per size with leave-one-out arms.

**Method**: dev workspace `zenrav1e--sizedecay` (dev arms preserved as workspace commit
`1428ecdd` on master `c9c2d5f7`) adds `ZENRAV1E_SD_DISABLE=<mech>` gates for the five tune
mechanisms (chromadq, qmcurves, boost, qmdist, lfsharp) + `ZENRAV1E_SD_RAMP=<mech>:<m256>`
long-edge strength-ramp trial arms (m = clamp((log2(maxdim)−8)/2, m256, 1.0); LONG EDGE, not
px area, so non-square 1024-class renditions keep m=1.0). Byte gates: env-unset == master
binary md5 (18/18 cells, local + box); every disable gate live (45/45); ramp m=1 == full-tune
md5; ramp m256=0 @256 == disable md5. Arms: {full, off, no_×5} × 12 photo-like TRAIN wedge
origins (plots excluded) × {256, 512, 1024|native} × 16-q grid (12-pt + 78/82/88/92 — the
decay lives in the high-q band), BUTTER on, cavif s2 depth 8, palette auto constant across
arms. Box zenavif-sweep-1 (snapshot restore), 252/252 train file-arms clean. Held-out: {full,
off} + fresh cpu2 refs on 12 VAL-LSD origins (palette-val corpus renditions). Decision rule
PRE-REGISTERED before any arm data (raw dir `DECISION_RULE.md`): convict X iff c(1024) ≤ −1.0
AND (c(256)−c(1024) ≥ +2.0 or c(256) ≥ +0.3), where c = median BD(full vs no_X). Full record:
`benchmarks/hyperparam_size_decay_ab_2026-07-03.tsv`; raw sweeps
`/mnt/v/output/zenavif/sizedecay-2026-07-03/` (Tower-mirrored).

### The attribution table (TRAIN, medians)

Mechanism contribution c(size) = direct ssim2 BD of full-tune vs tune-minus-mechanism
(negative = the mechanism saves bits at that size):

| mechanism | c(1024) | c(512) | c(256) | high-q band 1024→256 | verdict |
|---|--:|--:|--:|---|---|
| chroma delta-q | −2.23 | −2.67 | −3.17 | −0.93 → −3.15 (grows) | ACQUITTED — win GROWS toward small |
| **ss2 QM curves** | **−8.81** | **−8.28** | **−7.23** | −2.82 → −4.59 (grows) | **ACQUITTED — the top suspect keeps ≥82% of its win at 256** |
| variance boost | +0.24 | +0.29 | −0.86 | mixed | not convictable (fails the c(1024) ≤ −1.0 floor on this photo-like subset; helps only at 256) |
| **QM-dist ratio** | **−3.48** | **−2.13** | **−0.96** | **−0.90 → +0.35 → +0.53 (flips positive ≤512)** | **CONVICTED: decay +2.52 ≥ +2.0 on 8/12 origins; the only mechanism with a consistent positive decay slope (med +0.60 / mean +0.51)** |
| LF sharpness | −0.66 | −0.29 | −0.38 | flat | not convictable (fingerprint below the 1.0 floor) |

Butteraugli sharpens the qmdist story: at 256 its marginal is ssim2 −0.96 but butteraugli
ba3n +0.45 / bamax +1.33 (**the mechanism costs butteraugli exactly where its ssim2 win is
thinnest**); at 512/1024 both metrics favor it. Per-origin, the decay concentrates on the
wedge's steepest photo-like decayers (9098 +7.9, 1238 +5.0, 1480 +4.6, 9118 +3.8 BD points
lost 1024→256; 9098/9118 flip outright positive at 256), while doc/synthetic content
(6070/5318/9908) anti-decays.

### Most of the wedge decay is NOT the tune: vs-cpu2 decomposition

| | TRAIN full | TRAIN off | TRAIN delta | VAL full | VAL off | VAL delta |
|---|--:|--:|--:|--:|--:|--:|
| 1024 | −14.83 | −1.13 | −13.70 | −1.42 | +0.56 | −1.98 |
| 512 | −10.18 | +0.81 | −10.99 | +0.96 | +5.56 | −4.60 |
| 256 | −1.98 | +4.17 | −6.15 | +7.90 | +12.72 | −4.83 |

The tune-OFF baseline itself loses ~5.3 (train) / ~12.2 (val) BD points to cpu2 from
1024→256 — decay that no tune-constant scaling can fix. Within-zr the tune's total win
holds at every size (train −14.38 → −13.18 → −10.42, 12/12 better each; val −7.42 → −7.25 →
−5.61), and on VAL the tune's vs-cpu2 delta actually GROWS toward small (−1.98 → −4.83).
(The val corpus is screen-heavier — palette-program picks — which explains its weaker
absolute position; the decay SHAPE reproduces.)

### The qmdist strength ramp — inverted-U, m256=0.5 SHIPS (zenrav1e@b0098eb1)

Trials m(long edge) = clamp((log2(maxdim)−8)/2, m256, 1.0), BD(full vs ramp), positive =
ramp wins:

| arm | 256 | 512 | butteraugli 256 (3n/max) |
|---|--:|--:|---|
| m256 = 0 (ratio off at 256) | −0.96 (3/12) | +0.87 | +0.45 / +1.33 |
| m256 = 0.25 | +0.68 (10/12) | +0.87 | +1.67 / +3.00 |
| **m256 = 0.5** | **+1.03 (11/12)** | **+0.87 (9/12)** | **+1.94 / +3.32** |

The response is non-monotone: HALF strength beats BOTH full strength and off at small sizes
— the 1024-fitted ratio overshoots on downscaled content but still carries signal.
**VAL confirms the winner: +1.12 @256 (11/12 better), +1.00 @512, butteraugli agreeing
(+1.7/+2.9 @256).** Ship bar (pre-registered): ssim2 median ≥ +0.3 at both convicted sizes,
butteraugli veto, val confirm, byte-identity at 1024 — all pass; m256=0.5 beats 0.25 by
> 0.3 so no tie-break needed.

**Landed as `zenrav1e@b0098eb1`** (master): `qm_dist_ratio_m = clamp((log2(long_edge) −
8)/2, 0.5, 1.0)`, exact u128 path at m=1.0. Gates: tune-off 9/9 byte-identical to master;
tune-on @1024 == master md5; the landing binary reproduces the measured trial arm md5 on
9/9 cells (256/512/1024 × Q{30,60,85}); **conformance 180/180** (36-file size ladder ×
Q{30,50,60,75,90}, aomdec decode + aomdec/rav1d-safe raw-md5 agreement — the change is
RDO-only, no header/syntax change). 170 lib tests (3 new), clippy clean. Release-gated like
the rest of the tune: registry builds get it after the next zenrav1e release + dep bump.

### Consequences

- **The wedge #3 owner hypothesis ("re-fit the tune constants below 1024") is refuted for
  4 of 5 mechanisms** — their leave-one-out contributions show weakening them at small
  sizes LOSES bits.
- The remaining ≤512 vs-cpu2 decay lives in non-tune coding behavior (and cpu2's own
  small-size strength): partition/coding defaults at small px are the revised suspect.
- Boost's inverse profile (+0.24 @1024 / −0.86 @256 on photo-like content, inside noise)
  is a possible future inverse-conditional — noted, not actioned.
- The dev leave-one-out + ramp trial arms remain available as zenrav1e workspace commit
  `1428ecdd` for any future per-mechanism size program.

## Non-tune size-decay isolation A/B — MEASURED 2026-07-03: NO coding default convicted; the decay is zr's adaptive layer FADING on downscaled content; two conformance bugs found (one fixed upstream); the rdotx small-px lever landed byte-neutral pending sign-off

**Program**: the specified follow-up to the section above — the tune-OFF (Psychovisual)
baseline owns most of the small-rendition decay vs aomenc cpu2 (train −1.13 → +0.81 →
+4.17, val +0.56 → +5.56 → +12.72 median BD at 1024→512→256). These arms isolate the
DEFAULT coding-path suspects one at a time, unconditional at all q, tune OFF + palette
auto constant: the quality-keyed ravif SpeedTweaks gates (partition (4,16)@hi-q cap,
rdo_tx off at hi-q, CDEF+LRF off above ~Q50, Complex segmentation), chroma 444-vs-420,
Tune::Psnr (the whole activity/psy layer), and composites.

**Method**: driver `scripts/rd_gap/sizedecay_nontune_arms.sh` (ZENRAVIF_SD2_* dev
passthroughs in the ravif--wedge clone; env-unset verified byte-identical to the
sizedecay `off` arm on ALL 576 train cells — bytes exact, ssim2 exact, butteraugli
≤1e-6), 12 photo-like TRAIN wedge origins × {256,512,1024|native} × 16-q grid, BUTTER
on, per-armed-cell aomdec + rav1d-safe raw-md5 conformance (PALCONF; zero failures on
every kept arm). Decision rule PRE-REGISTERED before any arm data
(`/mnt/v/output/zenavif/sizedecay-nontune-2026-07-03/DECISION_RULE.md`, with a
post-registration deviations note): convict arm X iff w(256) ≥ +1.0 AND (w(256)−w(1024)
≥ +1.0 OR w(1024) ≤ +0.3), butteraugli veto; w = median BD(base vs arm), positive = the
arm saves bits. Analyzer: `scripts/hyperparam/analyze_sizedecay_nontune.py`; summary
TSV `benchmarks/hyperparam_sizedecay_nontune_2026-07-03.tsv`; raws Tower-mirrored.

### Pre-A/B structural evidence (aom `inspect --all` at byte-matched cells)

- 1480 nature @256, zr q85 vs aom-420 cq32 (±3% bytes): aom puts 36.5% of area in ≥32×32
  blocks (25.0% BLOCK_64X64 riding TX_64X64) where zr has 0% — the (4,16) hi-q cap
  binding (zr baseQIndex 36 < 80 ⇒ `high_quality`). zr pays 517b of segment_ids (0.9% of
  the file) vs aom 0; aom spends MORE on tx-type/tx-size/filter-intra signaling.
- Same image @1024: aom's ≥32-block share is >50% vs zr 0% — the cap binds at 1024 just
  as hard, yet zr holds −2.56 BD there. zr's coefficient-side strengths (psy-RDO
  allocation + segmentation AQ) mask the block-structure deficit at 1024 and fade at 256.
- 5343 doc-scan @256 (worst val decayer): aom 48% BLOCK_64X64 + 64% TX_32X32 area; zr 74%
  BLOCK_16X16/TX_16X16. Nearly equal coefficient bit pools (13.8kb vs 14.0kb) but aom
  rides 32×32 transforms to ssim2 92.8 vs zr 92.0 at −10% bytes.
- Screen tools are NOT the synthetic-content story at 256: BOTH encoders' AA-aware screen
  detection stays silent on downscaled renditions (zr palette auto==off byte-count-equal;
  aom `--enable-palette=0 --enable-intrabc=0` byte-identical).
- aom cpu0 headroom at 256 is modest: cpu0-default beats cpu2 by only −3.55 median BD
  (12 train origins; the win concentrates low-q: −9.1% vs −3.2% band medians) — cpu2 is
  a fair small-px target, not a strawman.

### The arms (train; w = median BD(base vs arm), positive = arm saves bits)

| arm | 256 | 512 | 1024 | verdict |
|---|--:|--:|--:|---|
| prange432 (hi-q 16→32 cap lift) | −0.32 | −0.49 | −0.26 | not convicted — loses everywhere ALONE (butteraugli −1.7..−2.2 agrees): without tx RDO the extra 32-blocks are mispriced. The clamp is not independently size-hostile |
| rdotx (tx RDO also at hi-q) | **+0.80** | **+0.88** | +0.70 | not convicted (uniform: 35/35 images better, butteraugli +1.3..+2.5) — the known matched-speed tradeoff. Its WIN is size-flat; its COST is size-conditional (~6.5× the changed hi-q cells: 0.3→2.0s @256, 1.2→7.6s @512, 4.6→32s @1024) |
| cdef (on above ~Q50) | −0.05 | +0.05 | +0.05 | not convicted — dead zero; ravif's gate is right |
| lrf (on above ~Q50) | −0.13 | −0.24 | −0.25 | not convicted — mild loss everywhere; gate right |
| segoff (drop Complex segmentation) | −1.83 | −3.00 | −4.12 | not convicted (removal loses everywhere) — but the VALUE FADES: 4.12@1024 → 3.00@512 → 1.83@256, strongest at low-q (−6.49 → −2.02) |
| psnr (Tune::Psnr vs the Psychovisual base) | −4.42 | −6.15 | −7.52 | not convicted (removal loses everywhere) — the whole activity layer's value FADES: 7.52@1024 → 4.42@256 (−3.10). Note Tune::Psnr also drops the activity-scored segmentation deltas, so this fade SUBSUMES most of segoff's — overlapping, not additive |
| combo32 (prange432+rdotx+cdef+lrf) | +0.23 | +0.09 | +0.23 | not convicted — underperforms rdotx alone (prange432 drags); mean ≫ median (a few smooth-gradient 9908-class images love 32-blocks) |
| combo64 ((4,64)+rdotx+cdef+lrf, post-#34 fix) | +0.01 | +0.17 | +0.11 | not convicted — the 64-block widening washes out even WITH tx RDO (medians ~0; butteraugli +2.1/+1.1 mildly positive; mean ≫ median again — outlier smooth-gradient images). rdotx ALONE dominates it at every size. 0 conformance failures across 576 cells = live validation of the #34 fix |
| yuv420 | — | — | — | NO DATA: every cell failed the aomdec gate → **zenavif#29** (ravif 4:2:0 emits non-conformant AV1 on registry AND master; rav1d-safe masks it — zenavif round-trips never noticed). Do not ship/benchmark 420 until fixed |
| prange464 alone | — | — | — | DROPPED mid-arm: 100% DECFAIL at q≥78 → **zenrav1e#34**, root-caused by bisect + probes and **FIXED upstream (`1dabba91`)**: the 3fa735dc sliver TX cap is decoder-followable only under TX_MODE_SELECT; with rdo_tx off (TX_MODE_LARGEST) decoders derive the uncapped TX_64X16/16X64 → guaranteed desync. Latent since 7d254289; also reachable via stock zenrav1e speeds 6-8 on intra frames. Intra 64-parent 4-ways now require `tx_mode_select` + hard asserts at both cap sites; 6/6 corrupt shapes verified clean, byte-identical at shipped configs, 170 lib tests |

### The verdict: what the small-px decay actually is

1. **No single coding default is size-hostile** under the pre-registered rule. The
   quality-keyed ravif gates are individually correct (cdef/lrf/prange) or uniformly
   valuable (rdotx).
2. **zr's content-adaptive layer fades on downscaled content.** The Psychovisual
   activity-masked metric + activity-scored segmentation AQ are worth 7.5 BD at 1024
   and only 4.4 at 256 (psnr arm); segmentation alone 4.1 → 1.8 (segoff arm;
   overlapping subsets of the same layer). Lanczos downscaling compresses the local
   activity range, so masking/AQ has less differentiation to exploit — while aom's
   uniform tools (keyframe coeff-opt trellis `rd_sf.perform_coeff_opt=2`,
   av1/encoder/speed_features.c:383/415 @632172a4; full intra tx-size search;
   filter_intra with pruning, speed_features.c:431; resolution-keyed speed features,
   `set_good_speed_feature_framesize_dependent` speed_features.c:711) hold their value.
   The RELATIVE position therefore decays even though nothing on zr's side "breaks."
   Re-calibrating the activity/segmentation machinery for small/dense renditions is the
   honest follow-up program (feature-hints: px is a free input).
3. **The one ship-bar-passing lever: rdotx below 1024** (uniform win, size-trivial cost
   at ≤512). TRAIN +0.80 @256 / +0.88 @512 (12/12 at both), butteraugli +2.5/+1.7;
   **VAL CONFIRMED +1.44 @256 / +1.30 @512 (12/12 at both, butteraugli +3.7/+2.9,
   0 conformance failures)** — the screen-heavy val corpus benefits ~2× train.
   vs-cpu2 medians: train 256 +4.17 → **+3.31**, 512 +0.81 → **−0.46** (flips to a
   win); val 256 +12.72 → **+9.22**, 512 +5.56 → **+4.04**. Landed byte-neutral as
   `ravif@bae4880` (`SMALL_PX_RDO_TX_LIVE=false`, from_my_preset now takes the long
   edge): NOT flipped live because the pre-registered ship rule covers convicted arms
   only and this is a policy enable (matched-effort normalization — aom's own
   resolution-keyed philosophy), not a conviction. Flip = 1 const + the zenavif
   encode_plan.rs mirror update; interaction with Tune::Ssimulacra2 unmeasured (measure
   at flip time).

### Consequences for the wedge map

- Wedge #3's residual (≤512 vs cpu2) is now FULLY attributed: tune constants (ramp
  shipped), the fading adaptive layer (measured, future re-calibration program), the
  rdotx effort lever (landed, pending flip), and aom's uniform toolset (coeff-opt/
  filter-intra — the documented implementable gaps).
- The val screen-heavy decay (5343/6091/9021-class) is NOT screen tools (both encoders'
  detection is dead at ≤512) — it is generic: TX_32X32 energy compaction on 64-blocks +
  trellis. Palette/intraBC remain native-scale-only levers (wedge #6).
- combo64's 1024-class result (median +0.11, mean +1.5 — outlier-driven by 9908-class smooth gradients) belongs to the LARGE-px s2 program (the
  1024 byte-identity gate excludes it from the small-px ship anyway); it revisits the
  ruled-out prange-(4,64) with the tx-RDO pairing the 2026-07-02 retest lacked.

**Full record**: `benchmarks/hyperparam_sizedecay_nontune_2026-07-03.tsv` (per-image
BDs), raws + decision rule + bug repros at
`/mnt/v/output/zenavif/sizedecay-nontune-2026-07-03/` (Tower-mirrored), label store
sweep_source `sizedecay-nontune-2026-07-03`.

## Confirmed findings (2026-07-01 audit — supersedes the "verify" framing below)

Source-read + measured, not hypothesized. See `scripts/rd_gap/palette_ablation.sh` +
`tool_ablation.sh` and `benchmarks/rd_gap_{palette,tool}_ablation_2026-07-01.tsv` for the harness
and raw numbers.

1. **Palette mode is 100% unimplemented in zenrav1e's encoder — not "early-terminated," never
   available at any speed.** *(2026-07-03: superseded — implemented and measured, see the
   status block above.)* `write_use_palette_mode` (`src/context/block_unit.rs:777`) is always
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
5. **Update 2026-07-01: two real search-completeness bugs found and fixed (topdown partition,
   tx-type early-exit), cutting the median BD-rate gap from +5.7% to +2.1% (~63% relative).**
   Palette (~0% on photos), CDEF (noise), loop restoration (noise), tx-type *coverage*
   (spec-complete), CfL search depth (SSE-only, widening doesn't help), large-block
   `partition_range` widening (RD-cost estimate at 32x32/64x64 not reliable enough to trust with
   a wider range — regresses), and filter_intra (severe pre-existing regression, unresolved) are
   all ruled out or blocked as further levers.
   Extended partition types remain the single largest still-open lever (10-13% area share) if
   its conformance bug gets resolved (zenrav1e#26). Beyond that: likely an aggregate of many
   small RDO/heuristic refinements accumulated in libaom over years (coefficient cost estimation
   precision, rate-control qindex mapping, mode-search ordering, etc.) rather than one remaining
   missing feature — harder to close than a single lever, and harder to measure via simple flag
   ablation since there's no single flag to toggle.

## Credible narrowing levers (priority order, updated 2026-07-01)

1. ~~Fix `encode_partition_topdown`'s hardcoded HORZ/VERT exclusion~~ **DONE** — see "Fixed
   2026-07-01" above. −1.8 to −2.8% median bpp at ssim2 70-85, BD-rate gap +5.7%→+3.6% median
   (narrower methodology; see caveat above). Unreleased.
1b. ~~Fix `rdo_tx_type_decision`'s first-iteration early-exit~~ **DONE** — see "Fixed 2026-07-01
   (3)" above. −0.3 to −1.2% median bpp at ssim2 70-82, BD-rate gap +3.6%→+2.1% median on top of
   the topdown+deblock fixes. Unreleased. (Distinct from the "tx-type completeness" finding below
   — that's about `RAV1E_TX_TYPES`' *coverage* of the spec's tx-type set, already spec-complete;
   this is about the RDO *search* over that set being cut short too early.)
2. **Implement the 6 extended partition types** (`HORZ_A/B`, `VERT_A/B`, `HORZ_4`/`VERT_4` — see
   "STILL OPEN" above). **DONE for HORZ_4/VERT_4, 2026-07-01** (2 of 6): the "unresolved
   bitstream-conformance bug" from the first prototype attempt turned out to be a real,
   fixable `BlockSize` ordinal-vs-dimension mismatch (see "Fixed 2026-07-01 (4)" above) — not
   a fundamental blocker. Fixed + implemented; median BD-rate gap +2.1%→+0.1%.
   [zenrav1e#26](https://github.com/imazen/zenrav1e/issues/26) closed. `HORZ_A/B`/`VERT_A/B`
   (the mixed 3-way types, the other 4) remain **unimplemented** — would need a new tracking
   issue if picked up; their area-share contribution is unquantified (overlaps plain HORZ/VERT
   sizes in a block-size histogram, unlike HORZ_4/VERT_4's own distinct sizes).
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
3c. ~~Widen ravif's `partition_range` speed-2 heuristic to unlock BLOCK_32X32/64X64~~ **RULED
   OUT, 2026-07-01.** See "RULED OUT... BLOCK_32X32/BLOCK_64X64" section above for the full
   root-cause + measurement. Explains (doesn't fix) the 0%-large-block inspect-diff anomaly:
   ravif's `SpeedTweaks::from_my_preset` caps `partition_range=(4,16)` at speed 2 for
   non-low-quality targets, a deliberate 2021/2022 heuristic, not a bug. Widening it to `(4,
   64.min(max_block_size))` regressed: +1.4-1.8% median bpp at ~0 ssim2 change in the affected
   quality band (-Q 50-75), 24-36% slower encode, 17/19 photos worse at -Q 60 (not a mixed bag).
   See `benchmarks/rd_gap_partitionrange_widen_2026-07-01.tsv`. Root cause: same shape as CfL
   widening — `rdo_partition_none`'s RD-cost estimate at large sizes isn't reliable enough to
   trust with a wider candidate range on this corpus. **Reverted, not on master.**
   **RE-TEST 2026-07-02 (on the fixed SPLIT estimate): first run exposed a latent corruption
   bug on master — root-caused, FIXED (`zenrav1e@3fa735dc`), re-test pending re-run.** At
   `partition_range` (4,64) the −Q 50-75 band (exactly where the widening activates) produced
   bitstreams BOTH `aomdec` ("Corrupted segment_ids") and rav1d-safe reject; 46/264 sweep
   cells failed and the harness silently dropped the rows (`run_gap.sh` needs a loud-failure
   fix). Initial attribution to `b073182c`'s deeper SPLIT estimate was **wrong** — a
   six-variant mechanism bisect exonerated it (corruption survives disabling the estimate
   entirely). True root cause: HORZ_4/VERT_4 at BLOCK_64X64 parents emit BLOCK_64X16/16X64
   slivers whose max transforms TX_64X16/TX_16X64 are dead code upstream and desync when
   coded. Fix: intra slivers cap to TX_32X16/16X32 + the tx-size RDO walk shrinks by the
   consumed level (else it writes an out-of-alphabet depth-3 symbol — a second corruption
   found during the fix; that depth bound is now a hard assert in all builds) + inter frames
   without `enable_inter_txfm_split` don't offer 64-parent 4-way candidates. Byte-identical
   at shipped configs; 16/16 previously-failing cells clean on both decoders. Validating the
   real 64-dim sliver transforms: zenrav1e#28.
   **Clean re-test verdict (same day): the RULING STANDS.** 264/264 cells, zero failures
   (the sweep doubles as full-corpus validation of the corruption fix). Direct isolation
   **+0.4839% median / +0.3249% mean, worse on 15/22**; vs cpu2 the median gives back parity
   (−0.6487% → +0.4015%). The estimate fix did help (2026-07-01 had 17/19 worse and no big
   winners; now 7/22 win, three at −1.8..−2.5%) but large-block NONE cost estimation remains
   untrustworthy in aggregate. Reverted again; NOT an s2 candidate. The s1 mode must ablate
   its own partition_range choice rather than assume (4,64). The win/loss split marks this
   as a future content-adaptive gate candidate (zenanalyze feature-hints track). Data:
   `benchmarks/rd_gap_prange_retest_2026-07-02.tsv`.
4. **Implement palette mode in zenrav1e** (scoped: screen-content win, ~zero photo-gap effect —
   see confirmed findings above). Substantial encoder feature: color quantization (k-means per
   candidate block/plane), RD-gated size search (2-8), bitstream signaling (palette colors + index
   map), `CdfContext` wiring for the already-spec-ported default CDF tables. rav1d-safe (zenavif's
   decoder) already supports palette decode (`ipred.rs`, `recon.rs`, `safe_simd/pal.rs`), so no
   decoder-side blocker for round-trip validation. Cross-repo work (zenrav1e) — needs explicit
   scope sign-off before starting. **Only worth it if screen content is part of the target traffic
   — it will not move the photo number.**
5. ~~Perceptual-tune parity~~ **CONFIRMED, 2026-07-01 — real, large, already active; no further
   action.** "Psy-tune" = `Tune::Psychovisual` (default, `encoder.rs:109-117`), which routes RDO
   distortion through an SSIM-derived activity mask (`apply_ssim_boost`, `activity.rs:159-186`)
   instead of plain SSE. Confirmed active in this project's build config (not dead code) and
   orthogonal to `with_qm` (independent mechanism, not a duplicate). A/B'd against `Tune::Psnr`
   (SSE-only) at matched ssim2 on the same 19-photo corpus: Psnr needs **+9.46% median more
   bits** (mean +8.42%), positive at essentially every ssim2 target 70-92 — a real, large,
   consistently-signed win, not a paper tiger. Since Psychovisual is already the active default
   in every measurement this session (including the current +2.1% median BD-rate gap), this
   finding closes the lever without a code change — there's no additional headroom to capture
   from "fixing" psy-tune, it's already working as intended. Also retroactively confirms the
   earlier "VAQ rejected, psy-tune already covers it" entry with a direct measurement instead of
   an assumption. See `benchmarks/rd_gap_psytune_verify_2026-07-01.tsv`. Diagnostic-only edit,
   reverted, never landed.
6. **Broader photo-gap root-causing** if 2-5 don't close it: per-block mode-decision diffing
   between aomenc and zenrav1e on shared test images (now tooled via `inspect_diff.sh` — extend to
   mode-level/RD-level instrumentation, not just block-size histograms), since the remaining
   residual after all of the above may be diffuse (many small refinements) rather than localized
   to one tool. **2026-07-01: aggregated the bit-cost breakdown across 8 photos (up from 3) — this
   found the `rdo_tx_decision` speed-gate below, no other single element stood out beyond what's
   already fixed/ruled out.**
6b. **Found: `rdo_tx_decision`'s `!high_quality` gate disables ALL tx-size/type RDO above -Q ~78
   — real win, but not adopted (speed/matched-comparison tradeoff, user call).** Distinct from
   the already-fixed tx-type first-iteration early-exit (that was premature abandonment *within*
   a running search; this is a switch that prevents the search from starting at all, for the
   upper third of this project's tested -Q range). Tested removing the gate: **median -5.7%
   bytes (mean -8.8%) AND slightly better ssim2 simultaneously** at -Q 80-95 (n=19 photos per
   point) — a dominant improvement, not a quality-for-size trade. But it costs **~7.5x more
   encode time** in that band (measured 14-23s vs 2-3s per photo), which exceeds libaom's own
   cpu-used=2 reference time on a comparable image (~8.5s measured directly) — adopting it as the
   default would mean the BD-rate comparison is no longer at matched speed, undermining the
   premise the whole investigation is built on. Profiled (perf, debug symbols): ~95%+ of the
   added time is legitimate RD-search cost (`encode_tx_block` transform+quantize+entropy-cost
   estimate per candidate, `quantize_with_qm`, coefficient-context modeling) — not waste, no
   inefficiency to fix. Tile-based multi-threading doesn't offer a free win either: at this image
   size/quality, `min_tile_size` doubling at `high_quality` makes the tile-count formula
   `threads.min((w*h)/min_tile_size²)` compute to 0 extra tiles — already single-tile/single-
   thread by design (forcing more tiles trades compression efficiency for speed, a different
   cost). A cheap pre-screening heuristic to prune RD candidates before full costing could reduce
   the time cost, but risks reintroducing exactly the premature-pruning bug class already found
   and fixed twice this session — not attempted without measuring it in isolation first, and not
   built speculatively. **Decision (user, 2026-07-01): leave the default unchanged** — the
   existing public `with_rdo_tx_decision(Option<bool>)` builder already gives users an explicit
   opt-in for this quality/speed tradeoff; no new API needed. Diagnostic edit reverted, not
   landed. Does not move the measured BD-rate number. See
   `benchmarks/rd_gap_txdecision_2026-07-01.tsv`.

**Explicitly NOT on the list:** deltaq/VAQ/trellis (rejected), EPT/NSDT (video-only gains, don't
transfer to stills — `AVIF_LEARNINGS §1`), learning-based intra / GPU search (out of scope for
zenrav1e's design), CDEF/restoration tuning (ruled out 2026-07-01, see confirmed findings).

## Honest final status (2026-07-02 — TRUE RD PARITY REACHED)

**Measured: −0.6487% median BD-rate (+0.2373% mean) vs libaom-slow, photos only, matched speed
(cpu-used=2, 1.057× median encode time). Started at +5.7% median / +7.5% mean. The ≤0% median
(true parity) target is MET.** The final lever was the trial-SPLIT-cost accuracy fix ("Fixed
2026-07-02" above), which the Phase 2 postmortem identified — its regression's root cause
turned out to be the last systematic search defect. **Every one of AV1's 10 partition types has
been genuinely attempted**: 6 are live on master (`NONE`, `HORZ`, `VERT`, `SPLIT`, `HORZ_4`,
`VERT_4`); the other 4 (`HORZ_A/B`, `VERT_A/B`) were implemented, verified conformance-clean at
the full 110-cell bar, measured as a net RD regression *under the then-biased SPLIT estimate*
(median +0.1%→+0.6%, ~1.46× encode time), and reverted — see "TRIED AND REVERTED 2026-07-01"
above. With the SPLIT estimate fixed, a Phase 2 re-attempt from the preserved implementation
(`a7630aee`) is unblocked and is the most promising follow-up for pushing the mean down further.

**Fixed and landed (zenrav1e `master`, unreleased):**
- Pessimistic SPLIT cost estimate in the topdown partition trial (b073182c) — **the parity
  crosser**: median BD-rate +0.0695%→−0.6487%, mean +2.1734%→+0.2373%, at 1.057× median
  encode time. See "Fixed 2026-07-02" above.
- `encode_partition_topdown` never offered `PARTITION_HORZ`/`VERT` (665e58e4)
- `sse_h_edge` passed the wrong axis to `deblock_size` (dc0a1165)
- `rdo_tx_type_decision`'s first-iteration early-exit (6b3b0493)
- `BlockSize` ordinal-vs-dimension mismatch in `angle_delta`/palette gates, blocking
  `PARTITION_HORZ_4`/`VERT_4` (2866397e) + Phase 1 of extended partition types
  re-implemented on top (7d254289) — see "Fixed 2026-07-01 (4)" above. Median BD-rate
  +2.1%→+0.1%.

**Landed, cosmetic/enabling (ravif `main`):**
- Widened speed-2 `non_square_partition_max_threshold` to `BLOCK_64X64` (b4853c68) — required by
  the topdown fix to have any effect.

**Ruled out with measured evidence (tried, regressed or no effect, reverted):**
- CfL alpha-search widening (SSE-only cost model, not RD-aware)
- `partition_range` widening to unlock `BLOCK_32X32`/`64X64` (regresses — same RD-cost-model
  accuracy limitation)
- `rdo_tx_depth` widening past 2 (hits a normative AV1 spec limit, not a heuristic)
- filter_intra enablement (re-confirmed zenrav1e#5's severe pre-existing regression is still
  live)

**Confirmed real and already fully captured (no code change, no headroom):**
- Perceptual tune (`Tune::Psychovisual`) — verified a genuine ~9.5% median BD-rate win over
  plain SSE-RDO, already the active default in every measurement.

**Found real, deliberately not adopted (user decision — preserves matched-speed comparison):**
- `rdo_tx_decision`'s `!high_quality` gate — real -5.7%/+quality win at -Q80-95, but costs ~7.5x
  encode time, pushing zenrav1e past libaom's own reference speed in that band. Available via
  the existing `with_rdo_tx_decision` opt-in for users who want it; not the default.

**Resolved this session (was "Blocked" as of the prior snapshot):**
- Extended partition types, `HORZ_4`/`VERT_4` (2 of the 6) — the earlier "unresolved
  bitstream-conformance bug" was a `BlockSize` ordinal-vs-dimension mismatch in the
  `angle_delta`/palette gates (see "Fixed 2026-07-01 (4)" above). Fixed + implemented;
  median BD-rate gap +2.1%→+0.1%.
  [zenrav1e#26](https://github.com/imazen/zenrav1e/issues/26) closed.

**Tried and reverted (measured net regression, not landed):**
- Extended partition types Phase 2, `HORZ_A/B`/`VERT_A/B`
  ([zenrav1e#27](https://github.com/imazen/zenrav1e/issues/27)) — implemented, 110/110-cell
  conformance-clean, all 4 types genuinely chosen by the search, but a net RD regression
  (direct isolation median +0.60%; vs libaom +0.1%→+0.6% median, worse on 12/19 images) at
  ~1.46× encode time. Root cause of the regression is a SPLIT-cost-estimation bias in the
  one-level topdown trial, not a defect in the new types — see "TRIED AND REVERTED
  2026-07-01" above. Two real bugs found during the attempt are preserved with the full
  implementation in workspace commit `a7630aee` for any future re-attempt.

**Explicitly out of scope (photos-only goal):**
- Palette mode — ~0% effect on photos, a real win only for screen content.

**How the last 0.1pp fell** *(historical note — the two paragraphs below described the state
before the 2026-07-02 SPLIT-cost fix)*: the "diffuse residual" assessment held for everything
EXCEPT one item the Phase 2 postmortem surfaced: the topdown trial's pessimistic SPLIT estimate
was a genuine, discrete, fixable defect after all — the fifth and final one. The "broader
RDO-cost-model accuracy project" predicted below turned out to have a small, contained first
step (the one-level-deeper SPLIT child estimate, ~172 lines in `rdo.rs`) that alone crossed
parity at 1.057× encode time. The prediction that it "would simultaneously make Phase 2's types
viable and improve every existing partition decision" was half-validated immediately (every
existing partition decision improved — that's the −0.72pp median swing); the Phase 2-viability
half remains untested (`a7630aee` re-attempt).

**What remains beyond parity (updated 2026-07-02 after the s1 ship):**
1. ~~Phase 2 re-attempt~~ / ~~rdo_tx high-quality gate~~ — **both SHIPPED in the `-s1` deep
   mode** (see "s1 deep mode" above): mixed 3-way types + unconditional tx RDO +
   partition_range (4,32), median −0.97% vs libaom cpu0 / −3.01% vs cpu2.
2. **The 8 remaining per-image cpu0 losers** (o_6629 +25.3 worst) — partition-type/depth
   levers are exhausted (all 10 types + depth-2 trials measured); the residual is
   coefficient-level RD (trellis-class optimization, large-block cost-model precision).
3. **Per-image s1 knob selection via the picker** — depth-2 and prange-64 each win on
   content the shipped config loses (o_5004 +11.1→+3.6 with 64+depth2; o_6629 +25.3→+13.1
   with 64) — an oracle picker would rescue 1 more image and large fractions of 4 others.
4. **CfL RD-aware alpha search** — unchanged from before (1-2pp bound).

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
drive down. **Target: true RD parity (median BD-rate ≤ 0% vs libaom-slow on the photo corpus,
matched speed) — MET as of 2026-07-02.** Current: median **−0.65%** (mean +0.24%), down from
+5.7%/+7.5% at the start of the investigation. Any future zenrav1e change should keep the median
at or below 0% — treat a re-run above 0% as a regression. See "Fixed 2026-07-02" and "Honest
final status" above for what's landed, ruled out, and optional beyond-parity follow-ups.

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
