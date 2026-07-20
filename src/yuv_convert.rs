//! YUV to RGB color space conversion
//!
//! Implements standard color space conversions for AVIF/AV1 images as ONE
//! strip-first kernel per chroma subsampling, generic over the output
//! pixel (RGB8 | RGBA8). The kernels are plain auto-vectorized loops
//! inside `#[magetypes]` tier regions (AVX-512, AVX2/FMA, NEON, wasm
//! SIMD, scalar): the canonical recipe (see below) is elementwise IEEE
//! f32, so LLVM vectorizes it at full register width and every tier,
//! lane width, and window produces byte-identical output.
//!
//! References:
//! - ITU-R BT.601 (SD video)
//! - ITU-R BT.709 (HD video)
//! - ITU-R BT.2020 (UHD video)

// YUV conversion functions naturally require plane/stride/dimension/matrix/range parameters.
#![allow(clippy::too_many_arguments)]
#![allow(dead_code)]

use archmage::prelude::*;
use imgref::ImgVec;
use rgb::{RGB8, Rgba};

/// Floor of a value that is already known to be **non-negative and finite**,
/// returned as the integer pixel index plus the f32 fractional remainder.
///
/// For any non-negative finite `x`, `x.floor()` equals `x` truncated toward
/// zero, which is exactly what `x as usize` produces. Replacing the libm
/// `floorf` call (an out-of-line call on aarch64 that does not inline into the
/// NEON dispatch region) with a direct truncating cast is **bit-for-bit
/// identical** here: every caller clamps the input to `[0.0, plane_dim - 1.0]`
/// before calling, so `x >= 0.0` always holds. The returned `(idx, frac)` pair
/// reproduces the original `(cx0, chroma_x - cx0)` exactly.
#[inline(always)]
fn floor_nonneg_idx(x: f32) -> (usize, f32) {
    debug_assert!(x >= 0.0 && x.is_finite());
    let idx = x as usize;
    (idx, x - idx as f32)
}

/// YUV color range
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvRange {
    /// Limited/studio range: Y [16..235], UV [16..240] for 8-bit
    Limited,
    /// Full range: Y [0..255], UV [0..255] for 8-bit
    Full,
}

/// YUV matrix coefficients (color space)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum YuvMatrix {
    /// ITU-R BT.601 (SD video, NTSC/PAL)
    Bt601,
    /// ITU-R BT.709 (HD video)
    Bt709,
    /// ITU-R BT.2020 (UHD video, HDR)
    Bt2020,
    /// Explicit (Kr, Kb) — FCC (0.30, 0.11), SMPTE 240M (0.212, 0.087),
    /// and chromaticity-derived matrices (H.273 MC=12/13). The canonical
    /// recipe needs nothing else from a matrix.
    Custom {
        /// Red luminance weight.
        kr: f32,
        /// Blue luminance weight.
        kb: f32,
    },
}

/// Chroma subsampling format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaSubsampling {
    /// 4:4:4 - no subsampling
    Cs444,
    /// 4:2:2 - horizontal subsampling
    Cs422,
    /// 4:2:0 - horizontal and vertical subsampling
    Cs420,
}

// ═══════════════════════ One canonical numeric recipe ══════════════════════
//
// Every conversion in this module — SIMD lanes, scalar remainders, the
// scalar fallback tier, and the test references — computes the SAME f32
// operation sequence:
//
//   y_n = (Y - y_off) * y_scale            (reciprocal multiply, not divide)
//   u_n = (U - 128)   * uv_scale
//   v_n = (V - 128)   * uv_scale
//   R = fma(v_n, Vr, y_n)
//   G = fma(v_n, Vg, fma(u_n, Ug, y_n))    (chained fused multiply-adds)
//   B = fma(u_n, Ub, y_n)
//   out = round(clamp(C * 255))            (max/min, round TIES-TO-EVEN)
//
// IEEE f32 ops are elementwise, and `f32::mul_add`/vector `mul_add` are
// both single-rounding FMAs on x86-64 (v3+) and aarch64 — so any two paths
// produce byte-identical output BY CONSTRUCTION, independent of lane width
// or where a SIMD/remainder boundary falls. (Divide-by-constant and split
// mul+add variants of this formula differ from it on ~8 per million input
// triples at rounding boundaries — measured exhaustively over all integer
// (Y,U,V) x range x matrix on 2026-07-20: 841 of 100,663,296 differ by ±1.
// One recipe, used everywhere, is what makes "identical" a structural
// property instead of a sampled observation.)
//
// wasm128 caveat: wasm SIMD has no FMA instruction; if magetypes polyfills
// `mul_add` unfused there, wasm vector lanes could differ from the fused
// scalar remainder on rounding-boundary pixels. The byte-identity reference
// tests run in wasm CI and will surface that; x86-64/aarch64 are fused.

/// Get matrix coefficients (Kr, Kb) for the specified color space.
pub(crate) fn matrix_coefficients(matrix: YuvMatrix) -> (f32, f32) {
    match matrix {
        YuvMatrix::Bt601 => (0.299, 0.114),
        YuvMatrix::Bt709 => (0.2126, 0.0722),
        YuvMatrix::Bt2020 => (0.2627, 0.0593),
        YuvMatrix::Custom { kr, kb } => (kr, kb),
    }
}

/// Convert one YUV sample to RGB — the scalar form of the canonical recipe
/// (module docs above). Used by the kernels' scalar remainders and by the
/// test references; `f32::mul_add` keeps it bit-identical to the SIMD lanes.
///
/// Formula (full range):
/// ```text
/// R = Y + Vr * (V - 128)
/// G = Y + Ug * (U - 128) + Vg * (V - 128)
/// B = Y + Ub * (U - 128)
///
/// where:
/// Vr = 2 * (1 - Kr)
/// Ug = -2 * Kb * (1 - Kb) / Kg
/// Vg = -2 * Kr * (1 - Kr) / Kg
/// Ub = 2 * (1 - Kb)
/// ```
fn yuv_to_rgb(y: f32, u: f32, v: f32, kr: f32, kg: f32, kb: f32, range: YuvRange) -> (u8, u8, u8) {
    let _ = kg; // derived inside RecipeConsts
    let c = RecipeConsts::new(YuvMatrixKrKb { kr, kb }, range, 8);
    let (r, g, b) = convert_one(y, u, v, &c);
    (r as u8, g as u8, b as u8)
}

/// (Kr, Kb) pair — the only matrix inputs the recipe needs.
#[derive(Clone, Copy)]
struct YuvMatrixKrKb {
    kr: f32,
    kb: f32,
}

/// Hoisted per-conversion constants of the canonical recipe, parameterized
/// by bit depth (8/10/12/16): normalization spans scale as `<< (d - 8)`
/// per BT.601/709/2100 convention, output values are native-depth
/// (`0..=2^d - 1`).
#[derive(Clone, Copy)]
struct RecipeConsts {
    y_off: f32,
    y_sc: f32,
    uv_cen: f32,
    uv_sc: f32,
    vr: f32,
    ug: f32,
    vg: f32,
    ub: f32,
    /// `2^d - 1` — the clamp ceiling and output scale.
    out_max: f32,
}

impl RecipeConsts {
    fn new(m: YuvMatrixKrKb, range: YuvRange, bit_depth: u8) -> Self {
        let (kr, kb) = (m.kr, m.kb);
        let kg = 1.0 - kr - kb;
        let shift = u32::from(bit_depth) - 8;
        let max = ((1u32 << bit_depth) - 1) as f32;
        let (y_off, y_sc, uv_cen, uv_sc) = match range {
            YuvRange::Full => (0.0f32, 1.0 / max, (128u32 << shift) as f32, 1.0 / max),
            YuvRange::Limited => (
                (16u32 << shift) as f32,
                1.0 / ((219u32 << shift) as f32),
                (128u32 << shift) as f32,
                1.0 / ((224u32 << shift) as f32),
            ),
        };
        Self {
            y_off,
            y_sc,
            uv_cen,
            uv_sc,
            vr: 2.0 * (1.0 - kr),
            ug: -2.0 * kb * (1.0 - kb) / kg,
            vg: -2.0 * kr * (1.0 - kr) / kg,
            ub: 2.0 * (1.0 - kb),
            out_max: max,
        }
    }
}

