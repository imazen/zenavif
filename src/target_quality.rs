//! Precise perceptual-quality targeting: converge the encoder on a requested
//! SSIMULACRA2 or zensim score instead of an abstract quality number.
//!
//! The `quality` knob (like every codec's) maps to a quantizer, and the same
//! quality produces very different perceptual results on different content.
//! [`encode_rgb8_with_target`] closes that loop: encode → decode (with
//! zenavif's own decoder) → score against the source → adjust quality →
//! repeat, using a bracketed secant/bisection search over the monotone
//! score-vs-quality curve. Typical convergence is 3–5 encodes for a ±0.5
//! tolerance from the content-blind anchor curve; with the `auto-tune`
//! feature the RGB8 + ssim2 path seeds the search from the
//! [`crate::q0_head`] content prediction instead (offline: mean 3.75 →
//! 2.72 encodes on held-out label-store curves).
//!
//! Enabled by the `target-quality` feature (pulls `fast-ssim2` and `zensim`).

use crate::config::DecoderConfig;
use crate::encoder::{EncodedImage, EncoderConfig, encode_rgb8_once, encode_rgba8_once};
use crate::error::{Error, Result};
use almost_enough::{Stop, StopToken};
use imgref::{ImgRef, ImgVec};
use rgb::Rgb;
use whereat::at;

/// Which perceptual metric to converge on, and the target score.
///
/// `#[non_exhaustive]`: metric variants are expected to keep arriving — this
/// enum has already gained `ZensimC`, and zensim's own profile generations
/// (`A`, `B`, `C`, `BHdr`, …) advance independently of this crate. Downstream
/// exhaustive matches must carry a `_` arm so a new metric is an ADDITIVE
/// change here instead of a breaking one for every consumer.
///
/// Matching within this crate is unaffected.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum TargetMetric {
    /// SSIMULACRA2 score (via `fast-ssim2`). Web-typical range 55–95;
    /// ~70 = "medium", ~80 = "high", ~90 = "visually near-lossless".
    Ssim2(f64),
    /// zensim similarity score (via `zensim`,
    /// [`ZensimProfile::codec_target`](zensim::ZensimProfile::codec_target) —
    /// the stable cross-codec contract profile), 0–100 scale calibrated
    /// similarly to SSIMULACRA2.
    ///
    /// **Score-scale note (zensim 0.3.0 re-pin, 2026-08-06):** this used to
    /// score with `ZensimProfile::latest()`, which was `PreviewV0_2` at
    /// zensim 0.2.x and became `B` (and deprecated) at 0.3.0. All zensim
    /// scoring in this crate now names `codec_target()` (= `B`) explicitly.
    /// A given `Zensim(t)` target therefore lands on a different quality
    /// than it did against 0.2.x.
    Zensim(f64),
    /// zensim **generation-C** score
    /// ([`ZensimProfile::C`](zensim::ZensimProfile::C)), 0–100.
    ///
    /// A different bake over a different feature regime, not a tweak of
    /// [`Self::Zensim`]: C is a 944-input MLP over the
    /// folded-720/append/append2 layout, where `B` is a 35-weight linear
    /// core over the v1 372. Both dials are calibrated to a 0–100 shape
    /// but **a given number does not mean the same encode** — see
    /// [`crate::zensim_c`] for the mechanics and
    /// `benchmarks/zensim_c_*` for the measured curves. Ask for `C`
    /// deliberately; `codec_target()` is still `B`, so
    /// [`Self::Zensim`]'s meaning is unchanged.
    ///
    /// **SDR only.** C's HDR feature slots are pruned from the bake, so
    /// HDR content is out of domain. Scoring an HDR-flagged pair is
    /// refused with a typed error rather than answered with a number —
    /// see [`crate::zensim_c::sdr_guard`]. There is no C-generation HDR
    /// profile to fall back to; zensim routes HDR to `BHdr`, which needs
    /// an absolute-luminance front end this crate does not have.
    ///
    /// **Cost.** Measurably slower than [`Self::Zensim`] — zensim's own
    /// numbers put the folded-944 score at roughly 1.6–2.5× the
    /// 372-feature score, and the 944 extraction has no
    /// `PrecomputedReference` reuse, so the source side is re-walked every
    /// iteration.
    ZensimC(f64),
}

