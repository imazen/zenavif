# AVIF dispatch and linked size, 2026-11-23

The retained decode/parser refactor saves **213,600 native consumer `.text`
bytes**, **106,186 stripped Wasm bytes**, and **23,171 gzip bytes**. All 234
native pixel/metadata witnesses are exact. The seeded AVX2 YUV benchmarks improve
45–76%, photo decode improves 1.8%, and photo streaming improves 3.8%.
Container-only parsing slows 2.7–11.9%; the isolated YUV benchmark's stripped
ELF grows 70,520 bytes despite a smaller `.text`. Complete measurements follow.

**Qualification limit:** the requested workspace/test/lint/format/Wasm commands
pass. Separate project encoder engineering envelopes already fail on the
baseline: 58 ladder tolerance failures → 57 after, and six new monotonicity
inversions → the same six after. No envelope, pixel tolerance, encoder default
or public API was changed. These are recorded qualification failures, not green
gates or a release qualification.

## Source, scope and implementation

Baseline codec `4221a7c0`; retained implementation `052de2d3`. The fixed consumer
is in `benches/leaf_dispatch.rs`, `examples/leaf_dispatch_probe.rs` and
`examples/support/leaf_dispatch.rs` (harness `313bff84`). The measured pre-squash
implementation `634fdf5a` and the retained commit have identical production
source trees. Root source tree before/after:
`d081cf161821fa21f9aa6ebd36133bddca03a8c0` /
`7b82143439ee760614453f82a3b57cdeb8815809`.
Parser source tree before/after:
`e43e64dab862730941569a981f2ff9b79e3de94e` /
`bccee9213e0f3fd0c9328e91e6f768fbc566f82d`.
Root lockfile SHA-256 stays
`1f21822146dde61e7b511363f2b657e0843982a7f6c8f956ba8ac5d6926d0ea2`.

- **Wasm relay:** native scoped-thread relay and its tests live behind
  `not(target_arch = "wasm32")`; Wasm invokes `f(None)`. Existing entry/phase
  checks remain. The production-source WASI probe failed on the baseline with
  Unsupported from thread spawning (exit 134), then passed after the change.
- **YUV stores:** adjacent odd/even chroma outputs are written as contiguous
  pairs. Their integer/f32 operations retain the original order and weights.
  This removes masked AVX2 stores in the high-depth conversion instantiations.
- **Strip workspace:** one checked row allocation is retained in the converter
  instead of three temporary vectors per strip. Public helpers retain their
  signatures and create temporary workspace; internal prepared-workspace entries
  are crate-private. The private converter becomes mutable at its two internal
  callers, retaining its Send/Sync-compatible field types.
- **Parser:** recursive box readers use a private `Read + Offset` trait object.
  Reader-object and borrow lifetimes remain separate. Remaining-byte limits and
  absolute offsets retain their original arithmetic and error behavior.
  Public generic reader signatures remain unchanged (68 parser and 15 YUV
  function signatures checked against the baseline).
- **Fixture maintenance:** the baseline stack gate lacked
  `abc_color_irot_alpha_irot.avif`. The downloader now obtains libavif v1.4.2
  through codec-corpus and verifies that original file's SHA-256
  `b371cc88244a873131e4d10ff9363d71ce4f41cf333bd4a491b38d970d9abd3b`.
  It copies only missing inputs and preserves existing files. Other download
  scratch uses the caller TMPDIR; parser fixtures use this workspace. No binary
  fixture was committed and no stack/pixel assertion was relaxed.

The open PR implementation files and `zenav1-svt` feature defaults are untouched.
There are no public API additions, dependency-pin changes, metric maps or
encoder-policy changes.

## Method

All reported benchmark/size captures ran on i134 through `remote_bench.sh`,
serialized under its shared lock: external `taskset -c 2`, nice 19, one build
job, 12 GiB memory guard. Rust 1.99.0 (`b940084d7`, LLVM 23.1.1), release/bench
optimization, debug disabled, native RUSTFLAGS empty, default panic/unwind and
LTO settings. No `target-cpu=native`.

The same five runnable benches were built with `_dev` before/after:
`yuv_conversion_benchmark`, `tier_isolation`, `unpremul_tiers`,
`decode_benchmark`, and `leaf_dispatch`. Every invocation uses
`--rounds=30 --cpu-policy=unrestricted --format=llm` and
`ZENBENCH_CPU_POLICY=unrestricted`. Three ABBA blocks per executable retain
six fresh processes per revision (60 processes total), with effective thread
affinity `[2]` recorded. Zenbench labels these runs
**UNVERIFIED_UNRESTRICTED**: external pinning is disclosed; delegated cpuset,
frequency validation and continuous quietness proof are absent. This is
exploratory repeated evidence, not formal non-inferiority inference.

