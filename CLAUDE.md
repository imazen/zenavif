# zenavif

Pure Rust AVIF encoder/decoder wrapping rav1d-safe (pure Rust AV1 decoder) and zenavif-parse.

## Workspace (since 2026-07-16)

This repo is a cargo workspace with three published crates: **zenavif**
(root package), **zenavif-parse/** and **zenavif-serialize/** — the two
container repos were absorbed with full histories (rewritten under their
subdirectories) and their tags imported crate-prefixed (`zenavif-parse-v*`,
`zenavif-serialize-v*`; upstream lineages `avif-parse-v*`, `mp4parse-v*`).
The old imazen/zenavif-parse and imazen/zenavif-serialize repos are
superseded — do all work here.

- **Releases are per crate via crate-prefixed tags**: `zenavif-vX.Y.Z`,
  `zenavif-parse-vX.Y.Z`, `zenavif-serialize-vX.Y.Z`. `release.yml` routes
  the tag prefix to `cargo publish -p <crate>` + a GitHub release. The full
  release gate sequence in the global CLAUDE.md applies per crate.
- **Submodules**: zenavif-parse's corpus tests fail-loud on
  `zenavif-parse/av1-avif` + `zenavif-parse/link-u-samples` — run
  `git submodule update --init` once per checkout (CI checks them out on
  the test/i686/coverage/release jobs only).
- **`cargo fmt --all` is BANNED here** (reaches the ../ravif and
  ../zenanalyze sibling path-deps); use `just fmt` / `just fmt-check`
  (package-scoped `-p` triple).
- Root `[patch.crates-io]` currently pins: zenanalyze-api → git `main`
  (until 0.1.1 publishes) and zenavif-serialize → workspace member (until
  0.2.0 publishes). **rav1d-safe is NOT patched** — it is a direct git-rev
  dependency in `[dependencies]` (a crates-io patch cannot supply 0.6.0
  against a 0.5.x requirement); `fuzz/Cargo.toml` mirrors that `rev` and
  must be bumped in lockstep with the root manifest.

## Quick Commands

```bash
just check        # cargo check --workspace --all-targets
just build        # cargo build --release
just test         # cargo test --workspace (members included)
just test-parse   # cargo test -p zenavif-parse --all-features
just clippy       # cargo clippy with warnings as errors (root pkg)
just clippy-members # member clippy legs
just fmt          # scoped cargo fmt + api-doc snapshots (all three crates)
just build-encode # cargo build --features encode
just test-encode  # cargo test --features encode
just publish-dry  # cargo publish --workspace --dry-run
just gates        # executable engineering-baseline gates (see below)
```

**`just fmt` cannot finish on macOS aarch64** — its third step (`just
api-doc`, the `apidoc/` snapshot runners) builds rustdoc JSON over *all*
manifest features, which includes `unsafe-asm`, whose rav1d `.S` sources
Apple `cc` refuses to assemble. `cargo fmt` (step 1) still runs and
applies. Consequence: **`docs/public-api/zenavif.features.txt` can only be
regenerated on Linux** and is already stale on `main` (its header feature
list predates `two-pass-zensim`). No CI job runs `api-doc`, so nothing
gates on it. Do NOT hand-edit the snapshots — they say
`DO NOT EDIT BY HAND` for good reason.

**Pre-existing gauntlet failures on this box** (verified 2026-08-07, not
caused by whatever you just changed): `clippy allsafe` fails on an unused
import in `examples/benchmark_simd.rs` (a `_dev` example); `clippy all` /
`nextest all` fail on the documented `unsafe-asm` Apple-`cc` assembler
problem; `nextest backends`/`allsafe` fail only
`product_aom_backend animations_decode_identically_across_backends`
(rav1d-safe#448/#449) — that one PASSED on 2026-08-29 at rav1d-safe
`66f58fa6` with codec-corpus present, but no A/B was run, so treat it as
observed-passing rather than fixed.

**`unsafe-asm` is compiled by NOTHING — not CI, not this box** (established
2026-08-29). `grep unsafe-asm .github/workflows/*.yml` is empty, and the only
`--all-features` run of the ROOT package is `release.yml:64`, a workflow that
has never succeeded (see the Workspace changelog entry: the crate is not in a
publishable state). Locally it cannot build either, per the Apple-`cc`
assembler problem above. So `src/decoder.rs` — the whole legacy FFI decoder
behind that gate — is edited blind by default. To type-check it without
assembling anything, temporarily drop the assembler half of the feature
(`unsafe-asm = ["dep:rav1d"]`, i.e. remove `"rav1d/asm"`), run
`cargo check --features unsafe-asm --lib`, then restore `Cargo.toml`. rav1d
builds clean on aarch64 macOS without its `asm` feature, so this is a real
type-check of that file — do it for any edit to `src/decoder.rs`.

**`linku_pixel_parity` cannot pass on a macOS/homebrew box, and the failure
is NOT yours** (established by A/B on 2026-08-29). `just`-less invocation is
`cargo test --test linku_corpus -- --ignored --nocapture linku_pixel_parity`,
and it compares zenavif against PNGs that `scripts/generate-linku-references.sh`
produces with whatever `avifdec` is on PATH. CI installs Ubuntu's
`libavif-bin`; homebrew ships libavif 1.4.2, and the two disagree in three
ways that have nothing to do with zenavif:

- 69 of 156 references never generate at all (homebrew `avifdec` fails on the
  `alpha-full`/`alpha-limited` plum-blossom set and the `*-with-alpha.avifs`
  animations), so those cells report `No ref`.
- 12 monochrome cells fail with a uniform `max delta 20` / similarity ~73 —
  a limited-vs-full-range difference in how the two `avifdec` versions write
  monochrome PNGs.
- 3 cells (`kimono.rotate90`, `kimono.rotate270`,
  `kimono.mirror-vertical.rotate270`) report `CMPERR size: 1024x722 vs
  722x1024` — the reference PNG has the `irot` transform applied where ours
  does not. These 3 are what trip the test's `Errored == 0` assertion.

Measured A/B at the rav1d-safe pin bump: **identical tallies at both revs**
(`140f9145` and `66f58fa6`) — Passed 72 / Failed 12 / No ref 69 / Errored 3,
same three CMPERR files. So this suite is useless as a local regression gate
on macOS; run `linku_decode_all` instead (156/156, no `avifdec` involved) and
let CI's `link-u corpus` workflow own the parity half.

**`cargo semver-checks` cannot run on this crate** (pre-existing, same root
cause as the unpublishable state — see the CHANGELOG Workspace section). The
substitute is **`dev/downstream-probe/`**: an out-of-crate consumer with its own
`[workspace]`, run with
`cd dev/downstream-probe && CARGO_TARGET_DIR=../../target cargo run --release`
(seconds, since it shares the parent's target dir). It is the only thing that
shows a `#[non_exhaustive]` break's shape — in-crate matches are unaffected by
that attribute — and it is what caught the 2026-09-02 refusal message that named
the wrong cargo feature. **Any consumer of zenavif needs a `[patch.crates-io]`
of its own** for `zenavif-serialize` and `zenanalyze-api`: `[patch]` tables in a
dependency's manifest are IGNORED, so this crate's root patch table does not
reach downstream. Measured blast radius of the 0.2.0 bump: every downstream
`zenavif = "0.1.x"` requirement must move to `"0.2.0"` (zenpipe pins `0.1.7`,
imageflow `0.1.4`), and imageflow additionally requests
`features = ["zencodec", ...]` — `zencodec` is a required dependency here, not a
feature, and has not been one on `main` for some time, so that request is
already stale independent of this bump.

**Executable gates (docs/ENGINEERING_BASELINE.md section A):** run
`just gates` (= `gate-determinism` + `gate-conformance` + `gate-ladder`,
all via `examples/gate_kit.rs` + `scripts/gates/gate_conformance.sh`)
before AND after every refactor commit. `gate-conformance` needs AOMDEC
(justfile wires the dev-box default) and optionally the sibling zenrav1e
CLI for the palette/intraBC-armed leg. `gate-ladder`'s envelope
(`benchmarks/gate_ladder_envelope.tsv`) is machine-scoped — re-pin with
`just gate-ladder-pin` only for intentional ladder movement, committing the
TSV diff in the same commit. zenrav1e's halves (`gate-identity`,
`gate-recon`) live in ../zenrav1e's justfile. CI runs the fast subsets
(identity on zenrav1e, determinism here).

## Architecture

### Decoding
- `src/lib.rs` - Public API, re-exports
- `src/error.rs` - Error types with whereat location tracking
- `src/config.rs` - DecoderConfig builder
- `src/image.rs` - DecodedImage enum, ImageInfo metadata
- `src/decoder_managed.rs` - Main decoder (100% safe Rust, rav1d-safe managed API)
- `src/decoder.rs` - Legacy FFI decoder (behind `unsafe-asm` feature gate)
- `src/convert.rs` - Alpha channel handling, unpremultiply

### YUV Conversion
- `src/yuv_convert.rs` - THE unified kernel family (2026-07-20): strip-first, depth-generic (8/10/12/16), RGB/RGBA/Gray outputs, one canonical f32 recipe, forward RGB->YUV420, AVX-512/AVX2/NEON/wasm/scalar tiers
- `src/yuv_convert_libyuv.rs` - Exact libyuv integer math (BT.709, BT.601)
- `src/yuv_convert_libyuv_simd.rs` - AVX2 SIMD libyuv path
- `src/yuv_convert_libyuv_autovec.rs` - Auto-vectorized libyuv variant
- `src/yuv_convert_fast.rs` - Fast fixed-point integer path

### Encoding
- `src/encoder.rs` - AVIF encoding via zenravif (behind `encode` feature; git-rev dep on zenravif 0.2.0 from imazen/cavif-rs since 2026-08-27 — it was the sibling path `../ravif/ravif`, which no git consumer could resolve; bump the `rev` in Cargo.toml to pick up newer zenravif, or add an uncommitted `[patch."https://github.com/imazen/cavif-rs"]` path override for sibling-lockstep work. Registry builds resolve PUBLISHED zenravif 0.1.3 — encode works on crates.io today, at zenrav1e 0.1.4 with the gated wins OFF)
- `src/encode_plan.rs` - `EncoderConfig::resolve_plan(PlanInput) -> EncodePlan` static resolution + provenance-tagged mirrors of zenravif's quality→quantizer curve and SpeedTweaks tables (`encode` feature)
- `src/sweep.rs` - Sweep planner + byte-identity fingerprint (`__expert` feature); see `docs/VARIANT_GENERATION.md` for the knob audit (dominance/trial/metric), the fingerprint exclusions, and the harness findings
- `examples/sweep_validate.rs` - Empirical axis validation; **re-run whenever bumping the zenravif dep or touching the sweep axes/fingerprint** (`benchmarks/sweep_validate_*.tsv`). Gotcha: encode+decode+score loops in rayon pools need `stack_size(32 MB)`. Per-task stack peaks at ~0.5 MB dominated by rav1d-safe's `rav1d_open` decoder-context construction (gdb-verified 2026-06-11; zenrav1e encode itself needs ≤128 KB even at speed 2/q10/noise — an earlier "partition RDO" attribution was wrong). The overflow mechanism is rayon work-stealing stacking whole task contexts on one worker stack at zensim's internal join points, so 2 MB defaults die probabilistically under load
- `tests/encode_contracts.rs` - Encode-level byte contracts (alpha quality fallback, quantizer mediation, subsampling liveness)

### Additional Source Files
- `src/decode_av1.rs` - AV1 bitstream decoding entry points
- `src/strip_convert.rs` - Strip-based pixel conversion utilities
- `src/detect.rs` - AVIF file detection / sniffing
- `src/codec/` - zencodec trait implementations, one submodule per adapter type
  (`encode_config`/`encode_job`/`encoder`/`anim_encoder`,
  `decode_config`/`decode_job`/`decoder`/`streaming`/`anim_decoder`) plus the
  shared decode helpers (`orientation`, `color`, `info`, `gain_map`,
  `negotiate`) and the thread-count policy (`threads`). `mod.rs` only declares
  and re-exports.
- `src/zennode_defs.rs` - zennode pipeline node definitions (behind `zennode` feature)
- `src/simd/` - SIMD acceleration modules

## Dependencies

- `rav1d-safe` - Pure Rust AV1 decoder (managed API, no C FFI)
- `zenavif-parse` - AVIF container parser (registry dep)
- `zenravif` / `ravif` - AVIF encoder (optional, `encode` feature)
- `zencodec` - Codec abstraction traits (registry dep)
- `zenpixels` - Pixel buffer types (registry dep)
- `archmage` / `magetypes` - Token-based safe SIMD
- `yuv` - YUV to RGB conversion (supplementary)
- `imgref` - Image buffer type
- `rgb` - RGB pixel types
- `enough` - Cooperative cancellation
- `whereat` - Error location tracking
- `thiserror` - Error derive macro

## Features

- `(default)` - Pure Rust decode only, safe SIMD via archmage
- `encode` - AVIF encoding via zenravif
- `encode-imazen` - Encoding with zenrav1e fork extras (QM, VAQ, still-image, lossless)
- `zenav1-svt` - EXPERIMENTAL `Av1Backend::Zenav1Svt` via zenav1-svt (git-rev dep on imazen/zenav1-svt — NOT a sibling path dep since 2026-09-02, and `resolve-standalone` in ci.yml gates it that way; stills only — RGB/RGBA 4:2:0 + grayscale Cs400 at 8 or 10 bits (#33: 16-bit input / `EncodeBitDepth::Ten` → native u16 `try_encode_frame_420_hbd` — since upstream hbd chunk 2 (`f319ec298`) the deblock/CDEF/Wiener post-filter searches read those native u16 planes too, nothing truncates; 10-bit alpha/gray only at speed ≥ 7 = SVT preset ≥ 9 per `svt_rs_depth_error`; HDR clli/mdcv container-side), alpha as Cs400 `auxl` aux item; dims: the 4:2:0 colour path takes ARBITRARY dimensions at ANY speed (the preset ≥ 6 floor was removed 2026-08-29 — see Known Bugs), a Cs400 alpha/gray stream needs speed ≥ 5 (SVT preset ≥ 6) AND multiples of 8, else multiples of 64 — `svt_rs_dims_error` in `src/encoder_svt_rs.rs` is the single gate (#32); coded-lossless (QP 0) is implemented upstream on 8-bit 4:2:0 but the seam's quality→QP mapping deliberately clamps to QP ≥ 1; muxes in-crate via zenavif-serialize)
- `zenav1-aom` - EXPERIMENTAL `DecodeBackend::Zenav1Aom` — zenav1-aom pure-Rust KEY-frame decoder behind the raw-OBU seam `decode_av1_obu_yuv` (git-rev dep on imazen/zenav1-aom; byte-identical to rav1d-safe on the 8-cell decode corpus; drives `examples/decode_4way_bench.rs`)
- `zenav1-aom-decode` - additive SYNONYM for `zenav1-aom`, spelled with the crate name. Neither is deprecated; `zenav1-aom` stays canonical.
- `zenav1-aom-encode` - EXPERIMENTAL `Av1Backend::Zenav1Aom` — zenav1-aom pure-Rust ENCODE backend via `aom_encode::key_frame::encode_key_frame` (git-rev dep on imazen/zenav1-aom at `c3e1b4ab`, a NEWER rev than the decode pin because `encode_key_frame` landed after it; cargo resolves the two as two sources). **KEY-frame / still scope only** — that entry point encodes one frame and has no inter path, so animation is absent from the ENCODER, not merely unwired. Wired: RGB → 4:2:0 BT.601 **limited** range (the port pins `color_range=0`, the OPPOSITE of the svt seam) at **8, 10 or 12 bits** from BOTH `encode_rgb8` and `encode_rgb16` (2026-09-03; `EncodeBitDepth::Twelve` is AV1 profile 2 and was added in the 0.2.0 break), plus 8-bit grayscale → Cs400. Depth conversion is `yuv_convert::rgbx_to_yuv420_u16` — the depth-generic recipe the svt seam already used, quantizing at the OUTPUT depth with the studio swing scaled `<< (d-8)`; the 8-bit-source-at-depth-8 cell keeps the u8 `rgb8_to_yuv420` kernel as conservatism, NOT as the reason 8-bit output is stable — MEASURED (`benchmarks/aom_bd8_identity_2026-09-03.*`): 60/60 cells byte-identical pre-vs-post the wiring, and routing that cell through the u16 recipe instead changes 0/60. Refused by name: animation, alpha (`auxl` item not built, at any depth), **high-bit-depth grayscale** (the Cs400 path passes u8 through as coded luma and there is no measured promotion rule), 4:4:4, 4:2:2, identity/RGB, full range, gain maps, lossless. **KNOWN DEFECT: quality 100 = `--cq-level 0` PANICS upstream** (`assert!(depth <= MAX_VARTX_DEPTH)`, `aom-dsp/src/entropy/partition.rs:675`) on flat content at every depth and on a gradient at 12 bits — a plain `assert!`, so release too; content-dependent, so NOT blanket-refused; pinned by `aom_cq0_still_panics_on_flat_content`. Muxes in-crate via zenavif-serialize. Gate: `tests/aom_encode_backend.rs` (25 tests, mutation proofs are `#[test]`s; the CI `Assert the aom encode gate actually ran` step floors the count at 25). See `src/encoder_aom.rs`.
- `encode-asm` - Encoding with hand-written assembly (fastest, unsafe)
- `encode-threading` - Encoding with multi-threading
- `unsafe-asm` - Decoding with hand-written assembly via C FFI (fastest, unsafe)
- `zencodec` - zencodec trait integration
- `_dev` - Expose internal YUV modules for profiling (not public API)

## Backend seam: zen cross-cutting compliance — SPEC (2026-07-20, re-audited 2026-09-02)

**Re-audit 2026-09-02 — obligations 1, 3 and 5 are DONE; the text below them
was written before the backends grew the APIs it was waiting on and is kept
only for the reasoning.** Read the code, not the obligation text:
`map_aom_error` (`src/decode_av1.rs:766`) maps every `aom_decode::DecodeError`
variant onto a distinct zenavif `Error` — Truncated/Malformed/Internal to
`Decode` with separate codes, `UnsupportedType`/`UnsupportedFeature` to
`Unsupported`, `LimitExceeded` to `ResourceLimit`, `AllocFailed` to
`OutOfMemory`, `Cancelled` to `Cancelled`, with a `#[non_exhaustive]` fallback
— so nothing flattens to `code: -1` any more (obligation 1, decode half).
`map_svt_encode_error` (`src/encoder_svt_rs.rs:133`) does the same for
`svtav1::types::EncodeError` (obligation 1, encode half). `aom_config_from`
(`:806`) threads `DecoderConfig.frame_size_limit` into `DecodeLimits.max_pixels`
and `alloc_pref` into `AllocMode`, and `decode_av1_obu_yuv_aomrs_with` (`:819`)
attaches the stop token (obligations 3 and 5).

Obligations 2 (panic isolation) and 4 (no silent corruption at the mux
boundary) are the ones still live. 4 is materially narrower than it reads
below: `reject_unsupported_config` and `svt_rs_dims_error` now refuse
out-of-envelope configs before muxing, and upstream returns a typed
`UnsupportedConfig`.

The two experimental AV1 backends are codec-only crates that do NOT yet meet the
zen cross-cutting contracts (limits / estimation / whereat / zencodec
`CategorizedError` granularity / panic-freedom+fallible-alloc / stop tokens).
Full per-repo specs live in each backend's CLAUDE.md:
`../zenav1-aom/CLAUDE.md` (zenav1-aom decode — untrusted-input, high bar) and
`../zenav1-svt/rust/CLAUDE.md` (zenav1-svt encode — trusted-input, lower bar).
(Those were `/root/aom-rs/...` and `/root/svtav1/...` here until 2026-09-02;
both repos were renamed and neither path exists.)
This section pins the **seam** obligations on the zenavif side.

**Status of the contracts on zenavif itself:** `main` already implements
`zencodec::CategorizedError for Error` (`src/error.rs`, two-level
`zencodec 0.1.26` `ErrorCategory`, `At<CodecError>` envelope, zencodec
required). (`svtav1-rs-backend` was merged and is now a literal ancestor of `main` — 0
commits ahead as of the 2026-09-02 branch audit — so the "rebase, do not
re-add" advice that stood here is spent. PR #27 `caterr-categorized-error` was
the stale original adoption, closed 2026-07-20 as superseded.) Stop tokens, fallible-alloc (`src/alloc_util.rs` `AllocPref`),
whereat (`at!`), limits (`DecoderConfig` caps + `zencodec::ResourceLimits`
mapping in `src/codec/`), and estimation (`src/heuristics.rs`) are all present
for the native rav1d-safe / zenravif paths.

**Seam obligations — enforce these when wiring a backend, and re-check at each
backend capability landing:**

1. **[DONE 2026-09-02 — see the re-audit note at the top of this section.]
   Preserve error granularity — do not flatten.** `decode_av1_obu_yuv_aomrs`
   (`src/decode_av1.rs`) currently maps zenav1-aom's 21 distinct `String`
   reasons to one generic `Error::Decode { code: -1 }`, so every aom failure
   lands in the coarsest `CategorizedError` bucket. When the backend gains a
   structured `DecodeError` (its spec §4), map each variant to the matching
   zenavif `Error` variant (`Parse`/`Decode`→`Image`, unsupported-feature→
   `Unsupported`, limit→`ResourceLimit`/`ImageTooLarge`, cancel→`Cancelled`) so
   the category survives to `error_category()`. Same for the svt seam
   (`src/encoder_svt_rs.rs`): once the pipeline returns a real `Result`, map its
   `EncodeError` variants instead of treating `is_empty()` as the only failure
   signal.

2. **Isolate panics at the seam until the backend is panic-free.** Both
   backends can `panic!`/`abort` on crafted decode input (aom) or a
   contract-violating encode config (svt); the seam's `map_err` only catches the
   `Err` branch, so a backend panic crosses into zenavif as a process crash.
   The aom decode path is on the UNTRUSTED input surface — until zenav1-aom is
   fuzz-clean and returns `Err` for malformed streams, treat `zenav1-aom` as
   NOT fuzz-safe and keep it non-default/experimental (it already is). Do not
   route untrusted decode through it in production, and do not add it to the
   default decode path, until its spec §5 lands.

3. **[DONE 2026-09-02 — see the re-audit note at the top of this section.]
   Plumb limits and stop through — a capability the seam drops is not
   "done".** When a backend accepts a `DecodeLimits`/`EncodeConfig`, a stop
   token, or an `AllocMode`, thread zenavif's existing `DecoderConfig` caps /
   `stop` token / `alloc_pref` into it in the SAME change. The decode seam
   currently supplies none because the aom API accepts none; that is the
   backend's gap to close first, then the seam's to consume.

4. **No silent corruption at the mux boundary.** The svt seam muxes any
   non-empty `Vec<u8>` the pipeline returns; per the backend's STATUS, un-gated
   configs emit decodable-but-wrong bitstreams. Until the encoder refuses
   out-of-envelope configs with `EncodeError::UnsupportedConfig` (its spec §5),
   the zenavif `zenav1-svt` path must stay experimental/off-by-default and
   its scope docs must name the verified envelope — never present a possibly-
   corrupt encode as a supported path. (Global rule: ZERO TOLERANCE for
   corruption applies at integration boundaries, not just within a single
   crate.)

5. **[DONE 2026-09-02 for the aom decode seam — see the re-audit note at the
   top of this section.] Alloc mode is a configurable perf/safety trade (both
   directions).** The
   decoder default is Fallible (untrusted → OOM-safe); the encoder default is
   Infallible (trusted → single-`calloc` fast). Expose the choice via the
   backend config and map it from `zencodec::AllocPreference` /
   `DecoderConfig.alloc_pref` at the seam — do not hardcode either side.

## Known Bugs

### `row_sink_decode_is_byte_identical_to_buffered_decode` CI flake — FIXED upstream (rav1d-safe#524 / `3426ebf7`), pin bumped 2026-08-29
Seen once on `ubuntu-latest` (run 33238501769) at rav1d-safe rev `140f914`:
`tests/cov_zencodec.rs::row_sink_decode_is_byte_identical_to_buffered_decode`
failed with `Decode { code: -1, "Failed to decode primary frame" }`, above which a
rav1d worker had panicked:

```
thread 'rav1d-worker-2' panicked at src/safe_simd/loopfilter.rs:5177:44:
  overlapping DisjointMut:
   current: &     _[73721..73729] at src/safe_simd/loopfilter.rs:5177:44
  existing: &mut  _[73728..74112] at src/owned_recon.rs:937:42
```

**The first diagnosis written here was WRONG, and it is worth knowing why.** It
read the one shared index (73728) as another `fdd6a35`-shaped false positive — "the
window reserves 8 bytes where the filter reads 7 taps, so the last byte is one past
what is read". The guard was in fact HONEST: the 8bpc x86_64 H kernels really do
load all 8 bytes. `loop_filter_4_8bpc_wd6_simd_h`'s `load_row_hi` pulls a 4-byte
chunk at `+1` and `..._wd16_simd_h`'s `load_chunk(_, 5)` pulls one at `+5`, but only
lanes 0/1 of each survive `transpose4` (`hi[2]`/`hi[3]` and `c3[2]`/`c3[3]` are
never bound), so the trailing lanes were loaded and discarded — narrowing the guard
would have made it lie about a read that genuinely happens.

The second, compounding defect is the one the panic geometry actually points at: the
H window was sized from the **plane's worst case** ((3,5) chroma / (7,9) luma)
rather than from the run's mask (`lf_run_reach`). `default_picture_alloc` pads a
stride only when it is a multiple of 1024, so a 768-wide 4:2:0 frame's chroma plane
(stride 384 = its width) gets no right-hand padding, and at the last 4-column group
`edge + 5` is the **first pixel of the next picture row** — which
`owned_recon::stitch_sbrow` legitimately holds `&mut` over while reconstructing the
next superblock row. Intermittent because it needs `t > 1` and that scheduling.

Fixed upstream in `3426ebf7`: both kernels load only the bytes they consume (dead
lanes zero-filled — bit-identical by construction, since `transpose4`'s outputs 0
and 1 depend only on input lanes 0 and 1), and both dispatchers size the H window
from `lf_run_reach` through one shared `lf_compact_window`. The SIMD-vs-scalar
fallback predicate deliberately keeps its old 9/5 values so the SIMD/scalar decision
— and therefore every output byte — is unchanged. `zenavif`'s pin moved
`140f9145` -> `66f58fa6` on 2026-08-29 to pick it up.

**Lesson to carry:** "the guard is wider than the taps" is not by itself evidence of
an over-reservation. Check what the kernel LOADS, not what the filter's tap ladder
reaches — a chunked SIMD load can read past the last tap and still be a real read.
And an H window has a picture ROW to stay inside, not just a superblock row.

### Grid AVIF + alpha is REFUSED, not decoded (zenavif#40, 2026-08-26) — alpha-grid stitching still unimplemented
Every grid decode path (buffered, `decode_full`, streaming sink, aom backend)
used to decode the colour tiles only and return OPAQUE pixels with `Ok` for
files carrying transparency — both the per-tile-`auxl` shape
(`tests/vectors/libavif/color_grid_alpha_nogrid.avif`) and the alpha-grid
shape (`color_grid_alpha_grid_gainmap_nogrid.avif`, whose `alpha_metadata()`
is `Some(Err)` because the alpha item's payload is an 8-byte grid descriptor,
not AV1). Now `reject_grid_alpha()` (`src/decoder_managed/grid.rs`) returns
`Error::Unsupported` whenever `AvifParser::has_alpha_aux_items()` is true
(container-wide `auxC` alpha scan; `alpha_data()` only sees a primary-item
`auxl`). **Follow-up to actually support it:** decode the alpha grid's own
`dimg` tiles (zenavif-parse must expose them — it currently extracts tiles
only for the primary grid), stitch with the same validated-uniform-tile
placement as the colour grid, then hand the stitched alpha plane to
`convert_to_image`; for the per-tile-`auxl` shape, pair each colour tile with
its own alpha tile before stitching. Regression: `tests/sweep_40_geometry.rs`.

### zenav1-svt MONOCHROME partial superblocks were mis-coded at SVT preset 6 (found 2026-08-27 while landing zenavif#32) — FIXED upstream `b6a1737a` + `1ed7db46` the same day; seam gate lowered to preset 6
History: the port's `try_encode_frame` (mono) asserted partial-SB support
from preset 6, but measured at `../zenav1-svt` @ 45aae91b5 (aarch64) every
8-aligned non-64-multiple mono cell at preset 6 was wrong: 96x80 decoded
to 18 dB garbage (rav1d-safe and zenav1-aom byte-agreeing, so encoder-side),
64x72 / 72x64 26 dB, 16x72 12 dB, 128x80 / 96x64 / 200x136 undecodable.
Presets 7–13 were clean on every cell; the 4:2:0 colour path was clean at
presets 6–13 including odd dims. The seam gated mono at preset ≥ 7 with
the canary `svt_rs_direct_mono_partial_sb_preset6_still_broken`.
**Root (upstream, `partition.rs::encode_fixed_tree`):** the spec-5.11.4
single-edge rule (a one-false PD0 leaf codes as the single legal HORZ/VERT
rect, never a NONE square) lived only in the funnel arm (4:2:0); the mono
arm fell through to a full-size PARTITION_NONE. Preset-6-specific because
the M6 PD0 keeps NSQ geometry on and TESTS one-false nodes; ≥ 7 force-splits
them. The pack's `debug_assert` names it, which is why the canary PANICKED
(not "still broken") the first time CI ran `cargo test` (dev profile —
`--release` compiles the assert out and produces the 18 dB garbage
instead). Fixed in zenav1-svt `b6a1737a` (regression test + spotcheck
decode cells). Fixing it exposed a SECOND mono defect, `1ed7db46`: with the
edge leaf coded as a rect, a thin right edge (aligned width ≡ 8 mod 64,
e.g. 200x136) makes the rect straddle the aligned width and the mono leaf
coder stored its full width at the aligned stride — wrapping into the next
row's columns 0..24 and corrupting the encoder's above reference for the
following SB row (rav1d-safe 27.9 dB at 200x136, first SB row clean, every
later row wrong from column 0 outward; aomdec still DECODED it, so a
decodability gate cannot see this class). Fixed with the same store clip
the funnel path already had; upstream gate = a recon-vs-aomdec spotcheck
cell on the `(x+y)` ramp. Seam: the mono preset gate dropped from 7 to 6
(`MONO_PARTIAL_SB_MIN_PRESET = 6` — since 2026-08-29 this floor is
mono-ONLY; the 4:2:0 colour path has no preset floor at all, see the
partial-SB entry below; the multiples-of-8 rule stays), and the canary
became the positive gate `svt_rs_direct_mono_partial_sb_preset6_roundtrips`
(7 geometries through rav1d-safe, zenav1-aom byte-agreeing; 96x80 56.18 dB at
qp 10). Lesson for the seam: a `still_broken` canary that drives the
backend directly must expect BOTH failure shapes of the bug it pins — a
typed/garbage result AND a debug-build assert — or it reads as a new
regression the day CI builds it unoptimised.

### zenav1-svt QP 0 (imazen/zenav1-svt#5, #9) — IMPLEMENTED upstream 2026-08-28 (was: corrupt, then refused); seam clamp RETAINED as a product choice
History runs corrupt -> refused -> implemented, and the seam gate followed it
each time:
1. rev 3e25f52b: every CQP QP-0 still encode emitted a valid-syntax bitstream
   decoding to garbage pixels (ssim2 ~ -700; rav1d-safe and zenav1-aom
   byte-agreed on the garbage => encoder-side; 115/115 sweep cells, boundary
   clean at QP 1), plus one 64x64 stream rav1d rejected.
2. `f0f0a70ca` (2026-07-22): base_qindex 0 became a typed
   `EncodeError::UnsupportedConfig` from `try_encode_frame*` — corruption no
   longer reachable. The seam test asserted THAT refusal.
3. `aeb619cd8` (frame header) + `75cf7b0f7` (tile half, fixes #5) +
   `129d45494` (`encode_yuv420` emits a real AV1 bitstream; `with_lossless`
   honoured on 4:2:0 — issue #9 items 6-7), 2026-08-28: **coded-lossless
   ENCODES** on the 8-bit 4:2:0 still path. This is the first CAPABILITY
   refusal that port has ever RETIRED (its inventory went 15 -> 14). Upstream
   gate `tools/lossless_gate.sh`: 112/144 byte-identical to C (presets 4-13
   all 96/96, incl. partial superblocks) + 32 self-promoting pinned cells,
   **144/144 decode to the source**. Still typed-refused at QP 0: monochrome,
   10-bit, HDR-fork mode, screen-content tools, superres, inter frames
   (`EncodePipeline::lossless_config_error`).

**This is why zenavif CI went red on run 33226351115** (2026-08-29): the seam's
`svt_rs_direct_qp0_rejected_typed` asserted a refusal that no longer exists, so
it failed on every platform. It was REPLACED (not deleted, not loosened) by the
stronger property the refusal was standing in for:
`svt_rs_direct_qp0_codes_lossless_420` — qp0 through the pipeline must produce a
stream our OWN decoders reconstruct EXACTLY (rav1d-safe recon == source on all
three planes at 64x64 + 128x64 x presets {6,7,9}, zenav1-aom byte-agreeing).
Mutation-verified 2026-08-29: re-running that body at qp 1 fails at the first
luma pixel, so the equality assertions are load-bearing. The surviving refusals
keep typed assertions in
`svt_rs_direct_qp0_typed_refusal_outside_420_8bit` (mono + 10-bit, each pinned
to its specific `lossless_config_error` arm by message).

The seam KEEPS the QP >= 1 clamp (`quality_to_qp_gated` in
`src/encoder_svt_rs.rs`), now as a deliberate **product** choice rather than a
corruption guard or a refusal workaround: quality 100 must ENCODE, and mapping
it to QP 0 would silently switch coding modes (WHT/TX_4X4, no in-loop filters)
and multiply file size; the RGB -> 4:2:0 conversion means a coded-lossless AV1
frame is still not a lossless IMAGE round-trip; and `EncoderConfig` has no
lossless request for this backend to hang the choice off. Clamp side stays
covered by `svt_rs_quality_100_does_not_corrupt`. **Open decision for the owner,
tracked as zenavif#42:** whether to add a lossless request to `EncoderConfig`
for this backend and let quality 100 (or an explicit flag) reach QP 0 — note an
RGBA encode emits a Cs400 alpha item, which upstream still refuses at QP 0, so
"lossless RGBA" needs an answer either way. Record:
`benchmarks/backend_sweep_2026-07-22.{tsv,meta}`.

### zenav1-svt partial superblocks below SVT preset 6 — seam floor REMOVED for the colour path 2026-08-29 (upstream premise retired)
The seam refused non-64-multiple dimensions below SVT preset 6 (zenavif speed
5) on the premise that "the zenav1-svt partition search is not C-identical on a
partial superblock" at presets 0-5. That premise is retired. Until 2026-08-04
upstream gated its C-faithful PD1 refinement walk on a COMPLETE superblock
(`refined = matches!(preset, 0..=5) && use_funnel && full_sb`), so a partial SB
at those presets fell back to a plain PD0 fixed tree — a search C never runs.
The `full_sb` gate is gone (the walk is edge-aware), and
`tools/partial_sb_gate.sh` grew a 23-cell presets-0-5 block, every cell
byte-identical to real SvtAv1EncApp v4.2.0. Gate total **146/146 on aarch64 /
145/145 on the x86-64 CI runner** — the one-cell difference is an ISA-scoped
C-side divergence (upstream `SUSPECTED-C-BUGS.md` #9; the port is proven to
emit one bitstream on any ISA by its own `tier_invariance` test), not a port
variable. Anti-vacuity measured adversarially: restoring `&& full_sb` drops the
gate to 118/141 with all 23 failures inside that block.

**The residual is NOT dimension-conditioned, which is why a dimension gate was
the wrong tool.** Measured over 36 cells per preset (9 non-64-aligned
geometries x {gradient, screen} x {q20, q48}): every `gradient` cell
byte-matches at p0..p3 and p5; the misses are `screen` content at p0/p1/p2 (+4
cells at p4) — upstream issue #71, the palette/IntraBC over-picking RD class,
which fires on **64-ALIGNED** 256/384/512 screen frames too. Those aligned
frames this seam has always accepted at every speed, so the 64-multiple rule
never protected anyone from that class; it only refused correct encodes. And
the residual is an RD divergence (different partition choices, different
bytes), not corruption: `tools/arbitrary_size_robustness.sh` is 128/128
panic-free-and-decodable with 0 refused across every preset.

Seam: `PARTIAL_SB_MIN_PRESET` is renamed `MONO_PARTIAL_SB_MIN_PRESET` and now
gates **only** the Cs400 mono path (alpha aux item, grayscale colour item),
because nothing upstream measures mono partial superblocks below preset 6 —
`partial_sb_gate` is bd8 4:2:0 by its own scope line, and the mono evidence is
the preset-6 edge-leaf fix (`b6a1737a` + `1ed7db46`). A floor with no
measurement under it stays where the measurement stops. Tests: the refusal test
`svt_rs_rejects_partial_sb_dims_below_preset_6` was REPLACED by
`svt_rs_partial_sb_roundtrip_at_low_presets` (96x96 / 65x72 / 100x37 x speeds
1-4 = SVT presets 0/1/3/4, decode at the TRUE size, measured 49.3-50.9 dB vs a
38 dB floor), and the mono floor keeps a refusal gate in
`svt_rs_mono_partial_sb_still_refused_below_preset_6` (which also asserts the
SAME geometry now validates without alpha).

### Cross-backend status — CURRENT (2026-09-02)
Both backend deps are **git-rev pins** again, not sibling path deps:
zenav1-aom `14124356`, zenav1-svt `ef0b122b` (`85af725`). The path deps had
two costs — a plain clone of this repo could not run any cargo command (the
Dependabot / git-consumer breakage in the CHANGELOG Workspace section), and the
svt seam was building against a **dirty** sibling working copy (71 uncommitted
lines in `svtav1-encoder` on 2026-09-02), so no local svt measurement was
reproducible or comparable with CI. The CI job `resolve-standalone` keeps them
pinned; it clones only `../zenanalyze`, asserts the AV1 siblings are absent and
requires `cargo metadata` to pass. `zenanalyze` / `zenpredict` are still
escaping path deps and are the remaining reason a bare clone cannot resolve.
The old "`imazen/svtav1` is UNFETCHABLE" premise was re-measured and is STALE —
that was the renamed URL, not a git dep on `imazen/zenav1-svt`, which resolves
clean with submodules.

Measured at those pins (`benchmarks/backend_sweep_2026-09-02.*` +
`..._partialsb_...`, 3720 rows total, M4 Pro, speed 6): cross-decoder gate
**3720/3720 byte-identical** rav1d-safe vs zenav1-aom; svt-rs bytes at matched
ssim2 0.94-1.05x zenravif at 64-128 and 1.03-1.18x at 200-512; encode wall
time 17-33x faster. Partial superblocks: 600/600 svt-rs cells encoded at sizes
{65,100,200,333,500} with no dimension refusal. Do NOT diff these against the
2026-07-22 numbers below — different pins, box, speed set and image count.

There **is** an aom ENCODE backend as of 2026-09-02 — `Av1Backend::Zenav1Aom`
behind the `zenav1-aom-encode` feature. The 2026-07 audit that said there was
not (CHANGELOG `[Unreleased]` "Investigated" entry, kept as written and marked
SUPERSEDED) ended by naming the one thing missing: *a key-frame encode that
authors its own headers and wraps a temporal unit*. That is now
`aom_encode::key_frame::encode_key_frame`, and both gaps the audit named are
closed — the port authors its own sequence header with no C bytes copied, and
emits its own `OBU_TEMPORAL_DELIMITER`.

Verified here, not taken on report: the sibling gate
`crates/aom-encode/tests/self_contained_key_frame.rs` re-run 2026-09-02 at
`c3e1b4ab` is 6/6 tests, **186/186 cells byte-identical to real aomenc**, 20
decode cells under both real libaom and the in-repo decoder. The zenavif seam
does NOT claim byte parity with aomenc; it gates decodability
(`tests/aom_encode_backend.rs`). Scope is KEY-frame / still only — see the
`zenav1-aom-encode` feature bullet above for what is refused and why.

### Cross-backend status (2026-07-22 session; svt pin re-bumped 2026-07-24) — SUPERSEDED by the section above
Pins bumped: zenav1-aom `7b972e50` (structured DecodeError/DecodeConfig +
bd8 lowbd perf), zenav1-svt `2d585bb2b` (2026-07-24, was `3e25f52b`; adds
the typed QP-0 rejection #5, the IBC screen-content vertical, bd8/bd10
real-photo p0-p3 exchange-sort parity, nz-map SIMD, and the pure-Rust-crate
repo restructure that moved the C reference to a submodule — zero seam API
breaks) — both seams variant-map errors, svt seam is on `try_encode_frame*` +
threads the stop token and the config's thread budget.
Cross-decoder gate: 7018/7018 sweep cells + `tests/cross_backend_decode.rs`
(incl. 10-bit) byte-identical rav1d-safe vs zenav1-aom on OUR encoders' output.
Unified ssim2 targeting: `encode_rgb8_with_target` converges over Zenav1Svt
(test pinned). RD (backend_sweep_2026-07-22): svt 0.93x bytes at matched
ssim2 at s2, 1.02-1.07x at s6, worse below q30; wall ~6x faster at s6 /
4.4x at s9 / slower at s2 (linear speed->preset ladders misaligned —
re-fit deliberately NOT shipped, user decision).

### yuv crate 4:2:0 bilinear drops the last row pair — FIXED UPSTREAM in 0.8.17; the in-repo wrapper is RETIRED (2026-08-29)
**Resolved. Read this before re-deriving anything below.** `yuv` 0.8.17 fixes the
defect; `yuv` moved 0.8.12 → 0.8.17, `src/yuv_bilinear_fix.rs` and all eight of
its call sites in `src/decoder.rs` are deleted, and the reverse tripwire is
replaced by the positive `tests/yuv_upstream_bilinear.rs` (mutation-verified: it
fails against `=0.8.15`). Byte-identity gate before the deletion — 2,304 cells,
all eight converter variants × 16 geometries × 2 ranges × 3 matrices × 3 content
classes — **2,304/2,304 byte-identical** between wrapper-on-0.8.15 and
direct-on-0.8.17, against a control that differs in 1,836. Harness
`dev/yuv-bilinear-retirement/`, results
`benchmarks/yuv_bilinear_retirement_2026-08-29.{tsv,meta}`.
**Scope fact worth keeping:** every one of those call sites was in
`src/decoder.rs`, i.e. behind `unsafe-asm`, which nothing compiles — so the
wrapper had been dead code in every buildable configuration, and the historical
claim below that the bug hit "every zenavif 4:2:0+alpha decode" overstates the
live blast radius as the code stands today. The gate proves the retirement is
safe even if `unsafe-asm` is revived. Historical record follows.

### (historical) yuv crate 4:2:0 bilinear drops the last row pair — was FIXED in-repo (d3ece8e), upstream then OPEN
Found 2026-07-19 by the Zenav1Svt RGBA round-trip test: yuvutils-rs 0.8.12–0.8.16
`yuv420_*_bilinear`/`i0xx_*_bilinear` zip luma row-pairs against overlapping
chroma `windows(stride*2).step_by(stride)` — one window short on even heights
with exact-size chroma planes, so the bottom two output rows stay unwritten
(black/alpha-0). Hit every zenavif 4:2:0+alpha decode, the exotic-matrix RGB
fallback, raw-OBU gain-map decode, and all legacy `unsafe-asm` 4:2:0 paths.
In-repo fix: `src/yuv_bilinear_fix.rs` wrapper at all 12 call sites, with a
canary test that fails when upstream fixes it (then the wrapper retires).
Upstream issue NOT yet filed (third-party repo — draft awaiting user
approval in the session scratchpad).

### zenrav1e lossless ±2 — FIXED upstream, release-gated
Root cause found and fixed 2026-06-11 (zenrav1e c3567081, zenrav1e#9
closed): "lossless" never reached qindex 0 — the rate path floored it
at 1, so every lossless encode was qi=1 lossy. The same fix resolves
the zenavif#8 size-vs-speed inversion (validated bit-exact + monotonic
via path-patch; `benchmarks/lossless_speed_sweep_fixed_2026-06-11.tsv`).
**Until zenrav1e 0.1.5 releases and the zenravif → zenavif dep chain
bumps**, builds still ship the qi=1-lossy behavior: the identity
roundtrip tests keep their documented ≤2 tolerance.
**2026-07-23: the #8 size-vs-speed inversion is MITIGATED zenavif-side
and #8 is closed** (zenavif@7198953): lossless encodes clamp their
running speed preset into the empirically size-optimal [6, 8] band
(`LOSSLESS_REGISTRY_SPEED_BAND` in `src/encode_plan.rs`, applied in
`build_ravif_encoder` / `resolve_plan` / the sweep fingerprint; lossy
untouched). Measured: requested s1 −5.0..−15.2% bytes at 3.2-3.8×
faster, requested s10 −6.9..−31.9% bytes on 4/5 sources; residual
in-band s6-vs-s8 inversion ≤ ~1.1%
(`benchmarks/lossless_speed_clamp_2026-07-23.tsv`). At the dep bump:
set `LOSSLESS_REGISTRY_SPEED_BAND` to `None` + delete its
`lossless_clamps_speed_into_registry_band` test (the fixed encoder is
byte-monotonic — slower = smaller), tighten
`tests/identity_roundtrip.rs` to exact, re-run
`examples/lossless_speed_sweep.rs`.


### zenrav1e topdown partition search missing HORZ/VERT — FIXED upstream, release-gated
Root cause found and fixed 2026-07-01 (zenrav1e@665e58e4, pushed to `master`):
`encode_partition_topdown` (the primary partition-search path cavif/zenavif use; ravif's
own `encode_bottomup` speed setting is off by default, but `encode_partition_bottomup`
is ALSO forced regardless of that setting for any superblock straddling the frame's
right/bottom edge — corrected 2026-07-01, see below) hardcoded its RDO candidate list to
`[SPLIT, NONE]`, so `PARTITION_HORZ`/`VERT` were never offered at any speed. This is also
why cavif's `-s1` and `-s2` were byte-identical (`non_square_partition_max_threshold`
only mattered in the bottom-up path). Companion `ravif@b4853c68` widens speed-2's
threshold to `BLOCK_64X64`. Measured: median bpp −1.8% to −2.8% at ssim2 70-85; BD-rate
gap vs libaom +5.7%→+3.6% median (same-day, narrower methodology — see
`docs/RD_GAP_VS_LIBAOM.md`). **Until zenrav1e releases past 0.1.4 and the zenravif →
zenavif dep chain bumps**, registry builds still ship the pre-fix behavior.

**2026-07-01: extended partition types (HORZ_4/VERT_4) — FIXED, root cause was a
`BlockSize` ordinal-vs-dimension mismatch, upstream, release-gated.** The 6-of-10-types
gap referenced above (measured 10-13% area share on libaom's side) turned out to be
blocked on a real bitstream-conformance bug, not a missing feature. Root cause: libaom's
`av1_use_angle_delta`/`av1_allow_palette` gate several per-block syntax elements on
`bsize >= BLOCK_8X8`, which is a genuinely *ordinal* comparison in libaom's C
`BLOCK_SIZE` enum (the six "extended" 4:1-aspect sizes are numbered after every classic
size, so the comparison is always `true` for them regardless of actual dimensions).
zenrav1e's textually-identical `bsize >= BlockSize::BLOCK_8X8` is NOT ordinal — `BlockSize`
has a custom width/height-based `PartialOrd`, under which `BLOCK_4X16`/`BLOCK_16X4` are
*incomparable* with `BLOCK_8X8` (one dimension smaller, one larger), so `>=` silently
evaluated `false` where it should have been `true`. The encoder skipped writing a
required `angle_delta` syntax element for directional-mode blocks of those two sizes —
a missing-symbol bitstream desync that `aomdec` correctly rejected as corrupt (the exact
symptom the previous attempt hit and reverted, per zenrav1e#26). Fixed via
`BlockSize::ge_8x8_ordinal()` (zenrav1e@2866397e) + Phase 1 of `PARTITION_HORZ_4`/`VERT_4`
re-implemented on top (zenrav1e@7d254289). Verified clean on 110 cells (22-image photo
corpus x 5 quality levels): 0 `aomdec` corruption (was 100% corrupt before the ordinal
fix, direct A/B confirmed causality), extended block-size area share 1.8-56% per cell,
rav1d-safe round-trip pixel diff scaling normally with quality. Measured RD impact:
`docs/RD_GAP_VS_LIBAOM.md` "Fixed 2026-07-01 (4)".

**2026-07-01 (later): `PARTITION_HORZ_A/B`/`VERT_A/B` (Phase 2, the other 4 of 6) —
implemented, verified conformance-clean, measured as a NET RD REGRESSION, reverted.**
Same 110-cell aomdec-clean bar as Phase 1, all 4 types genuinely chosen by the search —
but direct-isolation BD-rate median +0.60% (worse on 14/22 images), median BD-rate vs
libaom-slow +0.1%→+0.6%, ~1.46x encode time. Root cause of the regression is a
SPLIT-cost-estimation bias in zenrav1e's one-level topdown trial (SPLIT evaluated
pessimistically as 4 NONE-leaves while the mixed 3-way types are evaluated exactly),
not a defect in the new types. Not on zenrav1e master; the full implementation (plus 2
real bug fixes it required) is preserved as zenrav1e workspace commit `a7630aee`.
zenrav1e#27 stays open re-scoped to the trial-SPLIT-cost-accuracy dependency. See
`docs/RD_GAP_VS_LIBAOM.md` "TRIED AND REVERTED 2026-07-01" +
`benchmarks/rd_gap_extended_partitions_phase2_2026-07-01.tsv`.

**2026-07-02: trial-SPLIT-cost accuracy — FIXED, TRUE RD PARITY REACHED, upstream,
release-gated.** The dependency above is resolved: `zenrav1e@b073182c` (master) refines each
SPLIT child's trial cost to min(NONE-leaf, tell-metered child-SPLIT symbol + 4 quarter
NONE-leaves), eliminating the pessimistic bias. Measured: **BD-rate vs libaom-slow median
+0.0695% → −0.6487% (crosses the ≤0% parity target), mean +2.1734% → +0.2373%**, improved
16/19 images, 1.057x median encode time; 110/110 aomdec-clean + 110/110 rav1d-safe roundtrip.
This also unblocks a Phase 2 re-attempt from `a7630aee` (the types regressed against the
then-underestimated SPLIT). Registry builds ship pre-fix behavior until zenrav1e releases past
0.1.4 + the dep chain bumps. See `docs/RD_GAP_VS_LIBAOM.md` "Fixed 2026-07-02" +
`benchmarks/rd_gap_splitcost_2026-07-02.tsv`.

**2026-07-02 (later): Phase 2 v2 re-measured on the fixed estimate — the regression FLIPPED
TO A WIN** (direct isolation −0.5759% median, better 17/22; vs cpu2 −0.65%→−1.87% median) at
1.461× encode time → preserved as zenrav1e workspace commit `dfed8eda`, the prime `-s1`
deep-mode ingredient (above the 1.2× matched-speed gate for s2). See
`benchmarks/rd_gap_phase2v2_2026-07-02.tsv`.

**2026-07-02 (final): the `-s1` deep mode SHIPPED, release-gated.** zenrav1e master gained
two default-off knobs — `mixed_3way_partitions` (`efbe0cf2`, the gated Phase 2 v2) and
`split_trial_depth` (`2fac1af6`, recursive SPLIT-trial refinement) — both byte-identical
off (9/9-cell md5 each). ravif main (`9d2b97c`) arms s1 with mixed-3way + unconditional
rdo_tx_decision + partition_range (4,32) (winner of a 16/32/64 × depth{1,2} ablation)
behind `S1_DEEP_ARMS_LIVE = false` until the zenrav1e dep bump (byte-identical until then,
6/6-cell md5 vs b4853c68 on registry deps; flip the const + uncomment 2 apply lines at the
bump). Measured at the live config: **median BD-rate −0.97% vs libaom cpu-used=0
(slowest-best; s2 was +1.47%), −3.01% vs cpu2, 11/19 photos win per-image; 8 named photos
still lose (o_6629 +25.3 worst) — partition levers exhausted, residual is
coefficient-level RD.** 110/110 aomdec+rav1d conformance at the shipped config (+3 more
110-cell configs). ~3.7× cpu0 wall per cell (RD-first mode by design). Full record:
`docs/RD_GAP_VS_LIBAOM.md` "s1 deep mode" + `benchmarks/rd_gap_s1_2026-07-02.tsv`.

**2026-07-03 CONSOLIDATED CURRENT POSITION (one fresh measurement of the composed
shipped config — supersedes the stacked per-mechanism deltas as the status statement).**
cavif s2/s1 + Tune::Ssimulacra2 + palette=Auto at zenrav1e master `c9c2d5f7`: legacy
photos (n=19) ssim2 BD median **−12.29% vs cpu2 / −11.58% vs cpu0-default / +0.05%
(dead even) vs cpu0-ss2tune** at s2 (s1: −12.06/−11.31/+0.37), butteraugli negative vs
every ref at both speeds, and zr-s2 runs at **0.86–0.98× the wall cost of the
cpu0-ss2tune reference it ties** (1.15× cpu0-default; s1 = 2.2–2.6×/3.0–3.6× resp.).
train26 (24 TRAIN origins, first aom-referenced statement): vs cpu2 −13.33 (s2) /
−13.66 (s1) median, **tier-2 median crosses at both speeds** (−0.34/−0.11, means −3.2).
Holdouts: o_5004 RESCUED (−23 vs cpu2), o_6629 ~even (s1 wins vs cpu2+cpu0def), o_9051
remains the one material loser (+12 vs cpu2, ~even at tier-2). Zero reach failures on
legacy; conformance 220/220 at the composed config. Full record:
`docs/RD_GAP_VS_LIBAOM.md` "CURRENT POSITION (consolidated 2026-07-03)" +
`benchmarks/rd_gap_final_2026-07-03.tsv` (+ pointer to raws).

### zenrav1e 64×64-parent HORZ_4/VERT_4 sliver corruption — FIXED upstream, release-gated
Found 2026-07-02 by the partition_range re-test: BLOCK_64X16/16X64 slivers (only reachable
with `partition_range` max=64, e.g. via the public `override_partition_range`) coded their
never-validated TX_64X16/TX_16X64 max transforms and desynced BOTH `aomdec` ("Corrupted
segment_ids") and rav1d-safe. Initial attribution to `b073182c`'s deeper SPLIT estimate was
WRONG — exonerated by a six-variant bisect. Fixed `zenrav1e@3fa735dc`: intra slivers cap to
TX_32X16/16X32, the tx-size RDO walk shrinks by the consumed level (else an out-of-alphabet
depth-3 symbol — a second latent corruption caught in the same fix; that bound is now a hard
assert in all builds), inter frames without `enable_inter_txfm_split` don't offer 64-parent
4-way candidates. Byte-identical at shipped configs; 264/264-cell clean sweep at (4,64).
zenrav1e#28 tracks validating the real 64-dim sliver transforms. Registry builds can hit the
corruption via `override_partition_range` until zenrav1e releases past 0.1.4. The bisect
lesson (silent-corruption bug classes + the s.replace() trap) is in the project memory.

The prange (4,64) s2 widening itself REMAINS RULED OUT on clean data (+0.48% median direct,
worse 15/22; `benchmarks/rd_gap_prange_retest_2026-07-02.tsv`) — but now with 7/22 winners
(three at −1.8..−2.5%), making a content-adaptive large-block gate a feature-hints candidate.

**2026-07-03 follow-up (zenrav1e#34, FIXED upstream `1dabba91`): the 3fa735dc sliver cap was
itself only half-sound.** The cap is decoder-followable only via the written intra tx-size
depth — a TX_MODE_SELECT symbol. With `rdo_tx_decision=false` (frame header TX_MODE_LARGEST,
e.g. ravif's high-quality band, or stock zenrav1e speed 6-8) no tx-size symbol exists and
conforming decoders derived the uncapped TX_64X16/16X64 against capped coefficient units —
guaranteed desync at every 4-way 64-parent pick. Found by the sizedecay non-tune (4,64)
isolation arm (100% DECFAIL at q≥78); bisected latent-since-`7d254289` (pre-Phase-1
(4,64)+rdo-off was CLEAN, so plain TX_64X64/64X32/32X64 code fine under LARGEST — the
"never-validated 64-dim transforms" framing above was too broad). Fix: intra 64-parent 4-way
candidates now require `tx_mode_select`; hard asserts at both cap sites. 6/6 corrupt shapes
verified clean under aomdec+rav1d-safe; byte-identical at shipped defaults. Registry builds
carry BOTH sliver bugs until the release past 0.1.4.

### 4:2:0 sliver-chroma desync (was "ravif 4:2:0 non-conformant") — FIXED upstream, master-only (zenavif#29 → zenrav1e#35)
Found 2026-07-03 by the sizedecay non-tune `yuv420` diagnostic arm (PALCONF gating);
root-caused + fixed same day. **The issue's original scope was half wrong: registry
zenrav1e 0.1.4 is NOT affected** (re-verified 63/63 cells clean on plain ravif@main +
0.1.4 — the discovery sweep's "registry" leg had evidently run a master-backed binary).
Real scope: zenrav1e MASTER only, introduced by `7d254289` (HORZ_4/VERT_4, 2026-07-01,
git-bisected). Root cause: the chroma TU-grid math in `write_tx_blocks`/`write_tx_tree`
shifted mi dims by the subsampling with a 1×1 zero-fallback — correct for the classic
4x4..8x8 pairing shapes, but for `BLOCK_16X4`@420 it clobbered the 2×1-mi coded-chroma
extent and the divide by TX_8X4's 2-mi width truncated the TU loop to ZERO iterations:
no chroma TUs written while conforming decoders parse a TX_8X4 TU there
(`Subsampled_Size[16X4][1][1]=8X4`, spec 5.11.38). Only {16X4, 4X16} hit it = H4/V4 on
16×16 parents; 444 has no pairing → clean, which is why the partition program's 110-cell
sweep (444, cavif default) missed it. The zenrav1e CLI can't reach the types at any speed
(preset ≥2 caps the non-square threshold at 8×8; ≤1 is bottom-up) — ravif's s2
(topdown + threshold 64×64 + prange (4,16)) had maximum exposure. Fixed
`zenrav1e@17e67842`: TU grid from `BlockSize::subsampled_size`; regression gate
`tests/sliver_chroma_roundtrip.rs` (fails pre-fix, liveness-checked). Verified: 258/258
420 cells (43 renditions × 6 q) aomdec-clean + aomdec==rav1d-safe raw md5
(`benchmarks/conformance_420_sliver_fix_2026-07-03.tsv`); 36/36 444 cells byte-identical
pre/post. **No released ravif/zenavif version ever shipped it** (0.1.4 predates topdown
4-way types). rav1d-safe masked 33/33 corpus repros (decoded garbage without error; the
synthetic test stream it does reject — data-dependent) → filed imazen/rav1d-safe#422;
zenavif round-trip tests alone remain insufficient as a conformance gate — keep the
aomdec PALCONF leg in sweeps.

### Tune::Ssimulacra2 — SHIPPED upstream 2026-07-02, release-gated
`zenrav1e@a37faea8` adds `Tune::Ssimulacra2` (aom-parity chroma delta-q + ss2
QM curves; the other three aom mechanisms measured as regressions and were
dropped — see docs/TUNE_SSIMULACRA2_PLAN.md). Measured: s2 −4.28% / s1-deep
−3.57% median ssim2 BD vs tune-off, beats aom cpu0-default at both speeds,
tier-2 gap vs cpu0-ss2tune +11.08% → +8.71%. 220/220 conformance cells clean.
Since extended by per-SB delta_q Variance Boost (tier-2 → +5.63%/+5.02%),
the QM-weighted RD distortion ratio (2026-07-03, tier-2 s2 +5.63% → +2.12%,
s1 +5.02% → **−1.94% — tier-2 median crossed**;
docs/RD_GAP_VS_LIBAOM.md "QM-weighted RD distortion"), and the LF sharpness
schedule {7,5,3}@{80,160} (2026-07-03, zenrav1e#30 item 1 / zenrav1e@9a05d54a:
direct −0.43%/−0.42% med at s2/s1, butteraugli sign-divergent but far under
veto, 220/220 conformance; the groundwork commit c1fab5b3 also fixed
nonzero-sharpness encodes desyncing encoder recon from conforming decoders —
docs/RD_GAP_VS_LIBAOM.md "LF sharpness schedule"). Same-day desync fixes
(#32 LRF, #33 filter-intra) + these landed tier-2 medians CROSSED at BOTH
speeds pre-sharpness (s2 −1.54, s1 −2.10 on fresh 2026-07-03 baselines).
2026-07-03 (later): the size-decay isolation A/B acquitted 4/5 tune
mechanisms for the small-rendition decay and shipped a size-conditional
strength for the QM-dist ratio (`zenrav1e@b0098eb1`:
qm_dist_ratio_m = clamp((log2(longedge)−8)/2, 0.5, 1.0); train +1.03/+0.87
@256/512 vs full strength, VAL-confirmed, conformance 180/180 — see
docs/RD_GAP_VS_LIBAOM.md "Size-decay isolation A/B").
**At the zenrav1e dep bump:** wire tune selection through zenravif/zenavif
(the sweep used a dev-only ZENRAVIF_TUNE env passthrough, reverted) and decide
the default for still images. Also: **encode_plan mirror update (dep-bump)** —
`src/encode_plan.rs` mirrors the REGISTRY-era SpeedTweaks table by design;
refresh it to the released table in the same change (s1 deep arms, the
SMALL_PX_RDO_TX_LIVE small-rendition tx-RDO flip [ravif@2a69a9dc, user
sign-off 2026-07-03], partition_range arms, palette/tune knob forwards). From the libavif v1.4.0 study
(`docs/LIBAVIF_1_4_STUDY.md`, mechanisms c3/c5): (1) **never apply a
perceptual tune to the ALPHA channel** — libavif measured ringing from
perceptual tunes on alpha and pins alpha to tune=psnr; A/B our alpha path
(currently Tune::Psychovisual) and pin accordingly; (2) if the ss2 tune
becomes the still default, **re-fit the quality→quantizer curve** — libavif's
precedent (their new piecewise LUT compensates tune=iq's higher spend at
matched QP); our curve diverges from their mapping above q70.

### FASTWINS P0 (2026-07-04): tile-policy default FIXED (live) + s6-s8 tx-size RDO arms (release-gated)
FAST_TIER_PARITY_PLAN Phase P0, record `benchmarks/rd_gap_fastwins_2026-07-04.tsv`:
(1) **ravif main 55f8c935 (LIVE)** — default tile count now capped to ≥1 MP per
tile (`TILE_RD_MIN_AREA`): the old `min(threads, px/min_tile²)` default cost
+7.4% median ssim2 BD at s6 on 48 cores (0/24 images better at ANY tile count;
`--threads 1` byte-identical 18/18, default==threads-1 18/18; give-back 5.9×/6.8×
wall at s6/s4 — the pool is bitstream-inert and tiles are zenrav1e's only
intra-frame parallelism). zenavif inherits at the next zenravif dep bump.
(2) **zenrav1e d82c16ba** — decoupled `rdo_tx_size_override` / `rdo_tx_type_override`
/ `rdo_tx_size_depth` (default-off, 27/27 md5 byte-identical); **ravif 7baad5f9**
arms s6-s8 with SIZE-half depth-1 (DCT-only) behind `S6_TX_SIZE_RDO_LIVE=false`:
full-grid s6 −2.78/−3.95/−6.01 med (ssim2/ba3n/bamax) at 1.67× solo, s8
−2.89/−3.52/−5.49 at 1.43× — 51% of the whole s6→s4 step; type-half standalone
butteraugli-max-vetoed; reduced_tx_set standalone measured null. 4,176/4,176
armed cells aomdec+rav1d-safe clean. **At the zenrav1e dep bump:** flip
`S6_TX_SIZE_RDO_LIVE` + uncomment its two apply lines (same flip pattern as
S1_DEEP_ARMS_LIVE), and include both knobs in the encode_plan mirror refresh.

### S10 PROGRAM (2026-07-05): re-tiered s9'/s10' rows — LANDED release-gated (JPEG scoreboard)
User direction: at the ultra-fast class the competitor is JPEG, not aom.
Measured (docs/S10_PROGRAM.md + benchmarks/rd_gap_s10_2026-07-05.tsv, train26 +
doccharts + canonical-mine breadth): **registry s10 LOSES to mozjpeg-class
JPEG outright** (1.05-1.06x its bytes at matched ssim2<=60; >=1.0 in 7/12
families; doccharts 1.09-1.22); the ss2 tune alone rescues to 0.79-0.84 at
4.6x jpeg-moz encode time. Cliff decomposition: tx_domain_rate −7.45% BD at
1.14x (22/22), (16,16) partition floor −13.5..−20.7 at s9 (23/23), CDEF-on
−1.70% at 1.04x, size1 −7.8%; fdi/reduced-tx null, (8,32) ruled out.
**ravif@adb88ddc `S10_RETIER_LIVE=false`**: s10' = txdr off + CDEF on +
SATD-decides intra (num_modes_rdo_override 1; zenrav1e@071e9844) — **−5.7/
−6.9/−7.8 BD vs old s10 at 0.95x its time; 315 ms/MP solo = 4.3x jpeg-moz at
0.69-0.78x its bytes**; s9' = s10' + floor (8,16) + size1 — −15.1/−18.2/
−23.6 vs old s9 at 1.62x; 663 ms/MP = 9.0x jpeg-moz at 0.54-0.60x bytes
(s9-preset expression proven byte-identical to the s10-preset form, 0/24).
Byte-gate 6/6 md5 while false; 0 CELLFAIL/CONFFAIL across ~4,000 PALCONF
cells (3 chain rounds). Residual: 5000-nps 1.01-1.12 at s10' (the s4-tier
full-tx class; tune/coeff-program-owned). Harness: jpeg_cell.sh + run_gap
JPEG_CONFIGS/enc_int_ms (zenavif@ebb98c4d) + zenjpeg@d4f88211 sweep_cell.
**At the zenrav1e dep bump:** flip `S10_RETIER_LIVE` + uncomment its
num_modes_rdo apply line (same pattern as S1_DEEP/S6_* flips), alongside the
tune-default decision the rows were measured with.

### COEFF_RD_STACK (2026-07-05): the coefficient-level wall STUDIED + PORTED + REFUTED — honest negative at every posture; knob landed default-off upstream
The wall three residual hunts named (s1 8-photo, 6096 no-skip, SSIMRD
close-out) was attacked per TUNER2's order as ONE composed knob:
`zenrav1e@3e5ff155`+`@9bc2b71a` `EncoderConfig::coeff_rd_stack`
(CoeffRdStack: flat rounding [0=fitted-Valin sentinel] + always-on descent
at λ-scale + aom sharpness guards + per-TU zero-out; default-None
byte-identical, 36/36 sha gate; rides no tune — pure A/B infra). Study
docs/COEFF_RD_STACK.md (aom 632172a4): FP-quant↔trellis COUPLING
(`skip_trellis ? B : FP`), allintra trellis speed-invariant, dropout dead
code, no cq recode loop (the "rate loop" suspect closes by inspection),
per-TU zero-out unported, and zenrav1e's Valin eob dead zone == aom's zbin
(0.656q both) — a mean-field trellis. Measured (chain_coeffrd.sh, sweep-2,
byte-continuity 288/288, rule pre-registered at bcc02310): **all 7 arms
lose** — aom-ss2 posture +3.80, aom-default posture +14.02, un-gated
descent over Valin input +2.33 (λ1.0) / +0.97 (λ0.35) mass-wmed ssim2,
butteraugli vetoes everywhere, ≤5/144 strict-Pareto cells, doccharts
replicates, 6096/1236/9094 classes flat-to-harmed. **Verdict: aom's
coefficient-level edge is its internally-coherent valuation loop, not a
transplantable piece; zenrav1e's shipped Valin offsets + exact-tell rates
+ psy-pixel dist measured superior to every transplant; the 2026-06-18
trellis quality gate is vindicated.** No zenavif dep-bump action (knob
stays default-off infra). Record: RD_GAP "COEFF_RD_STACK" +
benchmarks/rd_gap_coeffrd_2026-07-05.tsv + DECISION_RULE_COEFFRD.md.

### zenrav1e per-SB delta_q + Variance Boost — SHIPPED upstream 2026-07-02 (later), release-gated
zenrav1e gained true per-SB delta_q coding (`d125713f`, inert syntax; the
encoder previously coded none) and `Tune::Ssimulacra2` now drives libaom's
DELTA_Q_VARIANCE_BOOST through it (`66733720` + `165e83b1`: strength 1.0
offline-fit on train26 — aom's 3.0 default over-boosts on top of zenrav1e's
activity masking; 4.5/6/keep-segmentation arms butteraugli-vetoed).
Segmentation is disabled while the boost is active. Measured (legacy-corpus
confirm, photos n=19): s2 tier-2 gap +10.10% → +5.63% median, vs cpu0-default
−3.43% → −5.07%, direct −1.81% on top of the tune; s1 numbers in
docs/RD_GAP_VS_LIBAOM.md "Per-SB delta_q + Variance Boost". 4×110-cell
conformance clean (both speeds × strength 3.0 + shipped 1.0). Known residual:
o_6629 (ultra-flat gradient TEST-split origin) regresses further with the
boost (+14.2 → +32.7 vs cpu0-default; q30-40 misallocation) — per-image
strength/gating via the picker is the tracked follow-up, alongside strength-2
for smooth photos (train26 5004_nps −15.0% at str2). **At the zenrav1e dep
bump:** nothing extra to wire beyond the tune itself (the boost rides
`Tune::Ssimulacra2`); re-run the QM benchmark note below still applies.

### Butteraugli diffmap two-pass (spatial closed loop) — SHIPPED 2026-07-03, release-gated
The libaom `tune=butteraugli` analog, adapted to the landed per-SB delta_q:
zenrav1e@c4047cec adds `FrameHints { sb_q_scale }` (metric-free external
per-SB AC-q scale input, byte-identical when absent/neutral, composes on top
of Variance Boost); ravif@13b1ca4b passes it through
`expert::InternalParams.sb_q_scale` behind `pub const FRAME_HINTS_LIVE =
false`; zenavif@2e8e9912 ships the driver `encode_rgb8_two_pass` (feature
`two-pass-butteraugli`): encode → decode (own decoder) → butteraugli diffmap
→ aom-formula pool per SB (12-norm, mse/ba weight, geomean, clamp
[0.4,2.5]) → λ→q translation (weight^(strength/2)) → re-encode. Evaluate-
first verdict on aom's own tune (CONFIG_TUNE_BUTTERAUGLI=1 local build +
libjxl 0.8.2 static): photos −2.4..−3.5% median butteraugli-3n BD at
cpu2/cpu6 with ssim2 neutral-to-better on BOTH corpora
(`benchmarks/aom_tune_butteraugli_eval_2026-07-03.tsv`). ~2.05× wall.
Full record + dep-bump checklist: `docs/DIFFMAP_TWO_PASS.md`. **At the
zenrav1e dep bump:** flip `FRAME_HINTS_LIVE`, uncomment ravif's hinted send,
re-run `tests/two_pass.rs` + the A/B on registry deps.

### zenrav1e encoder-recon desyncs: filter-intra (#33) + LRF sgr-skip (#32) — FIXED upstream 2026-07-03, release-gated
zenrav1e#33: filter-intra blocks got DC_PRED's edge prep (topleft=128, no left
at x==0) and read the left edge upside-down — encoder recon diverged from
aomdec/rav1d-safe by 17-25 luma RMSE at rav1e speeds ≤6 (fixed
zenrav1e@32477046; recon now byte-agrees, 120/120 train26 conformance cells).
zenrav1e#32 as reported was this same bug misattributed (bisect skipped s7;
LRF byte-exact on 27 isolation cells); one real latent class — signaled
sgrproj unapplied to recon at cdef-off+lrf-on (API-only) — fixed
zenrav1e@17cff82f (probe: examples/recon_probe.rs). **cavif/zenravif were
never exposed**: ravif pins complex_prediction_modes=Some(false) →
filter-intra sequence-off at every speed, so cavif encodes are byte-identical
across the fixes — no rd_gap history invalidated, tier-2 recovery 0.00%.
Arming filter-intra post-fix was measured and REJECTED (+1.82% ssim2 med,
ba3n veto, 1.70× time; `benchmarks/rd_gap_desyncfix_2026-07-03.tsv`) — the
old "12 dB regression" (zenrav1e#5) was the desync, the pin stays on RD
grounds. No zenavif-side action at the dep bump (decode path unaffected;
encode bytes unchanged).

### zenrav1e intraBC chunk A — SHIPPED upstream 2026-07-03, release-gated
`zenrav1e@7a59e569` adds intra block copy behind
`SpeedSettings.prediction.intrabc` / `--intrabc` (default off,
byte-identical off). Wedge-#1 anchors vs the palette+UV base: 7052
−34.9/−39.4% ssim2-BD (s2/s6), 7050 −17.6/−23.6%; `PaletteMode::Auto`
detection-gates it (photos verified byte-identical). fam-7 legacy ladder:
off +169% → palette +75% → +UV +55% → +intraBC +57% (chunk-B hash search
is the legacy-plot headroom, zenrav1e#30 item 3). Blanket-Always
regresses photos (+3..8%, spec filters-off trade) — never ship intrabc
without the Auto gate or a zenavif-side content gate.

### zenrav1e CDF undo-log cross-field overspill — FIXED upstream, release-gated
The RDO CDF undo log restored fixed-width (16-word) snapshots with
partition-sequenced rollback, resurrecting stale adaptive state across
field boundaries (bug-class 6, silent-corruption memory). Latent since
the log existed; became a live bitstream desync the moment the UV palette
began adapting `palette_uv_color_index_cdf[0][0]` (adjacent to the luma
map table). Fixed zenrav1e@e86235b5 (exact-length snapshots + compile-time
bounds + regression test). The UV palette itself landed zenrav1e@a3b72033
(same PaletteMode knob, default Off; plots −2.0/−2.6% ssim2-BD median vs
the luma-palette base, conformance 200/200 @420 + 84/84 @444 both-decoder
md5). Registry builds ship neither until the zenrav1e release past 0.1.4.

### Fast-tier partition liveness (P1 lever 1) — SHIPPED upstream 2026-07-04, release-gated
zenrav1e master gained `PartitionSpeedSettings::topdown_prune` (725f5f71 +
one-sided margin fix 767c8ff5; default-off, byte-identical off 27/27 md5 +
144/144 base2 sentinel): NONE-first top-down candidate walk + none_breakout /
NONE-dominance margins / 4×4-log-var homogeneity gate. ravif main landed the
s4-s8 arms behind `S6_PART_PRUNE_LIVE=false` (0191489b): rect threshold
8×8→16×16 + the measured gate triple {none_breakout 1.0, four_way_margin
0.0 = 4-ways only on SPLIT-dominant blocks, homogeneity_gate 2.0} — cheaper
than ungated liveness at every tier (solo 2.16/2.08/1.75× s6/s8/s4). The
threshold value is ALSO gated (live in bottom-up edge-SB coding on registry
builds); 18/18 md5 byte-identical gated. Full-grid confirms (train26
tune-ss2, ssim2/ba3n/bamax medians): s6 −2.89/−2.51/−2.45 (24/24), s8
−3.00/−2.49/−2.86 (24/24), s4 −1.94/−2.32/−2.74 (22/23); no bamax veto.
Ladder movement: s6 vs cpu4def-ai +1.4→−4.6/−6.3 (crossed), s8 vs
cpu6iq-ai +0.3→−3.6/−5.1 (crossed), s4 vs cpu2def-ai +2.8→−0.9/−5.6
(crossed); s6 wedge recovery of the remaining step: interiors 60%, food
68%, nps 63%, ALL 77%. The mapped prune-pareto (vargate arms keeping
88-104% of the remaining s6→s4 step at 2.4-2.9×) is recorded as P2
per-image-hint targets in `benchmarks/rd_gap_p1part_2026-07-04.tsv`. Margin
gates measured dead in both semantics; skip-gated breakout a null at every
τ. **At the zenrav1e dep bump:** flip the const + uncomment the
topdown_prune apply block in ravif `speed_settings()`.

### P2 per-image budget heads (fast_heads) — LANDED 2026-07-04, release-gated
`src/fast_heads.rs` (wired into `auto_tune`, recommend-only like the palette
gate): TX budget {Largest|Size1|Min} — withhold size-RDO on razor-edge
line tilings (`patch_fraction>0.8505 && dct_compressibility_y>100`), deepen
to size1+types+reduced on smooth low-α content (`pf<=0.8505 && dcty<8.352`),
s6-s8 — and partition budget {Ship|Max32} — 32-px blocks on flat/synthetic
content (`gradient_fraction_smooth<0.4105`), s6. Composed s6 mode measured
12q (record `benchmarks/rd_gap_p2heads_2026-07-04.tsv`): train26 −4.38 med
vs s6+size1 base vs global-ship −2.89 (deviators −5.13 mean, 10/11); VAL
14-origin transfer −3.98 med (deviators −2.41 mean, 6/8, worst +0.32);
photos-vs-cpu4iq-ai median +0.57 ssim2 / −0.94 ba3n (inside the ±1% parity
band; ship was +2.88/+0.91). The W-gate's conjunctive dcty bound is a VAL
attribution revision (pf-only withhold convicted by factoring cells: 8103
(none,ship) +18.1 vs (size1,m32) −1.9). Head-3 verdict: intra top-7
(ComplexKeyframes + filter_intra=Some(false)) is a GLOBAL arm candidate
(s6 −0.56/s8 −1.17 med, no per-image structure; the top-5 knob has since
been BUILT — see the s4-tier section below). **At the zenrav1e dep bump:**
(1) add zenravif expert passthroughs for `rdo_tx_type_override` /
`reduced_tx_set` / `topdown_prune` / `non_square_partition_max_threshold`
(additive, sb_q_scale shape); (2) forward `EncoderConfig::fast_tier_budgets`
in `build_ravif_encoder` (Largest → size override off; Min → +type override
+ reduced set; Max32 → part_max 32 + 4-ways ungated per r16m32_bkvg2);
(3) flip ravif's `S6_INTRA7_LIVE` + uncomment its apply block (landed
release-gated ravif@4b98f0f8: `ComplexKeyframes` + `filter_intra=Some(false)`
at s6-8 — measured −0.56/−1.17 med + composed+i7 val 13/13) — **re-weigh
the flip against top-5 first** (S4TIER: top-5 dominated top-7 at mode
level); (4) forward `num_modes_rdo_override` through zenravif (additive
expert passthrough) so the tiers can pick their measured intra arm.

### S4-tier column CLOSED-with-residual — fast-tier parity program measurement COMPLETE (2026-07-04)
The last open pairing (aom cpu2iq-allintra, +4.40/+4.04 vs composed-v2+i7
at 1.27× its wall) closed to **+2.80 ssim2 / +4.14 ba3n photos median at
0.97× cpu2iq's wall** via the v3 heads: the tx D bound refit per-tier
(8.352 → 23.69, LOOCV 22/24; `src/fast_heads.rs` requested-speed-4..=5
gates, release-gated recommend-only) + the NEW zenrav1e top-5 intra knob
(`num_modes_rdo_override`, zenrav1e@071e9844, default-None byte-identical
6/6 md5 + 288/288 chain continuity) which DOMINATED top-7 at mode level
(6.26× vs 7.61× plain-s6 for the same column). The residual is measured
structural per-family: 8414 screens +22.5 (intraBC, P3), 1236/9100/9118
iq-AQ class (+17.2/+2.5/+7.4 — cpu2def trails cpu2iq by +100-133 BD on
exactly these; tune-program-owned), 6096/6018 rescans (+15.8/+7.2,
near-lossless floor), 5000 brochures (full-tx headroom, no stable gate —
oracle extras reach +2.36/+2.04 at 10.1×). CDEF/LRF hi-q force probes:
null/adverse (aom-iq's CDEF edge is strength adaptation, not enablement;
LRF axis clouded by open zenrav1e#32). s4-native ruled out (+4.22 at ~10×).
**Plan verdict: parity ±1% MET at s6+s8, NOT met at the s4 tier
(structural); beat-at-≥2-tiers MET; quality-tip KEPT.** Record:
`benchmarks/rd_gap_s4tier_2026-07-04.tsv` + FAST_TIER_PARITY_PLAN
§s4-tier + §success-criteria-final.

### intraBC (screen content) — chunks A+B SHIPPED upstream, release-gated, default off
Chunk A (zenrav1e@7a59e569, 2026-07-03): DV coding/validity/copy-MC/diamond
search behind `SpeedSettings.prediction.intrabc` (default off), detection-
gated via `PaletteMode::Auto`. Chunk B (zenrav1e@d655a6ee + @184eb713,
2026-07-04): libaom `av1_hash_table` port — source-luma CRC-32C pyramid,
exact-match DV candidates for square 8..64 blocks (`intrabc_hash` sub-knob,
default true, inert without `intrabc`; hash-off byte-identical 81/81 vs
0d392334). Measured hash-on vs chunk A (`benchmarks/ibc_hash_ab_2026-07-04.tsv`):
legacy fam-7 trio −22..−29% ssim2-BD (aom's own hash edge there was −33%),
7058 −36.6/−40.1%, 8414 −4.6/−5.4%, photos byte-identical, enc median
1.00×/1.06×, 400/400 armed cells aomdec+rav1d-safe clean. Rescan diagnosis
(P3 item 2) same day: 6018 = iq-AQ class (tune-program handoff), 6096 =
coefficient-level no-skip valuation (rounding/dead-zone probe named) — see
RD_GAP "Near-lossless rescans residual". **At the zenrav1e dep bump:**
expose intrabc(+hash) through zenravif/zenavif config and let the
palette-gate/auto_tune arm it on screen content; re-run the fam-7 ladder
(off +169% → … → +intraBC-A +57% → chunk B moves the legacy trio).

### zenrav1e QM encodes diverged from conforming decoders — FIXED upstream, release-gated
Two composing bugs (zenrav1e#29, both fixed on master 2026-07-02) made
`enable_qm=true` encodes decode differently than the encoder's own
reconstruction: (1) `qm_v` was gated on frame-level diff_uv_delta instead of
the sequence's separate_uv_delta_q (u==v chroma delta-qs → aomdec-rejected
frames; fixed zenrav1e@9a8eaf61), and (2) every rectangular TX quantized with
transposed QM weights (rav1e stores coefficients transposed; the table lookup
didn't swap w/h like rav1d-safe's does; fixed zenrav1e@2310c7be). zenavif's
`with_qm(true)` default ships both until the zenrav1e release past 0.1.4 +
dep bump; effect at the shipped near-flat levels 12-15 is small but real.
**At the dep bump: re-run the QM benchmark** — the "~10% BD-rate win" for
with_qm(true) was measured with transposed rect weights.

### zenrav1e palette mode — IMPLEMENTED upstream 2026-07-03, release-gated
The screen-content palette tool (RD_GAP item 4) is fully implemented on
zenrav1e master (68a8d81f, 5f82e2d4, cda831e7, df27117c + changelog), default
OFF behind `SpeedSettings.prediction.palette = PaletteMode::{Off, Auto,
Always}` (`--palette` on the rav1e CLI). Luma-only search (UV flag coded
"off" — conformant); `Auto` ports libaom's AA-aware screen-content detection.
Conformance: every palette-on cell measured (720+ cells across sweeps)
decodes aomdec-clean AND byte-agrees (raw I420 md5) with rav1d-safe; the
in-repo roundtrip encodes synthetic screen content to LOSSLESS luma at ~1/8
the palette-off bytes. RD numbers: `docs/RD_GAP_VS_LIBAOM.md` item 4 status +
`benchmarks/palette_*_2026-07-03.tsv`.

**2026-07-03 (later): the zenavif-side palette GATE landed, val-confirmed,
release-gated.** `src/palette_gate.rs`: `patch_fraction > 0.197` →
`PalettePreference::Always` else `Auto` (degrades to Auto whenever features
are unavailable); `EncoderConfig::with_palette_preference` stores it;
`auto_tune` sets it via the Offer-reuse contract. Mechanism A/B (14 held-out
VAL origins × sizes {256,512,1024} × configs {isolated rav1e CLI, shipped
cavif} × s{2,6}; 6,216 cells, 0 conformance failures): where the gate fires
and the encoder's AA-detection is downscale-dead, the rule recovers
−10..−39% BD at s6 and −3.3..−15.2% at s2 @1024; photos never fire; false
fires ≈0 bytes + 1.06× median time. Record:
`docs/HYPERPARAM_FIRST_CUT_2026-07-03.md` rule-1 status +
`benchmarks/hyperparam_palette_mech_{ab,timing}_2026-07-03.tsv` + raw
`/mnt/v/output/rd-gap-palette-ab-2026-07-03/`.

**2026-07-03 (later still): the gate threshold is SPEED-CONDITIONAL —
measured + SHIPPED.** The mech A/B's val-refit anomaly (0.046-0.066)
resolved: real at fast speeds only. A/B of τ {0.197, 0.10, 0.05,
fire-always} × s{2,6,8}: 391 cells 100% OFFLINE (a threshold arm is a pure
per-cell selection over the already-measured off/always/auto outcomes —
mech-ab TSV + palette-ab-final2 store rows) + one fresh 1350-cell s8 iso
run (72 s box time, binary byte-continuity sha-proven against the kept
7052 IVF, 0 conformance failures, 900/900 armed cells md5-agree). s2 KEEPS
0.197 (refit plateau confirms; t0.05's val −0.028 is one iso cell
contradicted by the shipped config). s6+s8 confirm τ=0.05: deploy-mean vs
0.197 −0.047 train / −0.074 val (s6), −0.044 val (s8), every flipped
winner butteraugli-clean (6600-class scan-illustrations). fire-always is
nominally best (−0.14..−0.26 val) but its extra value sits at pf ≤ 0.05
INSIDE the photo pf mass — 100% photo firing at 1.80× (s6) / 2.13× (s8)
fired encode cost — rejected for the speed tier; residual ≈−0.19 mean is
feature-capacity (pf can't separate the 9094/1000-class fast-speed winners
from photos), not threshold placement. Shipped:
`palette_gate(pf, speed)` — s≤5 → 0.197 (byte-identical to the pre-change
rule), s≥6 → 0.05 (measured at 6+8; s7/s9/s10 same-tier assumption);
`palette_gate_for_rgb8(.., speed)`; `auto_tune` passes its picked speed.
Record: `benchmarks/hyperparam_palette_speed_ab_2026-07-03.tsv` +
`scripts/hyperparam/fit_palette_speed_threshold.py` (deterministic — all
tables re-derivable by re-running it); label store gained
`palette-mech-iso-s8-2026-07-03` (1,350 rows, 100% join; 29,358 rows / 78
arms total); s8 IVFs `/mnt/v/output/rd-gap-palette-ab-2026-07-03/ivf_s8/`
(Tower-mirrored, sha-verified).

**At the zenrav1e dep bump (>0.1.4):**
1. ravif: add the palette pass-through builder (same shape as its other
   `override_*` plumbing; the WEDGE-FINDER `ZENRAVIF_PALETTE` env passthrough
   in `ravif--wedge` is the reference implementation);
2. zenavif: uncomment the forward block in `src/encoder.rs`
   `build_ravif_encoder` (marked "UNCOMMENT at the zenrav1e dep bump") and
   drop the `let _ = &config.palette_preference;` placeholder;
3. re-run the PALCONF conformance protocol (`scripts/rd_gap/zenrav1e_cell.sh`
   PALCONF=1) on a palette-armed sample + re-measure the screen-content tier
   gap;
4. fold the `encode-mono` feature into `encode` (zenavif#6: true Cs400 gray
   encode; the gate exists only because local sibling ravif checkouts may
   predate cavif-rs@89668f13 — CI already builds it via clone-siblings).

### zenrav1e LRF + filter-intra desync encoder recon from decoders (OPEN upstream)
Found 2026-07-03 while measuring palette (zenrav1e#32, #33): at s<=7 (LRF)
and s<=6 (filter-intra, enabled when prediction_modes >= ComplexKeyframes),
zenrav1e's own recon diverges from what aomdec AND rav1d-safe (byte-agreeing
with each other) decode — running drift up to ~50 luma RMSE on smooth
content; a third composing bug (forced-skip intra blocks never wrote their
prediction into the recon) is FIXED upstream (zenrav1e@b30dd752).
**Measurement impact:** decoded-quality scores of zenrav1e/cavif output are
systematically depressed wherever these tools fire — ravif disables LRF at
normal quality (low-q cells affected only), but filter-intra is ON in cavif
at s1/s2, so rd_gap/tune-sweep photo numbers carry unintended error. Isolate
with `--lrf false --filter-intra false` (overrides added upstream). Re-check
tier measurements after the upstream fixes land.

### rav1d-safe Threading Race Condition (RESOLVED)
DisjointMut overlap panic was caused by frame threading. Fix: `max_frame_delay=1`
gives tile parallelism without frame threading. Default threads now 0 (auto-detect).

### rav1d-safe tile-threading CDEF/loop-filter race + panic wedge (zenavif#30) — FIXED upstream, release-gated
The two-pass conformance futex hang (4/220 cells frozen 76-90 min, 0 CPU) was
a rav1d-safe tile-worker `overlapping DisjointMut` panic — the loop filter's
compact-COW guards covered/rewrote tap rows CDEF legitimately touches (luma
window applied to chroma + write-back of unmodified pixels) — and a dead
worker wedged `rav1d_decode_frame`'s completion wait forever. In `unchecked`
builds the same defect could silently clobber concurrent CDEF output
(stale-byte write-back) instead of panicking. Fixed 2026-07-03 in
rav1d-safe@49df1fc0 (diff-based write-back + plane-accurate tap windows +
worker panics now fail decode with an error in ms; regression tests +
trigger vector committed upstream; 6,000/6,000 stress iterations + full md5
conformance clean). zenavif repro: `examples/hang_stress.rs` (~10+ parallel
instances; 420/q30 hottest). **Until rav1d-safe releases past 0.5.7 and the
zenavif dep bump**, registry builds ship the panic+wedge behavior — decode
under heavy parallel tile-threaded load can still rarely hang. At the dep
bump: raise the rav1d-safe minimum past 0.5.7 and re-run the hang_stress
verification. 2026-07-20: now that `codec-corpus` resolves on the dev-32gb
box (symlink /root/codec-corpus -> /root/work/codec-corpus), the
`codec::tests::animated_avif_animation_frame_decoder_roundtrip` test hit
this wedge REPRODUCIBLY on registry 0.5.3 under parallel suite load
(2 of 3 runs; futex_do_wait, 0 CPU) while passing solo — and the bump to
the imazen/rav1d-safe git rev 398b0bfa (0.6.0 staged, carries 49df1fc0)
cleared it: full suite green under the same parallel load. Return to a
registry dep at the 0.6.0 release.

## TODO: Encoding Enhancements

### Target-Quality Convergence — IMPLEMENTED for RGB8 (2026-07-02)
`encode_rgb8_with_target` (`target-quality` feature, `src/target_quality.rs`):
bracketed secant search over quality converging on `TargetMetric::Ssim2` or
`::Zensim` (per the 2026-07-02 user directive: quality/ssim2/zensim, not
Butteraugli). Selection policy: smallest file inside the target band;
honest `converged=false` when unreachable. RGBA8 covered
(`encode_rgba8_with_target`: zensim scores RGBA natively; ssim2 composites
on mid-gray); RGB16 covered (`encode_rgb16_with_target`, 10-bit AV1: ssim2
native 16-bit, zensim via identical 8-bit views). Contract tests:
`tests/target_quality.rs` (7 tests, ~0.6 s). NOT YET COVERED: RGBA16,
animation.

**§q0 seeding (2026-07-05): content-aware starting quality — LANDED,
`auto-tune` feature.** `src/q0_head.rs` (fitted-constants head, fast_heads
pattern): predicts q0 from 8 zenanalyze features + (ssim2 target, speed,
ln px); `encode_rgb8_with_target` seeds the search from it on Ssim2 targets
(zensim/RGBA/RGB16 keep the anchor curve; prediction failure degrades to
the anchor curve unchanged — liveness + fallback pinned by
`tests/target_quality.rs::q0_seed_is_live_on_ssim2_and_absent_on_zensim`).
Fit: `scripts/hyperparam/fit_q0_head.py` (deterministic; label store
cavif_q ship-of-era arms, LSD-train fit / LSD-val gate). **Honest verdict:
the pre-registered |q0−q*| p90 ≤ 6 val gate is NOT met — best simple model
7.25 (M5-l1-q-hinge-h80), and the sanctioned zenpredict-shape MLP
escalation measured WORSE on val (7.70 vs 7.25 while fitting train to 4.08
— origin-level transfer, not capacity, binds; cross-arm label noise floor
p90 ≈ 2.1).** Shipped anyway on the true objective: offline secant sim
(731 held-out val curves) mean encodes 3.75 → 2.72, converged 94.1% → 100%,
≤2-encodes 11.5% → 26.8% (`benchmarks/q0_head_fit_2026-07-05.tsv`);
real-encode A/B (5 val origins × t{60,80} × s6) mean 4.5 → 4.0, converged
7/10 → 8/10, t=80 saturation rescues 8103 6✗→2✓ / 6091 6✗→4✓, photo 1055
regressed 1→3 at t80 (`benchmarks/q0_seed_ab_2026-07-05.tsv`, harness =
`examples/target_hug_bench.rs` built with vs without `auto-tune`).
Per-family val p90: photos/people/illustrations 2.8-3.8 (gate met there);
tails are 9000 (15.6, n=26), 7000 plots (11.0), 6000 scans (8.0) —
saturating curves where the ssim2→q inversion is ill-conditioned. Tune-off
(registry-era) arms: p90 9.1 (fitted on tune-on ship arms; re-run
fit_q0_head.py at the dep bump if the tune default changes). Follow-ups:
RGBA8/RGB16 seeding, zensim-target fit, Offer pass-through from an
orchestrator.

### zensim generation-C (`ZensimProfile::C`) — WIRED 2026-08-07, `src/zensim_c.rs`

`C` is **not** a drop-in for `B` and the failure mode is quiet. Read
`src/zensim_c.rs`'s module docs before touching any zensim scoring here.
The four facts that cost the most time to establish:

1. **`Zensim::compute` is the wrong front end for C.** C consumes the
   folded-944 regime (`compute_folded720_features_streaming` with the
   append+append2 toggles, then `score_features_with_profile(C, …)`).
   `compute` returns `ModelForwardFailed` on a real pair — but
   byte-identical pairs short-circuit to 100 *before* the forward pass, so
   a naive smoke test passes and proves nothing.
2. **C HAS a per-pixel map, and it is not a diffmap.** `AttributionResult`
   is row-major `w × h` f32 + a SAT. It is **signed**, its unit is **score
   points**, and it is absolutely normalized — so the B pooling rule
   `clamp(e_b / geomean(e_b), 0.4, 2.5)^(−strength)` does not transfer
   (its premise is that SSIM error has no absolute unit, and geomean is
   undefined over sign-mixed data).
3. **There is a silent-wrong-number HDR path.** zensim's folded-944
   extractor does not error on an HDR-flagged `LinearF32Rgba`+opaque pair
   — it auto-routes to the PU/HDR front end and returns HDR-domain
   features, which C's SDR bake scores without complaint.
   `ZensimC::features` refuses `is_hdr()` before extraction; `sdr_guard`
   refuses CICP transfer 16/18. Do not remove either.
4. **The spatial arm does not steer, measured.** 0 of 48 matched-quantizer
   cells gained score without spending bytes; at |Δbytes| ≤ 2.5 % the
   median Δscore is −0.646. Default stays off. Record:
   `benchmarks/zensim_c_2026-08-07.meta`.

Anchors are refit for C (`ANCHOR_QUANTIZER_C`); B's table is wrong for C by
~30 quantizer steps at the coarse end. C's two-shot precision is **worse**
than B's at a matched size mix (median |err| 1.579 vs 1.032) even though
C's achievable lattice is finer — it is harder to predict, and the
prediction term is 92 % of the error (vs 85 % for B).

**Not wired:** `encode_rgb8_zensim_two_shot` / `encode_rgb8_zensim_loop`
still score with `B` only. `TargetMetric::ZensimC` covers the plain
`encode_*_with_target` search. A real-encode two-shot A/B under C has not
been run (only the offline lattice replay).

### Encoding Features (`encode-imazen` feature gate)
All wired through to zenrav1e fork. Benchmarked results (ravif 7265eea):
- `with_qm(true)` - only measurable win (~10% BD-rate). Default enabled.
- `with_vaq()` - hurts quality; psychovisual tune already includes SSIM boost.
- `tune_still_image` - no effect; ravif disables CDEF at high quality levels.
- `with_lossless` - implemented, works.

## Canonical training data + indexes (added 2026-05-20)

**The canonical index for all ML data lives at `~/work/zen/DATA_PROVENANCE.md`.**

Quick paths:
- Trainer input: `/mnt/v/zen/zensim-training/canonical-2026-05-21/`
- Master inventory: `~/work/zen/_ml-inventory-2026-05-20/00-MASTER-SYNTHESIS.md`
- Per-codec picker audit: `~/work/zen/_ml-inventory-2026-05-20/05-per-codec-pickers.md`

## ML/auto-tune status (2026-05-20)

zenavif is the **only zen codec with a production picker.** `EncoderConfig::auto_tune()` (feature `auto-tune`) loads `src/models/rav1e_picker_v0_1_1.bin` (ZNPR v2, ~217 KB) + the two LUT JSONs and returns optimal (speed, quality) knobs for a `QualityTarget`. Reference implementation for any future codec picker wiring.

Training intermediates from earlier 2026-05-04 sweep (`benchmarks/zenavif_picker_v0.{3,4,5}_2026-05-04.bin`) are kept for reproducibility; production wires `v0.1.1`.

The training pipeline that produces a new `rav1e_picker_v*.bin` lives in `~/work/zen/zenanalyze/zentrain/` (Python). See `~/work/zen/_ml-inventory-2026-05-20/05-per-codec-pickers.md` for cross-codec picker design.
