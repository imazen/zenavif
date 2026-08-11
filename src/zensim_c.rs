//! Generation-**C** zensim scoring (`ZensimProfile::C`) and its attribution
//! steering map.
//!
//! # Why C needs its own module and `B` did not
//!
//! Every other zensim call in this crate is one line —
//! `Zensim::new(codec_target()).compute(src, dst)` — because profile `B`
//! consumes the standard 372-feature v1 pipeline that `compute` produces.
//! **`C` does not.** It is a 944-input MLP over the folded-720 + append +
//! append2 regime, and the 372-wide vector `compute` hands it is the wrong
//! shape. Feeding C through `compute` fails with
//! `ZensimError::ModelForwardFailed` — *except* on a byte-identical pair,
//! which short-circuits to 100 before the forward pass ever runs. So the
//! naive smoke test passes and hides the breakage; [`c_via_compute_fails`]
//! (in this module's tests) pins that trap open on purpose.
//!
//! The working sequence, mirrored from zensim's own
//! `examples/avif_sb_hints.rs`:
//!
//! ```text
//! compute_folded720_features_streaming(src, dst, {append, append2})  -> 944 f64
//! score_features_with_profile(ZensimProfile::C, feats, w, h)         -> 0..100
//! ```
//!
//! # The steering map: C HAS one, but it is not a diffmap
//!
//! The two-shot / closed-loop spatial channel was built on
//! `DiffmapResult::diffmap()` — a per-pixel **SSIM error** plane. That call
//! belongs to the v1 pipeline and C cannot use it.
//!
//! What C has instead is the **attribution density** (`AttributionResult`,
//! `zensim` feature `custom-profiles`): a per-pixel `f32` plane of the same
//! `width × height` shape, plus a summed-area table for O(1) rectangle
//! queries. It is a real per-pixel map — [`ZensimC::steer`] returns one —
//! but its semantics are **not** the diffmap's, and the difference decides
//! whether the existing pooling rule transfers:
//!
//! | | v1 diffmap (`B`) | attribution density (`C`) |
//! |---|---|---|
//! | sign | non-negative | **signed** |
//! | unit | unitless SSIM error | **score points**, first-order |
//! | absolute scale | none — only ratios mean anything | **yes**: `query_rect(B) ≈ Δscore` from re-encoding `B` at reference quality |
//! | zero | locally identical | locally identical *or* a cancelling mix |
//! | model dependence | none (fixed SSIM formula) | the active bake's gradient `∂score/∂f_k` |
//!
//! Consequences, spelled out because they are easy to get wrong:
//!
//! 1. **The `clamp(e_b / geomean(e_b), 0.4, 2.5)^(−strength)` rule does not
//!    transfer.** Its whole justification (see [`crate::two_pass_zensim`]
//!    module docs) is that SSIM error has *no absolute unit*, so a valuation
//!    must be invariant to rescaling the map — hence the geometric mean.
//!    Attribution *does* have an absolute unit, and a geometric mean is not
//!    even defined over a signed quantity: negative blocks (refining here is
//!    predicted to *lower* the score — real, e.g. a ringing block inside a
//!    globally blurred image) and exact zeros both fall out of `ln`.
//! 2. The mechanism-side signal is the per-block density **mean**
//!    (`query_rect / area`), not a p-norm pool. Summing is the map's native
//!    reduction — that is what the SAT is for — and the mean is what makes
//!    ragged edge blocks comparable to interior ones.
//! 3. Sign convention (zensim's, verbatim): *"POSITIVE = perceptually
//!    damaged = wants bits."*
//!
//! [`sb_q_scale_from_attribution`] therefore implements a **derived, not
//! fitted** policy of its own, documented on the function. Nothing in this
//! crate turns it on by default.
//!
//! # SDR only, structurally
//!
//! `C`'s HDR-gated append2 slots are pruned from the bake, so it is
//! SDR-only by construction; zensim routes HDR to `ZensimProfile::BHdr`,
//! which needs an absolute-luminance (PU-linear) front end this crate does
//! not have. Both facts are enforced here rather than assumed — see
//! [`profile_for_transfer`] and [`sdr_guard`].

use crate::error::{Error, Result};
use whereat::at;
use zensim::{ImageSource, ZensimProfile};

/// CICP transfer characteristics that declare absolute-luminance HDR:
/// 16 = SMPTE ST 2084 (PQ), 18 = ARIB STD-B67 (HLG).
///
/// Matches the `is_hdr` test in [`crate::detect`], which classifies an
/// AVIF's colour signalling from the same two code points.
pub const HDR_TRANSFER_CHARACTERISTICS: [u8; 2] = [16, 18];

/// Whether a CICP transfer-characteristics code declares HDR.
#[must_use]
pub fn transfer_is_hdr(tc: u8) -> bool {
    HDR_TRANSFER_CHARACTERISTICS.contains(&tc)
}

/// The zensim profile a piece of content routes to in the **generation-C**
/// dial, given its declared CICP transfer characteristics.
///
/// - HDR (PQ / HLG) → [`ZensimProfile::BHdr`], the only HDR-domain bake
///   zensim ships. `C` is structurally SDR — its HDR-gated append2 slots
///   are pruned — so there is no C-generation HDR answer to route to.
/// - anything else, including `None` (unsignalled) → [`ZensimProfile::C`].
///   Unsignalled AVIF is sRGB/BT.709 by convention, which is SDR.
///
/// This function only names the profile. It does **not** imply this crate
/// can *drive* `BHdr`: that needs an absolute-luminance PU-linear feature
/// front end which zenavif does not have, so the HDR branch exists to be
/// refused loudly by [`sdr_guard`], not to be silently scored. See the
/// module docs.
#[must_use]
pub fn profile_for_transfer(tc: Option<u8>) -> ZensimProfile {
    match tc {
        Some(tc) if transfer_is_hdr(tc) => ZensimProfile::BHdr,
        _ => ZensimProfile::C,
    }
}

