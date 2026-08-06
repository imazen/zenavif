//! zensim-diffmap-driven closed loop: converge on a target zensim score in
//! as few encodes as possible, correcting **globally** from the score and
//! **spatially** from the per-pixel error map, both read from a single
//! `zensim` call per pass.
//!
//! # Shape
//!
//! ```text
//! precompute_reference(source)                 <- ONCE, outside the loop
//! repeat:
//!   encode(quality, sb_q_scale)  ->  decode  ->  compute_with_ref_and_diffmap
//!                                                  |         |
//!                                          score --+         +-- diffmap
//!                                            |                     |
//!                             global quality correction   per-SB quantizer scales
//! ```
//!
//! One `zensim` call per pass yields both signals, and the reference-side
//! pyramid is built once instead of once per iteration — that reuse is most
//! of why the loop is cheaper than the equivalent count of independent
//! [`crate::encode_rgb8_with_target`] iterations.
//!
//! # The two corrections
//!
//! **Global (live today).** The score says how far off the whole image is.
//! Rather than a fixed-slope Newton step, the loop moves along the fitted
//! population curve [`anchor_quality_for_zensim`]: it assumes this image's
//! score-vs-quality curve is a horizontal *translate* of the population's,
//! so
//!
//! ```text
//! q_next = q_now + (Q(target) − Q(score_now))
//! ```
//!
//! where `Q` is the anchor's score→quality inverse. That is strictly better
//! than a constant-slope step on a saturating curve (near score 90 the same
//! score error needs a much bigger quality move than near score 40), and it
//! needs only one fitted object — the same one that seeds pass 1. Once the
//! target is bracketed from both sides the loop switches to the bracketed
//! secant, which converges on the residual the translate assumption leaves.
//!
//! **Spatial (release-gated).** The diffmap says *where* the error is. Each
//! 64×64 superblock is p-norm pooled, normalized by the frame's geometric
//! mean, and turned into an AC quantizer scale. Because the scales are
//! normalized to geometric mean 1, the spatial correction is (to first
//! order) rate-neutral and therefore roughly orthogonal to the global one:
//! it moves bits between superblocks rather than changing how many there
//! are. It reaches the encoder through zenravif's `FrameHints::sb_q_scale`
//! passthrough, which is release-gated behind
//! [`SPATIAL_HINTS_LIVE`](crate::two_pass_zensim::SPATIAL_HINTS_LIVE)
//! (`zenravif::FRAME_HINTS_LIVE`) until the zenrav1e dep bump. Every result
//! reports [`ZensimLoopResult::spatial_applied`] so a caller can never
//! mistake a computed map for an applied one.
//!
//! Unlike [`crate::two_pass`] — which is purely spatial and so refuses to
//! run at all while the passthrough is gated off — this loop's global half
//! is live, so it runs and converges regardless; only the spatial term is
//! withheld.
//!
//! # What "converged" can mean at all: the achievable-score lattice
//!
//! [`ZensimLoopOptions::tolerance`] is not a free parameter. `quality`
//! resolves to an **integer** AV1 quantizer index, so for a given image the
//! reachable zensim scores form a discrete lattice, and no search — this
//! one, the secant, or a perfect oracle — can land between its points.
//! Measured (`benchmarks/zensim_score_lattice_2026-08-06.tsv`: every
//! integer quality in 50..=80, one photo and one screenshot at 256px, all
//! 31 qualities resolving to 31 distinct quantizer indices): adjacent
//! achievable scores are **1.05 / 0.82 apart at the median** and
//! **53% / 47% of the gaps exceed 1.0**. So a ±0.5 band contains no
//! achievable score at all roughly half the time.
//!
//! Read convergence numbers accordingly: at `tolerance = 0.5` a large
//! share of the misses are the lattice, not the search. Comparing two
//! searches on the same images and targets — which is what
//! `scripts/hyperparam/analyze_zensim_loop_ab.py` reports as paired
//! per-cell deltas — is unaffected, because both arms face the same
//! lattice.
//!
//! # How few encodes is possible
//!
//! Measured offline over the 720 (cell × target) combinations of the
//! anchor sweep, replaying this loop's own rules against the isotonized
//! per-cell curves (`benchmarks/zensim_anchor_2026-08-06.tsv`):
//!
//! | after | p50 \|err\| | p90 \|err\| | within ±0.5 | within ±2.0 |
//! |---|---|---|---|---|
//! | pass 1 (seed alone, open loop) | 4.14 | 14.59 | 11.9% | 30.4% |
//! | pass 2 (one anchor-translate correction) | 0.80 | 4.68 | 40.6% | 73.5% |
//!
//! Those are **ceilings**, and in-sample ones (the anchor was fitted on
//! the same corpus): they say a 1-encode answer is right about 1 time in 8
//! at ±0.5, and a 2-encode answer about 2 times in 5. A 1-encode mode
//! would therefore be an unverified open-loop *prediction*, not a
//! convergence, which is why this crate does not offer one — pass 1's
//! honest output is [`ZensimLoopResult::pass1_score`], already measured.
//!
//! The correction gain was swept on the same replay (`q_next = q + g·(Q(t)
//! − Q(s))`, g ∈ 0.6..1.4): the optimum is a flat plateau over 0.8–1.1
//! containing the principled g = 1 (pure translate), so no fitted gain
//! constant ships.
//!
//! # Deriving the diffmap → quantizer-scale mapping
//!
//! zensim's diffmap is **unitless SSIM error**, not a butteraugli distance,
//! so libaom's `tune=butteraugli` constants do not transfer: its valuation
//! is `min(mse / distance, 5) + K`, a ratio between an 8-bit squared-pixel
//! quantity and a perceptual-distance quantity whose scale is calibrated to
//! butteraugli's JND units. Feed SSIM error into that same expression and
//! typical photo blocks (mse ≈ 10–100, SSIM error ≈ 0.05–0.3) land at
//! ratios of 10²–10³, so the `min(·, 5)` clip saturates on essentially every
//! block and the map degenerates to a constant. The mapping here is derived
//! from scratch instead:
//!
//! 1. Only *relative* error is meaningful, since the map has no absolute
//!    unit — any valuation must be invariant to rescaling the whole map.
//!    So work with `r_b = e_b / geomean_b(e_b)`, the block's error relative
//!    to the frame (`e_b` = the block's p-norm pooled error).
//! 2. The allocation goal is equal perceptual error per superblock: blocks
//!    above the frame's typical error should get a finer quantizer, blocks
//!    below it should give bits back.
//! 3. Under the high-rate approximation that local error scales as a power
//!    of the quantizer step, `e ∝ step^γ`, driving `r_b` to 1 requires
//!    `step_scale_b = r_b^(−1/γ)`. So
//!
//! ```text
//! q_scale_b = clamp(r_b, lo, hi)^(−strength),     strength = 1/γ
//! ```
//!
//! `strength = 1.0` (the default) is the plain high-rate case γ = 1: error
//! amplitude proportional to step size. `strength = 0` disables the spatial
//! term exactly.
//!
//! **This mapping is DERIVED, not fitted** — and it cannot honestly be
//! fitted here yet: with `SPATIAL_HINTS_LIVE == false` the applied map has
//! no observable effect on the bitstream, so there is no end-to-end signal
//! to fit `strength`, `weight_clamp`, or `pool_exponent` against. Fitting
//! them is the first thing to do at the zenrav1e dep bump. The pooling
//! exponent (12) and the clamp shape are carried over from the butteraugli
//! driver as *mechanism*, which is how aom constants have transferred
//! throughout this program.
//!
//! What the 2026-08-06 sweep DOES say about the power law
//! (`benchmarks/zensim_anchor_2026-08-06.tsv`, 48 cells): the mean diffmap
//! error really does grow as a power of the quantizer, with
//! `d ln(error) / d ln(qindex)` = 1.79 median (p25 1.56, p75 2.10). That
//! supports the functional form. It does **not** pin `strength`, because
//! the exponent the derivation needs is against the dequant *step*, not
//! the qindex, and AV1's `ac_qlookup` between them is nonlinear. If that
//! elasticity is above ~1.8 then γ > 1 and the shipped `strength = 1.0`
//! steers harder than equal-error allocation wants. Do not "correct" it by
//! guessing the lookup's slope — measure `strength` end-to-end once the
//! hints are live.

