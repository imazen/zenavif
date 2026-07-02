# zenrav1e RD gap vs libaom-at-slow — measurement + narrowing plan

**Status: TRUE RD PARITY REACHED, 2026-07-02** — median BD-rate vs libaom-slow improved
**+5.7% → −0.65%** (mean +7.5%→+0.24%), crossing the ≤0% parity target at matched speed
(1.057× median encode time). The final lever was the trial-SPLIT-cost accuracy fix ("Fixed
2026-07-02" below), on top of the four 2026-07-01 bug fixes. Re-run `scripts/rd_gap/` after
any future zenrav1e change to check for regressions.

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
   real 64-dim sliver transforms: zenrav1e#28. The clean (4,64) RD re-test runs next.
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

**What remains beyond parity (optional follow-ups, in expected-value order):**
1. **Phase 2 re-attempt on the fixed SPLIT estimate** — `HORZ_A/B`/`VERT_A/B` from `a7630aee`,
   re-measured against the new baseline (zenrav1e#27). Most promising path to pushing the
   **mean** (+0.24%) below zero too.
2. **`rdo_tx_decision` high-quality gate** — still a real, large win at -Q80-95, still costs
   ~7.5× encode time in that band; remains a user scope/priority call via the existing
   `with_rdo_tx_decision` opt-in.
3. **CfL RD-aware alpha search** — the CfL-widening experiment's root cause (SSE-only search)
   is the same cost-model-accuracy family; a signaling-cost-aware search might recover the
   1-2pp the ablation bounded.

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
