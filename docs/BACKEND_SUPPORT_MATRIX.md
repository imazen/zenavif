# Backend support matrix

What each AV1 backend supports through zenavif's seams, as of 2026-07-23
(branch `svtav1-rs-backend`; pins: zenravif 0.2.0 path dep, zenav1-svt
`3e25f52b`, rav1d-safe `f9458f43`, zenav1-aom `7b972e50`, rav1d 1.1.0).
"Rejected" always means an honest structured error at validate/encode time,
never silent degradation. Sources: `src/encoder.rs`, `src/encoder_svt_rs.rs`,
`src/decode_av1.rs`, the per-backend audits, and
`benchmarks/backend_sweep_2026-07-22.*`.

## Encode backends (`EncoderConfig::backend`)

| Axis | Zenravif (default, `encode`) | SvtRs (`encode-svt-rs`, experimental) |
|---|---|---|
| Input: RGB8 | yes | yes |
| Input: RGBA8 (alpha item) | yes | yes (alpha as Cs400 `auxl` item, `alpha_quality` fallback honored) |
| Input: Gray8 → Cs400 mono | yes (`encode-mono` gate) | yes (`encode-mono` gate) |
| Input: RGB16/RGBA16 → 10-bit AV1 | yes (`encode_rgb16`/`encode_rgba16`) | rejected (8-bit only) |
| Bit depth 8 | yes | yes |
| Bit depth 10 | yes (`EncodeBitDepth::Ten`/`Auto`) | rejected (library has a narrow bd10 envelope; seam pins 8) |
| Bit depth 12 | no (no enum variant; AV1 profile 2) | no (`unimplemented!()` upstream) |
| Subsampling 4:4:4 | yes (default) | rejected |
| Subsampling 4:2:0 | yes | yes (the only mode) |
| Subsampling 4:2:2 | no (no enum variant) | no |
| Color model YCbCr (BT.601) | yes | yes (BT.601 full-range pinned) |
| Color model RGB (identity, MC=0) | yes (4:4:4 only) | rejected |
| Pixel range Full | yes (default) | yes (pinned) |
| Pixel range Limited | yes | rejected (SH writer pins color_range=1) |
| CICP primaries/transfer signaling | yes (config → SH + container `colr`) | yes (SH + container; matrix pinned BT.601) |
| Dimensions | arbitrary | multiples of 64 at any speed; arbitrary at speed ≥ 5 (SVT preset ≥ 6, partial SBs on the 4:2:0 path); with alpha or grayscale (Cs400 stream): multiples of 8 at speed ≥ 6 only — the port's mono path mis-codes partial SBs at preset 6 and pads no partial 8x8 block (#32; `svt_rs_dims_error`) |
| Quality dial semantics | fitted quality→qindex curve (0–255; `encode_plan.rs`) | linear quality→QP (63..1; **QP 0 clamped out** — corrupts upstream, zenav1-svt#5) |
| Speed dial | 1–10 → zenravif speed ladder | 1–10 → SVT preset 0–13 linear (wall-time NOT aligned with zenravif's ladder: ~6× faster at s6, slower at s2) |
| Lossless | yes (`encode-imazen`, release-gated exactness) | rejected (QP 0 ≠ lossless, and corrupt anyway) |
| Gain map / Ultra HDR (`with_gain_map`) | yes | rejected |
| HDR metadata boxes (CLL/mastering) | yes | yes (container-level) |
| EXIF/XMP/ICC, rotation/mirror | yes | yes (container-level, muxed in-crate) |
| Animation (`encode_animation_*`) | yes | rejected (stills only) |
| Target quality (`encode_*_with_target`, ssim2/zensim) | yes (RGB8/RGBA8/RGB16; q0 head seeds ssim2) | yes for RGB8/RGBA8 (search is backend-generic; anchor-curve seeding); RGB16 rejected |
| Two-pass butteraugli | yes (release-gated `FRAME_HINTS_LIVE`) | no |
| Auto-tune picker | yes (`auto-tune`) | no (calibrated for zenravif knobs) |
| Stop token | yes (per-superblock via zenravif) | yes (threaded into pipeline, SB cadence) |
| Structured errors at the seam | yes | yes (`try_encode_frame*` → variant-mapped; no `is_empty()` heuristic) |
| C-parity claim | n/a (rav1e lineage) | byte-identical to C-SVT on the verified envelope upstream; NOT asserted at the zenavif seam yet |
| Screen content (palette/intraBC) | palette + intraBC via zenravif gates (release-gated arms) | palette ported upstream (RD over-picks, decodable); intraBC unwired; nothing exposed at the seam |

## Decode backends (`decode_av1_obu_yuv(.., DecodeBackend::..)`)

The public container decode (`zenavif::decode*`) uses Rav1dSafe by default;
`DecoderConfig::decode_backend(AomRs)` routes NON-GRID decodes — stills
(primary/alpha/gain-map items, all depths/subsamplings, CICP/ICC/HDR
passthrough) animations (eager whole-track `decode_frames`; the
animated-AVIF inter envelope is byte-exact vs aomdec upstream), AND grid
AVIFs (per-cell aom decode + the shared byte-stitch) — through aom-rs
with byte-identical output (`tests/product_aom_backend.rs`). Only
row-sink streaming on aom returns honest Unsupported.
`Rav1dFfi` remains raw-OBU benchmark only.

| Axis | Rav1dSafe (default) | AomRs (`aom-backend`) | Rav1dFfi (`unsafe-asm`) |
|---|---|---|---|
| Safety | 100% safe Rust | 100% safe Rust | C FFI + hand-written asm |
| Frame types | full AV1 (KEY/INTER/INTRA_ONLY/SWITCH, show_existing) | KEY + the animated-AVIF inter envelope (zero-MV NEARESTMV/DC, 8-slot DPB, show_existing, CDF inheritance, temporal MVs; compound/sub-pel MC fail loud) | full AV1 |
| Bit depths | 8/10/12 | 8/10/12 | 8/10/12 |
| Monochrome (Cs400) | yes | yes | yes |
| 4:2:0 / 4:2:2 / 4:4:4 | yes | yes | yes |
| Film grain | yes | yes (byte-identical to C) | yes |
| Palette / intraBC decode | yes | yes (incl. color intraBC) | yes |
| Superres | yes | single-tile-column only | yes |
| Coded lossless (WHT) | yes | yes (mixed-segment streams rejected) | yes |
| Quant matrices | yes | yes | yes |
| Loop filter / CDEF / LR | yes | yes | yes |
| Tiling | yes (multi tile-group) | multi-tile, but ONE tile-group OBU per frame | yes |
| Threading | tile-parallel (frame delay pinned to 1) | single-threaded | full dav1d threading |
| Annex-B / IVF framing | OBU TU (zenavif demuxes) | OBU TU only (TD tolerated/prepended at the seam) | OBU TU |
| Limits / stop / alloc-mode | `DecoderConfig` caps; fallible alloc default | threaded via `decode_av1_obu_yuv_with` (limits + in-loop stop + `AllocMode`) | none (legacy; stop polled pre-call only) |
| Structured error mapping | yes (two-level `ErrorCategory`) | yes (variant-mapped at seam) | coarse |
| Fuzz posture | fuzzed upstream (open recurred crashes 2026-07) | cargo-fuzz harness + 1<<28 px DoS bound upstream; NOT default-path trusted | not fuzz-safe (asm) |
| Relative speed (this repo's cells) | 1× baseline | ~1.4× slower 8-bit, ~1.13× 10-bit | ~0.45–0.55× (i.e. ~2× faster) |
| Byte-identity | reference | identical on all 7018 sweep cells + conformance corpus | identical where present |

Cross-backend invariants pinned by tests: `tests/cross_backend_decode.rs`
(both decoders byte-agree on both encoders' output, incl. 10-bit),
`tests/svt_rs_backend.rs` (scope rejections + ssim2 target convergence +
QP-0 non-corruption).