use crate::DecoderConfig;
use crate::encoder::{EncodedImage, EncoderConfig, encode_rgb8_once};
use crate::error::{Error, Result};
use almost_enough::{Stop, StopToken};
use imgref::ImgRef;
use rgb::Rgb;
use whereat::at;

/// Whether the underlying zenravif build applies the per-superblock
/// quantizer-scale hints this loop computes (`zenravif::FRAME_HINTS_LIVE`).
///
/// `false` on registry builds until the zenrav1e dep bump: the loop still
/// runs and still converges (its global correction is live), but the
/// spatial term is computed and discarded. Mirrored per call as
/// [`ZensimLoopResult::spatial_applied`].
pub use ravif::FRAME_HINTS_LIVE as SPATIAL_HINTS_LIVE;

/// Options for [`encode_rgb8_zensim_loop`].
#[derive(Debug, Clone)]
pub struct ZensimLoopOptions {
    /// Acceptable `|achieved − target|`. Default 0.5 (matches
    /// [`crate::TargetOptions`]).
    pub tolerance: f64,
    /// Hard cap on encode+decode+score passes. Default 6.
    pub max_encodes: u8,
    /// Lower bound of the quality search range. Default 1.0.
    pub min_quality: f32,
    /// Upper bound of the quality search range. Default 100.0.
    pub max_quality: f32,
    /// Override the pass-1 quality. `None` (default) seeds from the fitted
    /// [`anchor_quality_for_zensim`] curve (see [`seed_quality`] for the
    /// content-aware seed that was tried and measured as harmful).
    pub seed_quality: Option<f32>,
    /// Spatial correction strength — the `1/γ` exponent derived in the
    /// [module docs](self). `0.0` disables the spatial term exactly (the
    /// loop becomes a pure global search). Default 1.0.
    pub spatial_strength: f64,
    /// Clamp applied to the relative per-superblock error `r_b` before it
    /// is exponentiated. Default `(0.4, 2.5)`.
    pub weight_clamp: (f64, f64),
    /// p-norm exponent of the per-superblock diffmap pool. Higher is closer
    /// to max-pooling, so one bad region is not averaged away by its clean
    /// neighbours. Default 12.0.
    pub pool_exponent: f64,
    /// Pooled-error floor below which a superblock carries no reliable
    /// signal and stays neutral. Default 1e-4 (zensim's own "identical
    /// images" tolerance is ~1e-4 of diffmap magnitude).
    pub map_eps: f64,
    /// zensim contrast-masking strength for the diffmap (`None` = off, the
    /// raw SSIM-error signal). Default `None`.
    pub masking_strength: Option<f32>,
    /// Include zensim's edge-artifact / edge-detail / MSE per-pixel
    /// features in the diffmap alongside SSIM error. Default `false`.
    pub include_edge_mse: bool,
}

