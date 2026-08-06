//! Superblock pooling primitives shared by the diffmap-guided closed loops
//! ([`crate::two_pass`], [`crate::two_pass_zensim`]).
//!
//! Both drivers do the same three mechanical things with a full-resolution
//! per-pixel error map: walk the frame's 64×64 superblock grid, reduce each
//! block to one scalar, and turn the per-block scalars into a
//! geometric-mean-normalized, clamped, exponentiated AC quantizer scale map.
//! What they do NOT share is the *valuation* — what the per-block scalar
//! means and how it maps to a weight — because the two backends' maps live
//! in different domains:
//!
//! - butteraugli's map is a perceptual **distance**, and libaom's
//!   `tune=butteraugli` values a block by `mse / distance` (how much more
//!   visible the error is than the raw pixel difference implies);
//! - zensim's diffmap is unitless **SSIM error** with no absolute scale, so
//!   only *relative* quantities are meaningful (see
//!   [`crate::two_pass_zensim`] for the derivation).
//!
//! So the valuation stays in each driver and the mechanics live here.

/// Frame superblock size (AV1 128×128 superblocks still carry their
/// delta_q at the 64×64 granularity zenrav1e's `FrameHints` grid uses).
pub(crate) const SB: usize = 64;

/// Superblock grid dimensions for a frame of `w × h` pixels.
pub(crate) fn sb_grid(w: usize, h: usize) -> (usize, usize) {
    (w.div_ceil(SB), h.div_ceil(SB))
}

/// Per-superblock p-norm pool of a full-resolution error map.
///
/// `exp` is the p-norm exponent (libaom's butteraugli shim uses 12 — close
/// to max-pooling, so one bad 8×8 region dominates its superblock rather
/// than being averaged away by 63 clean ones). Negative map values are
/// clamped to 0 first: every map backend's contract is "higher = worse,
/// non-negative", and a signed research weighting could violate it.
///
/// The pool is **pixel-count normalized** — `(Σ vᵖ / n)^(1/p)`, not
/// libaom's raw `(Σ vᵖ)^(1/p)`. libaom can skip the `/n` because it only
/// ever pools whole equal-size blocks; here the right and bottom
/// superblocks of a frame that isn't a multiple of 64 are ragged, and the
/// un-normalized form scales as `n^(1/p)`, so a 64×8 edge sliver would pool
/// to 0.84× the value of an interior block carrying identical error and be
/// handed a coarser quantizer for being at the edge. Normalizing makes a
/// uniform map pool to exactly its own value for every block shape.
///
/// `stride` is the number of `f32` elements per row (`>= w`) — strided maps
/// are handled natively, and the tight case (`stride == w`) costs nothing
/// extra since the row slicing is identical.
///
/// Returns `sb_cols * sb_rows` values in frame superblock raster order.
pub(crate) fn pool_pnorm(diffmap: &[f32], w: usize, h: usize, stride: usize, exp: f64) -> Vec<f64> {
    debug_assert!(stride >= w);
    debug_assert!(diffmap.len() >= stride * h.saturating_sub(1) + w);
    let (sb_cols, sb_rows) = sb_grid(w, h);
    let mut out = Vec::with_capacity(sb_cols * sb_rows);
    for sby in 0..sb_rows {
        let y1 = ((sby + 1) * SB).min(h);
        for sbx in 0..sb_cols {
            let x0 = sbx * SB;
            let x1 = (x0 + SB).min(w);
            let mut acc = 0.0f64;
            for y in (sby * SB)..y1 {
                for &v in &diffmap[y * stride + x0..y * stride + x1] {
                    acc += f64::from(v).max(0.0).powf(exp);
                }
            }
            let n = ((x1 - x0) * (y1 - (sby * SB))) as f64;
            out.push((acc / n).powf(1.0 / exp));
        }
    }
    out
}

