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
// Every DECODE conversion in this module — every tier, depth, sampling and
// output — computes the SAME fixed-point formula (d = bit depth, max =
// 2^d − 1; chroma arrives in 1/16 units, "U4", directly from the exact
// integer bilinear interpolation):
//
//   AY   = round(2^16 · max / y_span)
//   BU_c = round(2^12 · coef_u(c) · max / uv_span)   (2^12 = 2^16 / 16)
//   BV_c = round(2^12 · coef_v(c) · max / uv_span)
//   OFF_c = round(2^16 · (−y_off·max/y_span)) − (BU_c + BV_c)·cen·16 + 2^15
//           (offset built FROM the rounded coefficients, so neutral chroma
//            cancels exactly: gray in -> R = G = B out, at every depth)
//   out_c = clamp((AY·Y + BU_c·U4 + BV_c·V4 + OFF_c) >> 16, 0, max)
//
// with coef_(u,v) per channel: R = (0, Vr), G = (Ug, Vg), B = (Ub, 0);
// Vr = 2(1−Kr), Ug = −2·Kb(1−Kb)/Kg, Vg = −2·Kr(1−Kr)/Kg, Ub = 2(1−Kb).
// Full range: y_off = 0, y_span = uv_span = max, cen = 128·2^(d−8).
// Limited: y_off = 16·2^(d−8), y_span = 219·2^(d−8), uv_span = 224·2^(d−8).
//
// Why fixed point is the canonical form (it replaced an f32 FMA chain on
// 2026-07-20):
// * SINGLE rounding of a 2^-16-accurate value — closer to the exact
//   rational conversion than any multi-rounded float chain, and >> means
//   the result is defined by integer arithmetic alone: byte-identical on
//   every arch and tier BY CONSTRUCTION (no FMA availability, rounding
//   mode, or libm variance — wasm included).
// * i32 lane math vectorizes ~4x denser than f32 (the yuv crate's
//   fixed-point kernels measured ~6x our f32 chain at 10-bit).
// * Ties resolve as +2^15 then floor (round-half-up); a tie requires the
//   16 low accumulator bits to be exactly 0x8000, and the formula IS the
//   definition, so there is no "reference" to disagree with.
// Accumulators fit i32 for d ≤ 12 (worst |term| < 2^30; debug-asserted at
// constant build). AV1 codes 8/10/12-bit only; the 16-bit API entries keep
// the previous f32 recipe (documented at `convert_one_f32`).
//
// Chroma interpolation (the stage before this formula) is EXACT integer
// arithmetic in 1/16 units: 4:2:0 uses weights {9,3,3,1}/16 via separable
// {3,1}/4 passes, 4:2:2 {3,1}/4 · 4, 4:4:4 · 16 — all products of u12
// samples with ≤4-bit weights, no rounding anywhere before the formula.

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
    fn to_i32(self) -> i32;
}

impl YuvSample for u8 {
    #[inline(always)]
    fn to_f32(self) -> f32 {
        self as f32
    }
    #[inline(always)]
    fn to_i32(self) -> i32 {
        self as i32
    }
}

impl YuvSample for u16 {
    #[inline(always)]
    fn to_f32(self) -> f32 {
        self as f32
    }
    #[inline(always)]
    fn to_i32(self) -> i32 {
        self as i32
    }
}