impl Default for ZensimLoopOptions {
    fn default() -> Self {
        Self {
            tolerance: 0.5,
            max_encodes: 6,
            min_quality: 1.0,
            max_quality: 100.0,
            seed_quality: None,
            spatial_strength: 1.0,
            weight_clamp: (0.4, 2.5),
            pool_exponent: 12.0,
            map_eps: 1e-4,
            masking_strength: None,
            include_edge_mse: false,
        }
    }
}

/// Outcome of a [`encode_rgb8_zensim_loop`] run.
#[derive(Debug)]
#[non_exhaustive]
pub struct ZensimLoopResult {
    /// The selected encode (same policy as [`crate::TargetedEncode`]:
    /// smallest file among the iterates that reached the target band, else
    /// the closest iterate).
    pub encoded: EncodedImage,
    /// Quality that produced [`Self::encoded`].
    pub quality: f32,
    /// Measured zensim score of [`Self::encoded`].
    pub score: f64,
    /// Encode+decode+score passes spent.
    pub encodes: u8,
    /// Whether `|score − target| <= tolerance`.
    pub converged: bool,
    /// Whether the per-superblock scales actually reached the encoder — a
    /// copy of [`SPATIAL_HINTS_LIVE`]. `false` means the spatial term was
    /// computed and discarded and the run was a pure global search.
    pub spatial_applied: bool,
    /// The last per-superblock AC quantizer scale map the loop computed
    /// (frame superblock raster order, `1.0` = neutral). `None` when the
    /// loop finished in one pass or [`ZensimLoopOptions::spatial_strength`]
    /// is zero.
    pub sb_q_scale: Option<Box<[f32]>>,
    /// Score of the FIRST pass — how close the seed alone landed, i.e. what
    /// a one-encode open-loop prediction would have delivered.
    pub pass1_score: f64,
    /// Quality of the first pass (the seed).
    pub pass1_quality: f32,
}