/// Per-superblock mean squared error between two RGB8 images (all three
/// channels, `sse / (px * 3)` — libaom's 8-bit pixel-difference domain).
///
/// Only the butteraugli valuation needs it: the zensim loop's mapping is
/// derived from relative error alone (see [`crate::two_pass_zensim`]).
///
/// Returns `sb_cols * sb_rows` values in frame superblock raster order.
#[cfg(any(feature = "two-pass-butteraugli", test))]
pub(crate) fn pool_mse(
    src: imgref::ImgRef<'_, rgb::Rgb<u8>>,
    dec: imgref::ImgRef<'_, rgb::Rgb<u8>>,
) -> Vec<f64> {
    let (w, h) = (src.width(), src.height());
    let (sb_cols, sb_rows) = sb_grid(w, h);
    let mut out = Vec::with_capacity(sb_cols * sb_rows);
    for sby in 0..sb_rows {
        let y0 = sby * SB;
        let y1 = (y0 + SB).min(h);
        for sbx in 0..sb_cols {
            let x0 = sbx * SB;
            let x1 = (x0 + SB).min(w);
            let mut sse = 0.0f64;
            for y in y0..y1 {
                let src_row = &src[y];
                let dec_row = &dec[y];
                for x in x0..x1 {
                    let s = src_row[x];
                    let d = dec_row[x];
                    let dr = f64::from(s.r) - f64::from(d.r);
                    let dg = f64::from(s.g) - f64::from(d.g);
                    let db = f64::from(s.b) - f64::from(d.b);
                    sse += dr * dr + dg * dg + db * db;
                }
            }
            let px = ((x1 - x0) * (y1 - y0)) as f64;
            out.push(sse / (px * 3.0));
        }
    }
    out
}

