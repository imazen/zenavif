//! Precise perceptual-quality targeting: converge the encoder on a requested
//! SSIMULACRA2 or zensim score instead of an abstract quality number.
//!
//! The `quality` knob (like every codec's) maps to a quantizer, and the same
//! quality produces very different perceptual results on different content.
//! [`encode_rgb8_with_target`] closes that loop: encode → decode (with
//! zenavif's own decoder) → score against the source → adjust quality →
//! repeat, using a bracketed secant/bisection search over the monotone
//! score-vs-quality curve. Typical convergence is 3–5 encodes for a ±0.5
//! tolerance.
//!
//! Enabled by the `target-quality` feature (pulls `fast-ssim2` and `zensim`).

use crate::config::DecoderConfig;
use crate::encoder::{EncodedImage, EncoderConfig, encode_rgb8, encode_rgba8};
use crate::error::{Error, Result};
use almost_enough::{Stop, StopToken};
use imgref::{ImgRef, ImgVec};
use rgb::Rgb;
use whereat::at;

/// Which perceptual metric to converge on, and the target score.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TargetMetric {
    /// SSIMULACRA2 score (via `fast-ssim2`). Web-typical range 55–95;
    /// ~70 = "medium", ~80 = "high", ~90 = "visually near-lossless".
    Ssim2(f64),
    /// zensim similarity score (via `zensim`, latest profile), 0–100 scale
    /// calibrated similarly to SSIMULACRA2.
    Zensim(f64),
}

impl TargetMetric {
    fn value(self) -> f64 {
        match self {
            TargetMetric::Ssim2(v) | TargetMetric::Zensim(v) => v,
        }
    }
}

/// Search parameters for the targeting loop.
#[derive(Debug, Clone, Copy)]
pub struct TargetOptions {
    /// Acceptable |achieved − target| distance. Default 0.5.
    pub tolerance: f64,
    /// Hard cap on encode+decode+score iterations. Default 6.
    pub max_encodes: u8,
    /// Lower bound of the quality search range. Default 1.0.
    pub min_quality: f32,
    /// Upper bound of the quality search range. Default 100.0.
    pub max_quality: f32,
}

impl Default for TargetOptions {
    fn default() -> Self {
        Self {
            tolerance: 0.5,
            max_encodes: 6,
            min_quality: 1.0,
            max_quality: 100.0,
        }
    }
}

/// Result of a targeted encode.
#[derive(Debug)]
pub struct TargetedEncode {
    /// The chosen encode (see [`encode_rgb8_with_target`] for the policy).
    pub encoded: EncodedImage,
    /// The quality value that produced `encoded`.
    pub quality: f32,
    /// The measured metric score of `encoded`.
    pub score: f64,
    /// Number of encode+decode+score iterations spent.
    pub encodes: u8,
    /// Whether `|score − target| <= tolerance`.
    pub converged: bool,
}

/// Encode an RGB8 image to AVIF, converging on a target perceptual score.
///
/// Runs up to `options.max_encodes` full encode→decode→score iterations,
/// bracketing the target on the monotone score-vs-quality curve and
/// refining with a clamped secant step (bisection fallback).
///
/// **Selection policy:** among all iterates scoring within
/// `target − tolerance` or better, the smallest file is returned (the
/// score-vs-quality curve is monotone, so that is the lowest-quality
/// iterate that still reaches the target band). If the target is
/// unreachable even at `max_quality`, the highest-scoring encode is
/// returned with `converged = false`.
///
/// The `config`'s own quality value is ignored (it is overwritten by the
/// search); every other setting (speed, subsampling, tuning knobs) is
/// used as-is for every trial encode.
///
/// # Errors
///
/// Returns the first encode/decode/score error encountered, or
/// cancellation via `stop`.
pub fn encode_rgb8_with_target(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    target: TargetMetric,
    options: &TargetOptions,
    stop: StopToken,
) -> Result<TargetedEncode> {
    search_target(target, options, &stop, |q, stop| {
        let cfg = config.clone().quality(q);
        let enc = encode_rgb8(img, &cfg, stop.clone())?;
        let s = score_rgb8(target, img, &enc.avif_file, stop)?;
        Ok((enc, s))
    })
}