/// Anchor knots of the zensim-B score → AVIF quality inverse curve.
///
/// **Fit provenance:** `examples/zensim_loop_bench.rs sweep` over 12
/// sources (8 clic2025 photos + 4 gb82-sc screen-content) × long edges
/// {64, 256, 1024, 2048} × quality {1, 5, 10, …, 95, 98} at speed 6,
/// scored with `ZensimProfile::codec_target()` (profile B). Per (image,
/// size) the q→score curve is isotonized, then each knot's quality is the
/// MEDIAN over cells of the leftmost quality reaching that score — the same
/// "smallest file in band" convention the search's selection policy uses.
/// Raw data + the fit: `benchmarks/zensim_anchor_2026-08-06.tsv`.
///
/// Scores ascending; quality ascending (the curve is monotone by
/// construction). Values outside the knot range extrapolate linearly from
/// the end segment.
const ANCHOR_SCORE: [f32; 15] = [
    20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0, 60.0, 65.0, 70.0, 75.0, 80.0, 85.0, 90.0,
];
/// Quality knots paired with [`ANCHOR_SCORE`], from the fit described
/// there (`scripts/hyperparam/fit_zensim_anchor.py`, 48 cells x 21
/// qualities = 1,008 measured encodes).
///
/// Shape worth knowing: the curve is NOT the identity. Reaching zensim 90
/// costs quality 92.6 while zensim 50 costs only 42.5 — a straight line
/// over the 40-90 band fits at intercept -11.50, slope 1.0817 quality per
/// score point (R2 0.966) but leaves up to 6.78 quality points of
/// curvature, which is exactly the error a constant-slope Newton step
/// would carry and the piecewise table does not.
const ANCHOR_QUALITY: [f32; 15] = [
    24.546, 27.502, 30.540, 33.217, 35.999, 40.040, 42.495, 45.937, 50.294, 56.654, 61.060, 66.338,
    74.021, 81.451, 92.628,
];

/// The fitted zensim-B score → AVIF quality anchor curve: what quality a
/// typical image needs to reach `target`.
///
/// Used twice by [`encode_rgb8_zensim_loop`] — to seed pass 1, and (as a
/// difference, `Q(target) − Q(score)`) to take each un-bracketed correction
/// step. See [`ANCHOR_SCORE`] for fit provenance.
///
/// Monotone non-decreasing in `target`; linearly extrapolated outside the
/// knot range and clamped to `[1, 100]`.
#[must_use]
pub fn anchor_quality_for_zensim(target: f64) -> f32 {
    let t = target as f32;
    let n = ANCHOR_SCORE.len();
    let q = if t <= ANCHOR_SCORE[0] {
        let slope = (ANCHOR_QUALITY[1] - ANCHOR_QUALITY[0]) / (ANCHOR_SCORE[1] - ANCHOR_SCORE[0]);
        ANCHOR_QUALITY[0] + (t - ANCHOR_SCORE[0]) * slope
    } else if t >= ANCHOR_SCORE[n - 1] {
        let slope = (ANCHOR_QUALITY[n - 1] - ANCHOR_QUALITY[n - 2])
            / (ANCHOR_SCORE[n - 1] - ANCHOR_SCORE[n - 2]);
        ANCHOR_QUALITY[n - 1] + (t - ANCHOR_SCORE[n - 1]) * slope
    } else {
        let i = ANCHOR_SCORE.partition_point(|&s| s <= t).max(1) - 1;
        let (s0, s1) = (ANCHOR_SCORE[i], ANCHOR_SCORE[i + 1]);
        let (q0, q1) = (ANCHOR_QUALITY[i], ANCHOR_QUALITY[i + 1]);
        q0 + (t - s0) / (s1 - s0) * (q1 - q0)
    };
    q.clamp(1.0, 100.0)
}