/// One sample through the canonical recipe, scalar form; returns the three
/// channels rounded and clamped to `[0, out_max]` (cast at the store).
/// `f32::mul_add` (single-rounding FMA) and `round_ties_even` (vroundps /
/// NEON vrndn / wasm nearest semantics) keep this bit-identical to any
/// vector lowering — and auto-vectorizable: in a plain loop inside a
/// target_feature region, LLVM lowers it to the same vfmadd/vroundps
/// sequence at full width.
#[inline(always)]
fn convert_one(y: f32, u: f32, v: f32, c: &RecipeConsts) -> (f32, f32, f32) {
    let y_norm = (y - c.y_off) * c.y_sc;
    let u_norm = (u - c.uv_cen) * c.uv_sc;
    let v_norm = (v - c.uv_cen) * c.uv_sc;

    let r = v_norm.mul_add(c.vr, y_norm);
    let g = v_norm.mul_add(c.vg, u_norm.mul_add(c.ug, y_norm));
    let b = u_norm.mul_add(c.ub, y_norm);

    let r = (r * c.out_max).max(0.0).min(c.out_max).round_ties_even();
    let g = (g * c.out_max).max(0.0).min(c.out_max).round_ties_even();
    let b = (b * c.out_max).max(0.0).min(c.out_max).round_ties_even();
    (r, g, b)
}

/// Input sample type the kernels read (u8 for 8-bit planes, u16 for
/// 10/12/16-bit planes; values are native-depth).
pub(crate) trait YuvSample: Copy {
    fn to_f32(self) -> f32;
}

impl YuvSample for u8 {
    #[inline(always)]
    fn to_f32(self) -> f32 {
        self as f32
    }
}

impl YuvSample for u16 {
    #[inline(always)]
    fn to_f32(self) -> f32 {
        self as f32
    }
}

/// Output pixel type for the conversion kernels. `from_rgbf` receives
/// channels already rounded/clamped to `[0, max]` where `max = 2^d - 1`;
/// alpha (where present) is filled with `max` (native-depth opaque).
pub(crate) trait StripPixel: Copy + Default {
    fn from_rgbf(r: f32, g: f32, b: f32, max: f32) -> Self;
}

impl StripPixel for RGB8 {
    #[inline(always)]
    fn from_rgbf(r: f32, g: f32, b: f32, _max: f32) -> Self {
        RGB8 {
            r: r as u8,
            g: g as u8,
            b: b as u8,
        }
    }
}

impl StripPixel for Rgba<u8> {
    #[inline(always)]
    fn from_rgbf(r: f32, g: f32, b: f32, _max: f32) -> Self {
        Rgba {
            r: r as u8,
            g: g as u8,
            b: b as u8,
            a: 255,
        }
    }
}

impl StripPixel for rgb::Gray<u8> {
    #[inline(always)]
    fn from_rgbf(r: f32, _g: f32, _b: f32, _max: f32) -> Self {
        rgb::Gray::new(r as u8)
    }
}

impl StripPixel for rgb::Gray<u16> {
    #[inline(always)]
    fn from_rgbf(r: f32, _g: f32, _b: f32, _max: f32) -> Self {
        rgb::Gray::new(r as u16)
    }
}

impl StripPixel for rgb::Rgb<u16> {
    #[inline(always)]
    fn from_rgbf(r: f32, g: f32, b: f32, _max: f32) -> Self {
        rgb::Rgb {
            r: r as u16,
            g: g as u16,
            b: b as u16,
        }
    }
}

impl StripPixel for Rgba<u16> {
    #[inline(always)]
    fn from_rgbf(r: f32, g: f32, b: f32, max: f32) -> Self {
        Rgba {
            r: r as u16,
            g: g as u16,
            b: b as u16,
            a: max as u16,
        }
    }
}

// ═══════════════════ Unified strip-first conversion kernels ════════════════
//
// ONE implementation per chroma subsampling, generic over the output pixel
// (`StripPixel`: RGB8 | RGBA8), windowed by `(y_start, strip_height)` over
// the FULL planes (so 4:2:0 vertical interpolation is correct across strip
// boundaries). Full-image conversion is the `(0, height)` window. Tiers:
// AVX-512 (v4x/v4, needs the archmage `avx512` feature), AVX2/FMA (v3),
// NEON, wasm SIMD, scalar — one `#[magetypes]` list, no per-arch forks.
//
// Structure per kernel: separable vectorizable chroma passes feeding a
// plain per-pixel canonical-recipe loop that LLVM auto-vectorizes at full
// register width. Kernels are generic over the input sample (u8 | u16
// native-depth) and the output pixel (RGB/RGBA, 8/16-bit).

/// Interpolate one chroma row horizontally from `cw = ceil(width/2)`
/// samples to `width` positions (the AVIF/AV1 center-sited bilinear:
/// even x -> 0.25·c[k-1] + 0.75·c[k], odd x -> 0.75·c[k] + 0.25·c[k+1],
/// edges clamped). The weights are dyadic and the inputs are native-depth
/// integers (<= 16 bits), so
/// every product and sum needs <= 20 mantissa bits and is EXACT in f32 —
/// this separable formulation is
/// bit-identical to the direct 4-term bilinear, and each loop is a
/// contiguous windows(2) shape LLVM vectorizes.
#[inline(always)]
fn interp_chroma_row_h(src: &[f32], width: usize, dst: &mut [f32]) {
    let cw = width.div_ceil(2);
    let src = &src[..cw];
    let dst = &mut dst[..width];
    dst[0] = src[0];
    // Odd positions x = 2k+1: 0.75·c[k] + 0.25·c[k+1]; the last odd
    // position of an even width clamps to c[cw-1] (fx = 0 after clamping).
    for k in 0..cw.saturating_sub(1) {
        dst[2 * k + 1] = 0.75 * src[k] + 0.25 * src[k + 1];
    }
    if width.is_multiple_of(2) {
        dst[width - 1] = src[cw - 1];
    }
    // Even positions x = 2k (k >= 1): 0.25·c[k-1] + 0.75·c[k].
    for k in 1..cw {
        if 2 * k < width {
            dst[2 * k] = 0.25 * src[k - 1] + 0.75 * src[k];
        }
    }
}

/// Widen one chroma row to f32.
#[inline(always)]
fn widen_row<S: YuvSample>(src: &[S], n: usize, dst: &mut [f32]) {
    for (d, &v) in dst[..n].iter_mut().zip(&src[..n]) {
        *d = v.to_f32();
    }
}

/// 4:2:0: bilinear chroma upsampling in both dimensions, decomposed into
/// three vectorizable passes per output row — vertical chroma lerp
/// (weights 0.25/0.75/clamp-0, exact in f32), horizontal interpolation
/// ([`interp_chroma_row_h`]), then the plain per-pixel canonical-recipe
/// loop that LLVM auto-vectorizes at full register width. Exactness of the
/// dyadic-weight interpolation makes this bit-identical to the direct
/// 4-term bilinear at every pixel.
#[magetypes(v4x, v4, v3, neon, wasm128, scalar)]
fn yuv420_strip_kernel<S: YuvSample, P: StripPixel>(
    token: Token,
    y_plane: &[S],
    y_stride: usize,
    u_plane: &[S],
    u_stride: usize,
    v_plane: &[S],
    v_stride: usize,
    width: usize,
    total_height: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    bit_depth: u8,
    out: &mut [P],
) {
    let _ = token;
    let (kr, kb) = matrix_coefficients(matrix);
    let c = RecipeConsts::new(YuvMatrixKrKb { kr, kb }, range, bit_depth);
    let chroma_width = width.div_ceil(2);
    let chroma_height = total_height.div_ceil(2);

    // Row scratch: two vertically-lerped chroma rows (chroma_width) and two
    // width-interpolated rows.
    let mut uv_mid = vec![0f32; 2 * chroma_width];
    let mut uf = vec![0f32; width];
    let mut vf = vec![0f32; width];

    for row in 0..strip_height {
        let y_pos = y_start + row;

        // Vertical chroma position: same structure as the horizontal one
        // (0.25/0.75 alternate, clamped at both edges — fy is dyadic).
        let chroma_y_raw = (y_pos as f32 + 0.5) * 0.5 - 0.5;
        let chroma_y = chroma_y_raw.max(0.0).min(chroma_height as f32 - 1.0);
        let (cy0, fy) = floor_nonneg_idx(chroma_y);
        let cy1 = (cy0 + 1).min(chroma_height - 1);
        let fy1 = 1.0 - fy;

        // Pass 1: vertical lerp into u/v mid-rows (exact dyadic weights).
        {
            let (u_mid, v_mid) = uv_mid.split_at_mut(chroma_width);
            let u0 = &u_plane[cy0 * u_stride..][..chroma_width];
            let u1 = &u_plane[cy1 * u_stride..][..chroma_width];
            let v0 = &v_plane[cy0 * v_stride..][..chroma_width];
            let v1 = &v_plane[cy1 * v_stride..][..chroma_width];
            for k in 0..chroma_width {
                u_mid[k] = u0[k].to_f32() * fy1 + u1[k].to_f32() * fy;
                v_mid[k] = v0[k].to_f32() * fy1 + v1[k].to_f32() * fy;
            }
            // Pass 2: horizontal interpolation to full width.
            interp_chroma_row_h(u_mid, width, &mut uf);
            interp_chroma_row_h(v_mid, width, &mut vf);
        }

        // Pass 3: canonical recipe over contiguous rows (auto-vectorized).
        let y_row = &y_plane[y_pos * y_stride..][..width];
        let out_row = &mut out[row * width..][..width];
        for x in 0..width {
            let (r, g, b) = convert_one(y_row[x].to_f32(), uf[x], vf[x], &c);
            out_row[x] = P::from_rgbf(r, g, b, c.out_max);
        }
    }
}

