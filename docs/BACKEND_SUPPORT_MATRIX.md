# Backend support matrix

What each AV1 backend supports through zenavif's seams, as of 2026-07-23 for
the first two encode columns, **2026-09-02** for the Zenav1Aom encode
column's first landing, and **2026-09-03** for its bit-depth rows (10- and
12-bit 4:2:0 wired; `EncodeBitDepth::Twelve` added in the 0.2.0 break)
(pins: zenravif 0.2.0 path dep, zenav1-svt
`3e25f52b`, rav1d-safe `f9458f43`, zenav1-aom decode `7b972e50`, zenav1-aom
encode `45c53ddb` — bumped 2026-09-04 from `c3e1b4ab` for zenavif#45, which
re-verified the quality-dial, lossless, panic-freedom and C-parity rows —
rav1d 1.1.0). Where a row's Zenav1Svt cell cites an older
pin than `Cargo.toml`, that cell is as-of its own date, not re-verified.
"Rejected" always means an honest structured error at validate/encode time,
never silent degradation. Sources: `src/encoder.rs`, `src/encoder_svt_rs.rs`,
`src/encoder_aom.rs`, `src/decode_av1.rs`, the per-backend audits, and
`benchmarks/backend_sweep_2026-07-22.*`.

## Encode backends (`EncoderConfig::backend`)