/// Encode an RGB8 image to AVIF, converging on a target zensim score with a
/// diffmap-guided closed loop (see the [module docs](self)).
///
/// `config`'s own quality is ignored (the loop sets it); every other
/// setting is used as-is for every pass. The alpha plane's quantizer is not
/// searched.
///
/// # Errors
///
/// Returns the first encode, decode, or zensim error encountered, or
/// cancellation via `stop` (the stop token is threaded into zensim itself,
/// so a long scoring pass is interruptible too).
pub fn encode_rgb8_zensim_loop(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    target: f64,
    options: &ZensimLoopOptions,
    stop: StopToken,
) -> Result<ZensimLoopResult> {
    let tol = options.tolerance.max(0.0);
    let (min_q, max_q) = (
        options.min_quality.clamp(0.0, 100.0),
        options.max_quality.clamp(0.0, 100.0),
    );
    if !min_q.is_finite() || !max_q.is_finite() || min_q >= max_q {
        return Err(at!(Error::InvalidParameters(format!(
            "two-pass-zensim: empty search range [{min_q}, {max_q}]"
        ))));
    }
    let max_encodes = options.max_encodes.max(1);

    // The reference pyramid is built ONCE and reused by every pass — the
    // reason a closed loop is cheaper than N independent scorings.
    let z = zensim::Zensim::new(zensim::ZensimProfile::codec_target()).with_stop(stop.clone());
    let pre = z.precompute_reference(&img).map_err(|e| {
        at!(Error::Encode(format!(
            "two-pass-zensim: zensim reference: {e}"
        )))
    })?;

    let dm_opts = zensim::DiffmapOptions {
        masking_strength: options.masking_strength,
        include_edge_mse: options.include_edge_mse,
        ..Default::default()
    };

    let mut q = seed_quality(target, options).clamp(min_q, max_q);
    let (mut pass1_score, mut pass1_quality) = (f64::NAN, q);

    // Same selection policy as the secant search: the smallest file that
    // reached the band (monotonicity ⇒ the lowest such quality), else the
    // iterate closest to the target.
    let mut best_reaching: Option<(f32, f64, EncodedImage)> = None;
    let mut best_any: Option<(f32, f64, EncodedImage)> = None;
    let mut lo: Option<(f32, f64)> = None;
    let mut hi: Option<(f32, f64)> = None;

    let mut sb_q_scale: Option<Box<[f32]>> = None;
    let mut encodes = 0u8;

    while encodes < max_encodes {
        stop.check().map_err(|e| at!(Error::from(e)))?;

        let mut cfg = config.clone().quality(q);
        cfg.sb_q_scale = sb_q_scale.clone();
        let enc = encode_rgb8_once(img, &cfg, stop.clone())?;

        let decoded = crate::decode_with(
            &enc.avif_file,
            &DecoderConfig::new().prefer_8bit(true),
            &stop,
        )?;
        let dec_img: ImgRef<'_, Rgb<u8>> = decoded.try_as_imgref::<Rgb<u8>>().ok_or_else(|| {
            at!(Error::Encode(
                "two-pass-zensim: decoded image not RGB8-viewable".to_string()
            ))
        })?;

        // ONE zensim call yields both signals.
        let dr = z
            .compute_with_ref_and_diffmap(&pre, &dec_img, dm_opts)
            .map_err(|e| at!(Error::Encode(format!("two-pass-zensim: zensim: {e}"))))?;
        let s = dr.score();
        encodes += 1;
        if encodes == 1 {
            pass1_score = s;
            pass1_quality = q;
        }

        if best_any
            .as_ref()
            .is_none_or(|(_, bs, _)| (s - target).abs() < (bs - target).abs())
        {
            best_any = Some((q, s, enc.clone()));
        }
        if s >= target - tol && best_reaching.as_ref().is_none_or(|(bq, _, _)| q < *bq) {
            best_reaching = Some((q, s, enc));
        }
        if (s - target).abs() <= tol {
            break;
        }
        if s < target {
            lo = Some((q, s));
        } else {
            hi = Some((q, s));
        }

        // Spatial correction for the NEXT pass, from this pass's map.
        sb_q_scale = (options.spatial_strength != 0.0)
            .then(|| pool_sb_q_scale(dr.diffmap(), dr.width(), dr.height(), options));

        // Global correction for the next pass.
        let Some(next) = next_quality(q, s, target, lo, hi, min_q, max_q) else {
            break;
        };
        q = next;
    }

    let (quality, score, encoded) = best_reaching
        .or(best_any)
        .expect("max_encodes >= 1 guarantees at least one iterate");
    Ok(ZensimLoopResult {
        encoded,
        quality,
        score,
        encodes,
        converged: (score - target).abs() <= tol,
        spatial_applied: SPATIAL_HINTS_LIVE,
        sb_q_scale,
        pass1_score,
        pass1_quality,
    })
}