/// Turn per-superblock raw weights into an AC quantizer scale map.
///
/// `raw[i] = None` marks "no reliable signal here" — those blocks stay
/// exactly neutral (`1.0`) and are excluded from the normalizer, so a mostly
/// flat frame can't drag every real block off-neutral. The valid blocks are
/// divided by their **geometric** mean (the right center for a quantity that
/// enters multiplicatively and is compared by ratio), clamped to
/// `clamp = (lo, hi)` in that ratio domain, then raised to `exponent`.
///
/// `exponent` carries both the sign and the strength of the correction:
/// positive when a larger raw weight should mean a *coarser* quantizer
/// (butteraugli's rdmult-domain weight, exponent `+strength/2` for λ ∝ q²),
/// negative when a larger raw weight means *finer* (zensim's relative
/// error, exponent `−strength`). `exponent == 0` is exactly neutral.
///
/// Returns `1.0` everywhere when nothing was valid.
pub(crate) fn normalize_and_power(
    raw: &[Option<f64>],
    clamp: (f64, f64),
    exponent: f64,
) -> Box<[f32]> {
    let (lo, hi) = clamp;
    let mut log_sum = 0.0f64;
    let mut n_valid = 0u32;
    for w in raw.iter().flatten() {
        if *w > 0.0 && w.is_finite() {
            log_sum += w.ln();
            n_valid += 1;
        }
    }
    if n_valid == 0 {
        return vec![1.0f32; raw.len()].into_boxed_slice();
    }
    let geo_mean = (log_sum / f64::from(n_valid)).exp();

    raw.iter()
        .map(|w| match w {
            Some(w) if *w > 0.0 && w.is_finite() => {
                ((w / geo_mean).clamp(lo, hi)).powf(exponent) as f32
            }
            _ => 1.0f32,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_covers_ragged_frames() {
        assert_eq!(sb_grid(64, 64), (1, 1));
        assert_eq!(sb_grid(65, 64), (2, 1));
        assert_eq!(sb_grid(130, 70), (3, 2));
        assert_eq!(sb_grid(1, 1), (1, 1));
    }

    #[test]
    fn pnorm_pools_toward_the_block_max() {
        // 1×1 SB grid, one 8.0 spike among 4095 zeros. A 12-norm has to stay
        // far closer to the max than to the arithmetic mean (0.00195) — that
        // is the whole point of a high p. The normalized form's exact answer
        // is 8 / 4096^(1/12) = 4.0.
        let mut map = vec![0.0f32; 64 * 64];
        map[0] = 8.0;
        let pooled = pool_pnorm(&map, 64, 64, 64, 12.0);
        assert_eq!(pooled.len(), 1);
        assert!((pooled[0] - 4.0).abs() < 1e-6, "got {}", pooled[0]);
        assert!(pooled[0] > 100.0 * (8.0 / 4096.0), "pooled like a mean");
        // A uniform map pools to exactly its own value, for any p.
        let flat = vec![0.25f32; 64 * 64];
        assert!((pool_pnorm(&flat, 64, 64, 64, 12.0)[0] - 0.25).abs() < 1e-6);
        assert!((pool_pnorm(&flat, 64, 64, 64, 2.0)[0] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn pnorm_is_pixel_count_normalized_so_ragged_blocks_are_not_penalized() {
        // A 64-wide frame that is only 8 rows tall is ONE ragged superblock.
        // With libaom's un-normalized sum it would pool to 512^(1/12) = 1.85x
        // a full block's value for the same uniform error; normalized, the
        // two agree exactly. This is what keeps the frame's right/bottom edge
        // from being handed a systematically different quantizer.
        let full = vec![0.3f32; 64 * 64];
        let sliver = vec![0.3f32; 64 * 8];
        let a = pool_pnorm(&full, 64, 64, 64, 12.0)[0];
        let b = pool_pnorm(&sliver, 64, 8, 64, 12.0)[0];
        assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        assert!((a - 0.3).abs() < 1e-6, "{a}");
    }

    #[test]
    fn pnorm_handles_ragged_edge_blocks() {
        // 130×70 → 3×2 SBs, the right column is 2px wide and the bottom row
        // 6px tall. Every block must still pool its own uniform value.
        let map = vec![0.5f32; 130 * 70];
        let pooled = pool_pnorm(&map, 130, 70, 130, 12.0);
        assert_eq!(pooled.len(), 6);
        for v in pooled {
            assert!((v - 0.5).abs() < 1e-6, "ragged block pooled to {v}");
        }
    }

    #[test]
    fn normalize_is_scale_invariant_and_signed_by_exponent() {
        let raw = [Some(1.0), Some(2.0), Some(4.0)];
        let scaled: Vec<Option<f64>> = raw.iter().map(|w| w.map(|w| w * 1000.0)).collect();
        // Unitless by construction: multiplying every input by a constant
        // cannot move the output (the whole reason SSIM error can drive this).
        let a = normalize_and_power(&raw, (0.1, 10.0), -1.0);
        let b = normalize_and_power(&scaled, (0.1, 10.0), -1.0);
        for (x, y) in a.iter().zip(b.iter()) {
            assert!((x - y).abs() < 1e-6, "{x} vs {y}");
        }
        // exponent < 0: the worst block gets the FINEST quantizer.
        assert!(a[2] < a[1] && a[1] < a[0]);
        // exponent > 0 inverts the ordering; exponent 0 is exactly neutral.
        let up = normalize_and_power(&raw, (0.1, 10.0), 0.5);
        assert!(up[2] > up[1] && up[1] > up[0]);
        let neutral = normalize_and_power(&raw, (0.1, 10.0), 0.0);
        assert!(neutral.iter().all(|&s| (s - 1.0).abs() < 1e-6));
    }

    #[test]
    fn invalid_blocks_stay_neutral_and_do_not_normalize() {
        // The two valid blocks normalize against each other only; the
        // signal-free ones are pinned to 1.0.
        let raw = [Some(1.0), None, Some(4.0), Some(f64::NAN), Some(0.0)];
        let out = normalize_and_power(&raw, (0.01, 100.0), -1.0);
        assert_eq!(out.len(), 5);
        assert_eq!(out[1], 1.0);
        assert_eq!(out[3], 1.0);
        assert_eq!(out[4], 1.0);
        // geomean(1, 4) = 2 → 1/2 ^ -1 = 2, 4/2 ^ -1 = 0.5.
        assert!((out[0] - 2.0).abs() < 1e-6, "{}", out[0]);
        assert!((out[2] - 0.5).abs() < 1e-6, "{}", out[2]);
        // Nothing valid at all → all neutral.
        let none = normalize_and_power(&[None, None], (0.1, 10.0), -1.0);
        assert!(none.iter().all(|&s| s == 1.0));
    }

    #[test]
    fn clamp_bounds_the_ratio_before_the_power() {
        let raw = [Some(1.0), Some(1_000.0)];
        let out = normalize_and_power(&raw, (0.5, 2.0), -1.0);
        // geomean ≈ 31.6; both ratios land outside [0.5, 2] and clamp to the
        // bounds, so the scales are exactly 1/0.5 = 2 and 1/2 = 0.5.
        assert!((out[0] - 2.0).abs() < 1e-5, "{}", out[0]);
        assert!((out[1] - 0.5).abs() < 1e-5, "{}", out[1]);
    }

    #[test]
    fn mse_pool_matches_a_hand_computed_block() {
        use imgref::ImgVec;
        use rgb::Rgb;
        // 64×64, uniform 4/255 error on r, -4 on g, +4 on b → mse = 16.
        let src = ImgVec::new(vec![Rgb::new(120u8, 130, 140); 64 * 64], 64, 64);
        let dec = ImgVec::new(vec![Rgb::new(124u8, 126, 144); 64 * 64], 64, 64);
        let m = pool_mse(src.as_ref(), dec.as_ref());
        assert_eq!(m.len(), 1);
        assert!((m[0] - 16.0).abs() < 1e-9, "{}", m[0]);
    }
}
