# Changelog

All notable changes to zenavif are documented here. zenavif is an AVIF encoder
and decoder built on the excellent work of the [rav1d-safe](https://github.com/imazen/rav1d-safe)
decoder (our fork of [dav1d](https://code.videolan.org/videolan/dav1d) via
[rav1d](https://github.com/memorysafety/rav1d)),
the [zenrav1e](https://github.com/imazen/zenrav1e) encoder (our fork of
[rav1e](https://github.com/xiph/rav1e)), and the
[zenavif-parse](https://github.com/imazen/zenavif-parse) container parser.

## [Unreleased]

### Added
- Versioned public-API surface snapshot at `docs/public-api/zenavif.txt`,
  regenerated on every `cargo test` by `tests/public_api_doc.rs`
  (`ZEN_API_DOC=check` verifies in CI's clippy job, `=off` skips); justfile
  recipes `api-doc` / `api-doc-check`.
- `zencodec::GainMapRender` wired through the decode trait path:
  `BaseOnly` (default) ignores the gain map entirely — no extras attached
  (previously `GainMapSource` attachment was gated only on the legacy
  `with_extract_gain_map` flag, which still works); `Components` decodes the
  gain-map AV1 payload and surfaces BOTH `zencodec::decode::DecodedGainMap`
  (pixels + ISO 21496-1 params) and the raw `GainMapSource` (for transcode);
  `ReconstructHdr` downgrades to Components per the zencodec contract —
  zenavif surfaces, it does not apply (`reconstructs_hdr()` stays false), so
  the base is never silently SDR-labeled-HDR. Unknown future modes error.
  Tests in `tests/gainmap_decode.rs` (the stale always-on extras test was
  re-pointed at the opt-in Components contract; vectors via
  `just download-vectors`).
- zencodec 0.1.21 color-emit integration: `resolve_avif_color` drives the still and animation encode paths through `resolve_color_emit` (single source of truth); resolved CICP lowers to nclx on all three axes; AVIF declares CICP sole-safe (nclx is reader-authoritative per MIAF/HEIF), so a redundant ICC is dropped like JXL. Deps bumped to published zencodec 0.1.21 / zenpixels 0.2.11 / zenpixels-convert 0.2.12. Also aligns the zenanalyze/zenpredict path-dep reqs to the drifted 0.2.0 siblings, fixing the nightly Fuzz job's resolution failure (b3be82a6).

### Changed
- YUV420→RGB8 bilinear chroma upsampling now uses a direct truncating cast
  instead of `f32::floor` (libm `floorf`) for the per-pixel chroma index.
  Inputs are pre-clamped to `[0, dim-1]`, so the output is byte-identical —
  verified on x86 (AVX2, full lib suite) and aarch64 (NEON) via a byte-identity
  test against an independent libm-floor reference (6 sizes × 2 ranges × 3
  matrices). This is a correctness-preserving refactor that drops a libm call
  from the chroma-gather inner loop; a wall-time win was NOT demonstrated on ARM
  (the yuv_conversion_benchmark gates SIMD on an x86-only token, so on aarch64 it
  measures the scalar fallback, not the NEON path this change affects — measured
  no change there). See `benchmarks/zenavif_arm_yuv_floor_2026-05-30.{tsv,meta}`.
- `tests/fuzz_regression.rs` now uses the shared `zen-fuzz-regress`
  test-helper crate (DEDUP-J2). Behaviour is unchanged — same
  `fuzz/regression/` seeds, same four targets (`decode`,
  `decode_limited`, `decode_animation`, `probe`), same
  panic-propagation failure semantics. The ~60-line in-file
  scaffolding (`collect_seeds` walk + skip dotfiles + per-seed read +
  dispatch) is now provided by `RegressionSuite`.

### Added
- `tests/fuzz_regression.rs` regression-harness template ported from
  zenwebp (DEDUP-J). Walks `fuzz/regression/` (incl. per-target
  subdirs) and runs every seed through `decode_with`,
  `decode_animation_with`, `AnimationDecoder`, and
  `ManagedAvifDecoder::probe_info` on the stable toolchain — no
  nightly required. Drop minimized crash files into `fuzz/regression/`
  to gate future regressions of fixed bugs.

## [0.1.7] - 2026-05-02

### Changed
- Bump minimum `zenravif` dependency to 0.1.3 (published with the
  `__expert` + `InternalParams` surface). The local `[patch.crates-io]`
  override is no longer needed and has been removed.

### Added
- New `__expert` cargo feature exposing `expert::InternalParams`, an
  `Option<T>` struct of speed-preset overrides for the 4 deepest
  content-dependent knobs (`partition_range`, `complex_prediction_modes`,
  `lrf`, `fast_deblock`). Apply via
  `EncoderConfig::with_internal_params(InternalParams)`.
  `#[non_exhaustive]` + `Default` so callers tolerate field additions
  in any patch. Each `None` keeps the speed preset's default; each
  `Some(_)` overrides. Implies `encode-imazen` and pulls
  `ravif/__expert` for the underlying overrides. Used by the rav1e
  knob predictor MLP training harness; not for production code.
- Theory-of-operation docs on every `InternalParams` field covering
  the AV1 pipeline stage, why a caller might override it, the
  underlying mechanism, and the speed-preset interaction. Cross-
  references `zenravif::expert::InternalParams` for source-line
  citations into zenrav1e.
- `tests/expert_internal_params.rs` — 12 permutation, idempotency,
  combined-knob, default-as-baseline, reset, and forwarding-parity
  tests for `InternalParams`. Verifies that zenavif's wrapper produces
  byte-identical output to calling `zenravif::Encoder::with_internal_params`
  directly with the same values. Gated on `__expert`.

### Changed
- The 4 individual setters (`with_partition_range`,
  `with_complex_prediction_modes`, `with_lrf`, `with_fast_deblock`)
  added in Phase 0.5 are removed from the public API in favour of
  `with_internal_params(InternalParams)`. Never published, so no
  deprecation cycle. The other 11 `encode-imazen`-gated setters
  (`with_qm`, `with_vaq`, `with_seg_boost`, `with_cdef`, etc.) stay
  as-is — they're already in 0.1.6 published.
- `predictor_sweep` and `phase2_oat` examples now require the
  `__expert` feature (was `encode-imazen`); `phase2_oat`'s deep-knob
  perturbations rebuild on top of `ravif::expert::InternalParams`
  rather than the removed individual setters.
- `partition_range` doc and tests now reflect that zenrav1e
  debug-asserts `max <= 64×64`; `128` is invalid (would panic in
  debug builds). Valid values: `{4, 8, 16, 32, 64}`.

### Build
- `[patch.crates-io] zenravif` temporarily points at
  `../ravif--expert/ravif` while ravif's `feat/expert-internal-params`
  branch is unmerged. Revert to `../ravif/ravif` once that branch
  lands; remove the patch entirely once zenravif 0.1.3 publishes.

### Changed
- Bumped baked picker artifacts to `v0_1_1`: re-baked from a 4× larger
  Phase 1a corpus (448 images / 89,601 sweep rows vs. 116 / 23,200 in
  v0_1) with a wider 192³ MLP. Held-out student mean overhead drops
  from 4.07 % → 3.88 % on the same val split. ZNPR + per-(speed,
  size_class) encode_ms LUT + per-(cell, target_zq) quality LUT all
  rev v0_1_1; auto_tune.rs `include_bytes!` paths updated. Old v0_1
  artifacts dropped (git history preserves them).
- Dropped `composites` feature from zenanalyze path-dep — feature was
  removed upstream in zenanalyze@b1623ba.

### Added
- 11 new `EncoderConfig` builder methods exposing internal speed-preset
  overrides for content-aware encoding, all gated on `encode-imazen`:
  `with_cdef`, `with_rdo_tx_decision`, `with_sgr_full`,
  `with_lru_on_skip`, `with_segmentation_complex`, `with_encode_bottomup`,
  `with_seg_boost`, `with_trellis`, plus 4 deeper knobs newly plumbed
  through zenravif 0.1.3 — `with_partition_range`,
  `with_complex_prediction_modes`, `with_lrf`, `with_fast_deblock`. Each
  takes `Option<...>` where `None` keeps the speed-preset default.
- New `auto-tune` cargo feature + `EncoderConfig::auto_tune(...)` API
  that predicts optimal `(speed, quality)` knobs for a given image and
  target zensim score via a baked MLP picker. Supports time-budget
  constraints via `AutoTuneOptions::with_time_budget(Duration)` and
  Pareto blending between bytes and encode_ms via
  `with_pareto_weight(α ∈ [0, 1])`. The model file ships baked into
  the binary via `include_bytes!`; until the first bake lands the
  runtime returns `AutoTuneError::ModelNotBaked`. See
  `docs/RAV1E_PICKER_PLAN.md` for the training pipeline.
- `examples/predictor_sweep.rs` — multi-image, multi-size, multi-knob
  sweep harness for picker training; resumable via `--append`.
- `examples/extract_features.rs` — zenanalyze ~103-feature extractor
  for the same corpus (matches `zentrain/tools/train_hybrid.py`'s
  expected schema).
- `examples/auto_tune_smoke.rs` — end-to-end smoke test for the
  prediction path.
- `scripts/install_predictor_cron.sh` — installs nightly local cron
  jobs for incremental sweep growth + weekly retraining.
- `scripts/train_bake_pipeline.sh` — orchestrates train → bake →
  per-(speed, size) encode_ms LUT → per-(cell, target_zq) quality LUT
  in a single invocation.

## [0.1.6] - 2026-04-27

### Fixed
- QM (quantization matrix) quality across the encoding range is now monotonic
  and tracks QM-off within ±0.4 zensim from q=70 onward. Two bugs in zenrav1e
  fixed upstream and pulled in via the bumped minimum zenravif dep:
  (1) `qm_level_for_qindex` now uses libavif's still-image range `[4, 15]`
  instead of all-intra-video `[4, 10]`, so near-lossless qindex bypasses QM
  entirely instead of applying weights that multiplied the effective quantizer
  2-3× on high-frequency coefficients; (2) `using_qmatrix` is now cleared when
  the frame is coded-lossless or all selected QM levels are 15, fixing AV1
  spec 6.8.11 conformance and rav1d primary-frame decode at quality=100.
  Previously zensim collapsed from ~76 at q=95 (QM=on) to ~49 at q=100, and
  the whole q≥60 range was 11–22 zensim points worse with QM on. See
  imazen/zenrav1e#7. (0e7cefc, zenrav1e@30d37fc)

### Changed
- Bump minimum `zenravif` dependency to 0.1.2 (which requires zenrav1e 0.1.4)
  to pull in the QM and lossless-conformance fixes. The temporary q≥96 QM
  guard from 0e7cefc is removed — the fix lives upstream now.

## [0.1.5] - 2026-04-17

### Added
- Encoder `Auto` bit depth now matches input type: 8-bit input produces 8-bit
  AV1 and 16-bit input produces 10-bit AV1. Previously `Auto` always selected
  10-bit, which surprised callers decoding 8-bit sources back out as `Rgb16`
  (9bf934c).
- `DecoderConfig::prefer_8bit(bool)` (default `false`) for callers who want to
  downscale 10/12-bit AV1 to 8-bit RGB when decoding files produced by other
  encoders that default to higher bit depths (9bf934c).
- `RGBX8_SRGB` and `BGRX8_SRGB` pixel descriptors are now accepted by the
  encode dispatch. The padding byte is stripped before encoding and BGRX
  additionally swaps B/R channels; output is byte-identical to encoding the
  equivalent packed RGB8 (d9863f1).
- Encoder configuration guide in `README.md` covering speed/quality tradeoffs,
  quality parameter mapping, QM behaviour, and bit depth selection, with all
  numbers sourced from the committed sweep data on CID22-512 (a483b4a).
- Committed fine-grained encode sweep (q5-q100 step 5, speeds 1/2/4/6, QM
  on/off) under `benchmarks/` and a broader combinatorial sweep covering
  100 configurations, both measured with zensim-regress (53fff28, ef5cb08).
- `.workongoing` added to `.gitignore` for the main-with-lockfile agent
  workflow (d9863f1).

### Changed
- Default encoder profile for `[profile.test]` is now `opt-level = 2` so
  tests exercise optimised codec paths without requiring `--release`
  (9bf934c).
- Bumped `fast-ssim2` to `0.8.0`; `0.7.2` and `0.7.3` were yanked upstream
  after an accidental semver break from `yuvxyb` re-exports, which `0.8.0`
  removes (ef0500d).
- Minor documentation alignment between the zennode feature table in
  `README.md` and `Cargo.toml` (4b3a1df).
- Bump zencodec to 0.1.19

### Fixed
- `EncoderConfig::with_lossless()` and `with_lossless_mode()` (from the
  zencodec trait) now propagate `lossless = true` into the inner encoder
  config, so rav1e's lossless mode is actually engaged when requested via
  the trait API (ef5cb08).
- Quantization matrices (QM) are now auto-disabled when lossless encoding
  is selected. QM is still enabled by default for lossy quality levels
  (q5-q95), where it saves 9-13% on file size at speeds 4 and above with
  negligible quality impact, but combining QM with a quantizer of zero
  produced corrupt output (ef5cb08).
- `ColorAuthority::Cicp` is now set when the decoded image carries no ICC
  profile, reflecting the AVIF/MIAF precedence of `ICC > nclx > AV1 SPS
  CICP`. When no `colr` box is present, CICP (populated from nclx or SPS
  fallback) is the authoritative colour description (7d6b4e6, #3).
- `examples/encode_sweep.rs` — committed harness that regenerates the
  per-image TSVs under `benchmarks/`. CLI accepts `--image`, `--speeds`,
  `--qualities` (list or `START..=END:STEP`), `--qm {off,on,both}`, and
  `--force-bottomup {auto,off,on,both}` — the last is what reproduces
  zenrav1e#6's scenario now that ravif/40ddb66 disables bottom-up by
  default. `just sweep -- <flags>` is the shortcut. Gated on
  `encode-imazen,encode-threading`.

## [0.1.4] - 2026-04-05

### Added
- Generic SIMD YUV420-to-RGB8 path with autoversion dispatch covering NEON
  and WebAssembly in addition to x86, and 4:2:2 / 4:4:4 variants, built on
  the magetypes generic SIMD abstraction (0089d18, 0b7b333).
- Fuzz dictionary for AVIF and a nightly fuzz workflow that runs a short
  smoke fuzz on every push and a longer nightly run (b367344, ee107a7).
- Regression seed capturing the CDEF tile race fixed in rav1d-safe, so the
  race can never silently regress through the AVIF decode path (6d18489).

### Changed
- Replaced the platform-specific YUV-to-RGB SIMD implementations with a
  single magetypes-based generic dispatch. The x86-specific paths were
  collapsed into the generic implementation without measurable regression
  (0b7b333).
- Bumped `zencodec` to `0.1.13` (cfc1f7b).
- Committed `fuzz/Cargo.lock` for reproducible fuzz builds; `profraw` files
  and other tooling noise are now gitignored and excluded from published
  packages (193cbd0, ec244d8, 91912b7).

### Fixed
- Reverted the `max_frame_delay = 1` workaround added for a rav1d-safe CDEF
  threading race once the underlying race was fixed in rav1d-safe itself.
  The workaround served its purpose while the upstream fix was being
  developed (e089793, 1d1f838).

## [0.1.3] - Earlier

### Fixed
- Gated the `StopExt` import behind the `encode` feature so builds with
  `default-features = false` remain clean (812b817).

### Changed
- Bumped `zenavif-parse` to `0.6.0` and switched to the published `From`
  impl for gain map conversion (933db7a).
- Set correct minimum versions for `zenflate` and `linear-srgb`, and moved
  `linear-srgb` to a semver spec (d48d69b, a1c1131, 210c255).

## [0.1.1] - Earlier

### Changed
- Switched `rav1d-safe` from a git revision to the published `0.5.3`
  release on crates.io (55d009a).
- Removed local path overrides that broke CI (20500cd).

### Fixed
- Temporarily pinned `rav1d-safe` to a git revision containing the
  aarch64 panic fixes while the fix was making its way through a release
  (01c02d0).