/// Pass-1 quality: the caller's override, else the fitted anchor curve.
///
/// **A content-aware seed was tried here and MEASURED AS HARMFUL — do not
/// re-add it without re-measuring.** The [`crate::q0_head`] head predicts a
/// starting quality from zenanalyze content features, but it is fitted for
/// SSIMULACRA2 targets, so it cannot be asked for a zensim target directly.
/// The attempt was to use it as a pure *content offset*: query the head at
/// the ssim2 target whose content-blind anchor quality equals this zensim
/// anchor, subtract that same content-blind curve, and add the difference
/// to the zensim anchor.
///
/// It does not work, for a reason that is obvious in hindsight: `head(t)`
/// and [`crate::target_quality::initial_guess`]`(t)` do not share a
/// population level, so their difference carries the systematic gap
/// between the head's fitted mean and the cruder anchor's, not just the
/// per-image content term. Measured on the 2026-08-06 A/B corpus (360
/// real encodes at long edges 64 and 256, targets 20..90 step 5): the
/// implied "content offset" had median −9.72 quality points and saturated
/// its own ±12 bound, and the pass-1 seed error went from **p50 4.14 /
/// p90 14.59** (bare anchor) to **p50 10.67 / p90 20.58** zensim points,
/// dropping the 1-encode rate at ±0.5 from 11.9% to 1.1%. The bare
/// measured anchor ships.
///
/// Making the head genuinely usable here needs a zensim-target fit of its
/// own (`scripts/hyperparam/fit_q0_head.py` re-run against zensim labels),
/// not a cross-metric transfer.
fn seed_quality(target: f64, options: &ZensimLoopOptions) -> f32 {
    options
        .seed_quality
        .filter(|q| q.is_finite())
        .unwrap_or_else(|| anchor_quality_for_zensim(target))
}