/// Output pixel type for the conversion kernels. `from_rgbf` receives
/// channels already rounded/clamped to `[0, max]` where `max = 2^d - 1`;
/// alpha (where present) is filled with `max` (native-depth opaque).
pub(crate) trait StripPixel: Copy + Default {
    fn from_rgbf(r: f32, g: f32, b: f32, max: f32) -> Self;
    /// Store from the fixed-point formula's clamped u16 channels.
    fn from_rgb16(r: u16, g: u16, b: u16, max: u16) -> Self;
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
    #[inline(always)]
    fn from_rgb16(r: u16, g: u16, b: u16, _max: u16) -> Self {
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
    #[inline(always)]
    fn from_rgb16(r: u16, g: u16, b: u16, _max: u16) -> Self {
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
    #[inline(always)]
    fn from_rgb16(r: u16, _g: u16, _b: u16, _max: u16) -> Self {
        rgb::Gray::new(r as u8)
    }
}

impl StripPixel for rgb::Gray<u16> {
    #[inline(always)]
    fn from_rgbf(r: f32, _g: f32, _b: f32, _max: f32) -> Self {
        rgb::Gray::new(r as u16)
    }
    #[inline(always)]
    fn from_rgb16(r: u16, _g: u16, _b: u16, _max: u16) -> Self {
        rgb::Gray::new(r)
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
    #[inline(always)]
    fn from_rgb16(r: u16, g: u16, b: u16, _max: u16) -> Self {
        rgb::Rgb { r, g, b }
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
    #[inline(always)]
    fn from_rgb16(r: u16, g: u16, b: u16, max: u16) -> Self {
        Rgba { r, g, b, a: max }
    }
}

/// Integer constants of the canonical fixed-point recipe (d <= 12; module
/// docs above). Derived in f64, rounded once; the formula is then pure i32.
#[derive(Clone, Copy)]
struct FixedConsts {
    ay: i32,
    /// Per channel (R, G, B): U4 and V4 coefficients.
    bu: [i32; 3],
    bv: [i32; 3],
    /// Per channel offset, rounding bias (+2^15) folded in.
    off: [i32; 3],
    max: i32,
    /// Neutral chroma in U4 units (128·2^(d−8)·16) — the mono input.
    cen16: i32,
}

impl FixedConsts {
    fn new(m: YuvMatrixKrKb, range: YuvRange, bit_depth: u8) -> Self {
        debug_assert!(bit_depth <= 12, "fixed-point recipe is d<=12");
        let (kr, kb) = (m.kr as f64, m.kb as f64);
        let kg = 1.0 - kr - kb;
        let shift = u32::from(bit_depth) - 8;
        let max = ((1u32 << bit_depth) - 1) as f64;
        let cen = (128u32 << shift) as f64;
        let (y_off, y_span, uv_span) = match range {
            YuvRange::Full => (0.0, max, max),
            YuvRange::Limited => (
                (16u32 << shift) as f64,
                ((219u32 << shift) as f64),
                ((224u32 << shift) as f64),
            ),
        };
        let vr = 2.0 * (1.0 - kr);
        let ug = -2.0 * kb * (1.0 - kb) / kg;
        let vg = -2.0 * kr * (1.0 - kr) / kg;
        let ub = 2.0 * (1.0 - kb);
        let coef = [(0.0, vr), (ug, vg), (ub, 0.0)];
        let ay = (65536.0 * max / y_span).round() as i32;
        let mut bu = [0i32; 3];
        let mut bv = [0i32; 3];
        let mut off = [0i32; 3];
        // Offsets derive from the ROUNDED chroma coefficients so neutral
        // chroma (U4 = V4 = cen·16) cancels EXACTLY per channel: gray in,
        // gray out (R = G = B), and mono == planar-gray identically.
        let y_term_off = (65536.0 * (-y_off * max / y_span)).round() as i64;
        let cen16 = cen as i64 * 16;
        for (c, &(cu, cv)) in coef.iter().enumerate() {
            bu[c] = (4096.0 * cu * max / uv_span).round() as i32;
            bv[c] = (4096.0 * cv * max / uv_span).round() as i32;
            let off64 = y_term_off - bu[c] as i64 * cen16 - bv[c] as i64 * cen16 + (1i64 << 15);
            debug_assert!(i32::try_from(off64).is_ok());
            off[c] = off64 as i32;
            // i32 accumulator headroom proof for this constant set: the
            // worst |sum| over the whole input domain must clear 2^31.
            let worst = (ay as i64) * ((max as i64) + 1)
                + bu[c].unsigned_abs() as i64 * ((max as i64 + 1) * 16)
                + bv[c].unsigned_abs() as i64 * ((max as i64 + 1) * 16)
                + off[c].unsigned_abs() as i64;
            debug_assert!(worst < (1i64 << 31), "i32 overflow risk at d{bit_depth}");
        }
        Self {
            ay,
            bu,
            bv,
            off,
            max: max as i32,
            cen16: cen16 as i32,
        }
    }

    /// One sample through the canonical formula. `u4`/`v4` are chroma in
    /// 1/16 units (exact integers from the interpolation stage).
    #[inline(always)]
    fn convert(&self, y: i32, u4: i32, v4: i32) -> (u16, u16, u16) {
        let base = self.ay * y;
        let r = (base + self.bv[0] * v4 + self.off[0]) >> 16;
        let g = (base + self.bu[1] * u4 + self.bv[1] * v4 + self.off[1]) >> 16;
        let b = (base + self.bu[2] * u4 + self.off[2]) >> 16;
        (
            r.clamp(0, self.max) as u16,
            g.clamp(0, self.max) as u16,
            b.clamp(0, self.max) as u16,
        )
    }
}

/// Horizontally interpolate one chroma row from `cw` samples (in `in_scale`
/// = 1 or 4 sixteenths-units) to `width` positions in 1/16 units, weights
/// {3,1}/4 relative — pure integer, exact. `in_scale`: pass 4 for a
/// vertically pre-lerped /4-unit row (4:2:0) so weights (3,1) land at /16;
/// pass 1 with weights (12,4) semantics for a raw row (4:2:2) via the
/// `raw` flag.
#[inline(always)]
fn interp_row_h_int(src: &[u16], width: usize, raw: bool, dst: &mut [u16]) {
    let cw = width.div_ceil(2);
    let src = &src[..cw];
    let dst = &mut dst[..width];
    // Weights per position (module docs): odd x=2k+1 -> (3,1)·(c[k],c[k+1]),
    // even x=2k (k>=1) -> (1,3)·(c[k-1],c[k]); edges replicate. `raw`
    // inputs are 1-unit samples so weights scale by 4 to land at /16.
    let (wa, wb, we) = if raw { (12, 4, 16) } else { (3, 1, 4) };
    dst[0] = we * src[0];
    for k in 0..cw.saturating_sub(1) {
        dst[2 * k + 1] = wa * src[k] + wb * src[k + 1];
    }
    if width.is_multiple_of(2) {
        dst[width - 1] = we * src[cw - 1];
    }
    for k in 1..cw {
        if 2 * k < width {
            dst[2 * k] = wb * src[k - 1] + wa * src[k];
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
    let chroma_width = width.div_ceil(2);
    let chroma_height = total_height.div_ceil(2);

    if bit_depth <= 12 {
        // Fixed-point canonical path (module docs).
        let c = FixedConsts::new(YuvMatrixKrKb { kr, kb }, range, bit_depth);
        let mut uv_mid = vec![0u16; 2 * chroma_width];
        let mut uf = vec![0u16; width];
        let mut vf = vec![0u16; width];
        for row in 0..strip_height {
            let y_pos = y_start + row;
            let chroma_y_raw = (y_pos as f32 + 0.5) * 0.5 - 0.5;
            let chroma_y = chroma_y_raw.max(0.0).min(chroma_height as f32 - 1.0);
            let (cy0, fy) = floor_nonneg_idx(chroma_y);
            let cy1 = (cy0 + 1).min(chroma_height - 1);
            // fy is exactly 0, 0.25 or 0.75 — dyadic; /4-unit weights.
            let wy = (fy * 4.0) as u16;
            let wy1 = 4 - wy;
            {
                let (u_mid, v_mid) = uv_mid.split_at_mut(chroma_width);
                let u0 = &u_plane[cy0 * u_stride..][..chroma_width];
                let u1 = &u_plane[cy1 * u_stride..][..chroma_width];
                let v0 = &v_plane[cy0 * v_stride..][..chroma_width];
                let v1 = &v_plane[cy1 * v_stride..][..chroma_width];
                for k in 0..chroma_width {
                    u_mid[k] = wy1 * (u0[k].to_i32() as u16) + wy * (u1[k].to_i32() as u16);
                    v_mid[k] = wy1 * (v0[k].to_i32() as u16) + wy * (v1[k].to_i32() as u16);
                }
                interp_row_h_int(u_mid, width, false, &mut uf);
                interp_row_h_int(v_mid, width, false, &mut vf);
            }
            let y_row = &y_plane[y_pos * y_stride..][..width];
            let out_row = &mut out[row * width..][..width];
            for x in 0..width {
                let (r, g, b) = c.convert(y_row[x].to_i32(), uf[x] as i32, vf[x] as i32);
                out_row[x] = P::from_rgb16(r, g, b, c.max as u16);
            }
        }
        return;
    }

    // f32 recipe — the 16-bit API entries only (AV1 codes 8/10/12).
    let c = RecipeConsts::new(YuvMatrixKrKb { kr, kb }, range, bit_depth);

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
    let chroma_width = width.div_ceil(2);

    if bit_depth <= 12 {
        // Fixed-point canonical path (module docs).
        let c = FixedConsts::new(YuvMatrixKrKb { kr, kb }, range, bit_depth);
        let mut uv_mid = vec![0u16; 2 * chroma_width];
        let mut uf = vec![0u16; width];
        let mut vf = vec![0u16; width];
        for row in 0..strip_height {
            let y_pos = y_start + row;
            {
                let (u_mid, v_mid) = uv_mid.split_at_mut(chroma_width);
                let u_src = &u_plane[y_pos * u_stride..][..chroma_width];
                let v_src = &v_plane[y_pos * v_stride..][..chroma_width];
                for k in 0..chroma_width {
                    u_mid[k] = u_src[k].to_i32() as u16;
                    v_mid[k] = v_src[k].to_i32() as u16;
                }
                interp_row_h_int(u_mid, width, true, &mut uf);
                interp_row_h_int(v_mid, width, true, &mut vf);
            }
            let y_row = &y_plane[y_pos * y_stride..][..width];
            let out_row = &mut out[row * width..][..width];
            for x in 0..width {
                let (r, g, b) = c.convert(y_row[x].to_i32(), uf[x] as i32, vf[x] as i32);
                out_row[x] = P::from_rgb16(r, g, b, c.max as u16);
            }
        }
        return;
    }

    let c = RecipeConsts::new(YuvMatrixKrKb { kr, kb }, range, bit_depth);
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

    if bit_depth <= 12 {
        // Fixed-point canonical path: chroma scales straight to 1/16 units.
        let c = FixedConsts::new(YuvMatrixKrKb { kr, kb }, range, bit_depth);
        for row in 0..strip_height {
            let y_pos = y_start + row;
            let y_row = &y_plane[y_pos * y_stride..][..width];
            let u_row = &u_plane[y_pos * u_stride..][..width];
            let v_row = &v_plane[y_pos * v_stride..][..width];
            let out_row = &mut out[row * width..][..width];
            for x in 0..width {
                let (r, g, b) = c.convert(
                    y_row[x].to_i32(),
                    u_row[x].to_i32() * 16,
                    v_row[x].to_i32() * 16,
                );
                out_row[x] = P::from_rgb16(r, g, b, c.max as u16);
            }
        }
        return;
    }

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
    if bit_depth <= 12 {
        // Fixed-point canonical path: the formula with neutral chroma
        // (mono == planar gray, exactly).
        let c = FixedConsts::new(
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
                let (r, g, b) = c.convert(y_row[x].to_i32(), c.cen16, c.cen16);
                out_row[x] = P::from_rgb16(r, g, b, c.max as u16);
            }
        }
        return;
    }

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
                    .clamp(0.0, 255.0)
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
                .clamp(0.0, 255.0)
                .round_ties_even() as u8;
            v_out[k] = va
                .mul_add(c.uv_span, 128.0)
                .clamp(0.0, 255.0)
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

    /// Independent scalar evaluation of the canonical fixed-point formula
    /// in i64 (the kernels use i32 — the width difference is part of what
    /// makes this reference independent; the constants are shared because
    /// they ARE the recipe).
    fn ref_convert_fixed(y: i64, u4: i64, v4: i64, c: &FixedConsts) -> (u16, u16, u16) {
        let ch = |bu: i32, bv: i32, off: i32| -> u16 {
            let v = (c.ay as i64 * y + bu as i64 * u4 + bv as i64 * v4 + off as i64) >> 16;
            v.clamp(0, c.max as i64) as u16
        };
        (
            ch(c.bu[0], c.bv[0], c.off[0]),
            ch(c.bu[1], c.bv[1], c.off[1]),
            ch(c.bu[2], c.bv[2], c.off[2]),
        )
    }

    /// Direct (non-separable) 4-term bilinear chroma in 1/16 units for the
    /// reference: weights 16·(fx?,fy?) are the exact {9,3,3,1}/{12,4}/{16}
    /// integer patterns.
    #[allow(clippy::too_many_arguments)]
    fn ref_chroma_u4(
        plane: &[u16],
        stride: usize,
        cx0: usize,
        cx1: usize,
        cy0: usize,
        cy1: usize,
        fx: f32,
        fy: f32,
    ) -> i64 {
        let wx = (fx * 4.0) as i64;
        let wy = (fy * 4.0) as i64;
        let (wx1, wy1) = (4 - wx, 4 - wy);
        wx1 * wy1 * plane[cy0 * stride + cx0] as i64
            + wx * wy1 * plane[cy0 * stride + cx1] as i64
            + wx1 * wy * plane[cy1 * stride + cx0] as i64
            + wx * wy * plane[cy1 * stride + cx1] as i64
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
        let c = FixedConsts::new(YuvMatrixKrKb { kr, kb }, range, 8);
        let chroma_width = width.div_ceil(2);
        let chroma_height = height.div_ceil(2);
        let u16p: Vec<u16> = u_plane.iter().map(|&v| v as u16).collect();
        let v16p: Vec<u16> = v_plane.iter().map(|&v| v as u16).collect();
        let mut out = vec![RGB8::default(); width * height];
        for y_pos in 0..height {
            let chroma_y = ((y_pos as f32 + 0.5) * 0.5 - 0.5)
                .max(0.0)
                .min(chroma_height as f32 - 1.0);
            let cy0 = chroma_y.floor() as usize;
            let cy1 = (cy0 + 1).min(chroma_height - 1);
            let fy = chroma_y - cy0 as f32;
            for x in 0..width {
                let chroma_x = ((x as f32 + 0.5) * 0.5 - 0.5)
                    .max(0.0)
                    .min(chroma_width as f32 - 1.0);
                let cx0 = chroma_x.floor() as usize;
                let cx1 = (cx0 + 1).min(chroma_width - 1);
                let fx = chroma_x - cx0 as f32;
                let u4 = ref_chroma_u4(&u16p, u_stride, cx0, cx1, cy0, cy1, fx, fy);
                let v4 = ref_chroma_u4(&v16p, v_stride, cx0, cx1, cy0, cy1, fx, fy);
                let (r, g, b) = ref_convert_fixed(y_plane[y_pos * y_stride + x] as i64, u4, v4, &c);
                out[y_pos * width + x] = RGB8 {
                    r: r as u8,
                    g: g as u8,
                    b: b as u8,
                };
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
        let c = FixedConsts::new(YuvMatrixKrKb { kr, kb }, range, 8);
        let chroma_width = width.div_ceil(2);
        let u16p: Vec<u16> = u_plane.iter().map(|&v| v as u16).collect();
        let v16p: Vec<u16> = v_plane.iter().map(|&v| v as u16).collect();
        let mut out = vec![RGB8::default(); width * height];
        for y_pos in 0..height {
            for x in 0..width {
                let chroma_x = ((x as f32 + 0.5) * 0.5 - 0.5)
                    .max(0.0)
                    .min(chroma_width as f32 - 1.0);
                let cx0 = chroma_x.floor() as usize;
                let cx1 = (cx0 + 1).min(chroma_width - 1);
                let fx = chroma_x - cx0 as f32;
                // 4:2:2: chroma row == luma row (fy = 0).
                let u4 = ref_chroma_u4(&u16p, u_stride, cx0, cx1, y_pos, y_pos, fx, 0.0);
                let v4 = ref_chroma_u4(&v16p, v_stride, cx0, cx1, y_pos, y_pos, fx, 0.0);
                let (r, g, b) = ref_convert_fixed(y_plane[y_pos * y_stride + x] as i64, u4, v4, &c);
                out[y_pos * width + x] = RGB8 {
                    r: r as u8,
                    g: g as u8,
                    b: b as u8,
                };
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
        let c = FixedConsts::new(YuvMatrixKrKb { kr, kb }, range, 8);
        let mut out = vec![RGB8::default(); width * height];
        for y in 0..height {
            for x in 0..width {
                let (r, g, b) = ref_convert_fixed(
                    y_plane[y * y_stride + x] as i64,
                    u_plane[y * u_stride + x] as i64 * 16,
                    v_plane[y * v_stride + x] as i64 * 16,
                    &c,
                );
                out[y * width + x] = RGB8 {
                    r: r as u8,
                    g: g as u8,
                    b: b as u8,
                };
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
        let cf =
            (bit_depth <= 12).then(|| FixedConsts::new(YuvMatrixKrKb { kr, kb }, range, bit_depth));
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
                out[y * width + x] = if let Some(cf) = &cf {
                    let u4 = ref_chroma_u4(u_plane, cw, cx0, cx1, cy0, cy1, fx, fy);
                    let v4 = ref_chroma_u4(v_plane, cw, cx0, cx1, cy0, cy1, fx, fy);
                    let (r, g, b) = ref_convert_fixed(y_plane[y * width + x] as i64, u4, v4, cf);
                    rgb::Rgb { r, g, b }
                } else {
                    let (r, g, b) = convert_one(y_plane[y * width + x] as f32, u_val, v_val, &c);
                    rgb::Rgb {
                        r: r as u16,
                        g: g as u16,
                        b: b as u16,
                    }
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
                let cf = (bit_depth <= 12).then(|| {
                    FixedConsts::new(
                        YuvMatrixKrKb {
                            kr: 0.299,
                            kb: 0.114,
                        },
                        range,
                        bit_depth,
                    )
                });
                let expect = |yv: f32| -> (u16, u16, u16) {
                    if let Some(cf) = &cf {
                        ref_convert_fixed(yv as i64, cf.cen16 as i64, cf.cen16 as i64, cf)
                    } else {
                        let (r, g, b) = convert_one(yv, c.uv_cen, c.uv_cen, &c);
                        (r as u16, g as u16, b as u16)
                    }
                };
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
                        let (r, g, b) = expect(yv as f32);
                        assert_eq!(
                            (px.r, px.g, px.b, px.a),
                            (r, g, b, max as u16),
                            "d{bit_depth} {range:?} px {i} (y={yv})"
                        );
                    }
                } else {
                    let mut y = vec![0u8; w * h];
                    fill_rand(&mut y, 77);
                    let mut out = vec![Rgba::<u8>::default(); w * h];
                    yuv400_to_rgbx_strip::<u8, Rgba<u8>>(&y, w, w, 0, h, range, 8, &mut out);
                    for (i, (&yv, px)) in y.iter().zip(out.iter()).enumerate() {
                        let (r, g, b) = expect(yv as f32);
                        assert_eq!(
                            (px.r as u16, px.g as u16, px.b as u16, px.a),
                            (r, g, b, 255),
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
                        .clamp(0.0, 255.0)
                        .round_ties_even() as u8;
                    let vref = va
                        .mul_add(c.uv_span, 128.0)
                        .clamp(0.0, 255.0)
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

    /// The fixed-point formula must stay within ±1 of exact rational
    /// conversion (f64 single rounding) everywhere — the 2^-16 constant
    /// quantization can shift a rounding boundary but never move a value.
    #[test]
    fn fixed_formula_within_one_of_exact() {
        for &bit_depth in &[8u8, 10, 12] {
            let max = ((1u32 << bit_depth) - 1) as i64;
            let shift = u32::from(bit_depth) - 8;
            for &range in &[YuvRange::Full, YuvRange::Limited] {
                for &(kr, kb) in &[(0.299f64, 0.114f64), (0.2126, 0.0722), (0.2627, 0.0593)] {
                    let c = FixedConsts::new(
                        YuvMatrixKrKb {
                            kr: kr as f32,
                            kb: kb as f32,
                        },
                        range,
                        bit_depth,
                    );
                    let kg = 1.0 - kr - kb;
                    let cen = f64::from(128u32 << shift);
                    let (y_off, y_span, uv_span) = match range {
                        YuvRange::Full => (0.0, max as f64, max as f64),
                        YuvRange::Limited => (
                            f64::from(16u32 << shift),
                            f64::from(219u32 << shift),
                            f64::from(224u32 << shift),
                        ),
                    };
                    let coef = [
                        (0.0, 2.0 * (1.0 - kr)),
                        (-2.0 * kb * (1.0 - kb) / kg, -2.0 * kr * (1.0 - kr) / kg),
                        (2.0 * (1.0 - kb), 0.0),
                    ];
                    // Deterministic sample sweep incl. domain corners.
                    let mut s = 0x2545F491u32;
                    for i in 0..40_000 {
                        let (y, u4, v4) = if i < 8 {
                            let m16 = max * 16;
                            [
                                (0, 0, 0),
                                (max, m16, m16),
                                (0, m16, 0),
                                (max, 0, m16),
                                (max / 2, m16 / 2, m16 / 2),
                                (0, m16, m16),
                                (max, 0, 0),
                                (max / 2, 0, m16),
                            ][i]
                        } else {
                            s ^= s << 13;
                            s ^= s >> 17;
                            s ^= s << 5;
                            let y = (s as i64) % (max + 1);
                            s ^= s << 13;
                            s ^= s >> 17;
                            s ^= s << 5;
                            let u4 = (s as i64) % (max * 16 + 1);
                            s ^= s << 13;
                            s ^= s >> 17;
                            s ^= s << 5;
                            (y, u4, (s as i64) % (max * 16 + 1))
                        };
                        let got = ref_convert_fixed(y, u4, v4, &c);
                        for (ci, &(cu, cv)) in coef.iter().enumerate() {
                            let exact = (y as f64 - y_off) * (max as f64) / y_span
                                + (u4 as f64 / 16.0 - cen) * cu * (max as f64) / uv_span
                                + (v4 as f64 / 16.0 - cen) * cv * (max as f64) / uv_span;
                            let exact = exact.round().clamp(0.0, max as f64) as i64;
                            let g = [got.0, got.1, got.2][ci] as i64;
                            assert!(
                                (g - exact).abs() <= 1,
                                "d{bit_depth} {range:?} kr{kr} ch{ci} y{y} u{u4} v{v4}: \
                                 formula {g} vs exact {exact}"
                            );
                        }
                    }
                }
            }
        }
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

    // ═══════════════════ cross-tier (SIMD-tier) byte identity ═════════════
    //
    // Every test above runs at exactly ONE tier: the best one `incant!`
    // dispatches to on the host (NEON on aarch64, AVX2/AVX-512 on x86-64).
    // The `_scalar` copies of all five kernels — the code that runs on every
    // target without a SIMD tier, and the fallback the module docs promise is
    // byte-identical — measured **0 executions** across the whole feature
    // matrix (cargo-llvm-cov, 2026-08-11; docs/TEST_COVERAGE.md). The module
    // header claims "every tier, lane width, and window produces
    // byte-identical output"; nothing checked it.
    //
    // These tests re-run the whole conversion battery once per token
    // permutation (archmage disables tokens process-wide) and require every
    // tier to agree BYTE-FOR-BYTE with the host's best tier.

    /// One battery of conversions -> a flat byte vector, so any tier
    /// difference anywhere shows up as a single comparison failure. Covers
    /// all three chroma samplings x {RGB, RGBA} x {8-bit, 10/12/16-bit},
    /// monochrome at every depth incl. native Gray output, and the forward
    /// (encode-side) RGB8/RGBA8 -> YUV420 kernel.
    fn tier_battery() -> Vec<u8> {
        fn push16(v: &[u16], sink: &mut Vec<u8>) {
            for &x in v {
                sink.extend_from_slice(&x.to_le_bytes());
            }
        }
        /// Deterministic u16 plane, masked to the depth (`max + 1` overflows
        /// u16 at depth 16, so mask instead of modulo).
        fn rand16(seed: u32, n: usize, bit_depth: u8) -> Vec<u16> {
            let mask = ((1u32 << bit_depth) - 1) as u16;
            let mut s = seed | 1;
            (0..n)
                .map(|_| {
                    s ^= s << 13;
                    s ^= s >> 17;
                    s ^= s << 5;
                    (s as u16) & mask
                })
                .collect()
        }

        let mut sink: Vec<u8> = Vec::new();
        // Odd + even dims, sub-lane and multi-lane widths, plus a strip
        // window that does not start at row 0 (the 4:2:0 vertical
        // interpolation window differs there).
        let cases: [(usize, usize, usize, usize); 5] = [
            (1, 1, 0, 1),
            (3, 3, 0, 3),
            (9, 7, 0, 7),
            (17, 13, 4, 5),
            (32, 16, 0, 16),
        ];
        for &(w, h, y_start, sh) in &cases {
            let cw = w.div_ceil(2);
            let ch = h.div_ceil(2);
            let mut y8 = vec![0u8; w * h];
            fill_rand(&mut y8, 0x2468 + w as u32);
            // Chroma planes sized per sampling: 4:2:0 is (cw x ch), 4:2:2 is
            // (cw x h), 4:4:4 is (w x h).
            let mut u420 = vec![0u8; cw * ch];
            let mut v420 = vec![0u8; cw * ch];
            let mut u422 = vec![0u8; cw * h];
            let mut v422 = vec![0u8; cw * h];
            let mut u444 = vec![0u8; w * h];
            let mut v444 = vec![0u8; w * h];
            fill_rand(&mut u420, 0x1357 + h as u32);
            fill_rand(&mut v420, 0xBEEF + h as u32);
            fill_rand(&mut u422, 0x2468 + h as u32);
            fill_rand(&mut v422, 0x1BAD + h as u32);
            fill_rand(&mut u444, 0x0F0F + h as u32);
            fill_rand(&mut v444, 0x7070 + h as u32);
            for &range in &[YuvRange::Limited, YuvRange::Full] {
                for &matrix in &[YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
                    let mut o = vec![RGB8::default(); w * sh];
                    let mut oa = vec![Rgba::<u8>::default(); w * sh];
                    yuv420_to_rgb8_strip(
                        &y8, w, &u420, cw, &v420, cw, w, h, y_start, sh, range, matrix, &mut o,
                    );
                    sink.extend(o.iter().flat_map(|p| [p.r, p.g, p.b]));
                    yuv420_to_rgba8_strip(
                        &y8, w, &u420, cw, &v420, cw, w, h, y_start, sh, range, matrix, &mut oa,
                    );
                    sink.extend(oa.iter().flat_map(|p| [p.r, p.g, p.b, p.a]));
                    yuv422_to_rgb8_strip(
                        &y8, w, &u422, cw, &v422, cw, w, y_start, sh, range, matrix, &mut o,
                    );
                    sink.extend(o.iter().flat_map(|p| [p.r, p.g, p.b]));
                    yuv422_to_rgba8_strip(
                        &y8, w, &u422, cw, &v422, cw, w, y_start, sh, range, matrix, &mut oa,
                    );
                    sink.extend(oa.iter().flat_map(|p| [p.r, p.g, p.b, p.a]));
                    yuv444_to_rgb8_strip(
                        &y8, w, &u444, w, &v444, w, w, y_start, sh, range, matrix, &mut o,
                    );
                    sink.extend(o.iter().flat_map(|p| [p.r, p.g, p.b]));
                    yuv444_to_rgba8_strip(
                        &y8, w, &u444, w, &v444, w, w, y_start, sh, range, matrix, &mut oa,
                    );
                    sink.extend(oa.iter().flat_map(|p| [p.r, p.g, p.b, p.a]));
                    // 16-bit native depth: every AV1 depth above 8 plus the
                    // 16-bit arm (which takes the f32 recipe, not the
                    // fixed-point one), all three samplings, both pixel shapes.
                    for &bit_depth in &[10u8, 12, 16] {
                        let y16 = rand16(0x9E37 + u32::from(bit_depth), w * h, bit_depth);
                        for sampling in [
                            ChromaSubsampling::Cs420,
                            ChromaSubsampling::Cs422,
                            ChromaSubsampling::Cs444,
                        ] {
                            let (cstride, crows) = match sampling {
                                ChromaSubsampling::Cs420 => (cw, ch),
                                ChromaSubsampling::Cs422 => (cw, h),
                                ChromaSubsampling::Cs444 => (w, h),
                            };
                            let u16p =
                                rand16(0x85EB + u32::from(bit_depth), cstride * crows, bit_depth);
                            let v16p =
                                rand16(0xC2B2 + u32::from(bit_depth), cstride * crows, bit_depth);
                            let mut o16 = vec![rgb::Rgb::<u16>::default(); w * sh];
                            yuv16_to_rgbx_strip::<rgb::Rgb<u16>>(
                                sampling, &y16, w, &u16p, cstride, &v16p, cstride, w, h, y_start,
                                sh, range, matrix, bit_depth, &mut o16,
                            );
                            push16(
                                &o16.iter().flat_map(|p| [p.r, p.g, p.b]).collect::<Vec<_>>(),
                                &mut sink,
                            );
                            let mut oa16 = vec![Rgba::<u16>::default(); w * sh];
                            yuv16_to_rgbx_strip::<Rgba<u16>>(
                                sampling, &y16, w, &u16p, cstride, &v16p, cstride, w, h, y_start,
                                sh, range, matrix, bit_depth, &mut oa16,
                            );
                            push16(
                                &oa16
                                    .iter()
                                    .flat_map(|p| [p.r, p.g, p.b, p.a])
                                    .collect::<Vec<_>>(),
                                &mut sink,
                            );
                        }
                    }
                    // 8-bit samples carried in u16 planes with NARROW (8-bit)
                    // output: the aom-backend shape (`aom-decode` hands back
                    // u16 planes at every depth, and `wide_out = bit_depth > 8`
                    // keeps the output 8-bit at depth 8 —
                    // src/decoder_managed/aom.rs). Same pixels as the u8
                    // kernels by construction; asserted directly in
                    // `u16_planes_of_8bit_samples_match_the_u8_kernels`.
                    let y16_8: Vec<u16> = y8.iter().map(|&v| u16::from(v)).collect();
                    for sampling in [
                        ChromaSubsampling::Cs420,
                        ChromaSubsampling::Cs422,
                        ChromaSubsampling::Cs444,
                    ] {
                        let (src, cstride) = match sampling {
                            ChromaSubsampling::Cs420 => ((&u420, &v420), cw),
                            ChromaSubsampling::Cs422 => ((&u422, &v422), cw),
                            ChromaSubsampling::Cs444 => ((&u444, &v444), w),
                        };
                        let u16p: Vec<u16> = src.0.iter().map(|&v| u16::from(v)).collect();
                        let v16p: Vec<u16> = src.1.iter().map(|&v| u16::from(v)).collect();
                        let mut n8 = vec![RGB8::default(); w * sh];
                        yuv16_to_rgbx_strip::<RGB8>(
                            sampling, &y16_8, w, &u16p, cstride, &v16p, cstride, w, h, y_start, sh,
                            range, matrix, 8, &mut n8,
                        );
                        sink.extend(n8.iter().flat_map(|p| [p.r, p.g, p.b]));
                        let mut n8a = vec![Rgba::<u8>::default(); w * sh];
                        yuv16_to_rgbx_strip::<Rgba<u8>>(
                            sampling, &y16_8, w, &u16p, cstride, &v16p, cstride, w, h, y_start, sh,
                            range, matrix, 8, &mut n8a,
                        );
                        sink.extend(n8a.iter().flat_map(|p| [p.r, p.g, p.b, p.a]));
                    }
                }
                // Monochrome: u8 at depth 8 and u16 at 10/12/16, into RGB,
                // RGBA and the native Gray outputs the decoder uses.
                let mut g8 = vec![rgb::Gray::<u8>::new(0); w * sh];
                yuv400_to_rgbx_strip::<u8, rgb::Gray<u8>>(
                    &y8, w, w, y_start, sh, range, 8, &mut g8,
                );
                sink.extend(g8.iter().map(|p| p.value()));
                let mut m8 = vec![Rgba::<u8>::default(); w * sh];
                yuv400_to_rgbx_strip::<u8, Rgba<u8>>(&y8, w, w, y_start, sh, range, 8, &mut m8);
                sink.extend(m8.iter().flat_map(|p| [p.r, p.g, p.b, p.a]));
                let mut m8rgb = vec![RGB8::default(); w * sh];
                yuv400_to_rgbx_strip::<u8, RGB8>(&y8, w, w, y_start, sh, range, 8, &mut m8rgb);
                sink.extend(m8rgb.iter().flat_map(|p| [p.r, p.g, p.b]));
                // Monochrome, 8-bit samples in u16 planes + narrow output
                // (the aom-backend `mono!` shapes, aom.rs:337).
                let y16_8: Vec<u16> = y8.iter().map(|&v| u16::from(v)).collect();
                let mut ng = vec![rgb::Gray::<u8>::new(0); w * sh];
                yuv400_to_rgbx_strip::<u16, rgb::Gray<u8>>(
                    &y16_8, w, w, y_start, sh, range, 8, &mut ng,
                );
                sink.extend(ng.iter().map(|p| p.value()));
                let mut nga = vec![Rgba::<u8>::default(); w * sh];
                yuv400_to_rgbx_strip::<u16, Rgba<u8>>(
                    &y16_8, w, w, y_start, sh, range, 8, &mut nga,
                );
                sink.extend(nga.iter().flat_map(|p| [p.r, p.g, p.b, p.a]));
                let mut ngr = vec![RGB8::default(); w * sh];
                yuv400_to_rgbx_strip::<u16, RGB8>(&y16_8, w, w, y_start, sh, range, 8, &mut ngr);
                sink.extend(ngr.iter().flat_map(|p| [p.r, p.g, p.b]));
                for &bit_depth in &[10u8, 12, 16] {
                    let y16 = rand16(0x1234 + u32::from(bit_depth), w * h, bit_depth);
                    let mut g16 = vec![rgb::Gray::<u16>::new(0); w * sh];
                    yuv400_to_rgbx_strip::<u16, rgb::Gray<u16>>(
                        &y16, w, w, y_start, sh, range, bit_depth, &mut g16,
                    );
                    push16(
                        &g16.iter().map(|p| p.value()).collect::<Vec<_>>(),
                        &mut sink,
                    );
                    let mut m16 = vec![Rgba::<u16>::default(); w * sh];
                    yuv400_to_rgbx_strip::<u16, Rgba<u16>>(
                        &y16, w, w, y_start, sh, range, bit_depth, &mut m16,
                    );
                    push16(
                        &m16.iter()
                            .flat_map(|p| [p.r, p.g, p.b, p.a])
                            .collect::<Vec<_>>(),
                        &mut sink,
                    );
                    let mut m16rgb = vec![rgb::Rgb::<u16>::default(); w * sh];
                    yuv400_to_rgbx_strip::<u16, rgb::Rgb<u16>>(
                        &y16,
                        w,
                        w,
                        y_start,
                        sh,
                        range,
                        bit_depth,
                        &mut m16rgb,
                    );
                    push16(
                        &m16rgb
                            .iter()
                            .flat_map(|p| [p.r, p.g, p.b])
                            .collect::<Vec<_>>(),
                        &mut sink,
                    );
                }
                // Forward (encode-side) kernel, both input pixel shapes.
                let img: Vec<RGB8> = (0..w * h)
                    .map(|i| RGB8 {
                        r: y8[i],
                        g: u444[i],
                        b: v444[i],
                    })
                    .collect();
                let (mut yp, mut up, mut vp) =
                    (vec![0u8; w * h], vec![0u8; cw * ch], vec![0u8; cw * ch]);
                rgb8_to_yuv420(
                    &img,
                    w,
                    w,
                    h,
                    range,
                    YuvMatrix::Bt709,
                    &mut yp,
                    &mut up,
                    &mut vp,
                );
                sink.extend_from_slice(&yp);
                sink.extend_from_slice(&up);
                sink.extend_from_slice(&vp);
                let imga: Vec<Rgba<u8>> = img
                    .iter()
                    .map(|p| Rgba {
                        r: p.r,
                        g: p.g,
                        b: p.b,
                        a: 255,
                    })
                    .collect();
                rgba8_to_yuv420(
                    &imga,
                    w,
                    w,
                    h,
                    range,
                    YuvMatrix::Bt709,
                    &mut yp,
                    &mut up,
                    &mut vp,
                );
                sink.extend_from_slice(&yp);
                sink.extend_from_slice(&up);
                sink.extend_from_slice(&vp);
            }
        }
        sink
    }

    /// 8-bit samples carried in u16 planes must convert to EXACTLY the same
    /// pixels as the u8 planes.
    ///
    /// This is not a hypothetical: `aom-decode` returns u16 planes at every
    /// bit depth, so the `aom-backend` decode path feeds the `S = u16`
    /// kernel instantiations with 8-bit values and asks for 8-bit output
    /// (`wide_out = bit_depth > 8`, src/decoder_managed/aom.rs:222), while
    /// rav1d-safe's 8-bit path feeds the `S = u8` ones. Two implementations
    /// of one conversion — and the backends are required to agree
    /// byte-for-byte (tests/cross_backend_decode.rs asserts that on corpus
    /// files; this asserts it on the kernels directly, where a divergence is
    /// attributable).
    #[test]
    fn u16_planes_of_8bit_samples_match_the_u8_kernels() {
        for &(w, h) in &[(1usize, 1usize), (3, 3), (9, 7), (16, 8), (17, 13)] {
            let cw = w.div_ceil(2);
            let ch = h.div_ceil(2);
            let mut y8 = vec![0u8; w * h];
            let mut u8p = vec![0u8; w * h];
            let mut v8p = vec![0u8; w * h];
            fill_rand(&mut y8, 0x51A7 + w as u32);
            fill_rand(&mut u8p, 0x2C9F + h as u32);
            fill_rand(&mut v8p, 0x77E1 + (w * h) as u32);
            let y16: Vec<u16> = y8.iter().map(|&v| u16::from(v)).collect();
            let u16p: Vec<u16> = u8p.iter().map(|&v| u16::from(v)).collect();
            let v16p: Vec<u16> = v8p.iter().map(|&v| u16::from(v)).collect();
            for &range in &[YuvRange::Limited, YuvRange::Full] {
                for &matrix in &[YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
                    for sampling in [
                        ChromaSubsampling::Cs420,
                        ChromaSubsampling::Cs422,
                        ChromaSubsampling::Cs444,
                    ] {
                        // Chroma extent per sampling; the planes above are
                        // allocated at full size so every case fits.
                        let cstride = match sampling {
                            ChromaSubsampling::Cs444 => w,
                            _ => cw,
                        };
                        let mut narrow = vec![RGB8::default(); w * h];
                        let mut wide_src = vec![RGB8::default(); w * h];
                        match sampling {
                            ChromaSubsampling::Cs420 => yuv420_to_rgb8_strip(
                                &y8,
                                w,
                                &u8p,
                                cstride,
                                &v8p,
                                cstride,
                                w,
                                h,
                                0,
                                h,
                                range,
                                matrix,
                                &mut narrow,
                            ),
                            ChromaSubsampling::Cs422 => yuv422_to_rgb8_strip(
                                &y8,
                                w,
                                &u8p,
                                cstride,
                                &v8p,
                                cstride,
                                w,
                                0,
                                h,
                                range,
                                matrix,
                                &mut narrow,
                            ),
                            ChromaSubsampling::Cs444 => yuv444_to_rgb8_strip(
                                &y8,
                                w,
                                &u8p,
                                cstride,
                                &v8p,
                                cstride,
                                w,
                                0,
                                h,
                                range,
                                matrix,
                                &mut narrow,
                            ),
                        }
                        yuv16_to_rgbx_strip::<RGB8>(
                            sampling,
                            &y16,
                            w,
                            &u16p,
                            cstride,
                            &v16p,
                            cstride,
                            w,
                            h,
                            0,
                            h,
                            range,
                            matrix,
                            8,
                            &mut wide_src,
                        );
                        assert_eq!(
                            narrow, wide_src,
                            "u8 vs u16-carried-8bit planes disagree: {w}x{h} {sampling:?} \
                             {range:?} {matrix:?}"
                        );
                    }
                    // Monochrome, both plane widths, into RGB8.
                    let mut m8 = vec![RGB8::default(); w * h];
                    let mut m16 = vec![RGB8::default(); w * h];
                    yuv400_to_rgbx_strip::<u8, RGB8>(&y8, w, w, 0, h, range, 8, &mut m8);
                    yuv400_to_rgbx_strip::<u16, RGB8>(&y16, w, w, 0, h, range, 8, &mut m16);
                    assert_eq!(
                        m8, m16,
                        "mono u8 vs u16-carried-8bit disagree: {w}x{h} {range:?}"
                    );
                    let _ = ch;
                }
            }
        }
    }

    /// Every SIMD tier — including the scalar fallback that no other test in
    /// this crate reaches — must produce byte-identical output to the host's
    /// best tier, for every sampling, depth, pixel shape and strip window.
    ///
    /// `CompileTimePolicy::Fail` is the liveness assert: it panics if a token
    /// the host has cannot be disabled (i.e. the archmage `testable_dispatch`
    /// dev-dependency feature went missing, or RUSTFLAGS pinned
    /// `-Ctarget-cpu`), which is exactly the condition under which this test
    /// would otherwise run the same tier N times and pass vacuously.
    ///
    /// # !! This test disables SIMD tokens PROCESS-WIDE !!
    ///
    /// `for_each_token_permutation` flips global disable flags, and libtest runs
    /// unit tests concurrently in ONE process. **So any other test in this crate
    /// that observes tier state must hold
    /// `archmage::testing::lock_token_testing()`** — the same mutex this call
    /// takes — or it will see tokens vanish mid-test. Two kinds qualify:
    ///
    /// 1. tests asserting a token is available
    ///    (`simd::avg::tests::test_avg_neon_direct_matches_scalar`);
    /// 2. tests comparing **third-party** SIMD numerics across calls
    ///    (`zensim_c::tests::streaming_and_direct_folded944_agree` — zensim
    ///    dispatches on archmage too, and promises no cross-tier bit identity).
    ///
    /// This crate's *own* kernels are exempt, because byte identity across tiers
    /// is precisely what this test pins. Both cases above were real CI failures
    /// (runs 31520483088 / 31527165388 / 31530942816) that read as platform bugs
    /// for a while; measured locally, 6 of 40 iterations diverge under
    /// concurrent permutation churn without the lock and 0 of 40 with it.
    ///
    /// # Why the arch gate
    ///
    /// Only `x86_64` and `aarch64` have more than one tier to compare.
    /// archmage's permutation table is built by `build_token_slots()`
    /// (archmage 0.9.15 `src/testing.rs:206-246`), which pushes token slots
    /// under `cfg(target_arch = "x86_64")` and `cfg(target_arch = "aarch64")`
    /// and nothing else; every other target compiles the generated
    /// `*_stubs.rs` token set whose `summon()` returns `None` unconditionally.
    /// So on **i686** — `target_arch = "x86"`, where the x86-64-v1..v4x tokens
    /// are stubs even though the CPU has SSE2 — the slot list is empty, the
    /// permutation count is exactly 1, and
    /// `incant!(…, [v4x, v4, v3, neon, wasm128, scalar])` has exactly one
    /// reachable arm: `scalar`. There is no second implementation on that
    /// target to cross-compare against, so the anti-vacuity assert below
    /// (correctly) refuses to report green. Those targets run
    /// [`single_tier_targets_dispatch_to_the_scalar_kernels`] instead, which
    /// pins the premise that makes the rest of this module's tests scalar-tier
    /// coverage there.
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn every_simd_tier_is_byte_identical() {
        let baseline = tier_battery();
        assert!(
            baseline.len() > 100_000,
            "battery produced only {} bytes — the case list stopped covering anything",
            baseline.len()
        );
        let report = archmage::testing::for_each_token_permutation(
            archmage::testing::CompileTimePolicy::Fail,
            |perm| {
                let got = tier_battery();
                assert_eq!(
                    got.len(),
                    baseline.len(),
                    "battery length changed at tier permutation [{perm}]"
                );
                if got != baseline {
                    let at = got
                        .iter()
                        .zip(baseline.iter())
                        .position(|(a, b)| a != b)
                        .unwrap_or(0);
                    panic!(
                        "SIMD tier divergence at permutation [{perm}]: first mismatch at battery \
                         byte {at} (got {}, best-tier {}) — two implementations of the same \
                         conversion disagree",
                        got[at], baseline[at]
                    );
                }
            },
        );
        // At least one permutation must have actually disabled something,
        // otherwise the loop above only re-ran the host's best tier.
        assert!(
            report.permutations_run >= 2,
            "only {} tier permutation(s) run ({report}) — no fallback tier was exercised",
            report.permutations_run
        );
    }

    /// The other half of the tier story, for targets whose archmage
    /// permutation table is empty (i686, wasm, riscv, ppc, …). There is no
    /// second tier there to cross-compare, so this asserts the *premise* that
    /// makes every other test in this module scalar-tier coverage on those
    /// targets: no SIMD token is reachable at all, therefore
    /// `incant!(…, [v4x, v4, v3, neon, wasm128, scalar])` can only select
    /// `scalar`, and the battery really does run through the scalar kernels.
    ///
    /// This is deliberately NOT a skip: it fails if the premise breaks. If a
    /// future archmage grows real 32-bit-x86 (or wasm, or riscv) token slots,
    /// `permutations_run` becomes ≥ 2 here and this test fails, which is the
    /// signal to widen [`every_simd_tier_is_byte_identical`]'s `cfg` to
    /// include the new architecture rather than leave it uncompared.
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    #[test]
    fn single_tier_targets_dispatch_to_the_scalar_kernels() {
        use archmage::SimdToken as _;

        // This test observes token state, so it takes the same lock the
        // permutation runs use — see the note on
        // `every_simd_tier_is_byte_identical`. Nothing on a single-tier target
        // disables tokens today, but the rule is the rule.
        let _tokens = archmage::testing::lock_token_testing();

        // Every tier named in the module's `incant!` lists, other than
        // `scalar`, must be unreachable — these are archmage's `*_stubs`
        // tokens on this architecture, whose `summon()` is `None` by
        // construction.
        for (name, available) in [
            ("v3", archmage::X64V3Token::summon().is_some()),
            ("v4", archmage::X64V4Token::summon().is_some()),
            ("v4x", archmage::X64V4xToken::summon().is_some()),
            ("neon", archmage::NeonToken::summon().is_some()),
            ("wasm128", archmage::Wasm128Token::summon().is_some()),
        ] {
            assert!(
                !available,
                "tier `{name}` summons on this target, so `incant!` does not \
                 fall through to `scalar` — this target now has more than one \
                 reachable tier and belongs in \
                 `every_simd_tier_is_byte_identical`'s cfg, cross-compared \
                 rather than assumed"
            );
        }

        // archmage agrees there is nothing to permute (see that test's docs
        // for why: its slot table is x86_64/aarch64 only).
        let report = archmage::testing::for_each_token_permutation(
            archmage::testing::CompileTimePolicy::Fail,
            |_| {},
        );
        assert_eq!(
            report.permutations_run, 1,
            "expected exactly one (empty) permutation on a single-tier target, \
             got {report}"
        );

        // And the scalar kernels genuinely execute here, on this target's own
        // pointer width, producing real output rather than a zero buffer.
        let battery = tier_battery();
        assert!(
            battery.len() > 100_000,
            "battery produced only {} bytes — the case list stopped covering \
             anything",
            battery.len()
        );
        assert!(
            battery.iter().any(|&b| b != 0),
            "the whole conversion battery came back all-zero from the scalar \
             kernels"
        );
    }
}