/// 4:2:2: horizontal bilinear chroma upsampling only (full vertical
/// chroma resolution) — widen + horizontal pass + the auto-vectorized
/// per-pixel loop. See [`yuv420_strip_kernel`] for the exactness argument.
#[magetypes(v4x, v4, v3, neon, wasm128, scalar)]
fn yuv422_strip_kernel<S: YuvSample, P: StripPixel>(
    token: Token,
    y_plane: &[S],
    y_stride: usize,
    u_plane: &[S],
    u_stride: usize,
    v_plane: &[S],
    v_stride: usize,
    width: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    bit_depth: u8,
    out: &mut [P],
) {
    let _ = token;
    let (kr, kb) = matrix_coefficients(matrix);
    let c = RecipeConsts::new(YuvMatrixKrKb { kr, kb }, range, bit_depth);
    let chroma_width = width.div_ceil(2);

    let mut uv_mid = vec![0f32; 2 * chroma_width];
    let mut uf = vec![0f32; width];
    let mut vf = vec![0f32; width];

    for row in 0..strip_height {
        let y_pos = y_start + row;

        {
            let (u_mid, v_mid) = uv_mid.split_at_mut(chroma_width);
            widen_row(&u_plane[y_pos * u_stride..], chroma_width, u_mid);
            widen_row(&v_plane[y_pos * v_stride..], chroma_width, v_mid);
            interp_chroma_row_h(u_mid, width, &mut uf);
            interp_chroma_row_h(v_mid, width, &mut vf);
        }

        let y_row = &y_plane[y_pos * y_stride..][..width];
        let out_row = &mut out[row * width..][..width];
        for x in 0..width {
            let (r, g, b) = convert_one(y_row[x].to_f32(), uf[x], vf[x], &c);
            out_row[x] = P::from_rgbf(r, g, b, c.out_max);
        }
    }
}

/// 4:4:4: no chroma subsampling — a plain per-pixel loop over exact row
/// slices. Inside the tier's target_feature region LLVM auto-vectorizes
/// the canonical recipe (contiguous u8 loads -> f32, vfmadd, vroundps) at
/// full register width — measured faster than an explicit 8-lane gather
/// (which pays per-lane array traffic for data that is already
/// contiguous). Byte-identity with the other kernels is structural: same
/// elementwise IEEE ops, whatever the vector width.
#[magetypes(v4x, v4, v3, neon, wasm128, scalar)]
fn yuv444_strip_kernel<S: YuvSample, P: StripPixel>(
    token: Token,
    y_plane: &[S],
    y_stride: usize,
    u_plane: &[S],
    u_stride: usize,
    v_plane: &[S],
    v_stride: usize,
    width: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    bit_depth: u8,
    out: &mut [P],
) {
    let _ = token;
    let (kr, kb) = matrix_coefficients(matrix);
    let c = RecipeConsts::new(YuvMatrixKrKb { kr, kb }, range, bit_depth);

    for row in 0..strip_height {
        let y_pos = y_start + row;
        // Exact-width row slices: LLVM sees the bounds and vectorizes the
        // loop without per-element checks.
        let y_row = &y_plane[y_pos * y_stride..][..width];
        let u_row = &u_plane[y_pos * u_stride..][..width];
        let v_row = &v_plane[y_pos * v_stride..][..width];
        let out_row = &mut out[row * width..][..width];

        for x in 0..width {
            let (r, g, b) =
                convert_one(y_row[x].to_f32(), u_row[x].to_f32(), v_row[x].to_f32(), &c);
            out_row[x] = P::from_rgbf(r, g, b, c.out_max);
        }
    }
}

// ═══════════════════════════════ Public API ════════════════════════════════
//
// Full-image conversion is the `(0, height)` strip window; the strip entry
// points read from FULL planes and write `strip_height * width` pixels to
// `out` starting at index 0.

/// Convert YUV420 to RGB8 with bilinear chroma upsampling.
///
/// Automatically dispatches to the best SIMD path available: AVX-512
/// (with the archmage `avx512` feature) or AVX2/FMA on x86-64, NEON on
/// aarch64, wasm SIMD on wasm32, or scalar fallback.
pub fn yuv420_to_rgb8(
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: usize,
    height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
) -> ImgVec<RGB8> {
    let mut out = vec![RGB8::default(); width * height];
    yuv420_to_rgb8_strip(
        y_plane, y_stride, u_plane, u_stride, v_plane, v_stride, width, height, 0, height, range,
        matrix, &mut out,
    );
    ImgVec::new(out, width, height)
}

/// Convert YUV422 to RGB8 with horizontal bilinear chroma upsampling.
pub fn yuv422_to_rgb8(
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: usize,
    height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
) -> ImgVec<RGB8> {
    let mut out = vec![RGB8::default(); width * height];
    yuv422_to_rgb8_strip(
        y_plane, y_stride, u_plane, u_stride, v_plane, v_stride, width, 0, height, range, matrix,
        &mut out,
    );
    ImgVec::new(out, width, height)
}

/// Convert YUV444 to RGB8 (no chroma upsampling).
pub fn yuv444_to_rgb8(
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: usize,
    height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
) -> ImgVec<RGB8> {
    let mut out = vec![RGB8::default(); width * height];
    yuv444_to_rgb8_strip(
        y_plane, y_stride, u_plane, u_stride, v_plane, v_stride, width, 0, height, range, matrix,
        &mut out,
    );
    ImgVec::new(out, width, height)
}

