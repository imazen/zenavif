# Test coverage map

Measured 2026-08-11 on `svtav1-rs-backend` (base `77dc2d0`, final numbers at
`8795864`), Apple M4 Pro / macOS 26.5.2 / 12 cores, `cargo-llvm-cov 0.8.7`,
`cargo-nextest 0.9.140`, rustc 1.97.1.

This document is the map half of the coverage work; the fill half is the
ranked list in [§5](#5-ranked-gaps) with what landed and what did not. **Read
[§6](#6-not-worth-covering-and-why) before "improving" a low number** — two of
the worst files in the table are things a test would make look safer without
making them safer.

## 1. Re-running it

```bash
cargo install cargo-llvm-cov          # 0.8.7 here
bash scripts/coverage.sh              # every feature combo, ~6 min warm
bash scripts/coverage.sh allsafe      # one combo by name
python3 scripts/cov_summarize.py ~/tmp/zenavif-cov/*.json          # totals + per-file table
COV_MIN_COLD=8 python3 scripts/cov_summarize.py --cold ~/tmp/zenavif-cov/allsafe.json
python3 scripts/cov_summarize.py --file src/yuv_convert.rs ~/tmp/zenavif-cov/allsafe.json
```

`scripts/coverage.sh` reuses `scripts/gauntlet.sh`'s combo list **verbatim**, so
a coverage row and a clippy/nextest row describe the same build. JSON lands in
`$HOME/tmp/zenavif-cov/` (`COV_OUTDIR=` to change it) — never `/tmp`.

## 2. Three traps in measuring this crate

**(a) Feature gating, so per-combo or nothing.** Most of the crate does not
*compile* without its feature. A single blended figure reports 100 % of
whatever happened to be enabled — the metric CLAUDE.md bans. In the per-file
table a `--` means **the file was not built in that combo**; it is not
0 %-covered code, and averaging it in either direction is wrong.

**(b) `--no-clean` silently destroys per-combo isolation.** It looks like a
free speedup (keep the instrumented deps between combos) and it invalidates
the whole exercise: the previous combos' binaries and `.profraw` files stay in
the coverage target dir, `cargo-llvm-cov` globs them all, and every report
merges every other combo's execution data. Measured here: with `--no-clean`
the **`default`** report listed `src/encoder.rs` and `src/two_pass_zensim.rs`
(66 files, 22.8 k lines — code that combo cannot compile) and all eleven
combos landed within 0.1 % of each other. `scripts/coverage.sh` therefore does
NOT pass `--no-clean`; it pays a rebuild per combo instead (sccache absorbs
most of it). Numbers below are from an isolated run — verified by file count
(48 for `default`, 66 for `allsafe`) and by feature-marker files appearing only
in the combos that build them.

**(c) The per-FUNCTION list from `llvm-cov` is not per source function.** A
generic gets one record per test/example binary it was instantiated into, and
the copies inside never-run example binaries read cold whatever the library
executed. `--cold` reconstructs per-line counts from the merged segment stream
instead; use that, or `--file` (which merges instantiations by name and takes
the max count).

## 3. What is excluded, and why

* **`unsafe-asm` / literal `--all-features`** — cannot build on this box:
  rav1d's aarch64 `.S` sources are rejected by Apple `cc` (`-march=armv8.6-a`),
  documented in CLAUDE.md. `scripts/coverage.sh` prints it as `SKIP` with the
  reason rather than dropping it; `COV_ALL=1` attempts it anyway. **The legacy
  FFI decoder in `src/decoder.rs` (1410 lines) is therefore UNMEASURED here** —
  not covered, not uncovered. It needs a Linux or x86-64 run.
* **`apidoc/` snapshot runners** — separate packages, never built by `cargo
  test`; `just api-doc` also cannot finish on this box (same assembler
  problem). Not in the matrix.
* **Doctests** — not measured (`cargo llvm-cov --doctests` is unstable).

Pre-existing failures, unchanged by this work: `backends` and `allsafe` fail
`product_aom_backend::animations_decode_identically_across_backends`
(rav1d-safe#448/#449), exactly as CLAUDE.md records; `gauntlet.sh clippy`'s
`allsafe` leg fails on `_dev` example warnings — clippy stops at the first
failing example, so WHICH one it reports varies between runs. Three exist, all
pre-existing (`git diff 77dc2d0 -- examples/` is empty): unused
`zenavif::yuv_convert_libyuv_simd` import in `examples/benchmark_simd.rs` (the
one CLAUDE.md names) and in `examples/benchmark_autovec.rs:8`, plus a needless
`mut` at `examples/yuv_kernel_bench.rs:153`. `--ignore-run-fail` keeps
those combos in the map instead of dropping their reports entirely; the
per-combo status line says `TESTS-FAIL`, derived from nextest's summary rather
than the exit code.

## 4. Per-combo totals (isolated, 2026-08-11, at `8795864`)

The eleven-combo table is from `8795864`; the two headline combos were
re-measured at `87c2331` (the last test commit): `default` 81.3 % lines /
79.6 % regions (20708/26014), `allsafe` 84.5 % lines / 83.1 % regions
(32522/39113).

Whole workspace, i.e. `zenavif` + `zenavif-parse` + `zenavif-serialize`.

| combo | features | lines | regions | functions | tests |
|---|---|---|---|---|---|
| `default` | (avx512) | 81.2 % | 79.5 % (20628/25944) | 64.4 % | 359 |
| `encode` | encode,encode-imazen | 82.9 % | 81.3 % (24881/30589) | 67.9 % | 507 |
| `aom` | aom-backend | 78.3 % | 77.0 % (20636/26799) | 62.4 % | 359 |
| `svt` | encode-svt-rs | 82.6 % | 80.9 % (24893/30767) | 67.5 % | 504 |
| `tq` | target-quality | 82.9 % | 81.4 % (25922/31858) | 68.5 % | 515 |
| `backends` | encode-imazen,encode-svt-rs,aom-backend,target-quality | 83.2 % | 82.0 % (27599/33667) | 68.0 % | 553 (1 fail) |
| `expert` | __expert | 84.0 % | 82.3 % (27033/32841) | 70.1 % | 550 |
| `autotune` | auto-tune | 83.1 % | 81.6 % (26003/31874) | 68.6 % | 519 |
| `twopass` | two-pass-butteraugli,encode-imazen | 83.0 % | 81.5 % (25484/31259) | 68.0 % | 519 |
| `zloop` | two-pass-zensim | 83.2 % | 82.1 % (27338/33318) | 68.8 % | 560 |
| `allsafe` | everything pure-Rust incl. `_dev` | 84.4 % | 83.1 % (32430/39043) | 70.4 % | 652 (2 fail) |
| `all` | `--all-features` | — | SKIPPED (unsafe-asm) | — | — |

`aom` is the lowest because enabling the backend adds `src/decoder_managed/aom.rs`
+ the aom seam in `decode_av1.rs` without adding many tests that drive them.

Caveat on before/after comparisons: the new tests live in the same files as the
code they test, so region *counts* grew too (e.g. `src/yuv_convert.rs`
2744 → 3571 regions). Percentages include test code.

## 5. Ranked gaps

Ranked by what a bug there costs, per CLAUDE.md's priorities (wrong pixels
first, then untrusted-input decode, then error granularity), **not** by region
count.

### 5.1 STILL OPEN (highest cost first)

| # | Region | Coverage (`allsafe`) | Why it matters |
|---|---|---|---|
| R1 | `src/decoder.rs` (legacy FFI decoder, `unsafe-asm`) | **unmeasured on this box** | A whole decode implementation with no number at all. Needs a Linux/x86-64 coverage run. |
| R2 | `src/decoder_managed/plane_convert.rs` — the **alpha** arms | 75.6 % (521/689) | Depth-generic (8/10/12/16) × alpha × gray plumbing — wrong-pixel surface. The 4:2:2 RGB arm is now hot (`:456` count 4, see §5.2); still cold are **4:2:2 *with alpha*** (`:388-395`) and **identity(GBR) with alpha** (`:203-212`, verified line-by-line 2026-08-11). Both need a fixture the in-repo encoder cannot make (4:2:2 is not in `EncodeChromaSubsampling`; identity+alpha needs an alpha aux item on an MC=0 image) — the link-u corpus has 4:2:2-with-alpha candidates. |
| R3 | `src/decoder_managed/grid.rs` + the grid arms of `codec/decode_job.rs` (`:490-518`) | 79.4 % / 77.8 % | Grid (tiled) stitching, incl. the **streaming grid** path. No committed grid fixture — needs one before it can be tested honestly. |
| R4 | `src/codec/streaming.rs` | 53.6 % (128/239) | The strip/streaming decoder's own error + geometry arms (`:82-89`, `:96-104`). Partially covered by the orientation/mono/gainmap streaming tests. |
| R5 | `src/decode_av1.rs` aom seam | 68.9 % (867/1259) | `decode_av1_obu_aom_8bit`'s mono (`:679-692`) and identity (`:696-739`) arms are still cold — the aom twins of the two rav1d arms this work covered. Needs `aom-backend` + a raw-OBU driver. Also `map_aom_error`'s per-variant mapping (CLAUDE.md seam obligation #1). |
| R6 | `src/codec/encode_job.rs` / `codec/anim_encoder.rs` | 57.4 % / 57.9 % | Encode-adapter limit/threading/animation arms (`encode_job.rs:121-144`, `:223-244`). Cost is a refused or mis-limited encode, not wrong pixels. |
| R7 | `src/encoder_svt_rs.rs` | 72.0 % (449/624) | Experimental backend; its own docs say the envelope is narrow. Low priority *because* it is off by default. |
| R8 | `src/target_quality.rs` | 72.4 % (666/920) | Convergence/bracket-failure arms (`:338-347`, `:406-414`, `:421-428`, `:755-763`). Honest-`converged=false` reporting is the risk, not pixels. |
| R9 | `src/detect.rs` | 62.1 % (203/327) | Sniffing / QP→quality mapping on partially-valid input. |
| R10 | `src/decoder_managed/sink.rs` remaining half | 48.3 % (201/416) | The grid tile-row branch of the sink (`:276-298`) is still cold; the single-image branch is now covered. Blocked on the same missing grid fixture as R3. |

### 5.2 FILLED in this pass (13 of the 13 top-tier findings I could reach without new fixtures)

Every test below was mutation-proven: a defect was planted, the test observed
to FAIL with the quoted message, and the mutation reverted.

| Commit | Gap (measured cold) | Test | Mutation that proved teeth |
|---|---|---|---|
| `d0bb1e7` | **All `_scalar` copies of the five unified YUV kernels: count = 0 in every combo.** The tier that runs on every non-SIMD target, and the fallback the module header calls "byte-identical", was never executed — every existing reference test runs at exactly one tier (NEON here). | `yuv_convert::tests::every_simd_tier_is_byte_identical` — the whole conversion battery once per archmage token permutation, byte-compared to the host's best tier. `CompileTimePolicy::Fail` is the liveness gate; archmage gains `testable_dispatch` in dev-deps (without it no fallback tier is reachable on aarch64 at all). | scalar-tier-only pixel perturbation → "SIMD tier divergence at permutation [NEON, NEON+AES, … disabled]", which also proves the scalar tier is what runs |
| `019c42e` | The ten `S = u16, P = 8-bit` kernel instantiations — the **aom-backend shape** (`aom-decode` returns u16 planes at every depth; `wide_out = bit_depth > 8`). | `u16_planes_of_8bit_samples_match_the_u8_kernels` + the battery extension | `YuvSample for u16::to_i32 → saturating_add(1)` → per-pixel Rgb mismatch |
| `367dfd2` | `decode_av1::convert_monochrome` (74 regions) and `convert_identity_to_rgb` (34 regions), both **count = 0**: the raw-OBU mono and MC=0 (G,B,R) arms. Wrong-pixel paths; the container path was covered, the raw-OBU one was not. | `raw_obu_mono_matches_the_container_path` (4 committed fixtures, exact agreement with `decode_with`), `raw_obu_identity_reorders_gbr_planes_to_rgb` (+ a paired R/B-swap assertion proving the ±2 zenrav1e#9 tolerance cannot absorb a rotation) | swapped `out_row[x*3]`/`[x*3+2]` → "off by 155"; `YuvRange::Full` instead of the signalled range → "raw-OBU gray 34 != container gray 21 at (0,0)" |
| `6550186` | `convert::downscale_to_8bit`'s **RGBA16 and GRAY16 arms** — only the RGB16 arm ran. Each arm is its own hand-written per-channel `>> 8`; this is the `prefer_8bit` narrowing for 10/12-bit decodes. | `downscale_to_8bit_keeps_every_channel_in_place` (distinct per-channel values so a swap cannot cancel) | `a: px.a as u8` (truncate, not shift) → "RGBA16 → RGBA8 channel mismatch at pixel 0" |
| `c68b669` | `codec/threads.rs::policy_to_threads` — **0 of 6 regions in every combo** (`ResourceLimits::default()` is `Parallel`, which `effective_config` skips by design). | `policy_lowering_is_exact` (unit: the mapping, incl. all five deprecated variants) + `decode_is_byte_identical_under_every_threading_policy` (integration: a thread-dependent-output tripwire). Honest split — a wrong thread count still yields identical pixels, so the mapping *cannot* be pinned at decode level. | `Sequential => 0` → "Sequential must mean one thread, not auto" |
| `af4f408` | `AvifDecoderConfig::with_limits` (`decode_config.rs:60-73`) — the **config-level** ResourceLimits lowering. Every existing limit test goes through the job-level `with_limits` or the native `frame_size_limit`, so the path a zencodec consumer actually reaches was cold. A silent no-op here is a decode fail-open on untrusted input (the zenavif#22 class). | `config_level_limits_bound_an_untrusted_decode` (over-limit refused, generous cap still decodes) | dropped the `max_pixels` lowering → "max_pixels = 1 must refuse a 15 px image" |
| `f4e3299` | `decoder_managed/sink.rs` at 27.4 % — the row-sink decode path, the streaming counterpart of the buffered decode. | `row_sink_decode_is_byte_identical_to_buffered_decode` (colour 4:2:0, strict byte identity, **plus** an assertion that >1 strip was delivered so it cannot pass by measuring a whole-image conversion) and `row_sink_mono_content_matches_the_gray_path_despite_the_format_gap` | off-by-one source row in the sink's strip copy → both tests failed ("disagree on pixels at byte 145164"; "channel 0 at (0,63) is [16] but the gray path says [20]") |
| `87c2331` | **4:2:2 had no product-level coverage at all**: the in-repo encoder emits only 4:4:4 and 4:2:0, so nothing ever decoded a 4:2:2 AVIF — the 4:2:2 dispatch arms of `plane_convert.rs` (`:454-464`) and `decode_av1::convert_to_rgb` were cold. 4:2:2 is horizontal-only chroma upsampling: its own kernel, its own edge clamp. | `raw_obu_422_matches_the_container_path` — four link-u 8-bit 4:2:2 fixtures (incl. odd-width and odd-width+odd-height, the clamp edges) decoded through BOTH plumbings (raw-OBU strip entry vs the managed decoder's plane views), exact per-pixel agreement. 8-bit deliberately: at 10 bits the two paths narrow with different rounding. | plane_convert's `Cs422` arm switched to `yuv444_to_rgb8` → "4:2:2 raw-OBU vs container decode disagree at (1,0)" |

Net effect on the `default` combo: regions 75.0 % → **79.6 %**, lines 77.0 % →
81.3 %, functions 62.6 % → 64.2 %; `src/yuv_convert.rs` 74.7 % → 98.4 %,
`src/codec/threads.rs` 0 % → 100 %, `src/convert.rs` 73.2 % → 89.8 %,
`src/decoder_managed/sink.rs` 27.6 % → 48.2 %, `src/decode_av1.rs` 35.1 % →
53.8 %. `allsafe`: 83.1 % regions (32522/39113), `plane_convert.rs` 73.6 % →
75.6 %, `decode_av1.rs` 66.2 % → 68.9 %.

Counting the top tier honestly: **8 of the 8 cold regions I could reach without
a new committed fixture are now covered** (scalar SIMD tiers, u16-carried-8-bit
kernels, raw-OBU mono, raw-OBU identity, RGBA16/GRAY16 narrowing, thread-policy
lowering, config-level limits, row-sink single-image, 4:2:2 RGB). The ones I
could NOT reach are R1-R10 above, and each names its blocker.

## 6. Not worth covering, and why

* **`src/image.rs` (0 %, 0/17 regions).** The only executable code is
  `impl Default for ImageInfo` — 23 field initialisers. A test would move the
  number to 100 % and assert nothing anyone can get wrong. Left at 0 %
  deliberately.
* **`src/strip_convert.rs` (62.5 %, 287/459).** Its own header says
  "*WIP: strip converter is implemented and tested but not yet wired into the
  public API*" and the module carries `#![allow(dead_code)]`. Its cold arms are
  4:2:2 / 4:4:4 / alpha for a module the product never calls. Testing dead code
  buys the appearance of safety; the real decision is **wire it or delete it**.
  (The production strip path is `codec/streaming.rs` + `decoder_managed/sink.rs`,
  both measured above.)
* **`src/yuv_convert_libyuv*.rs` (66.7 – 98.1 %).** Alternative reference
  converters kept for A/B and benchmarking, reachable from the product only via
  `_dev` examples. Their cold parts are unused matrix/range combinations. Rank
  below anything on a decode path.

## 7. Findings that outrank the coverage work

**rav1d-safe#449 (upstream, sibling repo — reported, not fixed): CDEF
tap-window overlap is product-visible on aarch64.** At the rev zenavif pins
today (`a6a7e232`, which carries the 49df1fc0 wedge fix) a full-suite run
under load intermittently panics a rav1d worker:

```
thread 'rav1d-worker-3' panicked at src/safe_simd/cdef_arm.rs:124:38:
        overlapping DisjointMut:
 current:    & _[5150..5162] on ThreadId(110) at src/safe_simd/cdef_arm.rs:124:38
 existing: &mut _[4896..5152] at src/cdef_apply.rs:83:30
```

A 2-byte overlap between a NEON CDEF read and a neighbouring block's mutable
range; an earlier occurrence the same day pointed at `cdef_apply.rs:59:26`. The
wedge fix works — no hang, the panic surfaces as an error in milliseconds — but
**the decode fails**: `zensim_loop::two_shot_keep_closer_never_ships_the_worse_of_the_two`
and `animation_decode::decode_8bpc_with_alpha` returned
`Decode { code: -1, msg: "Failed to decode primary frame" }` on those runs.
Frequency: 2 occurrences in ~15 full-suite runs on 2026-08-11; 7 consecutive
repeats immediately afterwards were clean, so it is load/timing dependent.
Evidence posted to rav1d-safe#449. **This is distinct from the documented
`animations_decode_identically_across_backends` failure** — do not conflate a
flaky CDEF panic with that deterministic one.

**zenavif#35 (this repo — reported, deliberately not fixed): the row-sink
decode path ignores `preferred` and can never emit native Gray.** Probed
2026-08-11 on the committed mono fixtures:

| input | `preferred` | buffered | streaming | row sink |
|---|---|---|---|---|
| `mono_gradient_8b_full.avif` | `[]` | Gray8 | Gray8 | **Rgb8** |
| `mono_gradient_8b_full.avif` | `[Gray8]` | Gray8 | Gray8 | **Rgb8** |
| `kodim03_yuv420_8bpc.avif` | any | Rgb8 | Rgb8 | Rgb8 |

`push_decoder_inner` derives its descriptor from bit depth + alpha alone: it
never consults `preferred` and never calls `set_native_gray`, while
`DecodeCapabilities` advertises `native_gray`. Not corruption — the triples are
neutral and equal the gray samples — so it is a dropped-capability gap, and
`row_sink_mono_content_matches_the_gray_path_despite_the_format_gap` pins
exactly that: content parity per sample today, automatic strict byte identity
once the descriptors converge. Behaviour was left alone because the adapter's
public output format is not something to change unilaterally.

## 8. Coverage-adjacent facts worth not re-deriving

* `archmage`'s `testable_dispatch` feature in **dev-dependencies** is what makes
  any non-best SIMD tier reachable from a test on aarch64 (NEON is
  compile-time guaranteed there, so token disabling fails without it).
  Removing it makes `every_simd_tier_is_byte_identical` fail loudly rather than
  silently re-run one tier.
* `.config/nextest.toml` carries a `coverage` profile (`fail-fast = false`) only
  because `cargo llvm-cov` rejects `--no-fail-fast` next to
  `--ignore-run-fail`, and a coverage run needs both. `[profile.default]` is
  untouched.
* 9 tests are `#[ignore]`d across the suite and are NOT in these numbers, all
  of them corpus/reference work the caller opts into via
  `just test-integration` / `just test-linku*` / `just test-pixels`
  (`cargo nextest list --run-ignored ignored-only`):
  `detect::tests::test_probe_all_vectors`,
  `integration_corpus::{test_decode_all_vectors, test_decode_specific_formats}`,
  `linku_corpus::{linku_decode_all, linku_pixel_parity}`,
  `parity_test::test_decode_works`,
  `pixel_verification::{generate_references, verify_against_libavif, verify_pixel_accuracy}`.
  Caller-gated, not silent skips — but note that anything they alone would
  cover reads as uncovered above.