impl TargetMetric {
    fn value(self) -> f64 {
        match self {
            TargetMetric::Ssim2(v) | TargetMetric::Zensim(v) | TargetMetric::ZensimC(v) => v,
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
/// **q0 seeding (`auto-tune` feature):** for [`TargetMetric::Ssim2`]
/// targets the search starts from the [`crate::q0_head`] content-aware
/// prediction instead of the fixed anchor curve (offline: mean encodes
/// 3.75 → 2.72 on held-out label-store curves — see the module docs).
/// Prediction failure, zensim targets, or building without `auto-tune`
/// fall back to the anchor curve unchanged. The search semantics,
/// selection policy, and convergence contract are identical either way —
/// only the starting point (and so the iterate count) differs.
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
    #[cfg(feature = "auto-tune")]
    let q_start = match target {
        TargetMetric::Ssim2(t) => {
            let rgb = contiguous_rgb8(img);
            crate::q0_head::predict_q0_for_rgb8(
                &rgb,
                img.width() as u32,
                img.height() as u32,
                t,
                config.speed_value(),
                None,
            )
        }
        // Not fitted for zensim targets — keep the anchor curve.
        TargetMetric::Zensim(_) | TargetMetric::ZensimC(_) => None,
    };
    #[cfg(not(feature = "auto-tune"))]
    let q_start = None;

    search_target(target, options, q_start, &stop, |q, stop| {
        // The quality search varies q at a fixed speed — use the single-encode
        // primitive so the monotonicity probe never nests inside the loop.
        let cfg = config.clone().quality(q);
        let enc = encode_rgb8_once(img, &cfg, stop.clone())?;
        let s = score_rgb8(target, img, &enc.avif_file, stop)?;
        Ok((enc, s))
    })
}

/// Tightly-packed RGB8 bytes for feature extraction. Strided sources get
/// one row-wise copy (trivial next to a single trial encode); contiguous
/// buffers copy once end-to-end.
#[cfg(feature = "auto-tune")]
fn contiguous_rgb8(img: ImgRef<'_, Rgb<u8>>) -> Vec<u8> {
    let (w, h) = (img.width(), img.height());
    let mut out = Vec::with_capacity(w * h * 3);
    for row in img.rows() {
        for p in row {
            out.extend_from_slice(&[p.r, p.g, p.b]);
        }
    }
    out
}

/// The kept encode from [`encode_rgb8_monotone`] plus probe provenance.
// Provenance fields are asserted by the monotone tests and kept for
// diagnostics; the shipping path consumes only `encoded` today.
#[allow(dead_code)]
#[cfg(feature = "auto-tune")]
#[derive(Debug)]
pub(crate) struct MonotoneEncoded {
    /// The chosen AVIF (the requested speed, or the anchor if it won the probe).
    pub encoded: EncodedImage,
    /// The speed whose encode was kept.
    pub speed_used: u8,
    /// Whether the pf-gate fired and a probe encode was run.
    pub probed: bool,
    /// Whether the probe swapped the requested encode for the anchor.
    pub swapped: bool,
    /// SSIMULACRA2/zensim score of the kept encode.
    pub score: f64,
}

/// `patch_fraction` floor for probing. Content at or below is photo-like and
/// CANNOT suffer the pattern-2 inversion — measured: 0 photos inverted across the
/// 24-origin armed validation (photos pf ≤ 0.389, inverters pf ≥ 0.518). Probing
/// only above this is what keeps the guarantee near-1× on photo-heavy traffic.
/// See docs/MONOTONICITY_PROGRAM.md "SELECTIVE probe".
#[cfg(feature = "auto-tune")]
pub(crate) const PROBE_PATCH_FRACTION_MIN: f32 = 0.45;

/// The reliable anchor tier: s4 Pareto-dominates the s6/7/8 bundle tiers on
/// bundle-hurts structured content (line plots, some screenshots).
#[cfg(feature = "auto-tune")]
pub(crate) const PROBE_ANCHOR_SPEED: u8 = 4;

/// The requested tiers that can suffer pattern-2 (the armed s6+ bundle tiers).
#[cfg(feature = "auto-tune")]
fn probe_eligible_speed(speed: u8) -> bool {
    matches!(speed, 6..=8)
}

/// Encode RGB8 to AVIF with a per-image RD-vs-time monotonicity guarantee at
/// near-1× cost (the answer to "a solution that isn't 2×").
///
/// The armed s6+ bundle can produce a point that's both slower AND worse than the
/// slower-preset s4 on some structured content (line plots) — so a *faster* tier
/// Pareto-dominates a slower one (a monotonicity violation). Content features
/// can't predict *which* structured images invert, but they cleanly separate
/// photo (never inverts) from structured (might). So:
/// - `patch_fraction ≤ `[`PROBE_PATCH_FRACTION_MIN`]` (photo-like) → encode once
///   at the requested speed (1× — inversion impossible here);
/// - otherwise → also encode the reliable anchor [`PROBE_ANCHOR_SPEED`] and keep
///   whichever *Pareto-dominates* on (bytes, score); ties keep the request.
///
/// The pick is deterministic (bytes + perceptual score, no wall-clock timing).
/// Release-gated by [`crate::fast_heads::MONOTONE_GATE_LIVE`]: on registry (arms
/// off) there is no inversion to fix, so it never probes (identical to a plain
/// [`encode_rgb8_once`], and crucially it does NOT even score — one encode, out).
/// Only the eligible bundle tiers (6/7/8) are probed; other speeds and photo-like
/// content pass straight through. This is the core of the automatic guarantee in
/// [`crate::encode_rgb8`]; the pick metric is SSIMULACRA2.
///
/// # Errors
/// Returns the first encode/decode/score error, or cancellation via `stop`.
#[cfg(feature = "auto-tune")]
pub(crate) fn encode_rgb8_monotone(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    stop: StopToken,
) -> Result<MonotoneEncoded> {
    probe_monotone_core(
        config.speed_value(),
        |sp| encode_rgb8_once(img, &config.clone().speed(sp), stop.clone()),
        || patch_fraction_rgb8(img),
        |e| score_rgb8(TargetMetric::Ssim2(0.0), img, &e.avif_file, &stop),
    )
}

/// The shared selective-probe core used by every pixel type's monotone path.
///
/// `encode_at(speed)` produces an encode at that speed (the requested encode is
/// always run; the anchor is run only when probing). `patch_fraction()` is called
/// lazily — **never on registry / non-bundle speeds**, so no feature extraction
/// happens where the probe can't fire. `score(&enc)` gives an encode's perceptual
/// score (SSIMULACRA2 for the pick). Returns the requested encode unless the
/// anchor Pareto-dominates it on (bytes, score) with a 0.05 score tie-band. All
/// deterministic — no wall-clock timing enters the decision.
#[cfg(feature = "auto-tune")]
fn probe_monotone_core(
    requested: u8,
    encode_at: impl Fn(u8) -> Result<EncodedImage>,
    patch_fraction: impl FnOnce() -> Option<f32>,
    score: impl Fn(&EncodedImage) -> Result<f64>,
) -> Result<MonotoneEncoded> {
    let enc_req = encode_at(requested)?;
    // `score` is NaN on the no-probe paths — never computed there (the whole point
    // of the near-1× property is that registry/photo encodes pay nothing extra).
    let unscored = |e: EncodedImage, probed: bool| MonotoneEncoded {
        encoded: e,
        speed_used: requested,
        probed,
        swapped: false,
        score: f64::NAN,
    };

    // Release-gated + tier-gated: nothing to fix on registry / non-bundle speeds.
    if !crate::fast_heads::MONOTONE_GATE_LIVE || !probe_eligible_speed(requested) {
        return Ok(unscored(enc_req, false));
    }
    // Selective: photo-like content cannot invert — skip the probe (the near-1×
    // property). Missing/degenerate features degrade to no-probe (safe: the worst
    // case is leaving a rare structured inversion, never a wrong pixel).
    match patch_fraction() {
        Some(pf) if pf > PROBE_PATCH_FRACTION_MIN => {}
        _ => return Ok(unscored(enc_req, false)),
    }

    // Structured content: score the request and probe the reliable anchor. Keep
    // the anchor only if it Pareto-dominates on (bytes, score); a ~0.05 score band
    // treats near-equal quality as a tie (keep the request).
    //
    // Best-effort on scoring: the probe is an optimization, so if a score can't be
    // computed we keep the already-valid requested encode rather than failing the
    // user's encode (score_rgba8 handles opaque decodes; this is defensive depth).
    // The anchor encode propagates errors — a real failure or cancellation there
    // should surface, not be masked.
    let Ok(score_req) = score(&enc_req) else {
        return Ok(unscored(enc_req, true));
    };
    let enc_a = encode_at(PROBE_ANCHOR_SPEED)?;
    let Ok(score_a) = score(&enc_a) else {
        return Ok(unscored(enc_req, true));
    };
    let (ba, br) = (enc_a.avif_file.len(), enc_req.avif_file.len());
    let anchor_not_worse = ba <= br && score_a + 0.05 >= score_req;
    let anchor_strictly_better = ba < br || score_a > score_req + 0.05;
    if anchor_not_worse && anchor_strictly_better {
        Ok(MonotoneEncoded {
            encoded: enc_a,
            speed_used: PROBE_ANCHOR_SPEED,
            probed: true,
            swapped: true,
            score: score_a,
        })
    } else {
        Ok(MonotoneEncoded {
            encoded: enc_req,
            speed_used: requested,
            probed: true,
            swapped: false,
            score: score_req,
        })
    }
}

/// The automatic monotonicity path behind [`crate::encode_rgb8`]: runs
/// [`encode_rgb8_monotone`] and returns just the chosen encode. Inert (a single
/// [`encode_rgb8_once`]) on registry, non-bundle speeds, and photo-like content.
#[cfg(feature = "auto-tune")]
pub(crate) fn encode_rgb8_auto_monotone(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    stop: StopToken,
) -> Result<EncodedImage> {
    encode_rgb8_monotone(img, config, stop).map(|m| m.encoded)
}

/// RGBA8 counterpart of [`encode_rgb8_monotone`] — same selective probe, scoring
/// via [`score_rgba8`] (ssim2 composites alpha on mid-gray). `patch_fraction` is
/// taken over the color channels (alpha dropped).
#[cfg(feature = "auto-tune")]
pub(crate) fn encode_rgba8_monotone(
    img: ImgRef<'_, rgb::Rgba<u8>>,
    config: &EncoderConfig,
    stop: StopToken,
) -> Result<MonotoneEncoded> {
    probe_monotone_core(
        config.speed_value(),
        |sp| encode_rgba8_once(img, &config.clone().speed(sp), stop.clone()),
        || patch_fraction_rgba8(img),
        |e| score_rgba8(TargetMetric::Ssim2(0.0), img, &e.avif_file, &stop),
    )
}

/// The automatic monotonicity path behind [`crate::encode_rgba8`] — returns just
/// the chosen encode.
#[cfg(feature = "auto-tune")]
pub(crate) fn encode_rgba8_auto_monotone(
    img: ImgRef<'_, rgb::Rgba<u8>>,
    config: &EncoderConfig,
    stop: StopToken,
) -> Result<EncodedImage> {
    encode_rgba8_monotone(img, config, stop).map(|m| m.encoded)
}

/// Extract `patch_fraction` (zenanalyze id 23) from an RGB8 source, or `None`
/// when unavailable (degenerate dims). Mirrors the feature request in
/// [`crate::fast_heads::monotone_speed_gate_for_rgb8`].
#[cfg(feature = "auto-tune")]
fn patch_fraction_rgb8(img: ImgRef<'_, Rgb<u8>>) -> Option<f32> {
    use zenanalyze::feature::{AnalysisFeature, AnalysisQuery, FeatureSet};
    let (w, h) = (img.width() as u32, img.height() as u32);
    if w == 0 || h == 0 {
        return None;
    }
    let rgb = contiguous_rgb8(img);
    let query = AnalysisQuery::new(FeatureSet::new().with(AnalysisFeature::PatchFraction));
    let analysis = zenanalyze::analyze_features_rgb8(&rgb, w, h, &query);
    analysis.get_f32(AnalysisFeature::PatchFraction)
}

/// `patch_fraction` over an RGBA8 source's COLOR channels (alpha dropped — the
/// feature is defined on opaque RGB). `None` on degenerate dims.
#[cfg(feature = "auto-tune")]
fn patch_fraction_rgba8(img: ImgRef<'_, rgb::Rgba<u8>>) -> Option<f32> {
    use zenanalyze::feature::{AnalysisFeature, AnalysisQuery, FeatureSet};
    let (w, h) = (img.width() as u32, img.height() as u32);
    if w == 0 || h == 0 {
        return None;
    }
    let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for row in img.rows() {
        for p in row {
            rgb.extend_from_slice(&[p.r, p.g, p.b]);
        }
    }
    let query = AnalysisQuery::new(FeatureSet::new().with(AnalysisFeature::PatchFraction));
    let analysis = zenanalyze::analyze_features_rgb8(&rgb, w, h, &query);
    analysis.get_f32(AnalysisFeature::PatchFraction)
}

/// Encode an RGBA8 image to AVIF, converging on a target perceptual score.
///
/// Same search and selection policy as [`encode_rgb8_with_target`]. The
/// q0 content seed applies to the RGB8 path only — this variant always
/// starts from the anchor curve (seeding it is future work).
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
    search_target(target, options, None, &stop, |q, stop| {
        // Fixed-speed quality search — use the primitive so the monotone probe
        // never nests inside the loop.
        let cfg = config.clone().quality(q);
        let enc = encode_rgba8_once(img, &cfg, stop.clone())?;
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
    search_target(target, options, None, &stop, |q, stop| {
        let cfg = config.clone().quality(q);
        let enc = crate::encoder::encode_rgb16(img, &cfg, stop.clone())?;
        let s = score_rgb16(target, img, &enc.avif_file, stop)?;
        Ok((enc, s))
    })
}

/// The bracketed secant/bisection search shared by every input variant.
/// `encode_and_score(quality, stop)` performs one full trial iteration.
/// `q_start` overrides the anchor-curve initial guess (the q0 head's
/// content-aware seed); `None` keeps [`initial_guess`].
fn search_target(
    target: TargetMetric,
    options: &TargetOptions,
    q_start: Option<f32>,
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
        return Err(at!(Error::InvalidParameters(format!(
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

    let mut q = q_start
        .filter(|q| q.is_finite())
        .unwrap_or_else(|| initial_guess(t))
        .clamp(min_q, max_q);
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
pub(crate) fn initial_guess(t: f64) -> f32 {
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
        TargetMetric::ZensimC(_) => zensim_c_score(&source, &dec_img),
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
    // With real transparency the decode is RGBA8. But a fully-OPAQUE input lets the
    // encoder drop the alpha plane entirely, so the decode is RGB8 — score it as
    // opaque (source alpha was 255, so compositing on gray / re-adding opaque alpha
    // is exact). Without this fallback, encoding an opaque RGBA8 image would error.
    if let Some(dec_img) = decoded.try_as_imgref::<rgb::Rgba<u8>>() {
        return match target {
            TargetMetric::Ssim2(_) => {
                let a = composite_on_gray(source);
                let b = composite_on_gray(dec_img);
                ssim2_score(a.as_ref(), b.as_ref())
            }
            TargetMetric::Zensim(_) => zensim_score(&source, &dec_img),
            TargetMetric::ZensimC(_) => zensim_c_score(&source, &dec_img),
        };
    }
    if let Some(dec_rgb) = decoded.try_as_imgref::<Rgb<u8>>() {
        return match target {
            TargetMetric::Ssim2(_) => {
                let a = composite_on_gray(source);
                let b = to_triplet_img(dec_rgb);
                ssim2_score(a.as_ref(), b.as_ref())
            }
            TargetMetric::Zensim(_) | TargetMetric::ZensimC(_) => {
                // Re-add opaque alpha so both sides are RGBA for zensim.
                let opaque: ImgVec<rgb::Rgba<u8>> = ImgVec::new(
                    dec_rgb
                        .pixels()
                        .map(|p| rgb::Rgba::new(p.r, p.g, p.b, 255))
                        .collect(),
                    dec_rgb.width(),
                    dec_rgb.height(),
                );
                if matches!(target, TargetMetric::ZensimC(_)) {
                    zensim_c_score(&source, &opaque.as_ref())
                } else {
                    zensim_score(&source, &opaque.as_ref())
                }
            }
        };
    }
    Err(at!(Error::Encode(
        "target-quality: decoded image not RGB(A)8-viewable".to_string()
    )))
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
        TargetMetric::ZensimC(_) => {
            let a = downconvert8(source);
            let b = downconvert8(dec_img);
            zensim_c_score(&a.as_ref(), &b.as_ref())
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
    let z = zensim::Zensim::new(zensim::ZensimProfile::codec_target());
    z.compute(a, b)
        .map(|r| r.score())
        .map_err(|e| at!(Error::Encode(format!("target-quality zensim: {e}"))))
}

/// Generation-C score. A fresh [`crate::zensim_c::ZensimC`] per call — the
/// scorer's only reusable state is the extraction scratch, and threading it
/// through the search's closure would buy one allocation per iteration
/// against an encode+decode. Measure before optimising that.
fn zensim_c_score(a: &impl zensim::ImageSource, b: &impl zensim::ImageSource) -> Result<f64> {
    crate::zensim_c::ZensimC::new().score(a, b)
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

#[cfg(all(test, feature = "auto-tune"))]
mod monotone_probe_tests {
    use super::*;
    use almost_enough::Unstoppable;
    use imgref::ImgVec;

    /// A high-`patch_fraction` checkerboard (structured content — would be
    /// probe-eligible once the arms are live).
    fn checkerboard(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
        let px: Vec<Rgb<u8>> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if ((x / 8) + (y / 8)) % 2 == 0 {
                    Rgb::new(255u8, 255, 255)
                } else {
                    Rgb::new(0u8, 0, 0)
                }
            })
            .collect();
        ImgVec::new(px, w, h)
    }

    #[test]
    fn monotone_probe_release_gated_on_registry() {
        // Structured content at an eligible tier: the ONLY thing keeping the probe
        // from firing here is MONOTONE_GATE_LIVE. While it's false (registry, no
        // inversion to fix), the probe must not run — no wasted anchor encode, the
        // requested speed is kept, and the encode is still valid.
        let img = checkerboard(64, 64);
        let cfg = EncoderConfig::new().speed(6).threads(Some(1));
        let out = encode_rgb8_monotone(img.as_ref(), &cfg, StopToken::new(Unstoppable))
            .expect("monotone encode");
        assert!(!out.encoded.avif_file.is_empty(), "produces a valid AVIF");
        if !crate::fast_heads::MONOTONE_GATE_LIVE {
            assert!(
                !out.probed,
                "must not probe on registry (no inversion to fix)"
            );
            assert!(!out.swapped);
            assert_eq!(out.speed_used, 6, "requested speed kept");
        }
    }

    #[test]
    fn monotone_probe_passes_through_ineligible_speed() {
        // s4 is not a bundle tier — it can never be the dominated side, so the probe
        // is skipped regardless of content or the release flag.
        let img = checkerboard(64, 64);
        let cfg = EncoderConfig::new().speed(4).threads(Some(1));
        let out = encode_rgb8_monotone(img.as_ref(), &cfg, StopToken::new(Unstoppable))
            .expect("monotone encode");
        assert!(!out.probed, "speed 4 is ineligible — never probed");
        assert_eq!(out.speed_used, 4);
    }

    #[test]
    fn monotone_probe_rgba8_release_gated_on_registry() {
        // The RGBA8 default path shares the generic probe core. Opaque structured
        // content at an eligible tier is probe-eligible only once the arms are live;
        // on registry it must pass straight through (valid encode, no probe).
        let rgb = checkerboard(64, 64);
        let rgba: Vec<rgb::Rgba<u8>> = rgb
            .pixels()
            .map(|p| rgb::Rgba::new(p.r, p.g, p.b, 255))
            .collect();
        let img = ImgVec::new(rgba, 64, 64);
        let cfg = EncoderConfig::new().speed(6).threads(Some(1));
        let out = encode_rgba8_monotone(img.as_ref(), &cfg, StopToken::new(Unstoppable))
            .expect("rgba8 monotone encode");
        assert!(!out.encoded.avif_file.is_empty(), "produces a valid AVIF");
        if !crate::fast_heads::MONOTONE_GATE_LIVE {
            assert!(!out.probed, "must not probe on registry");
            assert_eq!(out.speed_used, 6, "requested speed kept");
        }
    }

    #[test]
    fn score_rgba8_handles_opaque_alpha_dropped_decode() {
        // A fully-opaque RGBA8 encode drops its alpha plane, so the decode is RGB8.
        // score_rgba8 must score it as opaque, not error "not RGBA8-viewable" — else
        // the probe (and encode_rgba8_with_target) would fail on opaque input.
        let rgb = checkerboard(64, 64);
        let rgba: Vec<rgb::Rgba<u8>> = rgb
            .pixels()
            .map(|p| rgb::Rgba::new(p.r, p.g, p.b, 255))
            .collect();
        let img = ImgVec::new(rgba, 64, 64);
        let cfg = EncoderConfig::new().speed(6).threads(Some(1));
        let enc =
            encode_rgba8_once(img.as_ref(), &cfg, StopToken::new(Unstoppable)).expect("encode");
        let score = score_rgba8(
            TargetMetric::Ssim2(0.0),
            img.as_ref(),
            &enc.avif_file,
            &StopToken::new(Unstoppable),
        )
        .expect("opaque RGBA8 must score, not error");
        assert!(
            (0.0..=100.0).contains(&score),
            "sane ssim2 score, got {score}"
        );
    }
}