| Axis | Zenravif (default, `encode`) | Zenav1Svt (`zenav1-svt`, experimental) | Zenav1Aom (`zenav1-aom-encode`, experimental) |
|---|---|---|---|
| Input: RGB8 | yes | yes | yes |
| Input: RGBA8 (alpha item) | yes | yes (alpha as Cs400 `auxl` item, `alpha_quality` fallback honored) | rejected at **both** `validate_for_input` and encode (zenavif#44, fixed 2026-09-03 — the validate half was missing, so an alpha config validated and then failed at `encode_rgba8`) — the Cs400 mono encode an `auxl` alpha item needs exists here, the item itself is not wired. Unchanged by the bit-depth work: alpha is refused at 8, 10 and 12 |
| Input: Gray8 → Cs400 mono | yes (`encode-mono` gate) | yes (`encode-mono` gate) | yes (`encode-mono` gate) |
| Input: RGB16/RGBA16 → 10-bit AV1 | yes (`encode_rgb16`/`encode_rgba16`, identity-RGB 4:4:4) | yes (#33; YCbCr 4:2:0 at 10-bit precision via the port's native u16 `try_encode_frame_420_hbd`; RGBA16 needs speed ≥ 7 for its 10-bit Cs400 alpha item) | **RGB16 yes** (2026-09-03; YCbCr 4:2:0 at 8/10/12 bits via `rgbx_to_yuv420_u16`, NOT identity-GBR — same shape as Zenav1Svt, and `validate_for_input` allows `16-bit + Yuv420` for these two backends only). RGBA16 rejected — the `auxl` alpha item is not built |
| Bit depth 8 | yes | yes | yes |
| Bit depth 10 | yes (`EncodeBitDepth::Ten`/`Auto`) | yes (#33; `Ten` on 8-bit input converts at 10-bit precision; 10-bit alpha/gray streams need speed ≥ 7 = SVT preset ≥ 9, the port's only bd10 mono level producer; post-filter searches upstream still decide on MSB-truncated planes — hbd chunk 2) | **yes** (2026-09-03; colour 4:2:0 only — the Cs400 grayscale path is 8-bit only and refuses by name. Gate: coded luma 49.58–49.69 dB vs a longhand H.273 expectation under rav1d-safe, bound 40; flat content exact) |
| Bit depth 12 | **no — refused by name** (`ravif::BitDepth` has no 12-bit representation, so `EncodeBitDepth::Twelve` is rejected rather than silently coded at 10; `encoder::reject_unspellable_coded_depth`) | **no — refused by name** (`svt_rs_depth_error`; the port has no 12-bit encode, matching C SVT-AV1 v4.2.0's own 8/10 check) | **yes** (2026-09-03; `EncodeBitDepth::Twelve`, AV1 profile 2, colour 4:2:0 only. Gate: coded luma 50.24–50.57 dB, bound 40; flat content within 1 code value of 4095 — `--cq-level 1` is not `base_qindex 0`; `av1C` `seq_profile`/`high_bitdepth`/`twelve_bit` read back and asserted) |
| Subsampling 4:4:4 | yes (default) | rejected | rejected at the seam (no forward RGB→YUV 4:4:4 kernel in `src/yuv_convert.rs`); byte-gated upstream. This is a CHROMA-FORMAT gap, independent of bit depth — conflating the two is what kept 10/12-bit refused until 2026-09-03 |
| Subsampling 4:2:0 | yes | yes (the only mode) | yes (the only mode) |
| Subsampling 4:2:2 | no (no enum variant) | no | rejected at the seam (no `EncodeChromaSubsampling` variant); byte-gated upstream |
| Color model YCbCr (BT.601) | yes | yes (BT.601 full-range pinned) | yes (BT.601 **limited**-range — the SH pins `color_range=0`) |
| Color model RGB (identity, MC=0) | yes (4:4:4 only) | rejected | rejected |
| Pixel range Full | yes (default) | yes (pinned) | rejected (SH writer pins `color_range=0`) |
| Pixel range Limited | yes | rejected (SH writer pins color_range=1) | yes (pinned; the mirror image of Zenav1Svt) |
| CICP primaries/transfer signaling | yes (config → SH + container `colr`) | yes (SH + container; matrix pinned BT.601) | container `colr` only — the SH pins CICP 2/2/2 (unspecified); `zenavif::decode` reads range+matrix from the **SH**, not `colr` (measured 2026-09-02), and the SH's unspecified matrix falls back to BT.601, which is what this seam converts with |
| Dimensions | arbitrary | 4:2:0 colour path: **arbitrary at any speed** (the preset ≥ 6 floor was removed 2026-08-29 — upstream `partial_sb_gate` gained a byte-identical presets-0–5 block and the residual it still names is a `screen`-content RD class that also fires on 64-aligned frames); with alpha or grayscale (Cs400 stream): multiples of 64 below speed 5, else multiples of 8 — the port's mono path pads no partial 8x8 block and nothing upstream measures mono partial SBs below SVT preset 6 (#32; `svt_rs_dims_error`; the preset-6 mono mis-coding was fixed in zenav1-svt `b6a1737a` + `1ed7db46`) | arbitrary (upstream byte-gates 16×16–512×512 plus 20 crops incl. 1×1; the seam refuses only zero) |
| Quality dial semantics | fitted quality→qindex curve (0–255; `encode_plan.rs`) | linear quality→QP (63..1; **QP 0 clamped out** — a product choice since zenav1-svt#5/#9 implemented coded-lossless: quality 100 must encode, not switch coding mode, and RGB→4:2:0 means QP 0 is not a lossless *image* round-trip anyway) | linear quality 1..100 → `--cq-level` 63..0, **no clamp** (cq 0 and cq 63 are both byte-gated upstream). **Quality 100 = cq 0 is coded-lossless and reconstructs the coded 4:2:0 planes exactly at 8, 10 and 12 bits** (zero-tolerance gate `aom_cq0_encodes_and_reconstructs_the_coded_planes_exactly`, 6 cells × rav1d-safe + aom-decode). Until 2026-09-04 that cell panicked in the port (`debug_assert!(depth <= MAX_VARTX_DEPTH)`, zenavif#45); fixed at the root in zenav1-aom `21544fde`, pin moved to `45c53ddb`, and the canary `aom_cq0_still_panics_on_flat_content` was retired into the gate |
| Speed dial | 1–10 → zenravif speed ladder | 1–10 → SVT preset 0–13 linear (wall-time NOT aligned with zenravif's ladder: ~6× faster at s6, slower at s2) | 1–10 → `--cpu-used` 0–9, one-to-one (whole range byte-gated). Wall time MEASURED 2026-09-02 (`benchmarks/aom_backend_2026-09-02.*`): 2.5–3.2× faster than zenravif at speed 1 and 3.9–8.0× faster at speed 9, but 2.0–3.2× **slower** at speed 5 — the ladders are misaligned, same as the svt seam |
| Lossless | yes (`encode-imazen`, release-gated exactness) | not exposed — upstream implements coded-lossless (QP 0) on the 8-bit 4:2:0 key-frame path (zenav1-svt#5/#9, gated by `svt_rs_direct_qp0_codes_lossless_420` here), but this backend's quality dial deliberately clamps to QP ≥ 1 and `EncoderConfig` has no lossless request to route to it (open decision: zenavif#42); still typed-refused upstream on mono, 10-bit, HDR-fork, screen-content tools, superres and inter frames | not exposed as `EncoderConfig::lossless` (refused by name) — but **quality 100 = cq 0 IS coded-lossless at 8/10/12 bits** and is gated exact on the coded planes (`aom_cq0_encodes_and_reconstructs_the_coded_planes_exactly`); still not a lossless *image* round trip, because the RGB → studio-range 4:2:0 conversion in front of it is lossy |
| Gain map / Ultra HDR (`with_gain_map`) | yes | rejected | rejected |
| HDR metadata boxes (CLL/mastering) | yes | yes (container-level) | yes (container-level) |
| EXIF/XMP/ICC, rotation/mirror | yes | yes (container-level, muxed in-crate) | yes (container-level, muxed in-crate) |
| Animation (`encode_animation_*`) | yes | rejected (stills only) | rejected — `encode_key_frame` is a ONE-KEY-FRAME entry point; there is no inter path to wire |
| Target quality (`encode_*_with_target`, ssim2/zensim) | yes (RGB8/RGBA8/RGB16; q0 head seeds ssim2) | yes for RGB8/RGBA8 (search is backend-generic; anchor-curve seeding; convergence pinned by `svt_rs_target_quality_search_converges_on_ssim2`); RGB16 reaches the backend since #33 but its convergence is not pinned by a test | untested at this seam (the search is backend-generic and reaches it; no convergence gate) |
| Two-pass butteraugli | yes (release-gated `FRAME_HINTS_LIVE`) | no | no |
| Auto-tune picker | yes (`auto-tune`) | no (calibrated for zenravif knobs) | no (calibrated for zenravif knobs) |
| Stop token | yes (per-superblock via zenravif) | yes (threaded into pipeline, SB cadence) | checked at the seam's phase boundaries only — `encode_key_frame` takes no token, so one frame is not interruptible once entered |
| Panic-freedom at the seam | n/a | n/a | yes on every gated cell since the `45c53ddb` pin (2026-09-04) — the `--cq-level 0` panic (zenavif#45) was the one known hole and is fixed upstream (`21544fde`); no `catch_unwind` at the seam |
| Structured errors at the seam | yes | yes (`try_encode_frame*` → variant-mapped; no `is_empty()` heuristic) | yes (`KeyFrameError` → `Error::Encode`; never a silent fallback) |
| C-parity claim | n/a (rav1e lineage) | byte-identical to C-SVT on the verified envelope upstream; NOT asserted at the zenavif seam yet | **427/427 cells byte-identical to real aomenc** upstream at `45c53ddb` (mono/4:2:0/4:2:2/4:4:4, bd 8/10/12, 16×16–512×512, 20 crops, all four CDEF×LR combos, `--cpu-used` 0..=9, multi-tile, SB128, explicit tiles; 189 of the cells are cq 0, plus `coded_lossless_reconstructs_the_source_exactly` 248/248 on libaom's decoder and aom-decode); NOT asserted at the zenavif seam, which gates decodability — and, at cq 0, exact reconstruction — instead. Open upstream: bd {10, 12} × `--cpu-used` 1..6 streams differ from libaom's bytes (the pre-existing `HBD_OPEN` band, pinned there), while still decoding losslessly at cq 0 |
| Screen content (palette/intraBC) | palette + intraBC via zenravif gates (release-gated arms) | palette ported upstream (RD over-picks, decodable); intraBC unwired; nothing exposed at the seam | detector ported (`screen_detect`); palette + IntraBC search are off in the gated envelope |

## Decode backends (`decode_av1_obu_yuv(.., DecodeBackend::..)`)

The public container decode (`zenavif::decode*`) uses Rav1dSafe by default;
`DecoderConfig::decode_backend(Zenav1Aom)` routes NON-GRID decodes — stills
(primary/alpha/gain-map items, all depths/subsamplings, CICP/ICC/HDR
passthrough) animations (eager whole-track `decode_frames`; the
animated-AVIF inter envelope is byte-exact vs aomdec upstream), AND grid
AVIFs (per-cell aom decode + the shared byte-stitch) — through zenav1-aom
with byte-identical output (`tests/product_aom_backend.rs`). Only
row-sink streaming on aom returns honest Unsupported.
`Rav1dFfi` remains raw-OBU benchmark only.

| Axis | Rav1dSafe (default) | Zenav1Aom (`zenav1-aom`) | Rav1dFfi (`unsafe-asm`) |
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
QP-0 coded-lossless recon-equals-source, plus the typed refusals that survive
outside its 8-bit 4:2:0 envelope), `tests/aom_encode_backend.rs`
(`aom_cq0_encodes_and_reconstructs_the_coded_planes_exactly`: cq-0
recon-equals-input on every coded plane at 8/10/12 bits, both decoders, with
its own q99 mutation proof `cq0_gate_can_fail_on_a_lossy_encode`).