For native Suite benches, each process statistic is the median of all retained
compensated batch times / actual iteration count; the final statistic is the
median of six process medians. The legacy criterion-compat YUV bench retains
30-sample per-process summaries rather than raw round vectors, so its recorded
summary median is used. No process is dropped. Positive throughput delta means
`before_ns / after_ns - 1` is positive. Consumer names disambiguate the neutral
input in the compatibility bench from seeded noise in tier isolation.

## Throughput: every measured cell

| Consumer | Group | Arm | Before µs | After µs | Throughput change |
|---|---|---|---:|---:|---:|
| decode_benchmark | decode_tiers/clap_irot_imir_non_essential.avif | forced_scalar | 68.757 | 68.238 | +0.76% |
| decode_benchmark | decode_tiers/clap_irot_imir_non_essential.avif | native_simd | 53.877 | 54.135 | -0.48% |
| decode_benchmark | decode_tiers/extended_pixi.avif | forced_scalar | 22.926 | 23.131 | -0.89% |
| decode_benchmark | decode_tiers/extended_pixi.avif | native_simd | 22.526 | 23.035 | -2.21% |
| decode_benchmark | decode_tiers/kodim03_yuv420_8bpc.avif | forced_scalar | 20837.042 | 20706.399 | +0.63% |
| decode_benchmark | decode_tiers/kodim03_yuv420_8bpc.avif | native_simd | 7250.059 | 7121.434 | +1.81% |
| leaf_dispatch | parse/animation_metadata/reader0 | parser | 10.669 | 11.262 | -5.26% |
| leaf_dispatch | parse/animation_metadata/reader1 | parser | 10.655 | 11.197 | -4.84% |
| leaf_dispatch | parse/animation_metadata/reader2 | parser | 20.619 | 21.422 | -3.75% |
| leaf_dispatch | parse/orientation/reader0 | parser | 3.188 | 3.586 | -11.11% |
| leaf_dispatch | parse/orientation/reader1 | parser | 3.167 | 3.593 | -11.85% |
| leaf_dispatch | parse/orientation/reader2 | parser | 4.125 | 4.581 | -9.95% |
| leaf_dispatch | parse/photo420/reader0 | parser | 2.998 | 3.165 | -5.27% |
| leaf_dispatch | parse/photo420/reader1 | parser | 2.935 | 3.174 | -7.52% |
| leaf_dispatch | parse/photo420/reader2 | parser | 35.734 | 36.706 | -2.65% |
| leaf_dispatch | parse/pixi/reader0 | parser | 1.850 | 2.050 | -9.73% |
| leaf_dispatch | parse/pixi/reader1 | parser | 1.838 | 2.057 | -10.66% |
| leaf_dispatch | parse/pixi/reader2 | parser | 2.341 | 2.554 | -8.33% |
| leaf_dispatch | stream/orientation | decode | 55.928 | 55.862 | +0.12% |
| leaf_dispatch | stream/photo420 | decode | 7202.781 | 6942.615 | +3.75% |
| leaf_dispatch | stream/pixi | decode | 24.083 | 23.792 | +1.22% |
| tier_isolation | yuv420_to_rgb8/1920x1080 | scalar | 4953.910 | 3875.446 | +27.83% |
| tier_isolation | yuv420_to_rgb8/1920x1080 | v3(avx2) | 3044.961 | 2094.043 | +45.41% |
| tier_isolation | yuv420_to_rgb8/512x256 | scalar | 317.087 | 243.984 | +29.96% |
| tier_isolation | yuv420_to_rgb8/512x256 | v3(avx2) | 205.059 | 130.458 | +57.18% |
| tier_isolation | yuv420_to_rgb8/64x64 | scalar | 10.568 | 8.467 | +24.81% |
| tier_isolation | yuv420_to_rgb8/64x64 | v3(avx2) | 8.539 | 4.860 | +75.70% |
| unpremul_tiers | unpremultiply8/17px | scalar | 0.081 | 0.075 | +7.39% |
| unpremul_tiers | unpremultiply8/17px | v3(avx2) | 0.031 | 0.031 | +0.27% |
| unpremul_tiers | unpremultiply8/1920px | scalar | 9.387 | 8.796 | +6.72% |
| unpremul_tiers | unpremultiply8/1920px | v3(avx2) | 1.533 | 1.531 | +0.12% |
| unpremul_tiers | unpremultiply8/1px | scalar | 0.008 | 0.008 | +0.15% |
| unpremul_tiers | unpremultiply8/1px | v3(avx2) | 0.018 | 0.018 | +0.10% |
| unpremul_tiers | unpremultiply8/4096px | scalar | 19.980 | 18.882 | +5.81% |
| unpremul_tiers | unpremultiply8/4096px | v3(avx2) | 3.248 | 3.283 | -1.06% |
| unpremul_tiers | unpremultiply8/512px | scalar | 2.499 | 2.340 | +6.80% |
| unpremul_tiers | unpremultiply8/512px | v3(avx2) | 0.423 | 0.415 | +1.80% |
| unpremul_tiers | unpremultiply8/64px | scalar | 0.308 | 0.285 | +7.84% |
| unpremul_tiers | unpremultiply8/64px | v3(avx2) | 0.066 | 0.067 | -1.24% |
| yuv_conversion_benchmark | yuv420_to_rgb8 | 1920x1080 | 3017.468 | 2061.494 | +46.37% |
| yuv_conversion_benchmark | yuv420_to_rgb8 | 512x256 | 222.768 | 142.844 | +55.95% |