/// Refuse HDR-declared content before it can be scored on an SDR profile.
///
/// Call this with [`EncoderConfig::transfer_characteristics`]'s value at
/// every entry point that scores with `C`. A typed error is the whole
/// point: an SDR bake fed HDR-coded pixels returns a *number*, and a wrong
/// number that looks right is worse than a refusal.
///
/// [`EncoderConfig::transfer_characteristics`]: crate::EncoderConfig::transfer_characteristics
///
/// # Errors
///
/// [`Error::Unsupported`] when `tc` is PQ (16) or HLG (18).
pub fn sdr_guard(tc: Option<u8>) -> Result<()> {
    match tc {
        Some(16) => Err(at!(Error::Unsupported(
            "zensim profile C is SDR-only (its HDR feature slots are pruned); \
             this content declares SMPTE ST 2084 (PQ). Score HDR with \
             ZensimProfile::BHdr through an absolute-luminance PU-linear \
             front end — zenavif does not have one wired."
        ))),
        Some(18) => Err(at!(Error::Unsupported(
            "zensim profile C is SDR-only (its HDR feature slots are pruned); \
             this content declares ARIB STD-B67 (HLG). Score HDR with \
             ZensimProfile::BHdr through an absolute-luminance PU-linear \
             front end — zenavif does not have one wired."
        ))),
        _ => Ok(()),
    }
}

/// Number of feature slots `ZensimProfile::C` consumes (the folded-720 +
/// append + append2 layout). The bake's *internal* layer-0 width is 667
/// after dead-column pruning; callers always size to 944.
pub const FOLDED_944: usize = 944;

/// A reusable generation-C scorer.
///
/// Holds the `Zensim` instance and the folded-944 extraction scratch so an
/// encode loop pays the buffer growth once. Construct once per source,
/// score every candidate encode against it.
///
/// **Reference reuse, honestly:** unlike the v1 path there is no
/// `PrecomputedReference` for the folded-944 extraction — the reference
/// side is re-walked on every [`score`](Self::score) call. Only the
/// `V2Scratch` allocation is reused. That is a real per-iteration cost
/// difference versus [`crate::two_pass_zensim`]'s B loop, not an oversight
/// on this side; zensim exposes no reference-reuse form of the 944
/// extraction at `e5627b56`.
pub struct ZensimC {
    z: zensim::Zensim,
    scratch: zensim::feature_v2::V2Scratch,
}

impl Default for ZensimC {
    fn default() -> Self {
        Self::new()
    }
}

/// The folded-944 toggle set: the append and append2 blocks on, everything
/// else at its default. Anything else produces a vector `C` will refuse.
fn toggles_944() -> zensim::feature_v2::V2NewFeatureToggles {
    zensim::feature_v2::V2NewFeatureToggles {
        append_block: true,
        append2_block: true,
        ..Default::default()
    }
}

impl ZensimC {
    /// A scorer with default parallelism and no stop token.
    #[must_use]
    pub fn new() -> Self {
        Self {
            z: zensim::Zensim::new(ZensimProfile::C),
            scratch: zensim::feature_v2::V2Scratch::new(),
        }
    }

    /// Install a cooperative-cancellation token (see `almost_enough`).
    ///
    /// **Honest limit:** at zensim `e5627b56` the token is checked by the
    /// v1 compare walks only — *"v2 extraction walks and the caller-paced
    /// strip APIs don't check the token yet"* (zensim CHANGELOG). So this
    /// stops [`steer`](Self::steer)'s attribution walk but **not** the
    /// folded-944 extraction that [`score`](Self::score) is made of. Set
    /// it anyway (it costs nothing and starts working the moment zensim
    /// threads it through), but do not rely on it to bound a 944 score.
    #[must_use]
    pub fn with_stop(mut self, stop: almost_enough::StopToken) -> Self {
        self.z = self.z.with_stop(stop);
        self
    }

    /// Force serial extraction — deterministic, and what a measurement
    /// harness wants.
    #[must_use]
    pub fn with_parallel(mut self, parallel: bool) -> Self {
        self.z = self.z.with_parallel(parallel);
        self
    }

    /// The folded-944 feature vector for a pair. Exposed because both the
    /// score and the gradient are functions of it and a caller that wants
    /// both should extract once.
    ///
    /// # Errors
    ///
    /// zensim's pair validations (dimension mismatch, too large, too
    /// small), or cancellation.
    pub fn features(
        &mut self,
        source: &impl ImageSource,
        distorted: &impl ImageSource,
    ) -> Result<Vec<f64>> {
        // THE SILENT PATH, closed. `compute_folded720_features_streaming`
        // does NOT refuse an HDR-flagged pair: when both sides are
        // `LinearF32Rgba` + `AlphaMode::Opaque` it quietly auto-routes to
        // the PU/HDR front end and returns 944 *HDR-domain* features
        // (zensim `feature_v2.rs`, `compute_folded720_streaming_impl`).
        // `score_features_with_profile` has no domain guard — it takes a
        // slice of the right width and forwards it — so C's SDR-trained
        // bake would return a finite, plausible, meaningless number.
        // `ZensimV2Result::regime()` cannot tell the two routes apart
        // either (both report `Folded720Append2`), so the flag has to be
        // checked here, before extraction.
        if source.is_hdr() || distorted.is_hdr() {
            return Err(at!(Error::Unsupported(
                "zensim profile C is SDR-only, and the pair declares HDR \
                 (ImageSource::is_hdr). The folded-944 extractor would \
                 silently return PU/HDR-domain features and C's SDR bake \
                 would score them without complaint. Route HDR to \
                 ZensimProfile::BHdr through an absolute-luminance \
                 PU-linear front end — zenavif does not have one wired."
            )));
        }
        let v2 = self
            .z
            .compute_folded720_features_streaming(
                source,
                distorted,
                toggles_944(),
                &mut self.scratch,
            )
            .map_err(|e| {
                at!(Error::Encode(format!(
                    "zensim-c: folded-944 extraction: {e}"
                )))
            })?;
        let feats = v2.into_features();
        if feats.len() != FOLDED_944 {
            return Err(at!(Error::Encode(format!(
                "zensim-c: folded extraction produced {} features, expected {FOLDED_944}",
                feats.len()
            ))));
        }
        Ok(feats)
    }

