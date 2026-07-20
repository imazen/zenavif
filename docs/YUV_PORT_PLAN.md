# YUV conversion: port the remaining `yuv`-crate surface in-house, unify the kernels

Status: EXECUTED through P6 (2026-07-20). P1-P6 shipped; P7 (dep removal)
is blocked only on the queued `Error::ColorConversion` breaking change and
the legacy `unsafe-asm` decoder swap. Remaining perf item: P8 — an i16
fixed-point kernel proven equal to the canonical recipe (the f32 kernels
measure ~250-330 Mpx/s vs the yuv crate's fixed-point ~1435 Mpx/s at
10-bit; asm-verified the f32 loops are already full-width zmm).
Companion facts: the yuv-crate bottom-row bug record (`src/yuv_bilinear_fix.rs`,
upstream awxkee/yuvutils-rs#129/#130) is what motivated auditing this seam.

## 1. Where conversions run today

**In-house (~4,350 lines, archmage/magetypes SIMD):**

| family | file | arch tiers | serves |
|---|---|---|---|
| full-image RGB8 (420/422/444, bilinear) | `yuv_convert.rs` | v3/neon/wasm128/scalar (magetypes `GenericF32x8`) | primary no-alpha 8-bit decode **and (P1) the 8-bit alpha decode** |
| strip RGB8/RGBA8 (420/422/444) | `yuv_convert.rs` | Desktop64 + wasm128 + scalar (`StripPixel`) | zenpipe strip pipeline |
| libyuv-exact integer ports | `yuv_convert_libyuv{,_simd,_autovec}.rs` | AVX2 / autovec | `_dev` profiling alternates |
| fixed-point fast path | `yuv_convert_fast.rs` | x86_64/aarch64 | `_dev` |

**Still on the external `yuv` crate (default build):**

1. 8-bit RGBA *exotic-matrix fallback only* (SMPTE-240M, FCC, derived KR/KB) — post-P1.
2. 8-bit RGB exotic-matrix fallback (same matrices).
3. **All high-bit-depth decode** — `i010/i012/i016` (+420 bilinear), `i210/i212/i216`, `i410/i412/i416` × rgb/rgba: ~20 functions. The largest surface.
4. **All monochrome decode** — `yuv400_to_rgb(a)`, `y010/y012/y016_to_*`: 12 functions.
5. Encode-side `rgb_to_yuv420` / `rgba_to_yuv420` (SvtRs backend).
6. The legacy `unsafe-asm` decoder duplicates 1–4.

## 2. The structural problem (why "port" must mean "unify")

Four independent implementations of the same math exist (full-image f32
SIMD, strip f32 SIMD, libyuv-int ports, external crate). Two consequences,
both observed this week:

- **Correctness surface multiplies.** The yuv-crate dropped-last-row-pair
  bug had to be fenced at 12 call sites; a kernel bug of ours would need
  the same hunt in reverse.
- **Byte-identity between independent float kernels is empirical, not
  structural.** The strip-vs-full probe caught a 1-ULP divergence (1 px in
  ~331k) from a split mul+add vs chained FMA. Op-order is now aligned at
  that site and near-parity (≤1) is pinned by
  `strip_rgba_kernels_near_parity_with_full_rgb_kernels`, but only a
  single shared kernel makes identity a non-theorem.

Target: **one generic kernel family** in `yuv_convert.rs`, everything else
a thin wrapper or deleted.

## 3. Target kernel architecture

```
yuv_to_rgbx_strip<P: StripPixel, const DEPTH: …>(
    y/u/v planes + strides,      // strided-rows mandate: stride-native,
    width, total_height,          // tight path = the fast path, never a
    y_start, strip_height,        // rejection
    range, matrix_kr_kb,          // (kr, kb) floats — ANY matrix, incl.
    out,                          // SMPTE-240/FCC/derived; kills the
)                                 // exotic-matrix fallback branch entirely
```

- **Strip-first**: the full-image functions become `strip(…, 0, h)` calls.
  One implementation, two APIs.
- **Output via `StripPixel`** (exists): RGB8/RGBA8 now; add RGB16/RGBA16
  impls for high-bit-depth. RGBA becomes a first-class kernel output again
  — deleting P1's interim widen pass (an extra alloc + sweep).
- **Sample depth**: generic over `u8`/`u16` with depth-parameterized
  normalization (`/(2^d − 1)`, limited-range offsets `16·2^(d−8)`,
  `219/224·2^(d−8)`). The f32 math is depth-independent; only load/widen
  lanes differ.
- **Matrix as (kr, kb)**: `cicp_resolve::resolve` already derives kr/kb
  for every matrix it accepts; passing floats instead of an enum removes
  the `to_our() → None` fallback class. BT.601/709/2020 keep their exact
  current constants (byte-stability gate below).
- **Dispatch**: one `#[magetypes(_v4x, v4, v3, neon, wasm128)]` set (per
  the house SIMD default), replacing the current split (magetypes
  full-image vs Desktop64 strip). The `_v4x` tier is new coverage —
  expected to matter most for 16-bit.
- **Chroma sampler**: one shared bilinear sampler for 420 (H+V), 422
  (H-only), 444 (copy) — the op-order divergence class dies here.

## 4. Phases

**P1 — DONE (2026-07-19/20).** 8-bit RGBA decode onto the full-image
kernels (+ widen); FMA op-order alignment; near-parity pin;
`rgb_and_rgba_decodes_of_same_color_payload_agree_exactly` invariant test.

**P2 — kernel unification (the refactor).** Merge full-image and strip
into the §3 shape for 8-bit; delete the widen pass; move zenpipe's strip
callers over. Gates: existing float-reference byte-identity tests must
pass unchanged for BT.601/709/2020 (byte-stability of shipped output);
strip == full becomes `assert_eq!` (structural now); zenbench A/B on
tiny/1080p/2160p per the sweep discipline, no regression beyond noise.
Estimate: 1 session. Risk: low — both sources live in one file already.

**P3 — high-bit-depth decode (10/12/16 → RGB16/RGBA16).** `u16` sample
impls + depth-parameterized normalization; port the managed decoder's
16-bit dispatch (`convert_16bit_planar_*`). Replaces ~20 yuv-crate fns.
Validation: float-reference grids mirroring the 8-bit tests ×
{10,12,16}-bit; differential vs the yuv crate on random planes
(document any ±1 rounding deltas — ours follows the reference); the b10
conformance cells through the real decoder. Estimate: 1–2 sessions. This
is the phase that pays: it retires the biggest dep surface *and* puts
16-bit on AVX-512.

**P4 — monochrome.** No-chroma kernel (normalize + replicate), 8/10/12/16
× RGB/RGBA. Trivial after P3's depth generics. Estimate: half a session
with tests.

**P5 — exotic matrices.** Thread (kr, kb) from `cicp_resolve` through the
unified kernel; delete both `*_yuvcrate` fallbacks and the RGBA fallback.
Validation: SMPTE-240/FCC vectors differential vs yuv crate + reference.
Small.

**P6 — encode-side RGB(A)→YUV420.** Forward matrix + 2×2 chroma
downsample for the SvtRs backend. Decision to make consciously: the
downsample filter (yuv crate "Balanced" = box average; keep box for
parity, note it). Validation: SvtRs roundtrip PSNRs must not regress;
differential vs yuv crate within ±1. Estimate: half a session.

**P7 — retire.** Legacy `unsafe-asm` decoder swaps to the same kernels
(or is deleted outright — it duplicates `decoder_managed` and survives
only for the FFI bench arm; decide then). Drop the `yuv` dep and
`yuv_bilinear_fix.rs` (its canary test already signals when upstream
ships #129, making the wrapper dead code anyway).

Order: P2 before P3 — generalizing two kernel families into u16 doubles
the work; unify first, then widen once.

## 5. Perf discipline

- zenbench interleaved A/B per phase (never criterion), sizes
  tiny/256/1080p/2160p, no `-C target-cpu=native`, committed to
  `benchmarks/` with `.meta`.
- Baselines to beat or match: current in-house 8-bit kernels; the yuv
  crate's AVX2 paths for 16-bit (it is well-optimized — P3 must measure,
  not assume; the `_v4x` tier is the expected edge).
- The P1 widen pass is a known temporary cost (one extra pass + alloc on
  8-bit alpha decodes); P2 deletes it — don't micro-optimize it before
  then.

## 6. What stays true throughout

- Every multi-row entry point takes strides natively (house rule); the
  tight path is a fast path, not a requirement.
- Reference implementations stay in the test module and stay authoritative:
  kernels are byte-checked against them per (depth, sampling, range,
  matrix) grid — that's what made the strip divergence and the upstream
  crate bug findable at all.
- Decode-level invariants guard the seams the unit grids can't see:
  RGB-vs-RGBA payload agreement (exists), and a 16-bit twin once P3 lands.