The parser cost is 0.17–0.97 µs per call on these four inputs. The tiny pixi
native full-decode median falls 2.21%; this is not omitted from the results.
Unpremultiply is an unchanged control: its movements are not a claimed
algorithmic improvement. Scalar means forced scalar dispatch, which still
permits baseline compiler auto-vectorization. No throughput is extrapolated to
other images, CPUs or bit depths.

## Linked sizes

The native parity consumer executes the same decode, stream, metadata and YUV
witnesses before/after. Libtest additionally contains the two new regression
guards after; it is a diagnostic size target, not an identical test-case list.
Wasm is a temporary `cdylib` using wasm-bindgen 0.2.130, root
`default-features = false`, identical root patches, and
`RUSTFLAGS='-C target-feature=+simd128'`. Exports retain `zenavif::decode`,
`AvifParser::from_reader`, and a live-Stop `decode_with` call. The native probe
and Wasm probe are different consumers; their sizes are not cross-target ratios.

| Artifact | Before bytes | After bytes | Delta bytes |
|---|---:|---:|---:|
| probe file | 7,904,944 | 7,649,776 | -255,168 |
| probe stripped file | 7,097,496 | 6,863,440 | -234,056 |
| probe .text | 5,530,217 | 5,316,617 | -213,600 |
| libtest file | 9,338,728 | 9,042,808 | -295,920 |
| libtest stripped file | 8,325,248 | 8,047,912 | -277,336 |
| libtest .text | 6,484,133 | 6,226,805 | -257,328 |
| yuv_conversion_benchmark file | 2,652,608 | 2,796,072 | +143,464 |
| yuv_conversion_benchmark stripped file | 2,081,664 | 2,152,184 | +70,520 |
| yuv_conversion_benchmark .text | 1,574,357 | 1,565,029 | -9,328 |
| Wasm unstripped module | 2,445,788 | 2,315,808 | -129,980 |
| Wasm stripped module | 2,209,926 | 2,103,740 | -106,186 |
| stripped Wasm gzip -n -9 | 652,537 | 629,366 | -23,171 |
| Wasm code-section payload | 1,754,130 | 1,648,529 | -105,601 |

The isolated compatibility bench's `.gcc_except_table` grows 71,416 → 169,268
bytes while `.text` falls 1,574,357 → 1,565,029 and `.rodata` falls 170,780 →
152,852. The stripped ELF therefore grows despite smaller executable code.
The combined native consumer and Wasm both shrink. These are combined changes;
no unmeasured per-edit size attribution is claimed.

## Symbols and masked accesses

`nm -CS --defined-only` substring counts below are reproducible linked symbol
family proxies, not source-level generic counts: aliases, inlining and closures
can change them. For example `read_ipco` is inlined after rather than absent
from the implementation.

| Consumer | Family | Before matching names | After matching names |
|---|---|---:|---:|
| probe | BMFFBox | 138 | 22 |
| probe | BoxIter | 6 | 1 |
| probe | read_iprp | 1 | 2 |
| probe | read_ipco | 2 | 0 |
| libtest | BMFFBox | 138 | 22 |
| libtest | BoxIter | 6 | 1 |
| libtest | read_iprp | 1 | 2 |
| libtest | read_ipco | 2 | 0 |
| yuv_conversion_benchmark | BMFFBox | 0 | 0 |
| yuv_conversion_benchmark | BoxIter | 0 | 0 |
| yuv_conversion_benchmark | read_iprp | 0 | 0 |
| yuv_conversion_benchmark | read_ipco | 0 | 0 |

The Wasm named bodies include two separate 19,842-byte `read_ipco` bodies
before; container parsing is shared behind the erased reader afterward. Total
Wasm defined functions fall 1,884 → 1,806. Name-body attribution may overlap
other families and is not additive to module savings.

| Native consumer | Total vmaskmov before → after | YUV-owned before → after |
|---|---:|---:|
| parity probe | 18 → 2 | 16 → 0 |
| libtest | 34 → 2 | 32 → 0 |
| yuv_conversion_benchmark | 0 → 0 | 0 → 0 |