    /// Score an SDR pair under [`ZensimProfile::C`], 0–100.
    ///
    /// # Errors
    ///
    /// Extraction errors as [`features`](Self::features), plus
    /// [`Error::Encode`] wrapping a scoring failure.
    pub fn score(
        &mut self,
        source: &impl ImageSource,
        distorted: &impl ImageSource,
    ) -> Result<f64> {
        let (w, h) = (source.width() as u32, source.height() as u32);
        let feats = self.features(source, distorted)?;
        score_features(&feats, w, h)
    }

    /// Score **and** build the per-pixel attribution steering map.
    ///
    /// Three steps, all of them zensim's: extract the 944 features, take
    /// the bake's central-difference gradient at that point
    /// (`score_features_fd_gradient_with_profile`), then lay that gradient
    /// down as a per-pixel density (`compute_attribution_density_full`).
    ///
    /// **This is expensive.** The gradient runs two bake forwards per live
    /// feature column, and the density is a second full multi-scale walk on
    /// top of the extraction. Do not put it in an inner loop without
    /// measuring it first — [`crate::two_pass_zensim`]'s B path gets score
    /// and map from *one* call, and this does not.
    ///
    /// # Errors
    ///
    /// As [`score`](Self::score), plus a gradient or attribution failure.
    pub fn steer(
        &mut self,
        source: &impl ImageSource,
        distorted: &impl ImageSource,
    ) -> Result<AttributionSteer> {
        let (w, h) = (source.width() as u32, source.height() as u32);
        let feats = self.features(source, distorted)?;
        let score = score_features(&feats, w, h)?;
        let grad = zensim::score_features_fd_gradient_with_profile(ZensimProfile::C, &feats, w, h)
            .map_err(|e| at!(Error::Encode(format!("zensim-c: FD gradient: {e}"))))?;
        let attr = self
            .z
            .compute_attribution_density_full(source, distorted, &grad)
            .map_err(|e| at!(Error::Encode(format!("zensim-c: attribution: {e}"))))?;
        Ok(AttributionSteer { score, attr, grad })
    }
}

/// Score a folded-944 feature vector under [`ZensimProfile::C`].
///
/// # Errors
///
/// [`Error::Encode`] wrapping zensim's scoring error — most usefully
/// `ModelForwardFailed` when the vector is not 944 wide.
pub fn score_features(features: &[f64], width: u32, height: u32) -> Result<f64> {
    zensim::score_features_with_profile(ZensimProfile::C, features, width, height)
        .map_err(|e| at!(Error::Encode(format!("zensim-c: score: {e}"))))
}

/// A generation-C score plus its attribution steering map.
pub struct AttributionSteer {
    score: f64,
    attr: zensim::AttributionResult,
    grad: Vec<f64>,
}

impl AttributionSteer {
    /// The generation-C score, 0–100 (the dial can extrapolate below 0 for
    /// worse-than-worst-codec input — that is `C`'s `--neg-tail` contract,
    /// not a bug).
    #[must_use]
    pub fn score(&self) -> f64 {
        self.score
    }

    /// The signed per-pixel density, row-major `width * height`. Positive =
    /// refining here is predicted to raise the score.
    #[must_use]
    pub fn density(&self) -> &[f32] {
        self.attr.density()
    }

    /// Map width in pixels.
    #[must_use]
    pub fn width(&self) -> usize {
        self.attr.width()
    }

    /// Map height in pixels.
    #[must_use]
    pub fn height(&self) -> usize {
        self.attr.height()
    }

    /// How many of the 944 gradient components were non-zero. A gradient
    /// that is identically zero means the probe never engaged and every
    /// downstream map is meaningless — worth asserting on, which is why it
    /// is reported rather than hidden.
    #[must_use]
    pub fn gradient_nonzero(&self) -> usize {
        self.grad.iter().filter(|g| **g != 0.0).count()
    }

    /// Per-64×64-superblock density **means**, in frame superblock raster
    /// order — the mechanism-side steering signal, in score points per
    /// pixel. Positive = wants bits.
    ///
    /// Edge blocks divide by their clipped area, so a ragged right/bottom
    /// block is directly comparable to an interior one (the same reason
    /// [`crate::sb_pool::pool_pnorm`] normalizes by pixel count).
    #[must_use]
    pub fn sb_means(&self) -> Vec<f64> {
        let (w, h) = (self.attr.width(), self.attr.height());
        let sb = crate::sb_pool::SB;
        let (cols, rows) = crate::sb_pool::sb_grid(w, h);
        let mut out = Vec::with_capacity(cols * rows);
        for by in 0..rows {
            let y0 = by * sb;
            let y1 = (y0 + sb).min(h);
            for bx in 0..cols {
                let x0 = bx * sb;
                let x1 = (x0 + sb).min(w);
                let area = ((x1 - x0) * (y1 - y0)) as f64;
                out.push(if area > 0.0 {
                    self.attr.query_rect(x0, y0, x1, y1) / area
                } else {
                    0.0
                });
            }
        }
        out
    }
}