/// Encode an RGBA8 image to AVIF, converging on a target perceptual score.
///
/// Same search and selection policy as [`encode_rgb8_with_target`].
/// **Alpha handling in scoring:** [`TargetMetric::Zensim`] scores the RGBA
/// pixels natively (zensim is alpha-aware, straight-alpha semantics);
/// [`TargetMetric::Ssim2`] composites both source and decode onto a mid-gray
/// (128) background first, since SSIMULACRA2 is defined on opaque RGB. The
/// alpha plane's own quantizer is NOT searched — set it via
/// [`EncoderConfig::alpha_quality`] as usual.
///
/// # Errors
///
/// Returns the first encode/decode/score error encountered, or
/// cancellation via `stop`.
pub fn encode_rgba8_with_target(
    img: ImgRef<'_, rgb::Rgba<u8>>,
    config: &EncoderConfig,
    target: TargetMetric,
    options: &TargetOptions,
    stop: StopToken,
) -> Result<TargetedEncode> {
    search_target(target, options, &stop, |q, stop| {
        let cfg = config.clone().quality(q);
        let enc = encode_rgba8(img, &cfg, stop.clone())?;
        let s = score_rgba8(target, img, &enc.avif_file, stop)?;
        Ok((enc, s))
    })
}

/// Encode a 16-bit RGB image to AVIF (10-bit AV1), converging on a target
/// perceptual score.
///
/// Same search and selection policy as [`encode_rgb8_with_target`]. Scoring
/// runs on the 16-bit decode (`prefer_8bit(false)`): SSIMULACRA2 natively at
/// 16-bit precision; zensim on an 8-bit view (`>> 8` of both sides — the
/// current zensim profile is calibrated for 8-bit input), applied
/// identically to source and decode.
///
/// # Errors
///
/// Returns the first encode/decode/score error encountered, or
/// cancellation via `stop`.
pub fn encode_rgb16_with_target(
    img: ImgRef<'_, Rgb<u16>>,
    config: &EncoderConfig,
    target: TargetMetric,
    options: &TargetOptions,
    stop: StopToken,
) -> Result<TargetedEncode> {
    search_target(target, options, &stop, |q, stop| {
        let cfg = config.clone().quality(q);
        let enc = crate::encoder::encode_rgb16(img, &cfg, stop.clone())?;
        let s = score_rgb16(target, img, &enc.avif_file, stop)?;
        Ok((enc, s))
    })
}

/// The bracketed secant/bisection search shared by every input variant.
/// `encode_and_score(quality, stop)` performs one full trial iteration.
fn search_target(
    target: TargetMetric,
    options: &TargetOptions,
    stop: &StopToken,
    mut encode_and_score: impl FnMut(f32, &StopToken) -> Result<(EncodedImage, f64)>,
) -> Result<TargetedEncode> {
    let t = target.value();
    let tol = options.tolerance.max(0.0);
    let (min_q, max_q) = (
        options.min_quality.clamp(0.0, 100.0),
        options.max_quality.clamp(0.0, 100.0),
    );
    if !min_q.is_finite() || !max_q.is_finite() || min_q >= max_q {
        return Err(at!(Error::Encode(format!(
            "target-quality: empty search range [{min_q}, {max_q}]"
        ))));
    }
    let max_encodes = options.max_encodes.max(1);

    // Best iterate at-or-above the target band (monotonicity ⇒ the lowest-q
    // one is the smallest file), and the overall closest as a fallback.
    let mut best_reaching: Option<(f32, f64, EncodedImage)> = None;
    let mut best_any: Option<(f32, f64, EncodedImage)> = None;

    // Bracket: lo = highest quality known BELOW target, hi = lowest known AT/ABOVE.
    let mut lo: Option<(f32, f64)> = None;
    let mut hi: Option<(f32, f64)> = None;

    let mut q = initial_guess(t).clamp(min_q, max_q);
    let mut encodes = 0u8;

    while encodes < max_encodes {
        stop.check().map_err(|e| at!(Error::from(e)))?;

        let (enc, s) = encode_and_score(q, stop)?;
        encodes += 1;

        let better_any = best_any
            .as_ref()
            .is_none_or(|(_, bs, _)| (s - t).abs() < (bs - t).abs());
        if better_any {
            best_any = Some((q, s, enc.clone()));
        }
        if s >= t - tol {
            let better_reaching = best_reaching.as_ref().is_none_or(|(bq, _, _)| q < *bq);
            if better_reaching {
                best_reaching = Some((q, s, enc));
            }
        }

        if (s - t).abs() <= tol {
            break;
        }
        if s < t {
            lo = Some((q, s));
        } else {
            hi = Some((q, s));
        }

        let next = match (lo, hi) {
            (Some((lq, ls)), Some((hq, hs))) => {
                // Secant within the bracket, clamped away from the endpoints
                // so every iteration provably shrinks the bracket.
                let span = hq - lq;
                if span <= 0.25 {
                    break; // bracket exhausted — scores are quantized here
                }
                let sec = if (hs - ls).abs() > 1e-9 {
                    lq + ((t - ls) / (hs - ls)) as f32 * span
                } else {
                    lq + span / 2.0
                };
                sec.clamp(lq + span * 0.1, hq - span * 0.1)
            }
            (Some((lq, ls)), None) => {
                // Under target with no upper bracket: extrapolate up.
                let step = ((t - ls) as f32 * 1.2).max(4.0);
                let n = (lq + step).min(max_q);
                if n <= lq + 0.25 {
                    break; // pinned at max_quality and still short
                }
                n
            }
            (None, Some((hq, hs))) => {
                // Over target with no lower bracket: extrapolate down.
                let step = ((hs - t) as f32 * 1.2).max(4.0);
                let n = (hq - step).max(min_q);
                if n >= hq - 0.25 {
                    break; // pinned at min_quality and still over
                }
                n
            }
            (None, None) => unreachable!("at least one bracket side is set"),
        };
        q = next;
    }

    let (quality, score, encoded) = best_reaching
        .or(best_any)
        .expect("max_encodes >= 1 guarantees at least one iterate");
    Ok(TargetedEncode {
        encoded,
        quality,
        score,
        encodes,
        converged: (score - t).abs() <= tol,
    })
}