/// Convert a strip of YUV420 rows to RGB8.
///
/// Reads from full YUV planes (for correct bilinear chroma upsampling at
/// strip boundaries) but only converts rows `y_start..y_start + strip_height`.
/// Output is written to `out` starting at index 0, tightly packed at `width`
/// pixels per row.
pub fn yuv420_to_rgb8_strip(
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: usize,
    total_height: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    out: &mut [RGB8],
) {
    incant!(
        yuv420_strip_kernel::<u8, RGB8>(
            y_plane,
            y_stride,
            u_plane,
            u_stride,
            v_plane,
            v_stride,
            width,
            total_height,
            y_start,
            strip_height,
            range,
            matrix,
            8,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// Convert a strip of YUV420 rows to RGBA8 (alpha channel set to 255).
pub fn yuv420_to_rgba8_strip(
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: usize,
    total_height: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    out: &mut [Rgba<u8>],
) {
    incant!(
        yuv420_strip_kernel::<u8, Rgba<u8>>(
            y_plane,
            y_stride,
            u_plane,
            u_stride,
            v_plane,
            v_stride,
            width,
            total_height,
            y_start,
            strip_height,
            range,
            matrix,
            8,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// Convert a strip of YUV422 rows to RGB8.
pub fn yuv422_to_rgb8_strip(
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    out: &mut [RGB8],
) {
    incant!(
        yuv422_strip_kernel::<u8, RGB8>(
            y_plane,
            y_stride,
            u_plane,
            u_stride,
            v_plane,
            v_stride,
            width,
            y_start,
            strip_height,
            range,
            matrix,
            8,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// Convert a strip of YUV422 rows to RGBA8 (alpha channel set to 255).
pub fn yuv422_to_rgba8_strip(
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    out: &mut [Rgba<u8>],
) {
    incant!(
        yuv422_strip_kernel::<u8, Rgba<u8>>(
            y_plane,
            y_stride,
            u_plane,
            u_stride,
            v_plane,
            v_stride,
            width,
            y_start,
            strip_height,
            range,
            matrix,
            8,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// Convert a strip of YUV444 rows to RGB8.
pub fn yuv444_to_rgb8_strip(
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    out: &mut [RGB8],
) {
    incant!(
        yuv444_strip_kernel::<u8, RGB8>(
            y_plane,
            y_stride,
            u_plane,
            u_stride,
            v_plane,
            v_stride,
            width,
            y_start,
            strip_height,
            range,
            matrix,
            8,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// Convert a strip of YUV444 rows to RGBA8 (alpha channel set to 255).
pub fn yuv444_to_rgba8_strip(
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    out: &mut [Rgba<u8>],
) {
    incant!(
        yuv444_strip_kernel::<u8, Rgba<u8>>(
            y_plane,
            y_stride,
            u_plane,
            u_stride,
            v_plane,
            v_stride,
            width,
            y_start,
            strip_height,
            range,
            matrix,
            8,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// Monochrome (Cs400): luma-only — replicate normalized luma to R=G=B.
/// This is exactly the planar recipe with chroma at center (the chroma
/// terms are fma(0, k, y_n) = y_n), so mono output is byte-identical to a
/// 4:4:4 decode of the same luma with flat-center chroma. No matrix input:
/// gray has no chroma to weigh.
#[magetypes(v4x, v4, v3, neon, wasm128, scalar)]
fn yuv400_strip_kernel<S: YuvSample, P: StripPixel>(
    token: Token,
    y_plane: &[S],
    y_stride: usize,
    width: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    bit_depth: u8,
    out: &mut [P],
) {
    let _ = token;
    // Any matrix: with zero chroma the coefficients cancel.
    let c = RecipeConsts::new(
        YuvMatrixKrKb {
            kr: 0.299,
            kb: 0.114,
        },
        range,
        bit_depth,
    );

    for row in 0..strip_height {
        let y_pos = y_start + row;
        let y_row = &y_plane[y_pos * y_stride..][..width];
        let out_row = &mut out[row * width..][..width];
        for x in 0..width {
            let y_norm = (y_row[x].to_f32() - c.y_off) * c.y_sc;
            let g = (y_norm * c.out_max)
                .max(0.0)
                .min(c.out_max)
                .round_ties_even();
            out_row[x] = P::from_rgbf(g, g, g, c.out_max);
        }
    }
}

/// Convert a strip of monochrome (Cs400) rows to RGB/RGBA at any depth —
/// generic entry point (u8 planes with `bit_depth` 8, u16 planes with
/// 10/12/16).
pub(crate) fn yuv400_to_rgbx_strip<S: YuvSample, P: StripPixel>(
    y_plane: &[S],
    y_stride: usize,
    width: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    bit_depth: u8,
    out: &mut [P],
) {
    incant!(
        yuv400_strip_kernel::<S, P>(
            y_plane,
            y_stride,
            width,
            y_start,
            strip_height,
            range,
            bit_depth,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

// ── 16-bit (10/12/16-bit native-depth) public strip API ─────────────────────
//
// Same kernels as the 8-bit API, instantiated at `u16` samples; `bit_depth`
// selects the normalization spans and the output ceiling (values are
// native-depth, e.g. 0..=1023 for 10-bit; RGBA alpha = the ceiling).

/// Convert a strip of 4:2:0 rows (u16 native-depth samples) to RGB16.
pub fn yuv420_to_rgb16_strip(
    y_plane: &[u16],
    y_stride: usize,
    u_plane: &[u16],
    u_stride: usize,
    v_plane: &[u16],
    v_stride: usize,
    width: usize,
    total_height: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    bit_depth: u8,
    out: &mut [rgb::Rgb<u16>],
) {
    incant!(
        yuv420_strip_kernel::<u16, rgb::Rgb<u16>>(
            y_plane,
            y_stride,
            u_plane,
            u_stride,
            v_plane,
            v_stride,
            width,
            total_height,
            y_start,
            strip_height,
            range,
            matrix,
            bit_depth,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// Convert a strip of 4:2:0 rows (u16 native-depth samples) to RGBA16.
pub fn yuv420_to_rgba16_strip(
    y_plane: &[u16],
    y_stride: usize,
    u_plane: &[u16],
    u_stride: usize,
    v_plane: &[u16],
    v_stride: usize,
    width: usize,
    total_height: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    bit_depth: u8,
    out: &mut [Rgba<u16>],
) {
    incant!(
        yuv420_strip_kernel::<u16, Rgba<u16>>(
            y_plane,
            y_stride,
            u_plane,
            u_stride,
            v_plane,
            v_stride,
            width,
            total_height,
            y_start,
            strip_height,
            range,
            matrix,
            bit_depth,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// Convert a strip of 4:2:2 rows (u16 native-depth samples) to RGB16.
pub fn yuv422_to_rgb16_strip(
    y_plane: &[u16],
    y_stride: usize,
    u_plane: &[u16],
    u_stride: usize,
    v_plane: &[u16],
    v_stride: usize,
    width: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    bit_depth: u8,
    out: &mut [rgb::Rgb<u16>],
) {
    incant!(
        yuv422_strip_kernel::<u16, rgb::Rgb<u16>>(
            y_plane,
            y_stride,
            u_plane,
            u_stride,
            v_plane,
            v_stride,
            width,
            y_start,
            strip_height,
            range,
            matrix,
            bit_depth,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// Convert a strip of 4:2:2 rows (u16 native-depth samples) to RGBA16.
pub fn yuv422_to_rgba16_strip(
    y_plane: &[u16],
    y_stride: usize,
    u_plane: &[u16],
    u_stride: usize,
    v_plane: &[u16],
    v_stride: usize,
    width: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    bit_depth: u8,
    out: &mut [Rgba<u16>],
) {
    incant!(
        yuv422_strip_kernel::<u16, Rgba<u16>>(
            y_plane,
            y_stride,
            u_plane,
            u_stride,
            v_plane,
            v_stride,
            width,
            y_start,
            strip_height,
            range,
            matrix,
            bit_depth,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// Convert a strip of 4:4:4 rows (u16 native-depth samples) to RGB16.
pub fn yuv444_to_rgb16_strip(
    y_plane: &[u16],
    y_stride: usize,
    u_plane: &[u16],
    u_stride: usize,
    v_plane: &[u16],
    v_stride: usize,
    width: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    bit_depth: u8,
    out: &mut [rgb::Rgb<u16>],
) {
    incant!(
        yuv444_strip_kernel::<u16, rgb::Rgb<u16>>(
            y_plane,
            y_stride,
            u_plane,
            u_stride,
            v_plane,
            v_stride,
            width,
            y_start,
            strip_height,
            range,
            matrix,
            bit_depth,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// Convert a strip of 4:4:4 rows (u16 native-depth samples) to RGBA16.
pub fn yuv444_to_rgba16_strip(
    y_plane: &[u16],
    y_stride: usize,
    u_plane: &[u16],
    u_stride: usize,
    v_plane: &[u16],
    v_stride: usize,
    width: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    bit_depth: u8,
    out: &mut [Rgba<u16>],
) {
    incant!(
        yuv444_strip_kernel::<u16, Rgba<u16>>(
            y_plane,
            y_stride,
            u_plane,
            u_stride,
            v_plane,
            v_stride,
            width,
            y_start,
            strip_height,
            range,
            matrix,
            bit_depth,
            out
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// One 16-bit dispatch over the sampling enum, generic over the output
/// pixel — the decoder's entry point. `total_height` is used by 4:2:0
/// (vertical interpolation window) and ignored by 4:2:2 / 4:4:4.
#[allow(clippy::too_many_arguments)]
pub(crate) fn yuv16_to_rgbx_strip<P: StripPixel>(
    sampling: ChromaSubsampling,
    y_plane: &[u16],
    y_stride: usize,
    u_plane: &[u16],
    u_stride: usize,
    v_plane: &[u16],
    v_stride: usize,
    width: usize,
    total_height: usize,
    y_start: usize,
    strip_height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    bit_depth: u8,
    out: &mut [P],
) {
    match sampling {
        ChromaSubsampling::Cs420 => incant!(
            yuv420_strip_kernel::<u16, P>(
                y_plane,
                y_stride,
                u_plane,
                u_stride,
                v_plane,
                v_stride,
                width,
                total_height,
                y_start,
                strip_height,
                range,
                matrix,
                bit_depth,
                out
            ),
            [v4x, v4, v3, neon, wasm128, scalar]
        ),
        ChromaSubsampling::Cs422 => incant!(
            yuv422_strip_kernel::<u16, P>(
                y_plane,
                y_stride,
                u_plane,
                u_stride,
                v_plane,
                v_stride,
                width,
                y_start,
                strip_height,
                range,
                matrix,
                bit_depth,
                out
            ),
            [v4x, v4, v3, neon, wasm128, scalar]
        ),
        ChromaSubsampling::Cs444 => incant!(
            yuv444_strip_kernel::<u16, P>(
                y_plane,
                y_stride,
                u_plane,
                u_stride,
                v_plane,
                v_stride,
                width,
                y_start,
                strip_height,
                range,
                matrix,
                bit_depth,
                out
            ),
            [v4x, v4, v3, neon, wasm128, scalar]
        ),
    }
}

// ═══════════════════════ Forward (encode-side) conversion ══════════════════
//
// RGB(A)8 -> YUV 4:2:0, the inverse of the canonical decode recipe:
// per-pixel chroma is computed in f32 at full resolution, box-averaged 2x2
// (the dyadic 0.25 weights keep the average EXACT in f32), then quantized —
// averaging BEFORE quantization, so chroma sites carry the true mean.
// Odd dimensions edge-replicate (clamp), matching the decode-side clamp.

/// Input pixel for the forward kernel: yields (r, g, b) as f32.
pub(crate) trait ForwardPixel: Copy {
    fn rgb_f32(self) -> (f32, f32, f32);
}

impl ForwardPixel for RGB8 {
    #[inline(always)]
    fn rgb_f32(self) -> (f32, f32, f32) {
        (self.r as f32, self.g as f32, self.b as f32)
    }
}

impl ForwardPixel for Rgba<u8> {
    #[inline(always)]
    fn rgb_f32(self) -> (f32, f32, f32) {
        (self.r as f32, self.g as f32, self.b as f32)
    }
}

/// Forward-recipe constants (8-bit).
#[derive(Clone, Copy)]
struct FwdConsts {
    kr: f32,
    kg: f32,
    kb: f32,
    /// 1 / (2 * (1 - kb)) — chroma-U projection.
    inv_ub: f32,
    /// 1 / (2 * (1 - kr)) — chroma-V projection.
    inv_vr: f32,
    y_off: f32,
    y_span: f32,
    uv_span: f32,
}

impl FwdConsts {
    fn new(matrix: YuvMatrix, range: YuvRange) -> Self {
        let (kr, kb) = matrix_coefficients(matrix);
        let kg = 1.0 - kr - kb;
        let (y_off, y_span, uv_span) = match range {
            YuvRange::Full => (0.0f32, 255.0, 255.0),
            YuvRange::Limited => (16.0, 219.0, 224.0),
        };
        Self {
            kr,
            kg,
            kb,
            inv_ub: 1.0 / (2.0 * (1.0 - kb)),
            inv_vr: 1.0 / (2.0 * (1.0 - kr)),
            y_off,
            y_span,
            uv_span,
        }
    }
}

/// Convert RGB(A)8 rows to YUV 4:2:0 planes (8-bit). `rgb` is
/// `rgb_stride`-strided in pixels; planes are tight (`width` /
/// `ceil(width/2)` strides). The luma pass is a plain auto-vectorized
/// loop; chroma is projected per-pixel in f32, box-averaged 2x2, then
/// quantized.
#[magetypes(v4x, v4, v3, neon, wasm128, scalar)]
fn rgbx_to_yuv420_kernel<P: ForwardPixel>(
    token: Token,
    rgb: &[P],
    rgb_stride: usize,
    width: usize,
    height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    y_plane: &mut [u8],
    u_plane: &mut [u8],
    v_plane: &mut [u8],
) {
    let _ = token;
    let c = FwdConsts::new(matrix, range);
    let cw = width.div_ceil(2);
    let inv255 = 1.0 / 255.0;

    // Per-pixel chroma rows for the current luma row pair (f32, pre-average).
    let mut u_rows = vec![0f32; 2 * width];
    let mut v_rows = vec![0f32; 2 * width];

    for cy in 0..height.div_ceil(2) {
        let y0 = 2 * cy;
        let y1 = (y0 + 1).min(height - 1); // edge-replicate on odd heights
        for (ri, y_pos) in [y0, y1].into_iter().enumerate() {
            let src = &rgb[y_pos * rgb_stride..][..width];
            let y_out = &mut y_plane[y_pos.min(height - 1) * width..][..width];
            let u_row = &mut u_rows[ri * width..][..width];
            let v_row = &mut v_rows[ri * width..][..width];
            for x in 0..width {
                let (r, g, b) = src[x].rgb_f32();
                let rn = r * inv255;
                let gn = g * inv255;
                let bn = b * inv255;
                let yl = c.kb.mul_add(bn, c.kr.mul_add(rn, c.kg * gn));
                y_out[x] = yl
                    .mul_add(c.y_span, c.y_off)
                    .max(0.0)
                    .min(255.0)
                    .round_ties_even() as u8;
                u_row[x] = (bn - yl) * c.inv_ub;
                v_row[x] = (rn - yl) * c.inv_vr;
            }
        }
        // 2x2 box average (0.25 weights are exact) -> quantize.
        let u_out = &mut u_plane[cy * cw..][..cw];
        let v_out = &mut v_plane[cy * cw..][..cw];
        for k in 0..cw {
            let x0 = 2 * k;
            let x1 = (x0 + 1).min(width - 1); // edge-replicate on odd widths
            let ua = 0.25 * (u_rows[x0] + u_rows[x1] + u_rows[width + x0] + u_rows[width + x1]);
            let va = 0.25 * (v_rows[x0] + v_rows[x1] + v_rows[width + x0] + v_rows[width + x1]);
            u_out[k] = ua
                .mul_add(c.uv_span, 128.0)
                .max(0.0)
                .min(255.0)
                .round_ties_even() as u8;
            v_out[k] = va
                .mul_add(c.uv_span, 128.0)
                .max(0.0)
                .min(255.0)
                .round_ties_even() as u8;
        }
    }
}

/// RGB8 -> YUV 4:2:0 (tight planes; see the kernel docs).
pub(crate) fn rgb8_to_yuv420(
    rgb: &[RGB8],
    rgb_stride: usize,
    width: usize,
    height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    y_plane: &mut [u8],
    u_plane: &mut [u8],
    v_plane: &mut [u8],
) {
    incant!(
        rgbx_to_yuv420_kernel::<RGB8>(
            rgb, rgb_stride, width, height, range, matrix, y_plane, u_plane, v_plane
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

/// RGBA8 -> YUV 4:2:0 (alpha ignored; encode it as its own Cs400 plane).
pub(crate) fn rgba8_to_yuv420(
    rgb: &[Rgba<u8>],
    rgb_stride: usize,
    width: usize,
    height: usize,
    range: YuvRange,
    matrix: YuvMatrix,
    y_plane: &mut [u8],
    u_plane: &mut [u8],
    v_plane: &mut [u8],
) {
    incant!(
        rgbx_to_yuv420_kernel::<Rgba<u8>>(
            rgb, rgb_stride, width, height, range, matrix, y_plane, u_plane, v_plane
        ),
        [v4x, v4, v3, neon, wasm128, scalar]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `floor_nonneg_idx` must equal `(x.floor() as usize, x - x.floor())` for
    /// every non-negative finite input, including exact-integer boundaries.
    #[test]
    fn floor_nonneg_idx_matches_libm_floor() {
        let probes = [
            0.0f32, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 7.0, 7.5, 15.0, 15.5, 16.0, 63.0, 63.5, 64.0,
            127.0, 127.999_99, 128.0, 1022.5, 1023.0, 2046.5, 2047.0,
        ];
        for &x in &probes {
            let (idx, frac) = floor_nonneg_idx(x);
            assert_eq!(idx, x.floor() as usize, "idx mismatch at x={x}");
            assert_eq!(
                frac.to_bits(),
                (x - x.floor()).to_bits(),
                "frac bits at x={x}"
            );
        }
    }

    /// xorshift PRNG for reproducible pseudo-random plane data.
    fn fill_rand(buf: &mut [u8], seed: u32) {
        let mut s = seed | 1;
        for b in buf.iter_mut() {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            *b = (s & 0xFF) as u8;
        }
    }

    /// Independent per-pixel reference for YUV420 bilinear upsample + convert,
    /// written straight from the spec formula (uses libm `floor`, no shortcuts).
    /// The dispatched `yuv420_to_rgb8` must be byte-identical to this.
    fn ref_yuv420_to_rgb8(
        y_plane: &[u8],
        y_stride: usize,
        u_plane: &[u8],
        u_stride: usize,
        v_plane: &[u8],
        v_stride: usize,
        width: usize,
        height: usize,
        range: YuvRange,
        matrix: YuvMatrix,
    ) -> Vec<RGB8> {
        let (kr, kb) = matrix_coefficients(matrix);
        let kg = 1.0 - kr - kb;
        let chroma_width = width.div_ceil(2);
        let chroma_height = height.div_ceil(2);
        let mut out = vec![RGB8::default(); width * height];
        for y_pos in 0..height {
            let chroma_y_raw = (y_pos as f32 + 0.5) * 0.5 - 0.5;
            let chroma_y = chroma_y_raw.max(0.0).min(chroma_height as f32 - 1.0);
            let cy0 = chroma_y.floor() as usize;
            let cy1 = (cy0 + 1).min(chroma_height - 1);
            let fy = chroma_y - cy0 as f32;
            for x in 0..width {
                let y_val = y_plane[y_pos * y_stride + x] as f32;
                let chroma_x_raw = (x as f32 + 0.5) * 0.5 - 0.5;
                let chroma_x = chroma_x_raw.max(0.0).min(chroma_width as f32 - 1.0);
                let cx0 = chroma_x.floor() as usize;
                let cx1 = (cx0 + 1).min(chroma_width - 1);
                let fx = chroma_x - cx0 as f32;
                let fx1 = 1.0 - fx;
                let fy1 = 1.0 - fy;
                let u00 = u_plane[cy0 * u_stride + cx0] as f32;
                let u01 = u_plane[cy0 * u_stride + cx1] as f32;
                let u10 = u_plane[cy1 * u_stride + cx0] as f32;
                let u11 = u_plane[cy1 * u_stride + cx1] as f32;
                let u_val = u00 * fx1 * fy1 + u01 * fx * fy1 + u10 * fx1 * fy + u11 * fx * fy;
                let v00 = v_plane[cy0 * v_stride + cx0] as f32;
                let v01 = v_plane[cy0 * v_stride + cx1] as f32;
                let v10 = v_plane[cy1 * v_stride + cx0] as f32;
                let v11 = v_plane[cy1 * v_stride + cx1] as f32;
                let v_val = v00 * fx1 * fy1 + v01 * fx * fy1 + v10 * fx1 * fy + v11 * fx * fy;
                let (r, g, b) = yuv_to_rgb(y_val, u_val, v_val, kr, kg, kb, range);
                out[y_pos * width + x] = RGB8 { r, g, b };
            }
        }
        out
    }

    #[test]
    fn yuv420_to_rgb8_byte_identical_to_reference() {
        // Odd + even dims, sub-lane, multi-lane, and a bench-sized case.
        let sizes: [(usize, usize); 6] = [(1, 1), (3, 3), (8, 8), (9, 7), (17, 13), (64, 48)];
        let ranges = [YuvRange::Full, YuvRange::Limited];
        let matrices = [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020];
        let mut seed = 1u32;
        for &(w, h) in &sizes {
            let cw = w.div_ceil(2);
            let ch = h.div_ceil(2);
            let mut yb = vec![0u8; w * h];
            let mut ub = vec![0u8; cw * ch];
            let mut vb = vec![0u8; cw * ch];
            for &range in &ranges {
                for &matrix in &matrices {
                    seed = seed.wrapping_mul(2654435761).wrapping_add(1);
                    fill_rand(&mut yb, seed);
                    fill_rand(&mut ub, seed ^ 0xABCD);
                    fill_rand(&mut vb, seed ^ 0x1234);
                    let got = yuv420_to_rgb8(&yb, w, &ub, cw, &vb, cw, w, h, range, matrix);
                    let want = ref_yuv420_to_rgb8(&yb, w, &ub, cw, &vb, cw, w, h, range, matrix);
                    let got_vec: Vec<RGB8> = got.into_buf();
                    assert_eq!(
                        got_vec.as_slice(),
                        want.as_slice(),
                        "mismatch {w}x{h} {range:?} {matrix:?}"
                    );
                }
            }
        }
    }

    /// Independent per-pixel reference for YUV422 horizontal-bilinear upsample +
    /// convert (vertical is 1:1 for 4:2:2). The dispatched `yuv422_to_rgb8` must
    /// be byte-identical to this.
    fn ref_yuv422_to_rgb8(
        y_plane: &[u8],
        y_stride: usize,
        u_plane: &[u8],
        u_stride: usize,
        v_plane: &[u8],
        v_stride: usize,
        width: usize,
        height: usize,
        range: YuvRange,
        matrix: YuvMatrix,
    ) -> Vec<RGB8> {
        let (kr, kb) = matrix_coefficients(matrix);
        let kg = 1.0 - kr - kb;
        let chroma_width = width.div_ceil(2);
        let mut out = vec![RGB8::default(); width * height];
        for y_pos in 0..height {
            for x in 0..width {
                let y_val = y_plane[y_pos * y_stride + x] as f32;
                let chroma_x_raw = (x as f32 + 0.5) * 0.5 - 0.5;
                let chroma_x = chroma_x_raw.max(0.0).min(chroma_width as f32 - 1.0);
                let cx0 = chroma_x.floor() as usize;
                let cx1 = (cx0 + 1).min(chroma_width - 1);
                let fx = chroma_x - cx0 as f32;
                let fx1 = 1.0 - fx;
                // 4:2:2: chroma row index == luma row index (no vertical interp).
                let u_val = u_plane[y_pos * u_stride + cx0] as f32 * fx1
                    + u_plane[y_pos * u_stride + cx1] as f32 * fx;
                let v_val = v_plane[y_pos * v_stride + cx0] as f32 * fx1
                    + v_plane[y_pos * v_stride + cx1] as f32 * fx;
                let (r, g, b) = yuv_to_rgb(y_val, u_val, v_val, kr, kg, kb, range);
                out[y_pos * width + x] = RGB8 { r, g, b };
            }
        }
        out
    }

    #[test]
    fn yuv422_to_rgb8_byte_identical_to_reference() {
        // Odd + even dims, sub-lane, multi-lane, and a bench-sized case.
        let sizes: [(usize, usize); 6] = [(1, 1), (3, 3), (8, 8), (9, 7), (17, 13), (64, 48)];
        let ranges = [YuvRange::Full, YuvRange::Limited];
        let matrices = [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020];
        let mut seed = 7u32;
        for &(w, h) in &sizes {
            let cw = w.div_ceil(2); // 4:2:2: half width, full height
            let mut yb = vec![0u8; w * h];
            let mut ub = vec![0u8; cw * h];
            let mut vb = vec![0u8; cw * h];
            for &range in &ranges {
                for &matrix in &matrices {
                    seed = seed.wrapping_mul(2654435761).wrapping_add(1);
                    fill_rand(&mut yb, seed);
                    fill_rand(&mut ub, seed ^ 0xABCD);
                    fill_rand(&mut vb, seed ^ 0x1234);
                    let got = yuv422_to_rgb8(&yb, w, &ub, cw, &vb, cw, w, h, range, matrix);
                    let want = ref_yuv422_to_rgb8(&yb, w, &ub, cw, &vb, cw, w, h, range, matrix);
                    let got_vec: Vec<RGB8> = got.into_buf();
                    if got_vec.as_slice() != want.as_slice() {
                        let mut n = 0;
                        for (i, (a, b)) in got_vec.iter().zip(want.iter()).enumerate() {
                            if a != b && n < 5 {
                                eprintln!(
                                    "DIFF {w}x{h} {range:?} {matrix:?} px {i} (x={} y={}): got {a:?} want {b:?}",
                                    i % w,
                                    i / w
                                );
                            }
                            if a != b {
                                n += 1;
                            }
                        }
                        panic!("mismatch {w}x{h} {range:?} {matrix:?}: {n} pixels");
                    }
                }
            }
        }
    }

    /// Independent per-pixel reference for YUV444 (no chroma upsampling),
    /// written straight from the canonical recipe via `yuv_to_rgb`. The
    /// dispatched `yuv444_to_rgb8` must be byte-identical to this.
    fn ref_yuv444_to_rgb8(
        y_plane: &[u8],
        y_stride: usize,
        u_plane: &[u8],
        u_stride: usize,
        v_plane: &[u8],
        v_stride: usize,
        width: usize,
        height: usize,
        range: YuvRange,
        matrix: YuvMatrix,
    ) -> Vec<RGB8> {
        let (kr, kb) = matrix_coefficients(matrix);
        let kg = 1.0 - kr - kb;
        let mut out = vec![RGB8::default(); width * height];
        for y in 0..height {
            for x in 0..width {
                let (r, g, b) = yuv_to_rgb(
                    y_plane[y * y_stride + x] as f32,
                    u_plane[y * u_stride + x] as f32,
                    v_plane[y * v_stride + x] as f32,
                    kr,
                    kg,
                    kb,
                    range,
                );
                out[y * width + x] = RGB8 { r, g, b };
            }
        }
        out
    }

    #[test]
    fn yuv444_to_rgb8_byte_identical_to_reference() {
        let sizes: [(usize, usize); 6] = [(1, 1), (3, 3), (8, 8), (9, 7), (17, 13), (64, 48)];
        let ranges = [YuvRange::Full, YuvRange::Limited];
        let matrices = [
            YuvMatrix::Bt601,
            YuvMatrix::Bt709,
            YuvMatrix::Bt2020,
            // FCC + SMPTE-240M as explicit (Kr, Kb) — the exotic-matrix path.
            YuvMatrix::Custom { kr: 0.30, kb: 0.11 },
            YuvMatrix::Custom {
                kr: 0.212,
                kb: 0.087,
            },
        ];
        let mut seed = 21u32;
        for &(w, h) in &sizes {
            let mut yb = vec![0u8; w * h];
            let mut ub = vec![0u8; w * h];
            let mut vb = vec![0u8; w * h];
            for &range in &ranges {
                for &matrix in &matrices {
                    seed = seed.wrapping_mul(2654435761).wrapping_add(1);
                    fill_rand(&mut yb, seed);
                    fill_rand(&mut ub, seed ^ 0xABCD);
                    fill_rand(&mut vb, seed ^ 0x1234);
                    let got = yuv444_to_rgb8(&yb, w, &ub, w, &vb, w, w, h, range, matrix);
                    let want = ref_yuv444_to_rgb8(&yb, w, &ub, w, &vb, w, w, h, range, matrix);
                    let got_vec: Vec<RGB8> = got.into_buf();
                    assert_eq!(
                        got_vec.as_slice(),
                        want.as_slice(),
                        "mismatch {w}x{h} {range:?} {matrix:?}"
                    );
                }
            }
        }
    }

    /// Full-image and strip conversion are the SAME kernel (the full image
    /// is the `(0, height)` strip window), and RGB vs RGBA differ only in
    /// the store — so byte-identity across all four combinations is
    /// structural. This pins the wrapper plumbing (windowing, dispatch,
    /// store) rather than float math.
    #[test]
    fn strip_rgba_kernels_match_full_rgb_kernels_exactly() {
        let sizes: [(usize, usize); 6] = [(1, 1), (3, 3), (8, 8), (9, 7), (17, 13), (64, 48)];
        let ranges = [YuvRange::Full, YuvRange::Limited];
        let matrices = [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020];
        let mut seed = 99u32;
        for &(w, h) in &sizes {
            for &range in &ranges {
                for &matrix in &matrices {
                    seed = seed.wrapping_mul(2654435761).wrapping_add(1);
                    for sampling in ["420", "422", "444"] {
                        let (cw, ch) = match sampling {
                            "420" => (w.div_ceil(2), h.div_ceil(2)),
                            "422" => (w.div_ceil(2), h),
                            _ => (w, h),
                        };
                        let mut yb = vec![0u8; w * h];
                        let mut ub = vec![0u8; cw * ch];
                        let mut vb = vec![0u8; cw * ch];
                        fill_rand(&mut yb, seed);
                        fill_rand(&mut ub, seed ^ 0xABCD);
                        fill_rand(&mut vb, seed ^ 0x1234);
                        let mut rgba = vec![
                            rgb::Rgba {
                                r: 0u8,
                                g: 0,
                                b: 0,
                                a: 0
                            };
                            w * h
                        ];
                        let rgb = match sampling {
                            "420" => {
                                yuv420_to_rgba8_strip(
                                    &yb, w, &ub, cw, &vb, cw, w, h, 0, h, range, matrix, &mut rgba,
                                );
                                yuv420_to_rgb8(&yb, w, &ub, cw, &vb, cw, w, h, range, matrix)
                            }
                            "422" => {
                                yuv422_to_rgba8_strip(
                                    &yb, w, &ub, cw, &vb, cw, w, 0, h, range, matrix, &mut rgba,
                                );
                                yuv422_to_rgb8(&yb, w, &ub, cw, &vb, cw, w, h, range, matrix)
                            }
                            _ => {
                                yuv444_to_rgba8_strip(
                                    &yb, w, &ub, cw, &vb, cw, w, 0, h, range, matrix, &mut rgba,
                                );
                                yuv444_to_rgb8(&yb, w, &ub, cw, &vb, cw, w, h, range, matrix)
                            }
                        };
                        let rgb_vec: Vec<RGB8> = rgb.into_buf();
                        for (i, (px3, px4)) in rgb_vec.iter().zip(rgba.iter()).enumerate() {
                            assert_eq!(
                                (px3.r, px3.g, px3.b, 255u8),
                                (px4.r, px4.g, px4.b, px4.a),
                                "{sampling} {w}x{h} {range:?} {matrix:?} px {i}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// xorshift PRNG for reproducible pseudo-random u16 plane data,
    /// clamped to the native depth.
    fn fill_rand16(buf: &mut [u16], seed: u32, bit_depth: u8) {
        let mask = (1u32 << bit_depth) - 1;
        let mut s = seed | 1;
        for b in buf.iter_mut() {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            *b = (s & mask) as u16;
        }
    }

    /// Independent per-pixel reference for the 16-bit kernels: DIRECT
    /// 4-term bilinear (not the kernels' separable form) + the canonical
    /// recipe at depth d. The dispatched strip kernels must be
    /// byte-identical to this at every depth and sampling.
    #[allow(clippy::too_many_arguments)]
    fn ref_yuv16(
        sampling: &str,
        y_plane: &[u16],
        u_plane: &[u16],
        v_plane: &[u16],
        width: usize,
        height: usize,
        range: YuvRange,
        matrix: YuvMatrix,
        bit_depth: u8,
    ) -> Vec<rgb::Rgb<u16>> {
        let (kr, kb) = matrix_coefficients(matrix);
        let c = RecipeConsts::new(YuvMatrixKrKb { kr, kb }, range, bit_depth);
        let (cw, ch, sub_x, sub_y) = match sampling {
            "420" => (width.div_ceil(2), height.div_ceil(2), true, true),
            "422" => (width.div_ceil(2), height, true, false),
            _ => (width, height, false, false),
        };
        let mut out = vec![rgb::Rgb::<u16>::default(); width * height];
        for y in 0..height {
            let chroma_y = if sub_y {
                ((y as f32 + 0.5) * 0.5 - 0.5).max(0.0).min(ch as f32 - 1.0)
            } else {
                y as f32
            };
            let (cy0, fy) = floor_nonneg_idx(chroma_y);
            let cy1 = (cy0 + 1).min(ch - 1);
            let fy1 = 1.0 - fy;
            for x in 0..width {
                let chroma_x = if sub_x {
                    ((x as f32 + 0.5) * 0.5 - 0.5).max(0.0).min(cw as f32 - 1.0)
                } else {
                    x as f32
                };
                let (cx0, fx) = floor_nonneg_idx(chroma_x);
                let cx1 = (cx0 + 1).min(cw - 1);
                let fx1 = 1.0 - fx;
                let u_val = u_plane[cy0 * cw + cx0] as f32 * fx1 * fy1
                    + u_plane[cy0 * cw + cx1] as f32 * fx * fy1
                    + u_plane[cy1 * cw + cx0] as f32 * fx1 * fy
                    + u_plane[cy1 * cw + cx1] as f32 * fx * fy;
                let v_val = v_plane[cy0 * cw + cx0] as f32 * fx1 * fy1
                    + v_plane[cy0 * cw + cx1] as f32 * fx * fy1
                    + v_plane[cy1 * cw + cx0] as f32 * fx1 * fy
                    + v_plane[cy1 * cw + cx1] as f32 * fx * fy;
                let (r, g, b) = convert_one(y_plane[y * width + x] as f32, u_val, v_val, &c);
                out[y * width + x] = rgb::Rgb {
                    r: r as u16,
                    g: g as u16,
                    b: b as u16,
                };
            }
        }
        out
    }

    #[test]
    fn yuv16_kernels_byte_identical_to_reference_all_depths() {
        let sizes: [(usize, usize); 4] = [(3, 3), (9, 7), (17, 13), (64, 48)];
        let ranges = [YuvRange::Full, YuvRange::Limited];
        let matrices = [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020];
        let mut seed = 33u32;
        for &bit_depth in &[10u8, 12, 16] {
            for &(w, h) in &sizes {
                for &range in &ranges {
                    for &matrix in &matrices {
                        for sampling in ["420", "422", "444"] {
                            let (cw, ch) = match sampling {
                                "420" => (w.div_ceil(2), h.div_ceil(2)),
                                "422" => (w.div_ceil(2), h),
                                _ => (w, h),
                            };
                            seed = seed.wrapping_mul(2654435761).wrapping_add(1);
                            let mut yb = vec![0u16; w * h];
                            let mut ub = vec![0u16; cw * ch];
                            let mut vb = vec![0u16; cw * ch];
                            fill_rand16(&mut yb, seed, bit_depth);
                            fill_rand16(&mut ub, seed ^ 0xABCD, bit_depth);
                            fill_rand16(&mut vb, seed ^ 0x1234, bit_depth);
                            let mut got = vec![rgb::Rgb::<u16>::default(); w * h];
                            let mut got_a = vec![Rgba::<u16>::default(); w * h];
                            match sampling {
                                "420" => {
                                    yuv420_to_rgb16_strip(
                                        &yb, w, &ub, cw, &vb, cw, w, h, 0, h, range, matrix,
                                        bit_depth, &mut got,
                                    );
                                    yuv420_to_rgba16_strip(
                                        &yb, w, &ub, cw, &vb, cw, w, h, 0, h, range, matrix,
                                        bit_depth, &mut got_a,
                                    );
                                }
                                "422" => {
                                    yuv422_to_rgb16_strip(
                                        &yb, w, &ub, cw, &vb, cw, w, 0, h, range, matrix,
                                        bit_depth, &mut got,
                                    );
                                    yuv422_to_rgba16_strip(
                                        &yb, w, &ub, cw, &vb, cw, w, 0, h, range, matrix,
                                        bit_depth, &mut got_a,
                                    );
                                }
                                _ => {
                                    yuv444_to_rgb16_strip(
                                        &yb, w, &ub, cw, &vb, cw, w, 0, h, range, matrix,
                                        bit_depth, &mut got,
                                    );
                                    yuv444_to_rgba16_strip(
                                        &yb, w, &ub, cw, &vb, cw, w, 0, h, range, matrix,
                                        bit_depth, &mut got_a,
                                    );
                                }
                            }
                            let want =
                                ref_yuv16(sampling, &yb, &ub, &vb, w, h, range, matrix, bit_depth);
                            let amax = (1u32 << bit_depth) - 1;
                            for (i, (g, w_)) in got.iter().zip(want.iter()).enumerate() {
                                assert_eq!(
                                    g, w_,
                                    "d{bit_depth} {sampling} {w}x{h} {range:?} {matrix:?} px {i}"
                                );
                            }
                            for (i, (g, w_)) in got_a.iter().zip(want.iter()).enumerate() {
                                assert_eq!(
                                    (g.r, g.g, g.b, g.a),
                                    (w_.r, w_.g, w_.b, amax as u16),
                                    "rgba d{bit_depth} {sampling} {w}x{h} {range:?} {matrix:?} px {i}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Monochrome kernel: R=G=B must equal the recipe's luma channel at
    /// every depth/range (== a 4:4:4 decode with flat-center chroma), and
    /// RGBA alpha must be the native ceiling.
    #[test]
    fn yuv400_matches_recipe_all_depths() {
        for &(bit_depth, is16) in &[(8u8, false), (10, true), (12, true), (16, true)] {
            for &range in &[YuvRange::Full, YuvRange::Limited] {
                let w = 33usize;
                let h = 9usize;
                let max = (1u64 << bit_depth) - 1;
                let c = RecipeConsts::new(
                    YuvMatrixKrKb {
                        kr: 0.299,
                        kb: 0.114,
                    },
                    range,
                    bit_depth,
                );
                if is16 {
                    let mut y = vec![0u16; w * h];
                    for (i, v) in y.iter_mut().enumerate() {
                        *v = ((i as u64 * 31) % (max + 1)) as u16;
                    }
                    let mut out = vec![Rgba::<u16>::default(); w * h];
                    yuv400_to_rgbx_strip::<u16, Rgba<u16>>(
                        &y, w, w, 0, h, range, bit_depth, &mut out,
                    );
                    for (i, (&yv, px)) in y.iter().zip(out.iter()).enumerate() {
                        let (r, g, b) = convert_one(yv as f32, c.uv_cen, c.uv_cen, &c);
                        assert_eq!(
                            (px.r, px.g, px.b, px.a),
                            (r as u16, g as u16, b as u16, max as u16),
                            "d{bit_depth} {range:?} px {i} (y={yv})"
                        );
                    }
                } else {
                    let mut y = vec![0u8; w * h];
                    fill_rand(&mut y, 77);
                    let mut out = vec![Rgba::<u8>::default(); w * h];
                    yuv400_to_rgbx_strip::<u8, Rgba<u8>>(&y, w, w, 0, h, range, 8, &mut out);
                    for (i, (&yv, px)) in y.iter().zip(out.iter()).enumerate() {
                        let (r, g, b) = convert_one(yv as f32, c.uv_cen, c.uv_cen, &c);
                        assert_eq!(
                            (px.r, px.g, px.b, px.a),
                            (r as u8, g as u8, b as u8, 255),
                            "d8 {range:?} px {i} (y={yv})"
                        );
                    }
                }
            }
        }
    }

    /// Forward kernel vs a direct per-pixel/per-site reference (same
    /// recipe, no strip structure): byte-identical, plus gray maps to
    /// exactly neutral chroma and a decode round-trip stays within
    /// subsampling-quantization error on smooth content.
    #[test]
    fn rgb_to_yuv420_reference_gray_and_roundtrip() {
        let (w, h) = (34usize, 18usize);
        let mut img = vec![RGB8::default(); w * h];
        for y in 0..h {
            for x in 0..w {
                img[y * w + x] = RGB8 {
                    r: ((x * 255) / w) as u8,
                    g: ((y * 255) / h) as u8,
                    b: (((x + y) * 255) / (w + h)) as u8,
                };
            }
        }
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);
        for &range in &[YuvRange::Full, YuvRange::Limited] {
            let matrix = YuvMatrix::Bt601;
            let mut yp = vec![0u8; w * h];
            let mut up = vec![0u8; cw * ch];
            let mut vp = vec![0u8; cw * ch];
            rgb8_to_yuv420(&img, w, w, h, range, matrix, &mut yp, &mut up, &mut vp);

            // Reference: same recipe, straight-line.
            let c = FwdConsts::new(matrix, range);
            for cy in 0..ch {
                for k in 0..cw {
                    let mut us = [0f32; 4];
                    let mut vs = [0f32; 4];
                    let mut yls = [[0f32; 2]; 2];
                    for (j, yy) in [2 * cy, (2 * cy + 1).min(h - 1)].into_iter().enumerate() {
                        for (i, xx) in [2 * k, (2 * k + 1).min(w - 1)].into_iter().enumerate() {
                            let p = img[yy * w + xx];
                            let rn = p.r as f32 / 255.0;
                            let gn = p.g as f32 / 255.0;
                            let bn = p.b as f32 / 255.0;
                            let yl = c.kb.mul_add(bn, c.kr.mul_add(rn, c.kg * gn));
                            yls[j][i] = yl;
                            us[j * 2 + i] = (bn - yl) * c.inv_ub;
                            vs[j * 2 + i] = (rn - yl) * c.inv_vr;
                        }
                    }
                    let ua = 0.25 * (us[0] + us[1] + us[2] + us[3]);
                    let va = 0.25 * (vs[0] + vs[1] + vs[2] + vs[3]);
                    let uref = ua
                        .mul_add(c.uv_span, 128.0)
                        .max(0.0)
                        .min(255.0)
                        .round_ties_even() as u8;
                    let vref = va
                        .mul_add(c.uv_span, 128.0)
                        .max(0.0)
                        .min(255.0)
                        .round_ties_even() as u8;
                    assert_eq!(up[cy * cw + k], uref, "{range:?} U site ({k},{cy})");
                    assert_eq!(vp[cy * cw + k], vref, "{range:?} V site ({k},{cy})");
                }
            }

            // Round-trip through the decode kernel: smooth gradient stays
            // close (subsampling + quantization bound the error).
            let mut back = vec![RGB8::default(); w * h];
            yuv420_to_rgb8_strip(
                &yp, w, &up, cw, &vp, cw, w, h, 0, h, range, matrix, &mut back,
            );
            let mut se = 0u64;
            for (a, b) in img.iter().zip(back.iter()) {
                for (x, y) in [(a.r, b.r), (a.g, b.g), (a.b, b.b)] {
                    let d = i64::from(x) - i64::from(y);
                    se += (d * d) as u64;
                }
            }
            let mse = se as f64 / (w * h * 3) as f64;
            let psnr = 10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10();
            assert!(
                psnr > 40.0,
                "{range:?} roundtrip PSNR {psnr:.2} dB below sanity floor"
            );
        }

        // Gray input -> exactly neutral chroma (full range).
        let gray = vec![
            RGB8 {
                r: 77,
                g: 77,
                b: 77
            };
            w * h
        ];
        let mut yp = vec![0u8; w * h];
        let mut up = vec![0u8; cw * ch];
        let mut vp = vec![0u8; cw * ch];
        rgb8_to_yuv420(
            &gray,
            w,
            w,
            h,
            YuvRange::Full,
            YuvMatrix::Bt601,
            &mut yp,
            &mut up,
            &mut vp,
        );
        assert!(yp.iter().all(|&v| v == 77), "gray luma passthrough");
        assert!(
            up.iter().chain(vp.iter()).all(|&v| v == 128),
            "gray chroma neutral"
        );
    }

    #[test]
    fn test_yuv_to_rgb_gray() {
        // YUV (128, 128, 128) should be gray (128, 128, 128)
        let (r, g, b) = yuv_to_rgb(128.0, 128.0, 128.0, 0.299, 0.587, 0.114, YuvRange::Full);
        assert_eq!(r, 128);
        assert_eq!(g, 128);
        assert_eq!(b, 128);
    }

    #[test]
    fn test_yuv_to_rgb_black() {
        // YUV (0, 128, 128) should be black (0, 0, 0)
        let (r, g, b) = yuv_to_rgb(0.0, 128.0, 128.0, 0.299, 0.587, 0.114, YuvRange::Full);
        assert_eq!(r, 0);
        assert_eq!(g, 0);
        assert_eq!(b, 0);
    }

    #[test]
    fn test_yuv_to_rgb_white() {
        // YUV (255, 128, 128) should be white (255, 255, 255)
        let (r, g, b) = yuv_to_rgb(255.0, 128.0, 128.0, 0.299, 0.587, 0.114, YuvRange::Full);
        assert_eq!(r, 255);
        assert_eq!(g, 255);
        assert_eq!(b, 255);
    }
}