// ============================================================================
// The generation-C anchor curve. NOT the B one, and not close to it.
// ============================================================================

/// Score knots of the generation-C score → AVIF quantizer anchor curve.
/// Same grid as [`crate::two_pass_zensim`]'s `ANCHOR_SCORE`, so the two
/// tables can be read side by side.
const ANCHOR_SCORE_C: [f32; 15] = [
    20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0, 60.0, 65.0, 70.0, 75.0, 80.0, 85.0, 90.0,
];

/// Quantizer knots paired with [`ANCHOR_SCORE_C`] — **refitted for
/// `ZensimProfile::C`.** The B table is wrong for C by 20–30 quantizer
/// steps at the coarse end; do not reuse it.
///
/// **Fit provenance.** `scripts/hyperparam/fit_zensim_two_shot.py` over a
/// dense per-QUANTIZER lattice measured with
/// `ZENSIM_BENCH_PROFILE=c examples/zensim_loop_bench lattice`, 2026-08-07:
/// 12 TRAIN sources × long edges {64, 256}, 5,575 measured encodes, held
/// out against 12 disjoint VAL sources (5,695 encodes). Raw data
/// `benchmarks/zensim_c_lattice_{train,val}_2026-08-07.tsv.zst`, report
/// `benchmarks/zensim_c_two_shot_fit_2026-08-07.txt`.
///
/// **Measured worth of refitting** (VAL, 684 cell×target combos, paired on
/// identical cells): running C's scores against the *B* knots gives median
/// |err| 2.616; these knots give 1.579 — mean −0.576 closer, better on
/// 54.5% of pairs, sign-test p = 0.0002. Using B's *quality*-space step
/// instead is worse again at 3.177.
///
/// **Honest gap vs the B table's provenance:** the B knots were fitted
/// over long edges {64, 256, 1024}; this C fit has no 1024 tier (a budget
/// truncation, stated rather than hidden). Placement is much easier at
/// larger sizes — VAL median |err| is 3.637 at 64 and 0.791 at 256 — so a
/// 1024 leg would pull the aggregate down and would most likely move the
/// low-score knots, which is where the 64-px cells concentrate.
///
/// Descending (a higher score needs a finer, i.e. lower, quantizer).
const ANCHOR_QUANTIZER_C: [f32; 15] = [
    235.500, 228.000, 222.500, 213.000, 201.500, 192.000, 179.000, 162.000, 155.000, 148.500,
    127.500, 116.000, 98.500, 70.000, 33.500,
];

/// Piecewise-linear interpolation with linear end-segment extrapolation.
fn pwl(xs: &[f32; 15], ys: &[f32; 15], x: f32) -> f64 {
    let n = xs.len();
    // The knot grids here are monotone in opposite directions depending on
    // which way the lookup runs, so orient once and share the body.
    let ascending = xs[n - 1] > xs[0];
    let idx = if ascending {
        if x <= xs[0] {
            0
        } else if x >= xs[n - 1] {
            n - 2
        } else {
            xs.partition_point(|&v| v <= x).max(1) - 1
        }
    } else if x >= xs[0] {
        0
    } else if x <= xs[n - 1] {
        n - 2
    } else {
        xs.partition_point(|&v| v >= x).max(1) - 1
    };
    let (x0, x1) = (xs[idx], xs[idx + 1]);
    let (y0, y1) = (ys[idx], ys[idx + 1]);
    f64::from(y0) + f64::from(x - x0) / f64::from(x1 - x0) * f64::from(y1 - y0)
}

/// What AV1 quantizer index a typical image needs to reach generation-C
/// score `target` — the C twin of
/// [`crate::anchor_quantizer_for_zensim`].
///
/// Monotone non-increasing in `target`, linearly extrapolated outside the
/// knot range, clamped to `[0, 255]`. Fit provenance on
/// [`ANCHOR_QUANTIZER_C`].
#[must_use]
pub fn anchor_quantizer_for_zensim_c(target: f64) -> f64 {
    pwl(&ANCHOR_SCORE_C, &ANCHOR_QUANTIZER_C, target as f32).clamp(0.0, 255.0)
}

/// The inverse: what generation-C score a typical image lands on at
/// quantizer `qi`. The C twin of [`crate::anchor_zensim_for_quantizer`].
#[must_use]
pub fn anchor_zensim_c_for_quantizer(qi: f64) -> f64 {
    pwl(&ANCHOR_QUANTIZER_C, &ANCHOR_SCORE_C, qi as f32)
}