/// The next quality to try. `None` means "the search cannot usefully move"
/// (bracket exhausted, or pinned at a range end and still short).
///
/// Un-bracketed: step along the fitted population curve
/// (`q + Q(target) − Q(score)`), the translate assumption from the module
/// docs. Bracketed: the same clamped secant the [`crate::target_quality`]
/// search uses, so the endgame behaviour and the guarantee that the bracket
/// provably shrinks are identical.
fn next_quality(
    q: f32,
    s: f64,
    target: f64,
    lo: Option<(f32, f64)>,
    hi: Option<(f32, f64)>,
    min_q: f32,
    max_q: f32,
) -> Option<f32> {
    match (lo, hi) {
        (Some((lq, ls)), Some((hq, hs))) => {
            let span = hq - lq;
            if span <= 0.25 {
                return None; // scores are quantized at this granularity
            }
            let sec = if (hs - ls).abs() > 1e-9 {
                lq + ((target - ls) / (hs - ls)) as f32 * span
            } else {
                lq + span / 2.0
            };
            Some(sec.clamp(lq + span * 0.1, hq - span * 0.1))
        }
        _ => {
            let shift = anchor_quality_for_zensim(target) - anchor_quality_for_zensim(s);
            // The translate can under-shoot near the curve's flat ends;
            // require a real move so the loop cannot stall in place.
            let step = if shift.abs() < 1.0 {
                if target > s { 4.0 } else { -4.0 }
            } else {
                shift
            };
            let n = (q + step).clamp(min_q, max_q);
            ((n - q).abs() > 0.25).then_some(n)
        }
    }
}