/// Initial quality guess for a target score, from measured photo-corpus
/// anchors of the quality→SSIMULACRA2 curve (Q30→~30, Q60→~70, Q90→~89).
/// Only a starting point — the search corrects content-dependent deviation.
fn initial_guess(t: f64) -> f32 {
    let t = t as f32;
    if t <= 30.0 {
        t.max(1.0)
    } else if t <= 70.0 {
        30.0 + (t - 30.0) * (30.0 / 40.0)
    } else {
        60.0 + (t - 70.0) * (30.0 / 19.0)
    }
}

/// Decode `avif` with zenavif's own decoder and score it against an RGB8 source.
fn score_rgb8(
    target: TargetMetric,
    source: ImgRef<'_, Rgb<u8>>,
    avif: &[u8],
    stop: &StopToken,
) -> Result<f64> {
    let dec_config = DecoderConfig::new().prefer_8bit(true);
    let decoded = crate::decode_with(avif, &dec_config, stop)?;
    let dec_img: ImgRef<'_, Rgb<u8>> = decoded.try_as_imgref::<Rgb<u8>>().ok_or_else(|| {
        at!(Error::Encode(
            "target-quality: decoded image not RGB8-viewable".to_string()
        ))
    })?;
    match target {
        TargetMetric::Ssim2(_) => {
            let a = to_triplet_img(source);
            let b = to_triplet_img(dec_img);
            ssim2_score(a.as_ref(), b.as_ref())
        }
        TargetMetric::Zensim(_) => zensim_score(&source, &dec_img),
    }
}

/// Decode `avif` and score it against an RGBA8 source. Zensim scores RGBA
/// natively (alpha-aware); SSIMULACRA2 gets both sides composited onto
/// mid-gray (see `encode_rgba8_with_target`).
fn score_rgba8(
    target: TargetMetric,
    source: ImgRef<'_, rgb::Rgba<u8>>,
    avif: &[u8],
    stop: &StopToken,
) -> Result<f64> {
    let dec_config = DecoderConfig::new().prefer_8bit(true);
    let decoded = crate::decode_with(avif, &dec_config, stop)?;
    let dec_img: ImgRef<'_, rgb::Rgba<u8>> =
        decoded.try_as_imgref::<rgb::Rgba<u8>>().ok_or_else(|| {
            at!(Error::Encode(
                "target-quality: decoded image not RGBA8-viewable".to_string()
            ))
        })?;
    match target {
        TargetMetric::Ssim2(_) => {
            let a = composite_on_gray(source);
            let b = composite_on_gray(dec_img);
            ssim2_score(a.as_ref(), b.as_ref())
        }
        TargetMetric::Zensim(_) => zensim_score(&source, &dec_img),
    }
}