/// Turn per-superblock attribution means into an AC quantizer scale map.
///
/// # This mapping is DERIVED, not fitted — and it is not the B rule
///
/// The B loop's rule normalizes by the **geometric** mean because SSIM
/// error has no absolute unit. Attribution does have one and is signed, so
/// that rule is not merely untuned here, it is undefined. The derivation
/// used instead:
///
/// 1. `query_rect(B) ≈ Δscore` from refining `B`. Per pixel that is
///    `m_b = query_rect / area`: the marginal score available per pixel of
///    block `b`. It is additive across blocks, so the frame's natural
///    centre is the **area-weighted arithmetic mean** `m̄`, not a geometric
///    one — and `m̄` is exactly the whole-frame density mean.
/// 2. A block with `m_b > m̄` has more score per pixel available than the
///    frame average, so bits moved there buy more score: it should get a
///    finer quantizer. A block at or below zero has nothing to buy and is
///    pinned neutral rather than deliberately coarsened, because the
///    negative tail is dominated by cancelling first-order terms, not by
///    real "spend fewer bits here" evidence.
/// 3. Ratio `r_b = m_b / m̄`, clamped, then `r_b^(−strength)` — the same
///    high-rate power law and the same sign convention as the B rule
///    (larger weight ⇒ finer quantizer ⇒ exponent negative). `strength = 0`
///    is exactly neutral.
///
/// **Nothing here is fitted end-to-end.** `strength` and `clamp` are
/// carried over as mechanism, exactly as the B rule carried aom's pooling
/// exponent.
///
/// # Measured: this does not steer. Leave it off.
///
/// Matched-quantizer A/B, 48 cells (4 held-out sources × long edges
/// {256, 1024} × quantizers {60, 100, 140} × `strength` {0.5, 1.0}), map
/// live (`zenravif::FRAME_HINTS_LIVE == true`), gradient non-degenerate on
/// every cell. Record: `benchmarks/zensim_c_steer_ab_2026-08-07.tsv`.
///
/// - **Zero of 48 cells gained score without spending bytes.**
/// - Among the 13 near-rate-matched cells (`|Δbytes| ≤ 2.5 %`), median
///   `Δ`score is **−0.646** and only 2 improved.
/// - The map is **not rate-neutral**: the same quantizer with the map on
///   moves the file by −17 % to +78 %. The B rule's rate-neutrality
///   argument rests on the scales having geometric mean 1, and this policy
///   normalizes on an arithmetic mean, so it does not inherit that.
/// - The apparent +0.05 median `Δ`score at `strength = 0.5` is bytes, not
///   steering.
///
/// That convicts **this policy on this channel**, not the map. The same
/// directional result has now come out of per-SB delta-q three times — B's
/// diffmap, this, and zensim's own AVIF probe (campaign appendix Y.R3, 9/9
/// cells worse at matched rate) — which points at the channel. Two shared
/// confounds: `sb_q_scale` hints make zenrav1e disable its own
/// segmentation, and per-SB `delta_q` costs syntax bits. The untried lever
/// is the λ side (zenrav1e's per-16×16 `ssim_rdmult`: no syntax cost,
/// composes with segmentation).
///
/// Returns `cols * rows` scales in frame superblock raster order, `1.0`
/// neutral — all `1.0` when nothing is positive.
#[must_use]
pub fn sb_q_scale_from_attribution(
    sb_means: &[f64],
    clamp: (f64, f64),
    strength: f64,
) -> Box<[f32]> {
    let (lo, hi) = clamp;
    let mut sum = 0.0f64;
    let mut n = 0u32;
    for &m in sb_means {
        if m > 0.0 && m.is_finite() {
            sum += m;
            n += 1;
        }
    }
    if n == 0 || strength == 0.0 {
        return vec![1.0f32; sb_means.len()].into_boxed_slice();
    }
    let mean = sum / f64::from(n);
    sb_means
        .iter()
        .map(|&m| {
            if m > 0.0 && m.is_finite() {
                ((m / mean).clamp(lo, hi)).powf(-strength) as f32
            } else {
                1.0f32
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zensim::RgbSlice;

    /// A deterministic photo-ish source: smooth gradient plus a textured
    /// patch, so the pair has real structure at more than one scale.
    fn source(w: usize, h: usize) -> Vec<[u8; 3]> {
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let base = ((x * 255) / w.max(1)) as u8;
                let tex = if x > w / 2 && y > h / 2 {
                    (((x * 7 + y * 13) % 61) * 4) as u8
                } else {
                    0
                };
                [base.wrapping_add(tex), (y * 255 / h.max(1)) as u8, 96]
            })
            .collect()
    }

    /// The same image with one quadrant crushed — a localized, obviously
    /// worse region the steering map has to find.
    fn damaged(src: &[[u8; 3]], w: usize, h: usize) -> Vec<[u8; 3]> {
        src.iter()
            .enumerate()
            .map(|(i, p)| {
                let (x, y) = (i % w, i / w);
                if x < w / 2 && y < h / 2 {
                    [p[0] & 0xC0, p[1] & 0xC0, p[2] & 0xC0]
                } else {
                    *p
                }
            })
            .collect()
    }

    #[test]
    fn c_scores_a_real_pair_and_disagrees_with_b() {
        let (w, h) = (192usize, 192usize);
        let s = source(w, h);
        let d = damaged(&s, w, h);
        let (ss, ds) = (RgbSlice::new(&s, w, h), RgbSlice::new(&d, w, h));

        let c = ZensimC::new().with_parallel(false).score(&ss, &ds).unwrap();
        let b = zensim::Zensim::new(ZensimProfile::codec_target())
            .compute(&ss, &ds)
            .unwrap()
            .score();

        assert!(c.is_finite(), "C score not finite: {c}");
        assert!(
            (-50.0..=100.0).contains(&c),
            "C score {c} outside the plausible dial range"
        );
        // Two different bakes over two different feature regimes: equal to
        // 1e-9 would mean one of them is not actually running.
        assert!(
            (c - b).abs() > 1e-6,
            "C ({c}) and B ({b}) produced the same number — C is not really scoring"
        );
        // Damage this gross has to read as damage on both dials.
        assert!(c < 99.0, "C scored a visibly crushed quadrant at {c}");
    }

    /// THE TRAP, pinned open: `compute` (the 372-feature v1 pipeline) is
    /// the wrong front end for C. A non-identical pair must FAIL rather
    /// than return a plausible-looking number — and an identical pair
    /// short-circuits to 100 *before* the forward pass, which is exactly
    /// why a naive smoke test passes and proves nothing.
    #[test]
    fn c_via_compute_fails() {
        let (w, h) = (96usize, 96usize);
        let s = source(w, h);
        let d = damaged(&s, w, h);
        let z = zensim::Zensim::new(ZensimProfile::C).with_parallel(false);

        let err = z
            .compute(&RgbSlice::new(&s, w, h), &RgbSlice::new(&d, w, h))
            .expect_err("compute() must refuse C's 944 bake a 372-wide vector");
        assert!(
            matches!(err, zensim::ZensimError::ModelForwardFailed { .. }),
            "unexpected error from compute() under C: {err:?}"
        );

        // The short-circuit that hides it.
        let identical = z
            .compute(&RgbSlice::new(&s, w, h), &RgbSlice::new(&s, w, h))
            .expect("identical pairs short-circuit before the forward pass");
        assert!((identical.score() - 100.0).abs() < 1e-9);
    }

    /// The extraction the module drives must agree with the one C's own
    /// rustdoc names. If zensim ever makes these differ, this fails rather
    /// than silently scoring a different vector.
    ///
    /// # Why every pairing here is an invariant zensim promises
    ///
    /// Both sides reach ONE zensim function with identical arguments (traced
    /// at the pinned rev `a390a182`):
    /// `Zensim::compute_folded720_features_streaming` forwards straight to
    /// `feature_v2::compute_folded720_streaming_impl(src, dst, self.max_pixels,
    /// self.parallel, toggles, scratch)` (metric.rs:1760-1775), and
    /// `Zensim::compute_folded720_append2_features` reaches the *same* function
    /// via `compute_folded720_append2_impl` — which forces exactly the two
    /// toggles `toggles_944()` sets (feature_v2.rs:5671-5683) — then
    /// `compute_folded720_impl_with_toggles`, which is nothing but
    /// `let mut scratch = V2Scratch::new();` plus that same call
    /// (feature_v2.rs:4993-5009). The materialized walk these wrappers used to
    /// run was deleted at zensim's C5 switchover; the pair entries route to the
    /// streaming walk now.
    ///
    /// So the ONLY thing that differs between these calls is which `V2Scratch`
    /// instance they were handed — and zensim's rustdoc calls the output
    /// "bit-for-bit" / "BITWISE equal", gated upstream by
    /// `streamed_foldapp_bitwise_vs_materialized` (which compares `to_bits()`,
    /// but only for the 720 and 924 layouts — the 944/append2 toggle set this
    /// module drives has no upstream bitwise gate).
    ///
    /// The four pairings are therefore all promises, checked at one tolerance
    /// and reported TOGETHER rather than short-circuiting on the first, so a
    /// single failing run says which promise broke: determinism under scratch
    /// reuse, independence from the scratch instance, streaming-vs-wrapper, and
    /// determinism of the wrapper.
    ///
    /// # Why the token lock, and why this looked like an upstream bug
    ///
    /// This test was red on `ubuntu-24.04-arm`, then on `windows-11-arm`, then
    /// on `macos-latest`, with a *different* worst feature and magnitude each
    /// run (f0 @ 2.7e-5, f143 @ 5.1e-4, f417 @ 4.6e-5, f864 @ 1.7e-3) — and
    /// never once locally, in 60/60 runs, at four geometries, or with `parallel`
    /// either way. It is not zensim's nondeterminism. It is this crate's own
    /// test suite:
    ///
    /// `yuv_convert::tests::every_simd_tier_is_byte_identical` disables archmage
    /// tokens **process-wide**, libtest runs unit tests concurrently in ONE
    /// process, and **zensim dispatches on archmage too** (`zensim/Cargo.toml`
    /// depends on `archmage` + `magetypes`). zensim's kernels make no
    /// cross-tier bit-identity promise — unlike this crate's YUV kernels, which
    /// that very test exists to pin — so a permutation run overlapping this test
    /// silently moves zensim's SIMD tier between two of the five calls below,
    /// and the pairing that straddles the change "diverges".
    ///
    /// Proven, not inferred: with `lock_token_testing()` held, disabling
    /// `NeonToken` process-wide and re-extracting the SAME pair moves f864 from
    /// `0.020007017572121793` to `0.020032447419402563` (rel 1.271e-3), and
    /// re-enabling it returns to the base value exactly (0.000e0). Those are the
    /// same feature and the same base value CI reported. The same mechanism made
    /// `simd::avg::tests::test_avg_neon_direct_matches_scalar` fail with "NEON
    /// must be available on aarch64" on macos-latest in run 31530942816.
    ///
    /// So the lock below is the fix, not a workaround: it takes the same mutex
    /// `for_each_token_permutation` holds, which is archmage's documented way to
    /// "observe stable `summon()` results alongside parallel permutation tests"
    /// (archmage 0.9.15 src/testing.rs:53-57). Any future test in this crate
    /// that compares third-party SIMD numerics across calls needs it too.
    /// imazen/zensim#60 was filed against zensim for this and is retracted.
    #[test]
    fn streaming_and_direct_folded944_agree() {
        let _tokens = archmage::testing::lock_token_testing();
        let (w, h) = (96usize, 96usize);
        let s = source(w, h);
        let d = damaged(&s, w, h);
        let (ss, ds) = (RgbSlice::new(&s, w, h), RgbSlice::new(&d, w, h));
        let z = zensim::Zensim::new(ZensimProfile::C).with_parallel(false);

        let mut scratch = zensim::feature_v2::V2Scratch::new();
        let streamed = z
            .compute_folded720_features_streaming(&ss, &ds, toggles_944(), &mut scratch)
            .unwrap();
        // Same scratch, second call: pins that the walk resets what it reuses.
        let reused_scratch = z
            .compute_folded720_features_streaming(&ss, &ds, toggles_944(), &mut scratch)
            .unwrap();
        // A different fresh scratch: pins independence from the instance, which
        // is the only argument that differs from the wrapper call below.
        let mut scratch_b = zensim::feature_v2::V2Scratch::new();
        let fresh_scratch = z
            .compute_folded720_features_streaming(&ss, &ds, toggles_944(), &mut scratch_b)
            .unwrap();
        let direct = z.compute_folded720_append2_features(&ss, &ds).unwrap();
        let direct_again = z.compute_folded720_append2_features(&ss, &ds).unwrap();

        for (what, r) in [
            ("streamed", &streamed),
            ("reused_scratch", &reused_scratch),
            ("fresh_scratch", &fresh_scratch),
            ("direct", &direct),
            ("direct_again", &direct_again),
        ] {
            assert_eq!(
                r.features().len(),
                FOLDED_944,
                "{what} is not the 944 regime"
            );
        }
        assert_eq!(
            streamed.regime(),
            direct.regime(),
            "the streaming call and the wrapper report different extraction \
             regimes — the toggle sets have diverged, which is a different \
             vector and not a rounding question"
        );

        let mut report = String::new();
        for (what, other) in [
            ("streamed vs same-scratch second call", &reused_scratch),
            ("streamed vs a second fresh scratch", &fresh_scratch),
            ("streamed vs the append2 wrapper", &direct),
            ("streamed vs the append2 wrapper, twice", &direct_again),
        ] {
            let mut worst: Option<(usize, f64, f64)> = None;
            for (i, (a, b)) in streamed
                .features()
                .iter()
                .zip(other.features().iter())
                .enumerate()
            {
                if (a - b).abs() > 1e-12 * a.abs().max(1.0) {
                    let rel = (a - b).abs() / a.abs().max(1e-12);
                    if worst.is_none_or(|(_, wa, wb)| rel > (wa - wb).abs() / wa.abs().max(1e-12)) {
                        worst = Some((i, *a, *b));
                    }
                }
            }
            if let Some((i, a, b)) = worst {
                let rel = (a - b).abs() / a.abs().max(1e-12);
                report.push_str(&format!(
                    "\n  {what}: worst at feature {i}: {a} vs {b} (rel {rel:.3e})"
                ));
            }
        }
        assert!(
            report.is_empty(),
            "zensim's folded-944 extraction is not reproducing itself. All of \
             these calls are the SAME zensim function with the SAME arguments, \
             differing only in the V2Scratch instance, and zensim documents the \
             output as bitwise equal. If this fires, check FIRST that no test in \
             this crate is disabling archmage tokens process-wide without \
             `lock_token_testing()` — that, not zensim, is what produced every \
             previous failure here (see this test's docs):{report}"
        );
    }

    #[test]
    fn c_anchor_curve_is_monotone_and_round_trips() {
        // Monotone non-increasing in the target, over and past the knots.
        let mut prev = f64::INFINITY;
        let mut t = -20.0f64;
        while t <= 120.0 {
            let q = anchor_quantizer_for_zensim_c(t);
            assert!(
                q <= prev + 1e-6,
                "anchor_quantizer_for_zensim_c not monotone at {t}: {q} > {prev}"
            );
            assert!((0.0..=255.0).contains(&q), "qi {q} out of range at {t}");
            prev = q;
            t += 0.5;
        }
        // The two directions agree at every knot (that is what makes the
        // two-shot's "predict the score at the quantizer it picked" step
        // meaningful at all).
        for (s, q) in ANCHOR_SCORE_C.iter().zip(ANCHOR_QUANTIZER_C.iter()) {
            let back = anchor_zensim_c_for_quantizer(f64::from(*q));
            assert!(
                (back - f64::from(*s)).abs() < 1e-3,
                "knot {s} -> qi {q} -> {back}"
            );
            let fwd = anchor_quantizer_for_zensim_c(f64::from(*s));
            assert!((fwd - f64::from(*q)).abs() < 1e-3, "knot {s}: {fwd} vs {q}");
        }
    }

    /// The C curve must not silently BE the B curve — if a future edit
    /// pastes B's knots in here, the refit is undone and the two-shot's
    /// placement error roughly doubles (measured: VAL median |err| 1.579
    /// with these knots vs 2.616 with B's).
    #[cfg(feature = "two-pass-zensim")] // where the B table is re-exported
    #[test]
    fn c_anchor_curve_is_not_the_b_curve() {
        for &s in &[20.0f64, 40.0, 60.0, 80.0] {
            let c = anchor_quantizer_for_zensim_c(s);
            let b = crate::anchor_quantizer_for_zensim(s);
            assert!(
                (c - b).abs() > 5.0,
                "C and B anchors agree to within 5 quantizer steps at score {s} \
                 ({c} vs {b}) — has the C table been overwritten with B's?"
            );
        }
    }

    #[test]
    fn hdr_is_refused_by_transfer_code_not_by_pixel_sniffing() {
        assert!(transfer_is_hdr(16) && transfer_is_hdr(18));
        assert!(!transfer_is_hdr(1) && !transfer_is_hdr(13) && !transfer_is_hdr(0));
        assert_eq!(profile_for_transfer(Some(16)), ZensimProfile::BHdr);
        assert_eq!(profile_for_transfer(Some(18)), ZensimProfile::BHdr);
        assert_eq!(profile_for_transfer(Some(13)), ZensimProfile::C);
        assert_eq!(profile_for_transfer(None), ZensimProfile::C);

        for tc in [16u8, 18] {
            let err = sdr_guard(Some(tc)).expect_err("HDR must be refused");
            assert!(
                matches!(err.error(), Error::Unsupported(_)),
                "wrong error type for tc {tc}: {err}"
            );
        }
        sdr_guard(Some(13)).expect("sRGB is SDR");
        sdr_guard(None).expect("unsignalled is SDR by convention");
    }

    /// A minimal HDR-flagged source in exactly the shape that reaches
    /// zensim's silent auto-route: `LinearF32Rgba` + `AlphaMode::Opaque`
    /// + `is_hdr() == true`. Anything else already errors inside zensim.
    struct PqHdrSource {
        data: Vec<u8>,
        width: usize,
        height: usize,
    }

    impl zensim::ImageSource for PqHdrSource {
        fn width(&self) -> usize {
            self.width
        }
        fn height(&self) -> usize {
            self.height
        }
        fn pixel_format(&self) -> zensim::PixelFormat {
            zensim::PixelFormat::LinearF32Rgba
        }
        fn alpha_mode(&self) -> zensim::AlphaMode {
            zensim::AlphaMode::Opaque
        }
        fn color_primaries(&self) -> zensim::ColorPrimaries {
            zensim::ColorPrimaries::Bt2020
        }
        fn is_hdr(&self) -> bool {
            true
        }
        fn row_bytes(&self, y: usize) -> &[u8] {
            let bpp = 16;
            &self.data[y * self.width * bpp..(y + 1) * self.width * bpp]
        }
    }

    /// The one path that would hand C a finite, plausible, MEANINGLESS
    /// number: an HDR-flagged linear-f32 opaque pair silently auto-routes
    /// the folded-944 extractor to the PU/HDR front end, and
    /// `score_features_with_profile` has no domain guard, so C's SDR bake
    /// scores HDR-domain features without complaint. `ZensimC::features`
    /// refuses before extraction; this test is what keeps it refusing.
    #[test]
    fn hdr_flagged_source_is_refused_before_it_can_be_silently_scored() {
        let (w, h) = (96usize, 96usize);
        let px = vec![0u8; w * h * 16];
        let src = PqHdrSource {
            data: px.clone(),
            width: w,
            height: h,
        };
        let dst = PqHdrSource {
            data: px,
            width: w,
            height: h,
        };
        let err = ZensimC::new()
            .with_parallel(false)
            .score(&src, &dst)
            .expect_err("HDR-flagged pair must not reach C's SDR bake");
        assert!(
            matches!(err.error(), Error::Unsupported(_)),
            "wrong error type: {err}"
        );
    }

    #[test]
    fn attribution_is_a_real_per_pixel_map_that_finds_the_damage() {
        let (w, h) = (192usize, 192usize);
        let s = source(w, h);
        let d = damaged(&s, w, h);
        let steer = ZensimC::new()
            .with_parallel(false)
            .steer(&RgbSlice::new(&s, w, h), &RgbSlice::new(&d, w, h))
            .unwrap();

        assert_eq!(steer.width(), w);
        assert_eq!(steer.height(), h);
        assert_eq!(steer.density().len(), w * h);
        assert!(
            steer.gradient_nonzero() > 0,
            "gradient identically zero — the probe never engaged"
        );
        assert!(
            steer.density().iter().all(|v| v.is_finite()),
            "attribution density contains non-finite values"
        );

        // 192/64 = 3x3 superblocks; the crushed quadrant covers block (0,0)
        // entirely and half of (1,0)/(0,1). Block (0,0) must be the frame's
        // hungriest, and the clean bottom-right corner must not beat it.
        let means = steer.sb_means();
        assert_eq!(means.len(), 9);
        let worst = means
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap();
        assert_eq!(
            worst, 0,
            "the crushed top-left superblock should want the most bits; means = {means:?}"
        );
        assert!(
            means[0] > means[8],
            "damaged block {} not above the clean corner {}",
            means[0],
            means[8]
        );
    }

    #[test]
    fn attribution_q_scale_is_neutral_at_zero_strength_and_signed_otherwise() {
        let means = [1.0, 2.0, 4.0, -1.0, 0.0];
        let neutral = sb_q_scale_from_attribution(&means, (0.4, 2.5), 0.0);
        assert!(neutral.iter().all(|&s| s == 1.0));

        // mean of the positives = (1+2+4)/3 = 7/3.
        let out = sb_q_scale_from_attribution(&means, (0.1, 10.0), 1.0);
        let m = 7.0 / 3.0;
        assert!((f64::from(out[0]) - m / 1.0).abs() < 1e-5, "{}", out[0]);
        assert!((f64::from(out[2]) - m / 4.0).abs() < 1e-5, "{}", out[2]);
        // A larger weight means a FINER quantizer.
        assert!(out[2] < out[1] && out[1] < out[0]);
        // Non-positive blocks are pinned neutral, never coarsened.
        assert_eq!(out[3], 1.0);
        assert_eq!(out[4], 1.0);

        // All non-positive => nothing to steer with.
        let dead = sb_q_scale_from_attribution(&[-1.0, 0.0], (0.4, 2.5), 1.0);
        assert!(dead.iter().all(|&s| s == 1.0));
    }
}