The two residual instructions are in unchanged
`zenpixels_convert::convert_kernels::__arcane_multiply_color_channels_tier_v3`.
That sibling is untouched. All three ELF targets retain zero
`__simd_inner_*` symbol definitions before/after.

## Correctness and qualification

The same 234 files match byte for byte on i134: 216 YUV→RGBA8 outputs
(12 widths including 1/7/8/9/15/16/17/31/32/33/65/257, 420/422/444, full/limited,
601/709/2020, poisoned row padding and three-row strip boundaries), 12 parser
metadata/payload snapshots (Cursor, slice reader and three-byte reader on four
fixtures), plus three buffered and three streaming decoded rasters. No missing
fixture skip exists; included inputs must be provisioned explicitly.

The original scalar interpolation formulas additionally compare integer/f32
bits over widths 1–65, 127/128/129/257, both integer scales, and high-depth
chroma values. The existing available-tier battery remains exact. Negative
controls were observed and restored: discarding warm scratch failed capacity
52 versus 4096; changing an f32 weight failed exact bits at width 3. The tiny
WASI probe imports the production cancellation module, failed with unsupported
thread spawning before, and executes the work exactly once with no relay after.

Final requested gates:

- Workspace release `--lib --tests --examples`: **530 passed, 0 failed,
  11 existing ignored** (baseline with the original missing fixture restored:
  528 / 0 / 11). No assertions, expected values or ignore annotations changed.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass; owned `_dev`
  bench/example lint and both new helper packages' lint pass too.
- Package-scoped format checks across all four workspace crates and the helper
  packages: pass. Public YUV/parser function signatures compare unchanged.
- `cargo check -p zenavif --target wasm32-unknown-unknown --no-default-features`
  and parser Wasm check: pass. Parser `--all-features` tests: pass.
- `just wasm-cancel-check`: actual wasm32-wasip1 execution under Wasmtime passes.
  Full Wasm image decode runtime parity is not claimed by compile checks.
- Engineering determinism and independent AOM decoder conformance pass before
  and after. The optional sibling-encoder conformance leg is explicitly disabled
  by the caller (`ZENRAV1E=''`); AOMDEC is `/usr/bin/aomdec`.

Inherited encoder envelope failures remain: ladder has 58 tolerance failures
in 27 cells before and 57 after; monotone has the same six new inversions before
and after. Example unchanged cells: s6/mixed/q60 SSIM2 64.170 vs pinned 67.227;
s10/mixed/q85 size 45,820 vs pinned 48,783. Timing-floor counts can vary with
host load. The envelope files and encoder implementation/defaults remain
unchanged; no re-pin or acceptance threshold was substituted. Owning backlog:
`CLAUDE.md`, encoder engineering envelopes on 4221a7c0.

The remote guard reports baseline peak RSS 1.28 GiB and incremental after/replay
peak 0.57 GiB; build-cache states differ, so no memory-improvement claim is made.
No ARM throughput, AVX-512 execution on i134, full Wasm pixel runtime, encoder
quality change or release qualification is claimed.

## Reproduction and evidence

Use the same revisions, toolchain, feature flags and provisioned fixtures.
Run all remote measurements through the serialized runner; do not change CPU
pinning or infer cpuset/frequency validation from the external pin.

```sh
export CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_BENCH_DEBUG=0 RUSTFLAGS=''
export ZENBENCH_CPU_POLICY=unrestricted
cargo bench --locked --features _dev --bench tier_isolation -- --rounds=30 --cpu-policy=unrestricted --format=llm
cargo bench --locked --features _dev --bench yuv_conversion_benchmark -- --rounds=30 --cpu-policy=unrestricted --format=llm
cargo bench --locked --features _dev --bench unpremul_tiers -- --rounds=30 --cpu-policy=unrestricted --format=llm
cargo bench --locked --features _dev --bench decode_benchmark -- --rounds=30 --cpu-policy=unrestricted --format=llm
cargo bench --locked --features _dev --bench leaf_dispatch -- --rounds=30 --cpu-policy=unrestricted --format=llm
cargo build --locked --release --features _dev --example leaf_dispatch_probe
# Run the example with a fresh explicit output directory.
just wasm-cancel-check
```

Private retained evidence: `~/tmp/bench-lanes-2026-11-23/results/zenavif/`, with
before/after consumers, raw named/disassembled binaries, witness hashes,
`manifest.json`, `sizes.json`, `summary-after.json` and 60 process packets/logs
in `abba/`. The legacy compatibility summaries are identified above. Temporary
Wasm probe source is preserved with the artifacts before its build directory is
removed. The benchmark and codegen scripts, logs and scalar summaries accompany
the archive copy. No generated binaries or large data are committed.