/// Decode `avif` at 16-bit and score against an RGB16 source (see
/// `encode_rgb16_with_target` for the per-metric precision notes).
fn score_rgb16(
    target: TargetMetric,
    source: ImgRef<'_, Rgb<u16>>,
    avif: &[u8],
    stop: &StopToken,
) -> Result<f64> {
    let dec_config = DecoderConfig::new().prefer_8bit(false);
    let decoded = crate::decode_with(avif, &dec_config, stop)?;
    let dec_img: ImgRef<'_, Rgb<u16>> = decoded.try_as_imgref::<Rgb<u16>>().ok_or_else(|| {
        at!(Error::Encode(
            "target-quality: decoded image not RGB16-viewable".to_string()
        ))
    })?;
    match target {
        TargetMetric::Ssim2(_) => {
            let a = to_triplet16_img(source);
            let b = to_triplet16_img(dec_img);
            fast_ssim2::compute_ssimulacra2(a.as_ref(), b.as_ref())
                .map_err(|e| at!(Error::Encode(format!("target-quality ssim2: {e}"))))
        }
        TargetMetric::Zensim(_) => {
            let a = downconvert8(source);
            let b = downconvert8(dec_img);
            zensim_score(&a.as_ref(), &b.as_ref())
        }
    }
}

fn to_triplet16_img(src: ImgRef<'_, Rgb<u16>>) -> ImgVec<[u16; 3]> {
    let (w, h) = (src.width(), src.height());
    let mut out = Vec::with_capacity(w * h);
    for row in src.rows() {
        out.extend(row.iter().map(|p| [p.r, p.g, p.b]));
    }
    ImgVec::new(out, w, h)
}

/// 16→8-bit view for zensim (calibrated on 8-bit input); identical
/// treatment of source and decode keeps the comparison unbiased.
fn downconvert8(src: ImgRef<'_, Rgb<u16>>) -> ImgVec<Rgb<u8>> {
    let (w, h) = (src.width(), src.height());
    let mut out = Vec::with_capacity(w * h);
    for row in src.rows() {
        out.extend(row.iter().map(|p| Rgb {
            r: (p.r >> 8) as u8,
            g: (p.g >> 8) as u8,
            b: (p.b >> 8) as u8,
        }));
    }
    ImgVec::new(out, w, h)
}

fn ssim2_score(a: ImgRef<'_, [u8; 3]>, b: ImgRef<'_, [u8; 3]>) -> Result<f64> {
    fast_ssim2::compute_ssimulacra2(a, b)
        .map_err(|e| at!(Error::Encode(format!("target-quality ssim2: {e}"))))
}

fn zensim_score(a: &impl zensim::ImageSource, b: &impl zensim::ImageSource) -> Result<f64> {
    let z = zensim::Zensim::new(zensim::ZensimProfile::latest());
    z.compute(a, b)
        .map(|r| r.score())
        .map_err(|e| at!(Error::Encode(format!("target-quality zensim: {e}"))))
}

/// fast-ssim2 wants `ImgRef<[u8; 3]>`; zenavif buffers are `Rgb<u8>`.
/// One copy per score is trivial next to the encode it follows.
fn to_triplet_img(src: ImgRef<'_, Rgb<u8>>) -> ImgVec<[u8; 3]> {
    let (w, h) = (src.width(), src.height());
    let mut out = Vec::with_capacity(w * h);
    for row in src.rows() {
        out.extend(row.iter().map(|p| [p.r, p.g, p.b]));
    }
    ImgVec::new(out, w, h)
}

/// Straight-alpha composite onto a mid-gray (128) background, producing the
/// opaque RGB view SSIMULACRA2 is defined on. Applied identically to source
/// and decode, so background choice cannot favor either side.
fn composite_on_gray(src: ImgRef<'_, rgb::Rgba<u8>>) -> ImgVec<[u8; 3]> {
    let (w, h) = (src.width(), src.height());
    let mut out = Vec::with_capacity(w * h);
    for row in src.rows() {
        out.extend(row.iter().map(|p| {
            let a = u16::from(p.a);
            let blend = |c: u8| -> u8 { ((u16::from(c) * a + 128 * (255 - a) + 127) / 255) as u8 };
            [blend(p.r), blend(p.g), blend(p.b)]
        }));
    }
    ImgVec::new(out, w, h)
}