/// Pools a zensim diffmap into per-64×64-superblock AC quantizer scales
/// (the derivation is in the [module docs](self)).
fn pool_sb_q_scale(diffmap: &[f32], w: usize, h: usize, options: &ZensimLoopOptions) -> Box<[f32]> {
    // Mechanics shared with the butteraugli driver; the valuation below is
    // the zensim-specific part.
    let pooled = crate::sb_pool::pool_pnorm(diffmap, w, h, w, options.pool_exponent);
    let raw: Vec<Option<f64>> = pooled
        .iter()
        .map(|&e| (e >= options.map_eps).then_some(e))
        .collect();
    // Relative error -> quantizer scale. A LARGER relative error must give
    // a FINER quantizer, hence the negative exponent.
    crate::sb_pool::normalize_and_power(&raw, options.weight_clamp, -options.spatial_strength)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_curve_is_monotone_and_bounded() {
        let mut prev = 0.0f32;
        let mut t = 0.0f64;
        while t <= 100.0 {
            let q = anchor_quality_for_zensim(t);
            assert!((1.0..=100.0).contains(&q), "q {q} out of range at t {t}");
            assert!(q >= prev - 1e-4, "not monotone at t {t}: {prev} -> {q}");
            prev = q;
            t += 0.5;
        }
        // Extrapolation below/above the knots must not blow up or invert.
        assert!(anchor_quality_for_zensim(-50.0) >= 1.0);
        assert!(anchor_quality_for_zensim(500.0) <= 100.0);
    }

    #[test]
    fn anchor_curve_interpolates_its_knots() {
        for (i, &s) in ANCHOR_SCORE.iter().enumerate() {
            let q = anchor_quality_for_zensim(f64::from(s));
            let want = ANCHOR_QUALITY[i].clamp(1.0, 100.0);
            assert!((q - want).abs() < 1e-3, "knot {s}: got {q}, want {want}");
        }
        // Midpoint of a segment is the midpoint of the knots.
        let mid = anchor_quality_for_zensim(f64::from((ANCHOR_SCORE[3] + ANCHOR_SCORE[4]) / 2.0));
        let want = ((ANCHOR_QUALITY[3] + ANCHOR_QUALITY[4]) / 2.0).clamp(1.0, 100.0);
        assert!((mid - want).abs() < 1e-3, "got {mid}, want {want}");
    }

    #[test]
    fn spatial_pool_gives_hot_blocks_a_finer_quantizer() {
        // 192×128 = 3×2 SBs; SB (1,0) carries 20× the error of the rest.
        let (w, h) = (192usize, 128usize);
        let mut dm = vec![0.05f32; w * h];
        for y in 0..64 {
            for x in 64..128 {
                dm[y * w + x] = 1.0;
            }
        }
        let opts = ZensimLoopOptions::default();
        let map = pool_sb_q_scale(&dm, w, h, &opts);
        assert_eq!(map.len(), 6);
        assert!(
            map[1] < 1.0,
            "hot SB must get a finer quantizer: {}",
            map[1]
        );
        for (i, &s) in map.iter().enumerate() {
            if i != 1 {
                assert!(s > 1.0, "cool SB {i} must give bits back, got {s}");
                assert!(map[1] < s);
            }
        }
    }

    #[test]
    fn spatial_pool_is_scale_invariant() {
        // The mapping must not depend on the diffmap's absolute magnitude —
        // it has no unit. Scaling the whole map by 1000 must be a no-op.
        let (w, h) = (128usize, 128usize);
        let mut dm = vec![0.02f32; w * h];
        for y in 0..64 {
            for x in 0..64 {
                dm[y * w + x] = 0.3;
            }
        }
        let scaled: Vec<f32> = dm.iter().map(|v| v * 1000.0).collect();
        let opts = ZensimLoopOptions::default();
        let a = pool_sb_q_scale(&dm, w, h, &opts);
        let b = pool_sb_q_scale(&scaled, w, h, &opts);
        for (x, y) in a.iter().zip(b.iter()) {
            assert!((x - y).abs() < 1e-5, "{x} vs {y}");
        }
    }

    #[test]
    fn spatial_pool_is_neutral_when_disabled_or_signal_free() {
        let (w, h) = (128usize, 128usize);
        let mut dm = vec![0.02f32; w * h];
        dm[0] = 5.0;
        let off = pool_sb_q_scale(
            &dm,
            w,
            h,
            &ZensimLoopOptions {
                spatial_strength: 0.0,
                ..Default::default()
            },
        );
        assert!(off.iter().all(|&s| (s - 1.0).abs() < 1e-6));
        // An all-zero map (identical images) has no signal anywhere.
        let zero = vec![0.0f32; w * h];
        let none = pool_sb_q_scale(&zero, w, h, &ZensimLoopOptions::default());
        assert!(none.iter().all(|&s| s == 1.0));
    }

    #[test]
    fn next_quality_steps_along_the_anchor_when_unbracketed() {
        // Under target with no upper bracket: must move UP by at least the
        // anchor translate, never past the range end.
        let n = next_quality(50.0, 40.0, 70.0, Some((50.0, 40.0)), None, 1.0, 100.0)
            .expect("must move");
        assert!(n > 50.0, "expected an increase, got {n}");
        let want = 50.0 + anchor_quality_for_zensim(70.0) - anchor_quality_for_zensim(40.0);
        assert!((n - want).abs() < 1e-3, "got {n}, want {want}");
        // Over target with no lower bracket: must move DOWN.
        let d = next_quality(80.0, 90.0, 70.0, None, Some((80.0, 90.0)), 1.0, 100.0)
            .expect("must move");
        assert!(d < 80.0, "expected a decrease, got {d}");
        // Pinned at the range end and still short -> no useful move.
        assert!(next_quality(100.0, 40.0, 99.0, Some((100.0, 40.0)), None, 1.0, 100.0).is_none());
    }

    #[test]
    fn next_quality_secants_inside_the_bracket() {
        let lo = Some((40.0f32, 60.0f64));
        let hi = Some((80.0f32, 90.0f64));
        let n = next_quality(80.0, 90.0, 70.0, lo, hi, 1.0, 100.0).expect("must move");
        // Strictly interior, and clamped 10% away from both endpoints.
        assert!(n > 40.0 + 4.0 - 1e-3 && n < 80.0 - 4.0 + 1e-3, "got {n}");
        // Linear interpolation to 70 between (40,60) and (80,90) is 53.3.
        assert!((n - 53.333).abs() < 0.05, "got {n}");
        // An exhausted bracket stops the search.
        assert!(
            next_quality(
                50.0,
                70.5,
                70.0,
                Some((50.0, 69.9)),
                Some((50.2, 70.5)),
                1.0,
                100.0
            )
            .is_none()
        );
    }
}
