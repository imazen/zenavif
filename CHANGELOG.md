# Changelog

All notable changes to zenavif are documented here. zenavif is an AVIF encoder
and decoder built on the excellent work of the [rav1d-safe](https://github.com/imazen/rav1d-safe)
decoder (our fork of [dav1d](https://code.videolan.org/videolan/dav1d) via
[rav1d](https://github.com/memorysafety/rav1d)),
the [zenrav1e](https://github.com/imazen/zenrav1e) encoder (our fork of
[rav1e](https://github.com/xiph/rav1e)), and the
[zenavif-parse](https://github.com/imazen/zenavif-parse) container parser.

## Workspace

- **2026-08-14 — decode negotiation converged across all five entry points
  (zenavif#39, #38, #37, #36).** `preferred = [Rgb8]` over an HDR file no
  longer panics: `negotiate_format` reduced formats through
  `PixelBuffer::to_rgb8()` / `to_rgba8()`, which `expect` on
  `RowConverter::new`, and the arm was selected by the CICP read out of the
  file — untrusted input, reachable from an ordinary caller preference. 72 of
  990 (fixture x `preferred` x entry point) cells panicked; now 0, with all
  918 previously-working cells byte-identical. The same defect on a second
  surface: the five `decode_into_*` convenience methods, which take no
  `preferred` list at all, panicked on 18 of 90 (fixture x method) calls and
  now return `Error::Unsupported`. The streaming decoder and the row sink now
  run the `preferred` reduction per strip (40 of 374 format disagreements with
  the buffered decode -> 0), describe their pixels with the container's CICP
  (15 of 34 lost tags -> 0; an HDR PQ image was handed over labelled
  `transfer: Unknown`), and refuse `GainMapRender::ReconstructHdr` instead of
  silently returning the SDR base as `Ok`. New gate
  `tests/negotiation_matrix.rs` (10 tests) sweeps the whole product; new fuzz
  target `fuzz_decode_negotiate` reaches the negotiation layer, which no
  existing target did.

- **2026-08-07 — zensim CQ target-hitting loop harness + ADDITIVE per-SB
  hint hook (zensim campaign appendix AC.4).** New example
  `zensim_cq_rd` (features `encode-imazen,two-pass-butteraugli`):
  seed-CQ encode → decode → folded-944 zensim scoring → the jxl-adopted
  proportional controller (exp 1.0 / per-step clamp 2.0), arms baseline /
  h3-mag (per-64px-superblock attribution steering) / outer CQ-bisection
  comparator; pre-registered study
  `benchmarks/zensim_avif_loop_2026-08-07.md` (G-AV2 smoke MEASURED and
  committed; the G-AV3 matrix runs when the wave-12 candidate bake
  lands, shipped C as control) + runner
  `scripts/zensim-loop/run_avif_loop.sh`. New public API:
  `EncoderConfig::with_sb_q_scale` / `sb_q_scale_value`
  (`two-pass-butteraugli`) — external access to the per-superblock AC
  quantizer scale map the butteraugli two-pass driver already forwards;
  release-gated inert until zenravif `FRAME_HINTS_LIVE` (the zenrav1e
  >0.1.4 dep bump), gated by `tests/sb_q_scale_hint.rs` which asserts
  the real behavior of BOTH gate states. Dev-deps: renamed `zensim03`
  path dep on sibling zensim 0.3.0 (registry 0.2.4 deps untouched); CI
  `clone-siblings` now also clones imazen/zensim.
- **2026-08-07 — `hdr_encode_cell` example stamps MEASURED clli (zensim
  campaign appendix AA).** The example previously wrote a hardcoded
  `content_light_level(1000, 250)` on every file regardless of content;
  it now measures MaxCLL/MaxFALL from the decoded PQ pixels via the
  zenpixels owner (`zenpixels_convert::CllMeasure::measure_max`, MaxRGB
  per CTA-861.3, ST-2084 EOTF → cd/m²), writes no clli for non-PQ inputs
  rather than a guessed one, and its self-check asserts the container
  echoes the measured values. The mdcv stand-in stays declared (ST 2086
  describes the mastering display — not measurable from pixels). The
  library's clli/mdcv setters remain caller-supplied passthroughs
  (declared-by-necessity; nothing is invented when absent).
  zenpixels/zenpixels-convert min 0.2.16 (+`hdr-experimental`).
- **2026-07-16 — absorbed the zenavif-parse and zenavif-serialize
  repositories.** Both crates now live here as workspace members
  (`zenavif-parse/`, `zenavif-serialize/`) with their complete git
  histories rewritten under those paths, and all tags imported under
  lineage prefixes: `zenavif-parse-v*` / `zenavif-serialize-v*` (fork
  releases), `avif-parse-v*` (kornelski upstream), `mp4parse-v*` (Mozilla
  lineage). GitHub releases were recreated here from the source repos.
  Releases are per crate via crate-prefixed tags (release.yml routes on
  the prefix to `cargo publish -p <crate>`); the sections below remain
  the zenavif crate's changelog.

## zenavif-parse

History and future entries: [`zenavif-parse/CHANGELOG.md`](zenavif-parse/CHANGELOG.md).
Queued at absorb time: 0.6.3 must publish from pre-break commit `c36b822`
(rewritten equivalent in this repo's history); next from head is 0.7.0
(breaking `At<Error>` returns, already on main).

## zenavif-serialize

History and future entries: [`zenavif-serialize/CHANGELOG.md`](zenavif-serialize/CHANGELOG.md).
Queued at absorb time: next release is 0.2.0 (breaking `At<SerializeError>`
write-path returns + gain-map interop additions, already on main).

## [Unreleased]

### QUEUED BREAKING CHANGES
- Remove `Error::ColorConversion(yuv::YuvError)` — the last public-API tie to
  the `yuv` crate. In-house kernels no longer construct it (they are
  infallible); the legacy `unsafe-asm` decoder still does. Removing the
  variant + the dep ships with the next 0.x minor bump.
- `Av1Backend`, `DecodeBackend` and `TargetMetric` are now
  `#[non_exhaustive]`. Downstream exhaustive matches need a `_` arm. Taken in
  the SAME release as the variant additions that already broke them
  (`Av1Backend::SvtRs`, `TargetMetric::ZensimC`) so consumers absorb one break
  rather than two: every future backend or metric is additive from here.
  `ValidationError` (already `#[non_exhaustive]`) gained
  `BackendUnsupportedParam` — ship with the next 0.x minor bump.
- `TargetMetric` gained the `ZensimC(f64)` variant. The enum is not
  `#[non_exhaustive]`, so downstream exhaustive matches must add an arm —
  ship with the next 0.x minor bump. No existing variant changed meaning.

### Changed
- **`zensim` re-pinned from registry 0.2.4 to git `main` (0.3.0)** for the
  diffmap API the closed loop needs (`compute_with_ref_and_diffmap`,
  `PrecomputedReference` reuse, `Zensim::with_stop`). Consequences:
  `zensim-regress` is pinned to the same git branch (registry 0.3.1 depends
  on zensim 0.2.x, which put two incompatible `Zensim`/`ImageSource` types
  in the graph and broke `tests/linku_corpus.rs`), and all six
  `ZensimProfile::latest()` call sites moved to
  `ZensimProfile::codec_target()` — `latest()` is deprecated in 0.3.0.
  **`TargetMetric::Zensim` scores on a different profile than before**:
  `latest()` was `PreviewV0_2` at 0.2.x and is `B` at 0.3.0, so a given
  target lands on a different quality. Unpin both at the zensim 0.3.0
  publish.
- zenav1-svt pin bumped `3e25f52b` → `2d585bb2b` (upstream master
  2026-07-24; zero seam API breaks). The pin is past the upstream repo
  restructure that moved the C reference into a git submodule — the
  `zenav1-svt*` crates are pure Rust and unaffected (crate names + pub API
  unchanged). Brings the typed QP-0 rejection
  (upstream #5: `try_encode_frame*` now returns `UnsupportedConfig` for
  base_qindex 0 instead of emitting garbage — the seam's quality→QP clamp
  keeps quality 100 encoding at QP 1 and is now composition-tested from
  both sides), the IBC screen-content vertical, the bd8/bd10 real-photo
  p0–p3 exchange-sort parity closures (photo p0 bd8 135/135, bd10 p0–p3
  187/187 upstream), and the nz-map SIMD port. Seam drift fixed in the
  same change: stale "no C bitstream identity yet" / "non-conformant"
  claims rewritten to the verified envelope, the 64-multiple dimension
  gate re-documented as a zenavif-side envelope choice (upstream pads +
  codes partial SBs since task #95), and `EncoderConfig::threads` is now
  threaded into the pipeline (`thread_count`; byte-inert, single-tile
  today).

### Added
- **`zensim_c`: scoring and steering on zensim generation-C
  (`ZensimProfile::C`), plus the additive `TargetMetric::ZensimC`.**
  `C` is a 944-input MLP over the folded-720+append+append2 regime, which
  the standard 372-feature `Zensim::compute` pipeline does not produce, so
  it needs its own front end: `compute_folded720_features_streaming` +
  `score_features_with_profile(C, …)`. Feeding `C` through `compute` fails
  with `ModelForwardFailed` — except on a byte-identical pair, which
  short-circuits to 100 before the forward pass, so a naive smoke test
  passes and proves nothing. `c_via_compute_fails` pins both halves.
  - **`C` has a per-pixel steering map, and it is NOT a diffmap.**
    `AttributionResult` is a real row-major `w × h` f32 plane (trimmed to
    the logical image, no SIMD padding) plus a summed-area table for O(1)
    rectangle queries. Its semantics differ from `DiffmapResult` in every
    way that matters to pooling: it is **signed**, its unit is **score
    points**, and it is absolutely normalized and mass-conserving —
    `query_rect(B)` is the linearized score gain from re-encoding block
    `B` at reference quality. The B rule
    `clamp(e_b / geomean(e_b), 0.4, 2.5)^(−strength)` therefore does not
    transfer: its whole justification is that SSIM error has no absolute
    unit, and a geometric mean is undefined over a sign-mixed quantity.
    `sb_q_scale_from_attribution` re-derives on the area-weighted
    arithmetic mean (the quantity is additive) with non-positive blocks
    pinned neutral. DERIVED, NOT FITTED; nothing enables it by default.
  - **HDR is refused twice, because one silent-wrong-number path is real.**
    `sdr_guard` rejects CICP transfer 16 (PQ) / 18 (HLG) with a typed
    `Error::Unsupported`, and `ZensimC::features` rejects
    `ImageSource::is_hdr()` before extraction — zensim's folded-944
    extractor does not error on an HDR-flagged `LinearF32Rgba` + opaque
    pair, it silently auto-routes to the PU/HDR front end and returns
    944 HDR-domain features, which `score_features_with_profile` has no
    domain guard against. `profile_for_transfer` names `BHdr` for HDR and
    says plainly that this crate cannot drive it (no absolute-luminance
    PU-linear front end).
  - **Honest costs.** No `PrecomputedReference` for the 944 extraction
    (zensim removed the prepared-reference forms; only `V2Scratch`
    allocation is reused), `Zensim::with_stop` is not checked by the v2
    extraction walks, and `steer()` is a second full walk plus two bake
    forwards per live column — not the one-call score+map the B loop gets.
    Measured numbers: `benchmarks/zensim_c_*_2026-08-07.*`.
- **`two-pass-zensim`: a zensim-diffmap-driven closed loop
  (`encode_rgb8_zensim_loop`).** Encode → decode → ONE
  `Zensim::compute_with_ref_and_diffmap` per pass → correct the quality
  **globally** from the score and the per-64×64-superblock quantizer
  **spatially** from the diffmap → re-encode. The reference pyramid is
  precomputed once (`precompute_reference`) and reused by every pass, and
  the caller's stop token is threaded into zensim itself
  (`Zensim::with_stop`), so a long scoring pass is interruptible.
  - **Global half is live.** It steps along the fitted population curve,
    `q_next = q + (Q(target) − Q(score))`, rather than a constant-slope
    Newton step — the score-vs-quality curve saturates, so the same score
    error needs a very different quality move at 40 than at 90. Once the
    target is bracketed it hands off to the same clamped secant
    `encode_rgb8_with_target` uses.
  - **Spatial half is release-gated** behind `zenravif::FRAME_HINTS_LIVE`
    (re-exported as `SPATIAL_HINTS_LIVE`) and reported per call as
    `ZensimLoopResult::spatial_applied`, so a computed map can never be
    mistaken for an applied one. Unlike `two-pass-butteraugli` — purely
    spatial, so it refuses to run at all while the gate is shut — this loop
    runs and converges regardless.
  - **The diffmap → quantizer-scale mapping is derived, not transplanted.**
    libaom's `tune=butteraugli` valuation `min(mse/distance, 5) + K` cannot
    take a zensim diffmap: the map is unitless SSIM error, so typical photo
    blocks land at ratios of 10²–10³ and the clip saturates on every block.
    Derived instead from scale-invariance + equal-error allocation:
    `q_scale_b = clamp(e_b / geomean(e_b), 0.4, 2.5)^(−strength)`,
    `strength = 1/γ` under `e ∝ step^γ`, default 1.0. DERIVED, NOT FITTED,
    and not honestly fittable until the gate opens (an applied map has no
    bitstream effect today).
  - **Measured against the existing secant baseline**, same images, same
    targets, same options on both arms (12 sources x long edges
    {64, 256, 1024} x zensim targets 20-90 step 5, plus a 4-source x 2048
    leg on a step-10 grid; `benchmarks/zensim_loop_ab_2026-08-06.tsv` and
    `..._tol1_...`, summaries alongside):

    | tolerance | arm | n | mean encodes | converged | 1 encode | <=2 encodes |
    |---|---|---|---|---|---|---|
    | 0.5 | secant | 572 | 4.14 | 66.3% | 8.0% | 14.7% |
    | 0.5 | loop | 572 | **3.75** | 64.9% | **12.9%** | **29.4%** |
    | 1.0 | secant | 360 | 3.35 | 82.2% | 17.8% | 31.7% |
    | 1.0 | loop | 360 | **2.90** | **82.8%** | **23.3%** | **51.4%** |

    Paired per-cell at tolerance 0.5: -0.39 encodes on average (fewer on
    43.4% of cells, more on 23.3%), achieved-vs-target error a wash (closer
    26.9%, further 26.9%, median delta 0), identical file sizes. The gain
    holds at every size and target band. At tolerance 0.5 the loop trades
    marginally lower convergence (64.9% vs 66.3%) for the encode saving --
    a tight early bracket runs out of lattice resolution where the
    baseline's coarse extrapolation gets more chances to land on a lattice
    point; at tolerance 1.0 that trade disappears.
  - Anchor-curve fit and its 1,008-encode sweep:
    `benchmarks/zensim_anchor_2026-08-06.tsv` (+
    `scripts/hyperparam/fit_zensim_anchor.py`, deterministic).
  - New public API: `two_pass_zensim` module, `ZensimLoopOptions`,
    `ZensimLoopResult`, `encode_rgb8_zensim_loop`,
    `anchor_quality_for_zensim`, `SPATIAL_HINTS_LIVE`.
- **The achievable-score lattice is now a measured, documented limit**
  (`benchmarks/zensim_score_lattice_2026-08-06.tsv`). `quality` resolves to
  an integer AV1 quantizer index, so the reachable zensim scores form a
  discrete lattice no search can land between: adjacent achievable scores
  are 0.82–1.05 apart at the median and ~50% of gaps exceed 1.0 over
  quality 50–80. A ±0.5 `tolerance` is therefore unreachable about half the
  time for reasons unrelated to the search — relevant to
  `TargetOptions::tolerance` as much as to the new loop.
- **CI now builds and tests the diffmap closed loops.** Neither
  `two-pass-butteraugli` nor `two-pass-zensim` appeared anywhere in
  `ci.yml`, so `tests/two_pass.rs` ran on no platform at all; a new test leg
  covers both. `scripts/gauntlet.sh` gains a `zloop` combo and adds
  `two-pass-zensim` to `allsafe`.
- **Memory-adaptive encode concurrency: encodes fit their thread count to
  the memory budget instead of blowing past it.** The zencodec encode
  pre-flight (still + animation paths) now checks the CALIBRATED
  thread-aware peak estimate (`heuristics::estimate_encode_threaded`) —
  not just the raw input-buffer size — against an explicit
  `ResourceLimits::max_memory_bytes`, or, absent one, against an implicit
  budget of 80% of detected available RAM (Linux `/proc/meminfo`
  `MemAvailable`; no implicit cap elsewhere). When the budget requires it,
  the encoder walks its thread count down (floor 1) and pins the reduced
  count on the native config — including under `ThreadingPolicy::Parallel`,
  which previously always kept the machine-wide default — and records the
  reduction on the `EncodeOutput` as a `String` extra
  (`output.extras::<String>()`); reductions are never silent. Only when
  even the single-threaded estimate exceeds the budget does the encode
  error (the memory-limit error; the implicit-budget message says to set
  `max_memory_bytes` to override — a clean error beats the kernel OOM
  kill measured on 32 GB boxes). `estimate_encode_resources` now returns
  thread-aware figures too: peaks carry the measured per-thread
  working-set term and wall time is divided by the fitted Amdahl speedup
  (with `cpu_ms` carrying the single-thread work). All helpers are
  crate-private — no new public API. (zensysbench CODEC-MEMORY-PLAN
  wave 2.)
- `DecoderConfig::decode_backend` — the aom-rs decoder is now a selectable
  PRODUCT decode backend (feature `aom-backend`, experimental): non-grid
  stills decode through zenav1-aom across the full still surface — 8/10/12
  bit, 4:2:0/4:2:2/4:4:4 + mono, RGBA alpha items, identity (MC=0),
  exotic matrices, ICC/CICP/HDR-metadata passthrough (upstream
  `FrameDecode` gained the sequence CICP fields), gain-map item decode
  (`decode_av1_obu_with_config`, both zencodec gain-map sites routed), and
  the prefer_8bit downscale — byte-identical to the rav1d-safe path by
  construction (both run the same canonical in-house kernel recipe; pinned
  across the whole grid by `tests/product_aom_backend.rs`). ANIMATION now
  decodes too: zenav1-aom landed the animated-AVIF inter envelope (8/8
  tracks / 40/40 frames byte-exact vs aomdec upstream, pin a06bce15) and
  `AnimationDecoder` on AomRs eagerly decodes each track in one
  `decode_frames` pass (DPB/CDF state spans samples) — all 5 libavif
  animated vectors byte-identical to the rav1d path, frame-by-frame
  (pixels + durations). Grid AVIFs decode too (each grid cell is an
  independent AV1 still; the byte-stitch is shared with the rav1d path) —
  identity pinned on the sofa/mixed-alpha grid vectors. Only row-sink
  streaming still returns honest `Unsupported` on AomRs.
- `decode_av1_obu_yuv_with` — the raw-OBU decode seam gains a config-carrying
  twin threading `DecoderConfig` + a stop token into the backends
  (seam obligation 3): `frame_size_limit` reaches rav1d-safe's managed
  `Settings` and aom-rs's `DecodeLimits::max_pixels`, the stop token is
  polled in-loop by aom-rs (SB-row/tile/frame cadence), and `alloc_pref`
  maps onto aom-rs's `AllocMode` (fallible pre-flight by default).
  Liveness pinned by `tests/cross_backend_decode.rs::config_threading`.
- Cross-backend validation + calibration (2026-07-22): every encode-backend
  bitstream is now cross-decoded on rav1d-safe AND zenav1-aom with plane
  byte-compare (`tests/cross_backend_decode.rs`, incl. 10-bit; 7018/7018
  sweep cells identical — first conformance coverage of OUR encoders'
  output), the ssim2 target search is pinned to converge over
  `Av1Backend::SvtRs` (the unified cross-backend quality mechanism), and
  the RD/speed calibration sweep is committed
  (`benchmarks/backend_sweep_2026-07-22.{tsv,meta}`, harness
  `examples/encode_backend_sweep.rs` + `examples/decode_backend_bench.rs`).

- The canonical YUV numeric recipe is now a documented FIXED-POINT formula
  (P8): single rounding of a 2^-16-accurate value, pure-integer arithmetic
  (platform-exact on every arch incl. wasm — no FMA/rounding-mode variance),
  offsets derived from the rounded coefficients so gray decodes to exactly
  R=G=B at every depth. d<=12 decode paths (all real AV1) run it; measured
  2-10x over the f32 kernels (benchmarks/yuv_fixedpoint_2026-07-20.csv:
  8-bit 420 490 Mpx/s, 444-rgba 3.4 Gpx/s, 10-bit 420 484 Mpx/s). Accuracy
  pinned within ±1 of exact rational conversion by test.
- One unified in-house YUV kernel family (`src/yuv_convert.rs`): strip-first,
  generic over sample depth (8/10/12/16-bit) and output pixel
  (RGB/RGBA/Gray x u8/u16), separable auto-vectorized chroma passes, one
  canonical f32 numeric recipe (reciprocal-mul normalize, chained FMA,
  round-ties-even), AVX-512/AVX2/NEON/wasm/scalar tiers. Replaces four
  independent kernel implementations; the managed decoder now converts every
  planar/mono path in-house (BT.601/709/2020 + FCC/SMPTE-240M/derived via
  explicit Kr,Kb), and the SvtRs encode path uses the in-house forward
  RGB(A)->YUV420 kernel (box-averaged f32 chroma; measured +1.2 dB on the
  q85 gradient round-trip). 8-bit decode conversion measured 2-6x faster
  (benchmarks/yuv_kernel_unify_2026-07-20.csv); new default-on `avx512`
  feature. The `yuv` crate remains only in the legacy `unsafe-asm` decoder
  and the public error type (queued above).
- SvtRs backend: RGBA8 encode (color 4:2:0 item + straight-alpha Cs400
  `auxl` auxiliary item honoring the `alpha_quality` fallback contract) and
  grayscale encode (monochrome Cs400 color item, `encode-mono` feature) —
  the backend now covers all three 8-bit still input shapes; round-trip +
  contract tests in `tests/svt_rs_backend.rs`
- `DecodeBackend` (renamed from the decode-seam `Av1Backend` to avoid
  colliding with the encoder enum of the same name in `encode` builds)
  gained `Rav1dFfi` (upstream rav1d 1.1.0 with full asm, `unsafe-asm`
  feature) as a third raw-OBU decode-seam arm; the 4-way decode benchmark
  interleaves it when built with `unsafe-asm`
- EXPERIMENTAL svtav1-rs AVIF encode backend: `Av1Backend::SvtRs` behind the
  new default-off `encode-svt-rs` feature (git-branch dep on imazen/svtav1
  `wave2/entropy-c-parity`, pinned rev conformance-verified upstream at
  525/525 mono + 700/700 4:2:0 aomdec cells). 8-bit 4:2:0 stills with
  64-px-aligned dimensions; BT.601 full-range; muxed in-crate via
  zenavif-serialize; out-of-scope configs rejected honestly at validate()
  and encode time. Bitstream identity vs C-SVT is NOT yet asserted — that
  parity gate lands with svtav1-rs decision-layer bitstream identity.
  (`src/encoder_svt_rs.rs`, `tests/svt_rs_backend.rs`)

- **Adopt the `zencodec` `CategorizedError` taxonomy (PR #103, final API).**
  `Error` now `impl zencodec::CategorizedError` with
  `codec_name() == Some("zenavif")` — a `&self` method (not an associated const,
  so the trait stays dyn-compatible) — and an exhaustive `category()` mapping
  every variant to one coarse `zencodec::ErrorCategory`, so consumers route on
  the category (HTTP status, retry policy, logging) without naming the enum. The
  `Parse` arm **delegates** to `zenavif_parse::Error`'s own `CategorizedError`
  (zenavif-parse PR #17): a malformed container stays `MalformedImage`, a
  truncated one `UnexpectedEof`, a parser cap `LimitsExceeded`. Other arms:
  `Unsupported` → `UnsupportedImageFeature`; `ImageTooLarge` →
  `LimitsExceeded(Pixels)`; `ResourceLimit` → `LimitsExceeded(Memory)` (the
  `String` variant is a catch-all, so a representative kind is reported, with
  the precise limit in `Display`); `OutOfMemory` → `OutOfMemory`;
  `Cancelled(r)` → `r.category()`; `UnsupportedOperation(op)` → `op.category()`;
  `Decode` / `ColorConversion` (foreign `yuv`) / `Encode` → `Internal`. `Decode`
  is a grab-bag the decode pipeline reuses for decoder setup/flush faults and
  internal invariant checks (e.g. "expected 8-bit planes") as well as some
  malformed-input cases; with no structural signal to split it, the conservative
  `Internal` is its best single category. Behind **temporary `[patch.crates-io]`
  path pins** to the sibling `../zencodec` checkout (the taxonomy is
  post-0.1.25, unreleased) and `../zenavif-parse` (0.7.0, unreleased) until both
  publish. Additive (`#[non_exhaustive]` enum + opt-in trait).

### Changed
- Backend pins: zenav1-aom ed29932f -> 7b972e50 (structured `DecodeError`
  + `DecodeConfig`, bd8 lowbd perf pipeline), svtav1 wave2/entropy-c-parity
  -> master 3e25f52b as `zenav1-svt` (bd10 palette panic gate, palette
  byte-parity #71, SB128 fix #91). Both seams now map every backend error
  variant onto the matching zenavif `Error` variant, the svt seam uses the
  fallible `try_encode_frame*` entries (no more `is_empty()` heuristic),
  and the caller's stop token is threaded into the svt pipelines.

- **Reshape `ErrorCategory` onto zencodec's origin-first, two-level taxonomy
  (zencodec PR #116, `caterr-reshape`), superseding the flat 17-variant shape
  the PR #103 entry below describes.** New shape: `Image(ImageError) /
  Request(RequestError) / Resource(ResourceError) / Policy(PolicyKind) /
  Stopped(enough::StopReason) / Io(CodecIoKind) / Internal(InternalKind)`.
  `error_from_rav1d` now classifies rav1d-safe's real cause instead of
  discarding it: `InvalidData` → `Malformed`, `NeedMoreData` → `UnexpectedEof`,
  `OutOfMemory` → the dedicated `OutOfMemory` variant; only the genuinely
  opaque `InvalidSettings`/`InitFailed`/`Other` setup faults still construct
  `Decode` (→ `Internal(Bug)`, not attributable to the input bitstream).
  `Unsupported(&str)` split by origin across `cicp_resolve.rs` (image-feature),
  `GainMapRender` mode (request), an internal grid invariant (internal), and
  alpha-size mismatch (request-buffer) — previously one string conflating four
  origins. `ErrorCategory::Lifecycle` (the original name in this reshape) was
  further renamed to `Stopped` for the same `StopReason` payload — naming
  only, done crate-wide across every zen codec in the same pass.
  `tests/whereat_trace_preservation.rs`'s `av1_decode_error_preserves_trace`
  updated to accept `Malformed` (the now-correct classification for a
  corrupted AV1 payload) alongside `Decode`, preserving its real assertion
  (non-empty trace across the `decoder_managed` boundary). Additive at the
  type level (new `#[non_exhaustive]` enum shape); `ErrorCategory` itself is
  still unreleased, so this is not a break of any published zencodec API.

### Fixed
- **Lossless no longer pessimizes at slow speeds (issue #8).** On the
  zenrav1e release the dep chain currently resolves (0.1.4, whose
  "lossless" is qi=1 lossy — zenrav1e#9), speeds 1-4 measurably produce
  +5..+19% larger lossless files than speed 8 at 4.6-11x the wall time,
  and speed 10 is larger still on most content (up to +42%): the slow
  tier's RDO optimizes against phantom distortion the fixed encoder
  wouldn't emit. Lossless encodes now clamp their running speed preset
  into the empirically size-optimal [6, 8] band
  (`LOSSLESS_REGISTRY_SPEED_BAND`); lossy encodes are untouched, the
  encode plan and sweep fingerprint mirror the clamp, and the residual
  in-band s6-vs-s8 inversion is ≤ ~1.1%. Measured before/after:
  `benchmarks/lossless_speed_clamp_2026-07-23.tsv` (requested s1: −5.0
  to −15.2% bytes at 3.2-3.8x faster; worst-case pixel delta at s1-2 on
  paris drops 8 → 2). REMOVE the band (set the const to `None`) at the
  zenrav1e >0.1.4 dep bump — the fixed encoder is bit-exact and
  byte-monotonic (slower = smaller), see the dep-bump checklist in
  CLAUDE.md and `EncoderConfig::speed_effective`.
- svtav1-rs QP-0 corruption gate: quality >= ~99.3 mapped to QP 0, which
  emits valid-syntax bitstreams decoding to garbage pixels on the pinned
  rev (imazen/zenav1-svt#5). The seam clamps QP to >= 1
  (`quality_to_qp_gated`), so quality 100 now encodes at the best verified
  tier instead of corrupting.
- **Decode corruption: the bottom two rows of every even-height 4:2:0 decode
  that routed through the `yuv` crate's bilinear converters came back
  unwritten (black / alpha-0).** Upstream defect in yuv 0.8.12–0.8.16
  (`yuv420_*_bilinear` / `i0xx_*_bilinear` pair luma row-pairs with
  overlapping chroma `windows()`, dropping the final pair on even heights;
  odd heights and 4:2:2 unaffected). Affected zenavif paths: RGBA composite
  decode (`decoder_managed`), the exotic-matrix RGB fallback, raw-OBU gain
  map decode (`decode_av1`), and all 8/10/12/16-bit 4:2:0 paths of the
  legacy `unsafe-asm` decoder. Fixed by `src/yuv_bilinear_fix.rs`: run the
  upstream converter, then re-run it on a tight 2-row sub-image with the
  last chroma row duplicated (the crate's own odd-height clamp semantics) —
  already-written rows stay byte-identical. Found by the SvtRs RGBA
  round-trip test (color PSNR 20.10 dB → 50.20 dB); regression-guarded by
  unit tests including a canary that fails when upstream fixes the bug.
- `cargo test` / `cargo test --features encode` failed to compile on any
  feature set that leaves `zencodec` or `encode` off: the
  `gainmap_render_probe`, `gainmap_reencode`, `twelvebit_probe`,
  `hdr_encode_cell`, and `hdr_fidelity_probe` examples landed without
  `required-features` gates (pre-existing; same batch as the red main CI)
- Executable engineering-baseline gates (`docs/ENGINEERING_BASELINE.md` A2/A3/A6):
  `examples/gate_kit.rs` (`determinism`/`cells`/`ladder` subcommands on pinned
  integer-synthetic content) + `scripts/gates/gate_conformance.sh` (the PALCONF
  protocol: aomdec-clean + aomdec==rav1d-safe raw md5) + justfile targets
  `gate-determinism`/`gate-conformance`/`gate-ladder`(-pin)/`gates` + a CI job
  for the determinism `--ci` subset + the machine-scoped ladder envelope
  `benchmarks/gate_ladder_envelope.tsv`. zenrav1e's halves (A1 identity, A5
  recon) landed as `zenrav1e@e0b5b44b`.
- SSIMRD (the TUNER2 "what remains" item (a) prosecuted; record
  `docs/RD_GAP_VS_LIBAOM.md` "SSIMRD"): aom's per-16×16 ssim-rdmult λ curve
  (`av1_set_mb_ssim_rdmult_scaling`, the LAST unported iq/ss2 rdmult
  mechanism) ported to zenrav1e (`zenrav1e@57de2815`,
  `EncoderConfig::ssim_rdmult_strength`, default-off byte-identical) and
  measured as a monotone HONEST NEGATIVE under the composed tune (strength
  ladder 0.25→2.0: mass-weighted ssim2 BD +2.11→+6.85, butteraugli agreeing
  from the aom-verbatim point; no photos-merit; the single train winner 6018
  refuted by its val class-sibling 6091 at +4.77/+2.85/+6.50). The composed
  tune's own distortion-side masking + variance-boost subsume the curve.
  1236/9094-class movement vs cpu2iq-ai: flat-to-worse — the iq-AQ residual
  owner is NOT this curve; the only iq machinery left unported is the
  CDEF_ADAPTIVE adaptation schedule. First program run under the 93b83401
  evaluation policy (per-family first, cluster-mass-weighted, pre-registered
  amended rule). Infra: `scripts/rd_gap/chain_ssimrd.sh` +
  `scripts/hyperparam/analyze_ssimrd.py`;
  `benchmarks/rd_gap_ssimrd{,_val}_2026-07-05.tsv` + raw-sweeps pointer.
- TUNER2 (the two P3 "Near-lossless rescans" handoffs prosecuted; record
  `docs/RD_GAP_VS_LIBAOM.md` "TUNER2"): FOUR measured honest negatives — the
  per-image boost-strength head (train-LOOCV + label-drift + val-transfer all
  negative; the 14-origin valstr label set fills the head's named data gap),
  the deeper-boost-curve ramp (never fires on the deep-AQ class: 1-bit scans /
  gradient illustrations are not low-8×8-variance), the anti-boost OFF-gate
  (blocked by a named corpus gap: document-charts absent from train26 while
  val charts pay +5.8/+7.3 at str1), and the 6096 dead-zone/rounding probe
  (QROUND=128 aom-parity: med +2.67, 20/23 butteraugli vetoes — the constant
  does not transplant without aom's whole valuation stack; zenrav1e#30 item-1
  closed). PLUS the drift discovery: the 2026-07-02 strength labels are STALE
  under the composed tune (qmdist+lfsharp subsumed 2-4 BD points of the
  boost's marginal). Boost default 1.0 stands (18/23 current-binary train
  wins). Infra: `scripts/rd_gap/chain_tuner2.sh` + `fetch_tuner2.sh`,
  `scripts/hyperparam/{refit_boost_strength_p3,fit_boost_gate,analyze_tuner2}.py`,
  label store sources `valstr-2026-07-04` + `tuner2-2026-07-04` (64,338 rows);
  benchmarks `tuner2_valstr_2026-07-04.tsv` +
  `hyperparam_boost_{refit,gate}_2026-07-04.tsv`. Conformance: 1,872 cells
  (1,488 knob-armed incl. the quantizer-rounding arms) all PALCONF-clean,
  0 CELLFAIL/CONFFAIL; byte-continuity 96/96 vs the speedladder store rows.
  zenrav1e-side knobs (`variance_boost_strength` / `variance_boost_deep` /
  `quant_rounding_bias`, zenrav1e@6435e6f9) stay default-None byte-identical.

- S4TIER (FAST_TIER_PARITY_PLAN, the last open fast-tier column): the
  s4-equivalent-tier operating point + the program's final scoreboard. The
  fast_heads tx D bound refit per-tier (`dcty<23.69` at requested speed 4..=5,
  LOOCV 22/24-stable; W + partition gates unchanged; `src/fast_heads.rs`
  s4-tier gates, release-gated recommend-only) + the NEW zenrav1e intra
  mode-RDO budget knob (`num_modes_rdo_override`, zenrav1e@071e9844 — the "no
  top-5 knob exists" gap from the P2 report; default None byte-identical,
  6/6-cell local md5 + 288/288 box chain byte-continuity). Measured (12q,
  PALCONF-clean): **v3+top5 = 6.26× plain-s6 solo (0.97× aom cpu2iq-allintra's
  wall): photos median +2.80 ssim2 / +4.14 ba3n vs cpu2iq-ai — the column
  closed from +4.40/+4.04; top-5 DOMINATED top-7 at mode level** (7.61× for
  +2.84/+4.04); cpu4iq deepens to −1.53/−2.62. The residual is measured
  structural (intraBC screens 8414 +22.5; iq-AQ interiors/illustrations
  1236/9100/9118; near-lossless rescans 6096/6018; ungated full-tx headroom —
  oracle extras +2.36/+2.04 at 10.1×). CDEF/LRF hi-q force probes measured
  null/adverse. Plan verdict: parity ±1% MET at s6+s8, NOT met at the s4 tier
  (structural, per-family quantified); beat ≥2 tiers MET; quality tip KEPT.
  Design fully offline (`scripts/hyperparam/fit_s4_tier.py` over the label
  store); record `benchmarks/rd_gap_s4tier_2026-07-04.tsv` + raw
  `/mnt/v/output/zenavif/s4tier-20260704/`.
- P2HEADS (FAST_TIER_PARITY_PLAN Phase P2, "prediction replaces search"): the
  per-image hyperparameter fast mode. Two new deterministic descriptor heads in
  `src/fast_heads.rs` (release-gated recommend-only, the palette-gate pattern;
  wired into `auto_tune`): a TX budget head ({Largest|Size1|Min} via
  `patch_fraction>0.8505 && dct_compressibility_y>100` withhold /
  `pf<=0.8505 && dcty<8.352` deepen, s6-s8) and a partition budget head
  ({Ship|Max32} via `gradient_fraction_smooth<0.4105`, s6). Fit from the label
  store's fastwins/p1part per-image surfaces (veto-adjusted, LOOCV), the
  conjunctive bounds from a val attribution factoring that convicted the
  pf-only withhold (8103: (none,ship) +18.1 vs (size1,m32) −1.9). Composed s6
  fast mode measured at 12q (box, zenrav1e 39f0ecdd): train26 −4.38 med /
  −7.07 mean vs the s6+size1 base (23/24) vs global-ship −2.89/−4.80; on the
  11 deviating images −5.13 mean vs ship (10/11); VAL (14 held-out origins)
  −3.98/−5.19 vs base, deviators −2.41 mean (6/8, worst real loss +0.32).
  Parity movement (photos vs cached aom-allintra refs): vs cpu4iq-ai
  +2.88/+0.91 (ship) → **+0.57 ssim2 / −0.94 ba3n median — inside the ±1%
  band** (−0.35/−1.70 with the ravif intra arm); below the curve vs
  cpu4def/cpu6iq/cpu6def; composed-v2 (measured wall 3.45× plain-s6)
  strictly dominates cpu2def-ai. Head-3 (intra budget)
  measured NOT-a-head: top-7 keyframe intra (ComplexKeyframes +
  `filter_intra=Some(false)`) is a small broad global win (s6 −0.56 / s8
  −1.17 med, composition-stable) with no per-image structure; no top-5 knob
  exists in zenrav1e (`num_modes_rdo` hardcoded 7|3) — recorded as a ravif
  SpeedTweaks arm candidate. All 0 CELLFAIL / 0 CONFFAIL (PALCONF every
  cell). Fits `benchmarks/hyperparam_{tx,partition}_budget_2026-07-04.tsv`;
  record `benchmarks/rd_gap_p2heads_2026-07-04.tsv` (+ pointer, Tower
  mirror); harness `scripts/rd_gap/chain_p2heads.sh`; label store
  `p2heads-2026-07-04` sources.
- FASTWINS P0 (FAST_TIER_PARITY_PLAN): both speed-ladder cheap wins measured and
  landed upstream — (1) the cavif default-tiling byte hazard is FIXED live on
  ravif main 55f8c935 (tiles capped to ≥1 MP each; old 48-core default cost
  +7.4% median ssim2 BD at s6 with 0/24 images better at any tile count;
  `--threads 1` and 1..48-core defaults byte-identical, 18/18+18/18 md5); (2)
  the s4→s6 rdo_tx cliff is decomposed via new default-off zenrav1e knobs
  (d82c16ba: `rdo_tx_size_override`/`rdo_tx_type_override`/`rdo_tx_size_depth`)
  and the winning size-half depth-1 arm landed release-gated at s6-s8 (ravif
  7baad5f9, `S6_TX_SIZE_RDO_LIVE`): 51% of the s6→s4 RD step at 1.67× solo
  (full-grid s6 −2.78/−3.95/−6.01 ssim2/ba3n/bamax median), s8 −2.89 at 1.43×;
  type-half standalone butteraugli-max-vetoed, reduced_tx_set standalone a
  measured null. 4,176/4,176 armed cells aomdec+rav1d-safe conformance-clean.
  Harness `scripts/rd_gap/chain_fastwins.sh`; verdicts
  `benchmarks/rd_gap_fastwins_2026-07-04.tsv` (+ pointer, Tower mirror); label
  store `fastwins-2026-07-04` sources; zenavif consumes both at the zenravif
  dep bump (see CLAUDE.md "FASTWINS P0").
- SPEED-LADDER GAP MAP: first fast-tier measurement of the RD-gap program —
  zenrav1e s{2,4,6,8,10} × {tune-ss2+palette, off} vs libaom `--allintra`
  cpu{2,4,6,8,9} × {default, `--tune=iq`}, train26 + legacy, per-cell aomdec +
  rav1d-safe conformance (5,520 cells clean), solo wall-time pass. Verdict:
  aom's allintra ladder pareto-dominates every zr arm at matched wall-time on
  photos (no fast-tier crossover); tune mandatory + nearly free at fast tiers;
  mechanism-liveness audit + ranked wedge list seed the fast-mode program.
  Harness `scripts/rd_gap/chain_speed_ladder.sh` + `analyze_speed_ladder.py`;
  `docs/SPEED_LADDER.md` + `benchmarks/rd_gap_speed_ladder_2026-07-04.tsv`;
  label store +9,776 fast-tier rows (speedladder-2026-07-04 sources)
  (5aacaca1, f4924263).
- **`examples/hang_stress.rs` — the zenavif#30 futex-hang repro/stress loop**
  (encode→decode→butteraugli→encode, `fast`/`full`/`decode`/`butter` modes).
  Found the root cause of the two-pass conformance hang: a rav1d-safe
  tile-worker `overlapping DisjointMut` panic (loop-filter compact-COW guards
  vs CDEF) wedged the decode wait forever. Fixed upstream in
  rav1d-safe@49df1fc0 (release-gated until the dep bump past 0.5.7 — see
  CLAUDE.md Known Bugs); verified 613/613 full-stack cells clean on the
  patched chain. Closes zenavif#30.
- **`encode-mono` feature — true monochrome (Cs400) encode for Gray8 input**
  (zenavif#6): `encode_gray8` / the codec Gray8 path route through zenravif's
  new `Encoder::encode_gray8`, coding a luma-only bitstream (no chroma planes,
  no chroma RDO — measured 2–3× faster at output-byte parity vs the gray→RGB
  expansion, `benchmarks/mono_encode_ab_2026-06-11.txt`). End-to-end gate
  `tests/mono_encode_roundtrip.rs` proves the sequence header signals
  `mono_chrome=1` and the file decodes back to native Gray8 via rav1d-safe.
  TEMPORARY non-default gate: requires zenravif ≥ cavif-rs@89668f13 (CI's
  clone-siblings provides it); fold into `encode` at the next zenravif bump.
  Without the feature, Gray8 keeps the pixel-safe RGB-expansion path.
- **HDR contract tests + conformance evidence**: 10-bit PQ/HLG encode→decode
  roundtrips assert CICP echo, clli/mdcv (verbatim G,B,R wire order), re-encode
  chain preservation, and measured pixel-fidelity bounds
  (`tests/hdr_roundtrip.rs`, 5720e260); 21-cell aomdec + rav1d-safe md5
  cross-decoder grid, all clean/agreeing, incl. a 12-bit cell
  (`benchmarks/hdr_conformance_2026-07-03.tsv`, ebc8f525). `examples/ivf_raw`
  now dumps 10/12-bit raw (2-byte LE, aomdec-compatible).
- **Gain-map render evidence**: `examples/gainmap_render_probe` — ReconstructHdr
  envelope verified on 4 SDR-base vectors × 3 headrooms; HDR-base (10-bit)
  reconstruction is an honest documented refusal (dfc878e5).
- `docs/HDR_GAINMAP_STATUS.md`: verified capability tables, measured roundtrip
  gaps, release gates (66ee0be5; finalized 302be267).
- `EncoderConfig::with_gain_map_alt_color` / `with_gain_map_alt_icc` +
  payload-derived gain-map `av1C` (subsampling/monochrome from the AV1
  sequence header; declared dims/depth validated against it — a mismatched
  or unparseable payload is now an honest encode error). Real-vector
  roundtrip contracts: `tests/gainmap_roundtrip.rs`, 4 classes (SDR-base
  4:4:4 multichannel + PQ alt, HDR-base backward + sRGB alt, small-map,
  ICC-base + ICC alt), asserting byte-carry, metadata normal-form equality,
  alt-colr roundtrip, and av1C honesty. Cross-validated with libavif 1.4.1:
  avifgainmaputil printmetadata identical 4/4 vs the original vectors
  (needed the zenavif-serialize tmap-brand/altr/ispe/pixi + seq_profile
  fixes and zenravif GainMapData carriers, all landed on their mains)
  (302be267).

- **Float YUV→RGB output is now byte-identical across arch, SIMD tier, and
  image width.** The 4:2:0/4:2:2 float kernels computed three subtly
  different pipelines: fused `mul_add` on x86-64-v3/NEON lanes, unfused
  mul+add on the scalar tier (what i686 runs — its CI leg had been red on a
  1-LSB green-channel mismatch since 2026-07-06) and on wasm128 (which has
  no FMA at all), and division-normalized ties-away rounding in
  `yuv_to_rgb` (the width-remainder path), so the same image could decode
  to different bytes per platform or per width%8. All paths now compute
  one spec — unfused multiply-add, reciprocal-multiply normalization,
  ties-to-even rounding — which every tier can produce exactly. Rare
  single-pixel ±1 shifts vs. the previous x86-64 fused output are the
  cost of cross-platform determinism; the libavif bilinear-parity suites
  (`test-pixels`, `test-linku`) should be re-run against references as
  follow-up confirmation. (Superseded for d<=12 decode by the fixed-point
  recipe below — all real AV1 depths now use platform-exact integer
  kernels; the one-spec f32 path survives only for 16-bit API entries,
  as scalar `f32::mul_add`, which is correctly-rounded on every target.)
- **rav1d-safe pinned to a git rev (now 398b0bfa, 0.6.0-staged) — registry
  0.5.7 wedge.** Registry 0.5.7 carries a tile-worker guard race whose panicked worker parks
  `rav1d_decode_frame`'s completion wait forever (zenavif#30 — on a
  28-thread box the animated codec roundtrip test hung ~3 of 4 runs). The
  dependency is now a direct git dep at `398b0bfa` (0.6.0-staged: the
  wedge fix 49df1fc0, the #14 aarch64 16bpc SGR rounding fix, and the
  current safe-SIMD state; the earlier f6aed27e `[patch.crates-io]` pin is
  gone — a 0.6.0-versioned rev cannot patch a 0.5.x requirement). Return
  to a registry dep at the 0.6.0 release. The pinned rev's managed API attaches `whereat::At` locations and
  adds cooperative cancellation, so rav1d-safe decode errors now keep
  their upstream trace frames (`At::map_error` at the boundary instead of
  rewrapping) and `Error::Cancelled` maps rav1d-safe's own `Cancelled`.
- **Default-feature `cargo test` compiles again**: four auto-discovered
  examples that call `encode`-gated APIs (`gainmap_reencode`,
  `hdr_encode_cell`, `hdr_fidelity_probe`, `twelvebit_probe`) had no
  `[[example]]` declarations, breaking every platform's CI test job since
  2026-07-12; they are now declared with `required-features = ["encode"]`.

- **Vector/corpus tests no longer silently skip** (no-graceful-skips
  policy, c1533f48): libavif-vector loaders fail loud (CI provisions the
  vectors; locally `just download-vectors`), and the codec-corpus animation
  roundtrip — whose `../codec-corpus` path never resolved, so it silently
  no-oped everywhere since inception — now really runs, with corpus-less
  environments declaring `ZENAVIF_NO_CODEC_CORPUS=1` in the visible CI chain.
- `MasteringDisplayConfig::primaries` doc: the slot order is the `mdcv` wire
  order **GREEN, BLUE, RED** (ST 2086 / HEVC SEI), not R,G,B as previously
  claimed — values were always written verbatim, so following the old doc
  produced swapped primaries for conforming readers.
- **Untrusted-input hardening, decode path** (zenavif#18 items 2–3):
  `StripConverter::new` no longer `panic!`s on unsupported
  (bit_depth, chroma) combinations — replaced by `try_new`, whose
  unsupported case hands the frames back and the decoder takes the
  full-conversion fallback (defense in depth; both values come from the
  attacker-supplied bitstream). The RGB output byte length
  (`pixel_count * 3`) is now a checked multiply returning
  `Error::OutOfMemory` instead of wrapping and under-allocating on
  i686/wasm32 (`rgb_byte_len` + unit test). `DecoderConfig::cpu_flags_mask`
  rustdoc now states the knob is currently inert rather than implying it
  gates SIMD dispatch.
- **Decode allocations are now fallible on the managed path** (zenavif#21):
  the raw-OBU RGB/gray output buffers (`decode_av1.rs`, five sites) and the
  animation frame table reservation (`decode_animation`) route through
  `alloc_util` `try_reserve` helpers, returning a graceful error instead of
  aborting the process on OOM — completing the coverage started by the
  grid-stitch canvas and `convert_to_image` buffers.
- rd_gap harness: per-worker result appends are now flock-serialized (drvfs
  append races silently dropped 315/576 rows on cache-hit-fast runs) and the
  per-cell WORK dir defaults to local disk (drvfs transiently EIOs whole
  worker batches under WSL memory reclaim) (6884b7b, 17c428a).

- `two-pass-butteraugli` feature: `encode_rgb8_two_pass` — butteraugli-
  diffmap-guided second pass (spatial closed loop; libaom `tune=butteraugli`
  analog through zenrav1e's per-SB delta-q). Release-gated at runtime behind
  `zenravif::FRAME_HINTS_LIVE` (fails honestly until the zenrav1e dep bump);
  evaluate-first verdict (aom's own tune: −2.4..−3.5% median butteraugli-3n
  BD on photos, ssim2 neutral-to-better) + mechanism + dep-bump checklist in
  `docs/DIFFMAP_TWO_PASS.md`; A/B harness `scripts/rd_gap/zenavif_2p_cell.sh`
  + `run_2p_ab.sh` + `examples/two_pass_cell.rs` (2e8e9912).
- Size-decay NON-TUNE isolation A/B: driver
  `scripts/rd_gap/sizedecay_nontune_arms.sh` (9 arms over the quality-keyed
  ravif SpeedTweaks gates + Tune::Psnr + composites, per-armed-cell
  aomdec+rav1d-safe conformance), analyzer
  `scripts/hyperparam/analyze_sizedecay_nontune.py`, label-store source block,
  summary `benchmarks/hyperparam_sizedecay_nontune_2026-07-03.tsv`. Verdict:
  no coding default convicted; the tune-off small-px decay decomposes into
  zr's content-adaptive strengths fading on downscaled renditions
  (Psychovisual metric value 7.5->4.4 BD, segmentation AQ 4.1->1.8 BD,
  1024->256) — see RD_GAP_VS_LIBAOM.md. Found + upstream-fixed zenrav1e#34
  (TX_MODE_SELECT sliver gate) and found zenavif#29 (ravif 4:2:0
  non-conformance, open).

### Documentation
- README overhaul: clickable badge row (CI/crates.io/lib.rs/docs.rs/MSRV/dual-license), `## Quick start` with a `[dependencies]` block, absolute links throughout, regenerated crosslink footer (now last), split crates.io README (`README.crates.md` via `readme =` + `include`), and a `benchmarks/README.md` methodology index.

### Added
- **Speed-conditional palette-gate threshold** (follow-up A/B to the mechanism
  A/B; release-gated like the gate itself): `palette_gate(patch_fraction, speed)`
  now uses per-speed-tier thresholds — 0.197 at speed ≤ 5 (byte-identical to the
  prior rule) and 0.05 at speed ≥ 6 (`PALETTE_GATE_PATCH_FRACTION_FAST` +
  `palette_gate_threshold(speed)`); `palette_gate_for_rgb8` gains the speed
  param and `auto_tune` passes the speed it just picked. Measured: arms
  {0.197, 0.10, 0.05, fire-always} × s{2,6,8}, 391/481 BD cells derived offline
  from the label store + mech-A/B TSV (a threshold arm is a per-cell selection
  over already-measured off/always/auto outcomes) + one fresh 1,350-cell s8
  iso run (byte-continuity sha-proven, 0 conformance failures). s2 keeps
  0.197; s6 confirms 0.05 (deploy −0.047 train / −0.074 val vs 0.197, flips
  butteraugli-clean); s8 corroborates (−0.044 val). fire-always measured
  nominally best but rejected: its extra value sits inside the photo
  patch_fraction mass at 1.80×/2.13× (s6/s8) fired encode cost.
  `benchmarks/hyperparam_palette_speed_ab_2026-07-03.tsv` +
  `scripts/hyperparam/fit_palette_speed_threshold.py`; label store +1,350 rows
  (`palette-mech-iso-s8-2026-07-03`).
- **Size-decay isolation A/B (wedge #3) — verdicts + the qmdist size ramp**:
  leave-one-out Tune::Ssimulacra2 mechanism arms × {256,512,1024} renditions
  (`scripts/rd_gap/sizedecay_arms.sh` driver + `scripts/hyperparam/`
  `analyze_size_decay_ab.py`, train/val sample ladders in `scripts/rd_gap/`)
  acquitted 4 of 5 tune mechanisms for the small-size decay (the top suspect
  ss2-QM curves keeps ≥82% of its win at 256), convicted the QM-dist ratio
  (−3.48 → −0.96 median 1024→256), and shipped its half-strength long-edge
  ramp upstream (`zenrav1e@b0098eb1`, release-gated; train +1.03/+0.87 @
  256/512 vs full strength, VAL +1.12/+1.00, butteraugli agreeing,
  conformance 180/180). Most of the wedge decay measured as the tune-OFF
  baseline's own vs-cpu2 decay — the wedge #3 owner moves to non-tune
  small-px coding defaults. Record:
  `benchmarks/hyperparam_size_decay_ab_2026-07-03.tsv` (+ raw pointer),
  `docs/RD_GAP_VS_LIBAOM.md` "Size-decay isolation A/B".
- **Hyperparameter-expert label store + first threshold-rule heads**
  (FEATURE_HINTS_PLAN §E): `scripts/hyperparam/build_label_store.py` aggregates
  every mechanism fit sweep + the wedge dataset into one queryable parquet
  (14,880 rows / 50 arms, per-row arm knobs + encoder_rev + q_kind + LSD split
  + feature-join; block storage + Tower, pointer in `benchmarks/`), and three
  first-cut rule fits land their evaluations in
  `benchmarks/hyperparam_*_2026-07-03.tsv`: the zenanalyze palette gate
  (`patch_fraction > 0.197` → Always — the graduating head), the size-decay
  attribution (1024→512 decay is a high-quality-band loss; ss2-QM top suspect),
  and the per-image variance-boost rule (not deployable at n=24; global 1.0
  stands). Report: `docs/HYPERPARAM_FIRST_CUT_2026-07-03.md`.
- **Precise perceptual-quality targeting** (`target-quality` feature):
  `encode_rgb8_with_target` converges the encoder on a requested SSIMULACRA2
  or zensim score via an encode→decode→score bracketed secant search
  (typically 3–5 encodes at ±0.5 tolerance). New `TargetMetric` /
  `TargetOptions` / `TargetedEncode` types; selection policy returns the
  smallest file inside the target band and reports `converged` honestly
  when the target is unreachable. RGBA variant
  (`encode_rgba8_with_target`): zensim scores alpha natively; SSIMULACRA2
  composites both sides onto mid-gray. 16-bit variant
  (`encode_rgb16_with_target`, 10-bit AV1): ssim2 scored natively at
  16-bit, zensim on an identical 8-bit view of both sides. Contract tests
  in `tests/target_quality.rs`.
- **Honor `zencodec::AllocPreference` at zenavif's own decode allocations.**
  The full-image RGB(A) output buffer, the grid-stitch canvas, the crop
  destination, and the per-row YUV→RGB scratch now route through a 3-mode,
  per-site allocation helper (`src/alloc_util.rs`). Big buffers sized from the
  (untrusted) AV1 frame / grid dimensions default to the fallible
  `try_reserve` path (graceful `Error::ResourceLimit` instead of an allocator
  abort); the width-bounded per-row scratch defaults to the infallible `vec!`
  path. `ResourceLimits::prefer_fallible_allocations` (`Fallible` /
  `Infallible`) overrides every site; `CodecDefault` (the default) keeps each
  site's own default, so behavior is unchanged. Wired only at the `zencodec`
  `DecodeJob` boundary (`effective_config`) — the direct `zenavif::decode` API
  leaves it `CodecDefault`. The AV1 frame/tile buffers live in the
  `rav1d-safe` dependency and are out of scope (noted as a follow-up).
- **`DecoderConfig::estimate_decode_resources` (zencodec `estimate` API).**
  Mirrors `estimate_encode_resources`: delegates to the calibrated
  `heuristics::estimate_decode` (peak memory + time from the decoded
  bytes-per-pixel) and maps a new conservative `heuristics::decode_threading_info`
  onto `zencodec::estimate::ThreadingInformation` (AVIF decode is only partly
  parallel — tile decode parallelises, the YUV→RGB conversion does not).

### Changed
- **zencodec trait impls return the `At<CodecError>` envelope (Pattern B).** Every
  `zencodec` trait method (`EncoderConfig`/`EncodeJob`/`Encoder`/`AnimationFrameEncoder`
  + `DecoderConfig`/`DecodeJob`/`Decode`/`StreamingDecode`/`AnimationFrameDecoder`)
  now declares `type Error = whereat::At<zencodec::CodecError>` instead of
  `At<Error>`. A generic consumer driving zenavif through `Dyn*` dispatch — where
  the error is erased to `Box<dyn Error + Send + Sync>` — now recovers the coarse
  `ErrorCategory` **and** the `"zenavif"` codec name via `CodecErrorExt`
  (`error_category()` / `codec_error()`), which return `None` under the previous
  native-error type. The native `Error` enum (the detail + category source, with
  its `CategorizedError` impl including the `Parse` delegation) is **unchanged**,
  and zenavif's inherent rich public API still returns `At<Error>` for direct
  callers — only the zencodec trait boundary changed. Internally each trait method
  keeps its verbatim logic in a private `*_inner` helper returning `At<Error>` and
  re-wraps once at the boundary with `CodecError::of` (already-located errors,
  trace preserved) or the new `From<Error> for At<CodecError>` bridge (bare errors
  at reject / sink-wrap sites). Additive at the type level; visible only to
  generic `zencodec`-trait consumers.
- **`zencodec` is now a required (non-optional) dependency.** The optional
  `zencodec` cargo feature is removed; the codec-trait integration is always
  built (the `codec` module implementing `Encoder`/`Decoder` +
  `SourceEncodingDetails`, the `CategorizedError` impl, and
  `From<zencodec::AllocPreference>`). `ultrahdr-core` (gain-map `ReconstructHdr`
  math, previously pulled only by the `zencodec` feature) becomes a hard dep for
  the same reason. The `cov_zencodec` / `color_context_decode` / `mono_decode` /
  `gainmap_decode` integration tests are ungated (they run under default
  `cargo test`); CI drops the now-redundant `--features zencodec`
  test/check/clippy steps and provisions the downloaded AVIF vectors in the
  `i686` and `coverage` jobs, which now run those decode tests.
- **Migrate to zenavif-parse 0.7.0 `whereat::At<Error>` Result API.** zenavif-parse
  now returns `whereat::At<Error>` (location-tracing) from its parser entry points;
  zenavif's parse-error boundaries (`decoder_managed.rs`, `decoder.rs`,
  `detect.rs`) switch to the trace-preserving `ResultAtExt::map_err_at(Error::Parse)`
  / `At::map_error(Error::Parse)` (and `e.error()` for the probe-classification
  match) instead of dropping-and-rewrapping the inner error. Required to consume
  zenavif-parse's `CategorizedError`. Decode output is unchanged (error-path only).
- **Re-calibrate the ENCODE peak-memory model (was over-conservative).** A fresh
  VmHWM + heaptrack sweep (`examples/mem_probe_encode`, RGB8, threads=1, sizes
  256–2048 px × speed {6,8,10} × photo/screenshot × q {50,85}) showed the prior
  "2026-06-14" constants over-predicted typical peak by up to 2.0× at small sizes
  (the 8 MiB fixed term dominated a measured ~4.6 MiB intercept) and 1.35× at
  4 MP. Tightened `ENCODE_FIXED_OVERHEAD` 8 MiB → 5.5 MiB and `ENCODE_BPP`
  40 → 37 B/px in `src/heuristics.rs`. The TYP still clears the measured
  worst-case marginal + 10 % at every swept size (min margin 1.107× at 1 MP — it
  never under-predicts), and the MAX (1.8×) tier clears the heaptrack-requested
  heap by 1.6–1.9×; over-prediction is now 1.11–1.49×. Found that encode memory
  is driven mostly by **quality** (q85/q50 ≈ 1.28×) and **content** (photo/shot
  ≈ 1.26×), and only ≈ 1.09× by speed — so the model's speed-independence is
  conservatively fine (constants fit to the speed-10/q85/photo worst case). The
  alpha (+7 B/px) and 10-bit (×1.55) memory factors were not re-measured
  (RGB8-only sweep) and are carried forward unchanged. Provenance:
  `benchmarks/zenavif_encode_mem_2026-06-23.tsv`.
- **deps: migrate to published `zencodec 0.1.24` estimate API; drop git-rev
  patch.** Removed the temporary `[patch.crates-io] zencodec = { git, rev =
  "0f71295" }` now that `zencodec 0.1.24` is on crates.io. Migrated the
  `estimate_encode_resources` mapping in `src/codec.rs` for the refined
  `ResourceEstimate`: `new(peak, wall_ms: u64)` (was `f32`),
  `with_peak_max(max)` (the `min` arg is gone), dropped the removed
  `with_output_bytes`, and the encode `ThreadingInformation::parallel` is now
  1-arg (`parallel(max_useful_threads)`; the `fraction` / `mem_per_thread` args
  are gone). `cargo update -p zencodec` pulled published 0.1.24.
- **Preserve the encoder's whereat trace across the ravif boundary.** The
  `encode_*` paths now convert ravif (zenravif) errors with
  `.map_err_at(|e| Error::Encode(e.to_string())).at_crate(…)` instead of
  `at!(Error::Encode(e.to_string()))`. The old form started a *fresh* trace and
  discarded ravif's frames; `map_err_at` keeps the underlying `At<ravif::Error>`
  trace (the originating encode site) and `at_crate` annotates the zenavif
  boundary. Also bumps the `ravif`/`zenravif` dep to `0.2.0` (its `At<Error>`
  public-error API) — the conversion now takes the inner `ravif::Error` via
  `map_err_at`, which both preserves the trace and matches the new signature.
- **Decode is bounded by default (closes the decode fail-open part of #22).**
  `DecoderConfig::default()` now defaults `frame_size_limit` to `120_000_000`
  (120 MP, admits ~108 MP photos) instead of `0`. The pre-flight dimension
  check (`decoder_managed.rs` / `decoder.rs`) only fires when the limit is
  `> 0`, so `zenavif::decode(untrusted)` was previously unbounded by total
  pixels; it now rejects over-120-MP frames with `Error::ImageTooLarge`
  before any frame allocation. Non-breaking: `frame_size_limit(0)` still opts
  out (unbounded), and the parser already inherits zenavif-parse's own caps
  (512 MP / 1 GB peak). The frame-granularity cancellation sub-point of #22
  stays open pending a rav1d-safe Stop hook.

### Fixed
- **Preserve whereat traces across the decode strip-conversion / animation
  boundaries.** Three decode-path sites discarded the inner `At<Error>` trace
  with `.map_err(|e| e.decompose().0)?` (`codec.rs` streaming `next_batch` +
  `render_next_frame`, and `decoder_managed.rs` `decode_to_sink`). They now use
  `.at()?`, which keeps the originating trace (`StripConverter::convert_strip` /
  `AnimationDecoder::next_frame`) and adds a frame at the propagation site. New
  `tests/whereat_trace_preservation.rs` drives a corrupt and a truncated AVIF
  through `decode_with` and asserts the surfaced error carries a non-empty trace
  (`frame_count() >= 1`). The parser boundaries (`parser.frame` / `tile_data` /
  `primary_data`, etc.) keep `at!(Error::from(e))` — zenavif-parse `0.6.2`
  returns a *bare* `Error`, so `at!` correctly starts the trace there (matching
  the in-file `TODO(whereat)`); they switch to `map_err_at` only once
  zenavif-parse ships its `At<Error>` API.

### Added
- **vCPU-aware resource estimation via zencodec's unified `estimate` API.**
  `AvifEncoderConfig::estimate_encode_resources(&ImageCharacteristics,
  &ComputeEnvironment)` overrides the `zencodec::encode::EncoderConfig` default,
  delegating to the calibrated `heuristics::estimate_encode` (memory / time /
  output, keyed on the AV1 `speed` preset + input bytes-per-pixel) and folding
  in core count via `ResourceEstimate::at_cores`. The crate's local
  `heuristics::ThreadingInfo` is kept (decoupled from the optional `zencodec`
  dep so a decode-only build still compiles) and mapped onto the shared
  `zencodec::estimate::ThreadingInformation` only at the trait-impl boundary.
- **Sweep generator: trained-scalar-head + compute-budget surface**
  (`__expert`, VARIANT_GENERATION patterns 17–18). `SweepAxes::scalar_dense()`
  gives each CONTINUOUS knob a dense isolated ladder (speed `2..=10`,
  VAQ-strength, seg_boost) with every categorical axis pinned to its
  production default — the per-knob response curves a scalar regression head
  fits; pair with `with_max_deviations(1)`. `compute_tier(&EncoderConfig) -> u8`
  is the ordinal CPU-cost proxy, driven primarily by speed **inverted** (AV1
  `speed` is higher-is-faster, so tier = `10 − speed`; +2 trellis, +1 QM).
  `SweepBuilder::with_compute_limit(max_tier)` drops cells over budget (the
  fast/high-speed end survives), reported in the new
  `SweepPlan::compute_tier_skipped` field — no silent caps.
  `SweepBuilder::with_max_deviations(max)` scopes to main-effects-only.
  `scalar_dense` is resolvable via `SweepAxes::by_name`. All additive.
- `examples/heaptrack_decode.rs`: a reusable heaptrack/valgrind harness that
  decodes an AVIF file from bytes via `zenavif::decode(..)` in a loop, for
  profiling heap-allocation behaviour. Defaults to the committed
  `tests/vectors/libavif/kodim03_yuv420_8bpc.avif` (768×512, 4:2:0 8-bit) decoded
  8×; a path + iteration count can be passed. Driven by `just heaptrack-decode`.
  Profiled result: heap **size** and leaks are healthy — peak 2.41 MiB (O(image)),
  leaked pinned at ~28 across 2/8/16 iterations (one-time thread-pool statics, no
  per-decode growth). Allocation **count** is a pathology — ~37,000/decode, of
  which 99% are transient (0 B net) and concentrated in the **rav1d-safe** backend:
  `compact_read_per_row` (325,917 total across 9 decodes) stages a strided plane
  region into a fresh contiguous heap buffer per CDEF/loopfilter/intra-pred block.
  The churn originates in the rav1d-safe dependency's safe-SIMD filter path, not
  in zenavif's own container/YUV code. Tracked as a resource follow-up.
- **Calibrated resource-estimation module (`heuristics`).** New
  `zenavif::heuristics` with `EncodeEstimate` (min/typical/max peak memory +
  `time_ms` + `output_bytes`), `DecodeEstimate` (peak memory + time +
  output), and `estimate_encode(w,h,input_bpp,speed)` /
  `estimate_decode(w,h,output_bpp)` — mirrors the zen per-codec pattern
  (`zenwebp::heuristics`). Calibrated from real measurement, not guesses: a
  new `examples/avif_probe` measures the marginal working set (`VmHWM`
  delta) plus wall and user/sys CPU (`/proc/self/stat`, threads=1, one
  process per op), swept by `scripts/avif_resource_calibrate.py` over
  5 content classes × 256–2048 px × speed 4/6/8/10 × rgb/rgba × 8/10-bit
  (`benchmarks/avif_resource_{main,alphadepth}_2026-06-14.tsv`). Model
  captures that AVIF encode time is dominated by the AV1 `speed` preset
  (~7.6/2.0/1.2/0.55 us/px at speed 4/6/8/10, single-thread, ~14× spread),
  that AVIF is light on memory (encode ~40 B/px, decode ~18 B/px), and the
  alpha (+7 B/px, +30 % time) and 10-bit (×1.55 mem, +40 % time) deltas.
  Times are single-thread CPU; divide by thread count for wall latency.
  (7d00689)

### Added (sweep planner — dense-sweep program)
- **SCALAR ladder densification** for `zenpicker-train --scalar-axes` heads
  (zenmetrics `docs/PLAN_SWEEPS.md` §5 gaps): seg_boost probes
  {0.75, 1.5, 2.5, 4.0} (de-boost direction + validate endpoint) and
  vaq_strength probes {0.25, 2.0, 3.0} (+ the 0.5 vaq-axis stratum). New
  values ride the existing `-sb<f>`/`-vaqs<f>` id grammar and resolved-state
  fingerprint. Direct quantizer (qp) axis documented as **blocked on encoder
  knob** (no public quantizer setter in zenavif/zenravif; the resolved
  quantizer is already the picker mediator via `feature_row`).
- **Still-envelope equivalence finding (proven by encode, 28/28 × 3 pairs)**:
  `vaq_strength(x)` ≡ `seg_boost(x)` byte-identically on still encodes (both
  are the same log-domain exponent on the `spatiotemporal_scores` →
  `segmentation_scores` chain; zenrav1e `internal.rs:1379`). Curated ladders
  interleave values so the joint 8-point effective ladder is alias-free,
  with a test guarding disjointness; the fingerprint deliberately
  under-merges the spellings (animated/inter encodes may diverge). See
  `docs/VARIANT_GENERATION.md`.

### Added (SIMD platform parity — issue #2)
- **NEON bilinear AVG** (`src/simd/avg.rs`): 16 pixels/iteration via
  `vqrdmulhq_s16` (bit-exact pmulhrsw for the 1024 multiplier — the
  saturating corner is unreachable), wired into the runtime dispatch.
  Parity vs scalar verified under qemu (`cross test
  --target aarch64-unknown-linux-gnu`), including non-multiple-of-16
  tails and a loud failure if the NEON token can't summon. Closes the
  one production-relevant gap from the cross-platform audit: the
  remaining table rows (`yuv_convert_fast`, `yuv_convert_libyuv_simd`)
  are `_dev`/benchmark-only modules with no production call sites — the
  production YUV strip/full converters in `yuv_convert.rs` already
  cover wasm32 via `#[magetypes]` (v3/neon/wasm128).

### Investigated + fixed upstream (lossless speed inversion — issue #8)
- Root cause found and fixed in zenrav1e (c3567081, closes zenrav1e#9):
  "lossless" never reached qindex 0 — the rate path floored it at 1, so
  every lossless encode was qi=1 lossy with ±2 error on 7-28% of pixels,
  and the speed "inversion" was RDO rationally spending bits against
  that phantom distortion (slow speeds were 8-13% wrong pixels vs 17-27%
  at fast speeds — size inversely tracked exactness;
  `benchmarks/lossless_speed_sweep_2026-06-11.tsv`). With the fix,
  roundtrips are bit-exact (0 mismatched pixels) on every source at
  every speed and bytes are monotonically non-increasing with effort
  (`benchmarks/lossless_speed_sweep_fixed_2026-06-11.tsv`).
  `examples/lossless_speed_sweep.rs` is the permanent harness. The fix
  reaches zenavif when zenrav1e 0.1.5 releases and zenravif bumps;
  `tests/identity_roundtrip.rs` tolerances then tighten to exact.

### Changed (gray + RGB-ICC refinement)
- **Derivable RGB-class profiles no longer block native gray.** Gray
  files carrying RGB ICC profiles are common in the wild; when the
  profile's CICP is derivable (embedded `cICP` tag or normalized-hash
  identification — the same chain the load-bearing reduction uses), the
  decoder emits native Gray8/Gray16 with a **CICP-only context** (white
  point + transfer remain fully meaningful for single-channel data, per
  moxcms guidance) instead of expanding to RGB just to honor the
  profile. Only underivable RGB-class profiles keep the RGB layout. The
  class-gate fallback now also prefers the profile-DERIVED CICP over the
  signaled nclx (the profile outranked it per MIAF). New fixture:
  `mono_gradient_8b_p3icc.avif` (real Display P3 profile → gray + CICP
  (12, 13)).

### Changed (devil's-advocate follow-ups on self-describing buffers)
- **Streaming strips now carry the context** — the "known gap" was a
  plain drop bug: every emission path copies rows into a per-batch
  scratch buffer and returned slices of that. The class-gated context is
  now stored on the streaming decoder and re-attached to every emitted
  strip (baked/grid/converter paths); the converter path uses the
  frame-era info the converter already returned (previously discarded),
  fixing a probe-vs-frame CICP divergence the new test exposed.
- **Mono files carrying an RGB-class ICC no longer decode native gray**
  — the profile is the most accurate color description present, so the
  decoder keeps the RGB layout it validly describes instead of stripping
  it (a gray preference then resolves through the load-bearing ICC
  rules: swap when derivable, honest suppression otherwise). GRAY-class
  ICC mono files decode native gray with the profile riding along. New
  ICC-tagged mono fixtures cover both.
- **HDR reconstruction output now carries a synthesized linear CICP**
  (source primaries raw, H.273 transfer 8, identity matrix, full range)
  instead of being context-free — no SDR signaling carries over, but the
  linear output is describable and the raw primaries code point survives
  where the descriptor enums fold.

### Added (self-describing decoded buffers)
- **Decoded buffers now carry a `zenpixels::ColorContext`** — the
  authoritative source color, selected through zencodec's drop-dupe
  rules (`SourceColor::to_color_context`: ICC > nclx per MIAF, the
  non-authoritative duplicate dropped) and **class-gated** so an
  RGB-class ICC never rides a Gray-layout buffer (the raw H.273 CICP —
  code points the descriptor enums fold away — stays as the fallback).
  Attached on the buffered decode, streaming-bake, streaming-reconstruct
  (SDR base), and animation paths; conversions, orientation bake, and
  the load-bearing gray reduction all propagate it, and
  `downscale_to_8bit` now carries it across its rebuilds. HDR
  reconstruction output is deliberately context-free (no SDR profile
  describes linear f32; the descriptor is the honest carrier). Known
  gap, test-pinned: the strip-converter streaming path does not yet
  attach contexts to strips.

### Added (load-bearing ICC contract tests)
- `codec::tests::negotiate_gray` pins the gray-collapse ICC rules in
  zenavif's exact feature configuration (zenpixels-convert without
  `icc-db`): no-context collapse with exact channel equality; sRGB ICC →
  collapse with the RGB-class profile dropped to CICP-only (an RGB
  profile must never ride on a Gray buffer); underivable ICC →
  suppression that falls through to the next preference; and lying
  metadata (mono-flagged but colorful pixels) → byte-verified refusal.
  Upstream 0.2.13 audit: plan logic + all five signal outcomes tested,
  174/174 bundled gray profiles GRAY-class and byte-identical to fresh
  moxcms synthesis (zero-tolerance gate verified green locally — it is
  `cms-moxcms`-gated and not exercised by upstream CI; reported).

### Changed (load-bearing API integration)
- The mono-source gray negotiation arm now uses zenpixels-convert's
  `reduce_to_load_bearing_format_in_place` instead of `to_gray8()`: the
  R==G==B collapse is byte-verified (not metadata-trusted), rewrites in
  place with no allocation, and inherits the load-bearing module's color
  signaling rules (an RGB-class ICC profile can't describe a Gray layout
  — a gray-class variant is swapped in when derivable, otherwise the
  collapse is suppressed and negotiation falls through honestly).
- `ReconstructHdr` output now tags alpha `AlphaMode::Opaque` (the apply
  kernels emit constant 1.0 structurally) so downstream encoders know
  the lane isn't load-bearing without rescanning.

### Added (native grayscale decode — issue #5)
- **Monochrome AVIFs decode to native `Gray8`/`Gray16`** (1-2 bytes/pixel
  instead of the 3-8x RGB expansion) through the zencodec adapter:
  `native_gray` capability is now `true`, `GRAY8_SRGB`/`GRAY16_SRGB` join
  the supported descriptors, and an alpha-free mono source decodes gray
  when the caller passes no preference (gray IS the native format) or
  ranks a Gray descriptor first. Range expansion runs through the same
  `yuv` kernel as the RGB path — gray output equals the R channel of an
  RGB decode bit-for-bit (test-pinned, both ranges, 8/10-bit). Streaming
  emits gray strips identical to the buffered decode. Gray preferences on
  color images are never satisfied by luma synthesis; grid composition,
  animations, and the non-zencodec `decode_with` API keep their RGB
  behavior. Genuine Cs400 fixtures live at `tests/vectors/zenavif/`
  (generated by zenavif-serialize's `make_mono_avif` example).

### Added (HDR reconstruction — issue #17)
- **`GainMapRender::ReconstructHdr` is now honored** (`reconstructs_hdr()`
  capability is `true`): the decoder applies the ISO 21496-1 gain map to
  the SDR base via `ultrahdr-core`, producing linear f32 RGBA (1.0 = SDR
  white / 203 nits, base-image primaries). `target_headroom: Some(h)`
  renders for an h×-SDR-white display; `None` reconstructs at the map's
  encoded maximum. MaxCLL/MaxFALL are **measured** from the reconstructed
  pixels per the zencodec envelope obligation, and the gain-map components
  are still surfaced for transcode use. The streaming decoder reconstructs
  whole-image and emits fixed-height strips (same shape as the
  orientation-bake path); buffered and streaming outputs are bit-identical.
  Alpha-carrying and >8-bit bases are refused loudly (use `Components` and
  apply downstream); files without a gain map decode as honest SDR.

### Fixed (corpus integrity — issue #16)
- The libavif corpus test now asserts the two `unsupported_gainmap_*`
  fixtures are **rejected with their version-gate errors** — they are
  libavif's tmap version probes, designed to be refused; decoding one would
  be the bug. The other 10 corpus failures (all Apple-style HDR gain-map
  vectors with a size=0 `mdat`) were a zenavif-parse regression — fixed
  upstream in zenavif-parse f3c9f043 (45/57 → 57/57-equivalent validated via
  path-patch); the corpus goes fully green here when zenavif-parse 0.6.3 is
  released and the dependency bumps.

### Fixed (pixel corruption — issues #14 + #15, landed in lockstep)
- **Identity (MC=0) decode**: identity-RGB AVIFs were decoded through BT.601
  matrix math (`_ => Bt601` blind arms) — every pixel wrong (pure red came
  back R=111 G=0 B=0). Identity now takes a no-matrix GBR passthrough
  (8-bit + 10/12-bit, full + limited range; subsampled identity is rejected
  per H.273). The strip fast-path routes identity to the full-conversion
  path.
- **16-bit encode plane order**: `encode_rgb16`/`encode_rgba16` fed
  `[r,g,b]` plane tuples under identity signaling where the AV1 convention
  (and zenravif's own 8-bit path) is **G,B,R** — every 16-bit encode was
  channel-rotated for conforming decoders. Both bugs masked each other in
  self-roundtrips; `tests/identity_roundtrip.rs` pins them together
  (tolerance ≤2 documents zenrav1e#9: `with_lossless` is not bit-exact).
- **H.273 matrix resolution** (`src/cicp_resolve.rs`, the consumer-side of
  zenpixels#36's `Cicp::resolve_matrix` spec — migrates there when it
  lands): unspecified/reserved MC resolves through the container `nclx`
  matrix, else the documented AVIF-spec default (1/13/6) — never a silent
  guess; MC=12 derives coefficients from the colour primaries (CP=9 → the
  canonical BT.2020-NCL; P3 and friends decode **exactly** via the yuv
  crate's custom-KR/KB path — `cosmos1650_yuv444_10bpc_p3pq` previously
  decoded silently wrong through BT.601); MC=4 (FCC) now uses exact FCC
  coefficients instead of a 601 approximation; genuinely unimplemented math
  (YCgCo, BT.2020-CL, Y'D'zD'x, ICtCp, MC=13) errors loudly with the code
  point named. 8-bit RGB conversions for matrices outside the in-house SIMD
  tables (240M/FCC/derived) route through the yuv crate.

### Deprecated
- `Av1Backend::Svtav1`. The `encode-svtav1` feature was never shipped
  (svtav1-rs produces non-conformant bitstreams in most configurations, and
  the draft path returned raw AV1 OBUs in the `avif_file` field instead of an
  AVIF container), so no build could ever encode with it; `validate()` rejects
  it unconditionally. The variant stays for enum compatibility — a working
  svtav1 integration would land as a new variant.

### Removed
- The unreachable `encode-svtav1`-gated code: the `encode_rgb8_svtav1` draft
  path, the backend dispatch in `encode_rgb8`, three differential test files
  (`differential_svtav1.rs`, `differential_comprehensive.rs`,
  `differential_4k.rs` — all cfg'd on the never-shipped feature and
  referencing the commented-out path dep), the commented `svtav1` dependency
  and feature lines, and the `check-cfg` allowance. None of this compiled in
  any build; git history before this commit has the experiment if it resumes.
  Consequence: the `matrix_coefficients` CICP field is now consumed by no
  backend (zenravif derives the signaled matrix from `color_model`) — it is
  retained, documented as informational, and excluded from the sweep
  fingerprint on all backends.

### Added (pattern 7: cell ids as durable identity)
- `sweep::config_from_cell_id(base_id, q)` — reconstructs the exact
  `EncoderConfig` from a sweep cell id (the ledger contract: ids stored in
  TSV/parquet identity columns are regenerable years later). Parses through
  the same stratum builder the planner uses; grammar documented on the
  parser, additive-only, lossless numbers. `SweepAxes::by_name` resolves the
  named plans for executor wiring. The grammar-totality test
  (`cell_ids_roundtrip_to_their_configs`, fingerprint-exact over canonical +
  alias spellings of the full `modes_full_alpha × Step5` plan) caught a real
  tokenizer bug on its first run (`part4.16`'s separator dot eaten by the
  float scanner). The zenmetrics executor wiring (checklist step 8)
  landed the same day (zenmetrics 96a31b90): both execution models,
  e2e declare→jobexec→AVIF-bytes roundtrip + tampered-fp tripwire.

### Added (expert-knob parity + MLP training bridge)
- `sweep::feature_columns()` + `SweepCell::feature_row(PlanInput)` — numeric
  knob vectors for picker/MLP training (zentrain): one column per knob,
  resolved values where a mediator exists (quantizer, post-override search
  settings), bool 0/1 / small-int enum / −1-sentinel encodings, append-only
  columns.
- Alpha-plane sweep probes: `KnobProbe::AlphaQualityDelta` (a **delta against
  the grid q**, ±25 curated — deltas dodge the absolute-value-vs-moving-grid
  trap zenjpeg documents for `chroma_quality`) and `KnobProbe::AlphaMode`
  (Dirty / Premultiplied), in the new `SweepAxes::modes_full_alpha()` /
  `SweepAxes::alpha_probes()` presets for RGBA corpora. Kept out of
  `modes_full` (byte-inert without an alpha plane); the validation harness
  gained an RGBA leg proving each probe live on alpha content and
  non-coupling on color-only encodes (`benchmarks/sweep_validate_2026-06-11.tsv`).
- `EncodePlan.alpha_color_mode` — the alpha handling mode is pixel-changing
  resolved state and is now reported by the plan. `KnobProbe::apply` is
  public so harnesses can exercise single probes outside a full plan.

### Fixed
- **zencodec adapter now honors `OrientationHint` (`irot`/`imir`).** The
  `AvifDecodeJob` decode paths ignored the orientation hint, so `Correct` was
  silently treated as `Preserve` — pixels stayed in stored orientation. The
  adapter now bakes the container's intrinsic rotation/mirror into the decoded
  pixels on the bake path (`Correct`/`CorrectAndTransform`) via
  `zenpixels_convert::orient::apply_orientation`, reporting display dims +
  `Orientation::Identity`; `Preserve` (the default) is unchanged (stored dims +
  intrinsic tag). Covers the single-image, row-sink, streaming (grid + non-grid,
  baked after tile stitching), and animation paths. Adds
  `AvifDecoderConfig::with_orientation` + the `DecodeJob::with_orientation`
  override; the native (non-zencodec) API is unchanged. Requires
  zenpixels-convert 0.2.13 (the `orient` module). Pinned by
  `tests/cov_zencodec.rs::orientation_*`.
- **`alpha_quality` unset now follows the color quality, as its docs always
  promised.** zenravif's built-in default pins the alpha quantizer to the
  quality-80 equivalent and `with_quality` never touches it, so an
  alpha-bearing encode at `quality(30)` was silently encoding alpha at q80;
  zenavif now forwards `alpha_quality.unwrap_or(quality)` explicitly. Output
  bytes change for alpha-bearing encodes whose color quality ≠ 80. Pinned both
  directions by `tests/encode_contracts.rs::alpha_quality_unset_follows_color_quality`.

### Added
- **Variant-generation infrastructure** (the zenjpeg patterns, adopted per
  `docs/VARIANT_GENERATION.md`):
  - `EncoderConfig::resolve_plan(PlanInput) -> EncodePlan` — full static
    resolution of what an encode will do: quantizers (color + alpha), the
    qm×lossless gate, every speed-preset-derived search setting after
    overrides, chroma subsampling, resolved bit depth / color model / CICP
    matrix, and the tile count — including `TilesResolution::MachineDependent`
    when `threads` is unset, because zenravif substitutes the *host core
    count* into the tile formula and tile structure changes the bitstream
    (default-config encodes are not byte-reproducible across machines).
  - `EncoderConfig::validate()` now rejects `Av1Backend::Svtav1` when the
    backend isn't compiled in (previously a silent fallback to zenravif) and
    `Yuv420 × Rgb`; new `validate_for_input()` additionally rejects
    `Yuv420 × 16-bit input` (the 16-bit entry points force identity-RGB).
  - `EncoderConfig::chroma_subsampling(EncodeChromaSubsampling)` — 4:4:4
    (default, unchanged) vs 4:2:0. The biggest AVIF rate knob was previously
    hardwired to 4:4:4.
  - `zenavif::sweep` (behind `__expert`): `SweepAxes` (`rd_core` /
    `modes_full` + the `KnobProbe` single-deviation axis), `QualityGrid`,
    `SweepBuilder` with a no-silent-caps budget ladder, validity filtering,
    main-effects-first queue ordering, and a byte-identity `fingerprint`
    over RESOLVED state (quality mediated by quantizer; override==preset
    aliases merged; `vaq_strength` excluded when VAQ is off;
    `matrix_coefficients` excluded on the zenravif backend — every exclusion
    encode-proven). Sweep cells pin `threads(Some(1))` for cross-machine
    reproducibility.
  - `examples/sweep_validate.rs` — empirical axis validation (inert-step
    detection, fingerprint contracts on real encodes, tiles/threads claims,
    ssim2 sanity floor); results committed as
    `benchmarks/sweep_validate_2026-06-10.tsv`. Its first runs caught two
    real defects, both root-caused structurally and fixed: (1)
    `with_vaq(true, 1.0)` is byte-identical to VAQ off — the
    psychovisual/still tunes always compute the activity mask and zenrav1e
    skips the strength rescale at 1.0 (`api/internal.rs:1379`); the sweep
    vaq axis is `Option<f64>` now and the fingerprint hashes the active
    form. (2) `lru_on_skip` is byte-inert on still-image encodes at speeds
    2–8 (28/28 comparisons incl. skip-heavy flat content) — removed from
    the curated probe set with the evidence in the provenance table.
  - `tests/encode_contracts.rs` — encode-level pins for the alpha contract,
    quantizer mediation (q 80.0 ≡ q 80.2 ≠ q 81.0 at the byte level),
    subsampling liveness, and the new validate() rejections.
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
