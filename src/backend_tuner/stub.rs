//! **The measured-default tuner** — a real decision with no model file.
//!
//! [`StubTuner`] exists so a consumer can wire the whole production path
//! ([`AvifTuning`](super::AvifTuning), [`TuneRequest`](super::TuneRequest),
//! [`AvifTune`](super::AvifTune)) *before* the trained bake exists, then
//! swap in [`AvifTuner`](super::AvifTuner) by changing which value it
//! holds. Everything downstream — the config it builds, the masks it
//! honours, the errors it raises — is the same code path.
//!
//! # It is not a guess, and it is not a model
//!
//! Every constant below is transcribed from a committed campaign table,
//! with its provenance. Where the campaign measured *no* usable effect,
//! this module encodes that too — as an **absent** knob (the encoder's
//! own default), never an invented one.
//! [`AvifTune::source`](super::AvifTune::source) always reports
//! [`TuneSource::Stub`](super::TuneSource::Stub) and
//! [`AvifTune::expected_bytes`](super::AvifTune::expected_bytes) is
//! always `None` — the stub predicts nothing about size.
//!
//! # What it decides, and on what evidence
//!
//! Two axes, both measured; the knob axes are deliberately left alone
//! (see [`default_cell`] for why each one was not set).
//!
//! 1. **Reach** — at a high quality target, prefer zenravif.
//!    `avif_backend_selection_2026-09-03.md` §3.3 measured that
//!    svt-as-configured cannot reach ssim2 90 on **16 of 32** references
//!    at any q and any speed (**6 of 6** plots, **5 of 5** screenshots,
//!    0 of 7 photos), and that *"the best of all 118 svt arms does not
//!    fix it"*; zenravif misses on **1 of 32**.
//! 2. **Time** — under a budget, prefer the fastest cell that fits, from
//!    the measured `alpha + beta * MP` table. At 1 MP and speed 6 that is
//!    66 ms (svt) against 2,971 ms (zenravif); the backend doc's iso-time
//!    read is that at a **100 ms** budget zenravif is over budget on
//!    **31 of 32** references.
//!
//! When the two conflict, **the budget wins and the reach preference is
//! recorded as unmet** — a caller that asked for 100 ms cannot be handed
//! a 2-second encode because the tuner preferred it.
//!
//! # What is deliberately NOT here
//!
//! - **No content classification.** The largest single-knob win in the
//!   DOE (`scm3`, median **-50.08%** where it fires) is confined to
//!   screen-adjacent content and is **byte-identical to the control at
//!   speeds 4 and 6** — it exists only at speed 7. Routing on it needs
//!   the per-image signal a trained bake carries; the stub has none, and
//!   a corpus median would read it as dead (corpus-wide median **0.0000%**).
//! - **No aom row.** `zenav1-aom` has **no** measured knob, RD or speed
//!   number anywhere in the campaign (block A3 was never declared).

use super::contract::TuneCell;
use super::{
    AllowedBackends, AvifTune, AvifTuning, TuneRequest, TuneSource, cell_is_viable, config_for_cell,
};
use crate::Av1Backend;
use crate::auto_tune::AutoTuneError;

/// A measured wall-time fit for one (backend, speed) arm.
///
/// **`alpha + beta * megapixels`, and `alpha` is load-bearing.** The
/// speed instrument measured that quoting a bare ms/MP — dropping
/// `alpha` — misprices small images by roughly **20x** (at svt speed 1
/// and a 64² rung, the slope contributes 39 ms and the intercept
/// hundreds). So there is no ms/MP-only form of this type.
///
/// # Read the caveats before quoting a number from this
///
/// All from `benchmarks/avif_speed_instrument_2026-09-03.md` (zenmetrics):
///
/// - **The pooled fit is wrong by up to 24.3x per image.** Per-source
///   fits are clean (median R² 0.9928–0.9997) but the pooled fit reaches
///   only R² 0.625–0.906, and `beta` spreads **1.95x to 24.33x** across
///   sources. Every one of the 20 arms is flagged
///   `linear_model_failed = True, fail_reason = POOLING_NOT_MODEL`.
///   Six arms fit a **negative** intercept, which is that failure
///   showing through — not a real fixed-cost saving.
/// - **q45 only.** Registered q-flatness was **falsified**: the measured
///   relative spread across q{15,45,90} is 75.1% (svt) and 42.4%
///   (zenravif), ~23x the registered tolerance.
/// - **Wall time, not CPU time.** svt threads internally to ~1.638 mean
///   cores at native size while the ladder was fitted at 1.000.
/// - **Absolute ms are host-specific** (r7900x). Backend *ratios* travel;
///   absolute times do not.
/// - **Knob time is not measured at all** — only the speed dial's cost.
///
/// A budget checked against this is therefore a preference, not a
/// deadline, which is what
/// [`TuneRequest::time_budget_ms`](super::TuneRequest::time_budget_ms)
/// documents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallTimeModel {
    /// Fixed per-call cost in milliseconds, independent of pixel count.
    ///
    /// May be **negative** on the six arms where the pooled fit failed;
    /// [`estimate_ms`](Self::estimate_ms) clamps the result at zero
    /// rather than pretending a negative time is meaningful.
    pub alpha_ms: f32,
    /// Milliseconds per megapixel.
    pub beta_ms_per_mp: f32,
}

impl WallTimeModel {
    /// Estimated wall time in milliseconds for `megapixels`, clamped at
    /// zero (six arms carry a negative intercept — see the type docs).
    pub fn estimate_ms(&self, megapixels: f32) -> f32 {
        (self.alpha_ms + self.beta_ms_per_mp * megapixels.max(0.0)).max(0.0)
    }
}

/// The measured wall-time estimate for a cell, or `None` when the
/// campaign has no fit for that (backend, speed) pair.
///
/// `None` means **NOT MEASURED** — the time budget then cannot mask that
/// cell, and the tune reports `expected_wall_ms: None`. It never means
/// "free".
pub fn estimated_wall_ms(cell: &TuneCell, megapixels: f32) -> Option<f32> {
    wall_time_model(cell.backend(), cell.speed()).map(|m| m.estimate_ms(megapixels))
}

/// Look up the measured `alpha + beta * MP` fit for a (backend, speed)
/// pair, or `None` when there is none.
///
/// Never interpolates across speed presets: `beta` is not monotone in
/// the preset for either backend (zenravif reads 2,124.68 ms/MP at speed
/// 5 and 2,920.30 at speed 6), so an interpolated row would be a
/// fabrication.
pub fn wall_time_model(backend: Av1Backend, speed: u8) -> Option<WallTimeModel> {
    MEASURED_WALL_TIME
        .iter()
        .find(|(b, s, _)| *b == backend && *s == speed)
        .map(|(_, _, m)| *m)
}

/// Measured `(backend, speed, alpha + beta*MP)` rows.
///
/// # Provenance
///
/// Transcribed verbatim from
/// `/mnt/v/output/avif-speed-instrument-2026-09-03/speed_alpha_beta.tsv`
/// (sha256 `c7f63157de85c68527c949ffa4fa1d797dfead4606774a5f1160ce28012837e7`,
/// 20 rows, `_MANIFEST.json` `build_commit d1928710`, host r7900x), the
/// **min-of-3** definitive form. Record:
/// `zenmetrics/benchmarks/avif_speed_instrument_2026-09-03.md`.
///
/// ⚠ The markdown's own §6.2 table is the *partial-pass* version and its
/// first rows are superseded twice. **Read the TSV, never the prose
/// table** — these values came from the TSV.
///
/// `zenav1-aom` has **no rows**: the campaign never measured it. That
/// absence is the honest state, not an omission to be filled by analogy
/// with another backend.
static MEASURED_WALL_TIME: &[(Av1Backend, u8, WallTimeModel)] = &[
    // ── zenav1-svt (svt-rs) ──
    (Av1Backend::Zenav1Svt, 1, w(-222.85, 13133.5)),
    (Av1Backend::Zenav1Svt, 2, w(59.7307, 5664.84)),
    (Av1Backend::Zenav1Svt, 3, w(-8.99486, 1623.15)),
    (Av1Backend::Zenav1Svt, 4, w(-23.3809, 957.977)),
    (Av1Backend::Zenav1Svt, 5, w(-11.4648, 330.729)),
    (Av1Backend::Zenav1Svt, 6, w(1.16066, 65.0011)),
    (Av1Backend::Zenav1Svt, 7, w(0.518145, 28.2273)),
    (Av1Backend::Zenav1Svt, 8, w(0.518446, 28.1575)),
    (Av1Backend::Zenav1Svt, 9, w(0.513644, 28.1242)),
    (Av1Backend::Zenav1Svt, 10, w(0.531631, 28.1311)),
    // ── zenravif (zenrav1e) ──
    (Av1Backend::Zenravif, 1, w(2164.41, 48777.9)),
    (Av1Backend::Zenravif, 2, w(1964.04, 31989.0)),
    (Av1Backend::Zenravif, 3, w(346.932, 18116.6)),
    (Av1Backend::Zenravif, 4, w(242.809, 9296.02)),
    (Av1Backend::Zenravif, 5, w(32.2129, 2124.68)),
    (Av1Backend::Zenravif, 6, w(50.99, 2920.3)),
    (Av1Backend::Zenravif, 7, w(30.5523, 1956.17)),
    (Av1Backend::Zenravif, 8, w(30.8903, 1956.1)),
    (Av1Backend::Zenravif, 9, w(-1.91471, 706.543)),
    (Av1Backend::Zenravif, 10, w(-0.523373, 392.458)),
];

const fn w(alpha_ms: f32, beta_ms_per_mp: f32) -> WallTimeModel {
    WallTimeModel {
        alpha_ms,
        beta_ms_per_mp,
    }
}

/// The quality target at or above which svt-as-configured is measured to
/// be unreliable, so the stub prefers zenravif.
///
/// `avif_backend_selection_2026-09-03.md` §3.3: svt-as-configured
/// (4:2:0) **cannot reach ssim2 90 on 16 of 32 references** at any q and
/// any speed — including **6/6 plots and 5/5 screenshots** — while
/// zenravif misses on **1 of 32**. Below 90 the two are much closer; in
/// the 70–85 byte band the sign test does not separate them at all
/// (13/27, p = 1.00).
pub const SVT_REACH_CEILING_TARGET: f32 = 90.0;

/// The measured still-image default cell for a backend.
///
/// # Provenance — AVIF knob DOE Stage A, 2026-09-02
///
/// `zenmetrics/benchmarks/avif_doe_stageA_2026-09-02.md`: 49,120 cells,
/// zero failed, five integrity gates passed, quality matched on ssim2.
/// Sign convention, verbatim from §6: *"NEGATIVE BD-rate = the arm needs
/// FEWER bits at matched quality = the arm WINS."*
///
/// # Why every knob is left absent
///
/// This is the part that matters. Each was checked and deliberately not
/// set:
///
/// - **`scm=3` (screen content mode)** — at speeds 4 and 6 it is a
///   **byte-identical no-op**: 288/288 cells at s4 and 288/288 at s6,
///   reproduced at the new era pin on all 576 cells
///   ([imazen/zenav1-svt#17](https://github.com/imazen/zenav1-svt/issues/17)).
///   It exists only at speed 7, fires on **90 of 288 cells (10 of 32
///   images)**, and its famous **-50.08%** is the median *over the images
///   where it fires* — corpus-wide the median is exactly **0.0000%**.
///   It also does **not** fire on AI-generated content (0 of 81 cells);
///   the winning class is plot / screenshot / scan. Setting it blind
///   would be right 31% of the time and inert the rest.
/// - **`svttune=3`** — the largest single main effect (**-7.69%** BD-rate
///   at native speed 6) but *"by far the most variable knob in the
///   block"*: **8 of 30 images regressed**, worst **+19.8%**, and at
///   speed 4 the CI crosses zero (-6.03%, [-11.05, +0.27]). The
///   backend doc's own verdict is that it is *"a per-image decision, not
///   a default"* — which is exactly what a trained bake is for and the
///   stub is not.
/// - **`svttune=0`** — not a knob at all. It is the encoder's default;
///   288/288 cells byte-identical to the control.
/// - **QM (`qm=1,qmmin=2,qmmax=10`)** — a real win that **grows with
///   preset** (-0.29% at s4, -2.59% at s6, -4.89% at s7, 29/32 images).
///   Not set here for two measured reasons: it moves **plots in the
///   opposite direction** (+1.20% at s6 while every other family is
///   negative), and the axis is **categorical, not ordinal** — the
///   `min=8,max=15` window reads only -0.66% and is byte-identical to
///   the control on 11.1% of its cells. Choosing a window per image is a
///   model's job.
/// - **`sharp=7`** — the most expensive knob measured (**+7.02%** at
///   native s6) *alone*, but the QM×sharpness joint reads **-0.03%**: a
///   **-4.70 pp** synergy residual (CI [-5.80, -3.99], 26/30 images,
///   p 5.9e-5). Neither half belongs in a default without the other, and
///   the pair is certified at **speed 6 only**.
/// - **`chroma`** — *not a tuning choice*. No chroma knob is wired for
///   AVIF at all; 4:2:0 on the svt cells is the **seam's** constraint
///   (`encoder_svt_rs.rs` encodes 4:2:0 only), and 4:4:4 is zenavif's
///   own `EncoderConfig` default for zenravif. Every measured
///   "backend difference" in the campaign is **totally confounded** with
///   this chroma split — verified by reading the `av1C` box out of 1,114
///   bitstreams, zero exceptions — so nothing here may be read as a
///   statement about the encoders themselves.
fn default_cell(backend: Av1Backend) -> Result<TuneCell, AutoTuneError> {
    let label = match backend {
        // Production-proven default backend (see `Av1Backend::Zenravif`).
        // Speed 6 is this crate's still-image default across every seam.
        Av1Backend::Zenravif => "rav1e,speed=6",
        // 4:2:0 is the svt-rs seam's scope, not a tuned choice.
        Av1Backend::Zenav1Svt => "svt,chroma=420,speed=6",
        // zenav1-aom's still seam wires RGB -> 4:2:0 only.
        Av1Backend::Zenav1Aom => "aom,chroma=420,speed=6",
        other => {
            return Err(AutoTuneError::LutMalformed(format!(
                "no measured default cell for backend {other:?}"
            )));
        }
    };
    TuneCell::parse(label)
}

/// Preference order when nothing else separates two allowed backends:
/// most production-proven first.
///
/// `Zenravif` leads because it is the crate's default, the only backend
/// with no scope caveats (alpha, animation, bit depth), and the one that
/// reaches ssim2 90 on 31 of 32 campaign references. The two
/// experimental seams follow.
const PRODUCTION_ORDER: [Av1Backend; 3] = [
    Av1Backend::Zenravif,
    Av1Backend::Zenav1Svt,
    Av1Backend::Zenav1Aom,
];

/// A model-free tuner over measured defaults. See the [module
/// docs](self).
///
/// Holds no state; construct with [`StubTuner::new`].
#[derive(Debug, Clone, Copy, Default)]
pub struct StubTuner {
    _private: (),
}

impl StubTuner {
    /// The measured-default tuner.
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl AvifTuning for StubTuner {
    /// Pick the measured default cell for the best allowed backend.
    ///
    /// `rgb` and `offer` are unused — the stub runs no analysis, which
    /// is precisely what distinguishes it from
    /// [`AvifTuner`](super::AvifTuner). They stay in the signature
    /// because the trait is the integration surface and a consumer must
    /// not have to change its call site to swap implementations.
    ///
    /// # Errors
    ///
    /// [`AutoTuneError::NoCellAllowed`] when the backend, alpha and
    /// time-budget masks eliminate every candidate.
    fn tune(
        &self,
        _rgb: &[u8],
        _offer: Option<&zenanalyze_api::Offer<'_>>,
        request: &TuneRequest,
    ) -> Result<AvifTune, AutoTuneError> {
        let allowed = request.allowed_backends.intersect(AllowedBackends::built());
        if allowed.is_empty() {
            return Err(AutoTuneError::NoCellAllowed);
        }
        let mpx = request.megapixels();

        // Candidates: the measured default cell for each allowed backend
        // that survives the alpha mask, in production order.
        let mut candidates: Vec<(TuneCell, Option<f32>)> = Vec::new();
        for backend in PRODUCTION_ORDER {
            if !allowed.contains(backend) {
                continue;
            }
            let cell = default_cell(backend)?;
            if !cell_is_viable(&cell, allowed, request.has_alpha) {
                continue;
            }
            let ms = estimated_wall_ms(&cell, mpx);
            candidates.push((cell, ms));
        }

        // The time budget is a hard mask on cells with a MEASURED
        // estimate. A cell with no measurement is NOT masked out — the
        // absence of a number is not evidence that it is slow — but it
        // also cannot win the "fastest that fits" tie-break below.
        let budget = request.time_budget_ms;
        let surviving: Vec<&(TuneCell, Option<f32>)> = candidates
            .iter()
            .filter(|(_, ms)| match (budget, ms) {
                (Some(b), Some(est)) => *est <= b,
                _ => true,
            })
            .collect();

        let chosen = if budget.is_some() {
            // Under a budget: the fastest MEASURED cell that fits. This
            // is the axis criterion 4 asks for ("routes to the optimal
            // AV1 encoder per the time + resource budget, measured"), and
            // it deliberately outranks the reach preference below — a
            // caller who asked for 100 ms must not be handed a 2 s encode.
            surviving
                .iter()
                .filter(|(_, ms)| ms.is_some())
                .min_by(|a, b| {
                    a.1.unwrap_or(f32::INFINITY)
                        .total_cmp(&b.1.unwrap_or(f32::INFINITY))
                })
                .or(surviving.first())
        } else if request.target_value() >= SVT_REACH_CEILING_TARGET {
            // No budget, high target: prefer a backend measured to reach
            // it. svt-as-configured misses ssim2 90 on 16/32 references.
            surviving
                .iter()
                .find(|(c, _)| c.backend() == Av1Backend::Zenravif)
                .or(surviving.first())
        } else {
            surviving.first()
        };
        let (cell, ms) = chosen.ok_or(AutoTuneError::NoCellAllowed)?;

        Ok(AvifTune {
            backend: cell.backend(),
            config: config_for_cell(cell, request.target_value())?,
            cell_label: cell.label().to_string(),
            // The stub predicts nothing about size.
            expected_bytes: None,
            expected_wall_ms: *ms,
            // Not a scored decision, so there is no runner-up gap.
            margin: None,
            source: TuneSource::Stub,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_tune::QualityTarget;

    #[test]
    fn stub_reports_its_own_provenance() {
        let req = TuneRequest::new(QualityTarget::Zensim(82.0), 512, 512);
        let t = StubTuner::new()
            .tune(&[], None, &req)
            .expect("a built backend must exist in a test build");
        assert_eq!(t.source(), TuneSource::Stub);
        assert_eq!(t.expected_bytes(), None, "the stub predicts no size");
        assert_eq!(t.margin(), None, "the stub scores nothing");
    }

    #[test]
    fn stub_never_returns_a_backend_outside_the_mask() {
        let built = AllowedBackends::built();
        for backend in PRODUCTION_ORDER {
            if !built.contains(backend) {
                continue;
            }
            let req = TuneRequest::new(QualityTarget::Zensim(82.0), 256, 256)
                .with_allowed_backends(AllowedBackends::none().with(backend));
            let t = StubTuner::new().tune(&[], None, &req).expect("one allowed");
            assert_eq!(t.backend(), backend);
        }
    }

    #[test]
    fn empty_mask_is_an_error_not_a_silent_default() {
        let req = TuneRequest::new(QualityTarget::Zensim(82.0), 256, 256)
            .with_allowed_backends(AllowedBackends::none());
        assert!(matches!(
            StubTuner::new().tune(&[], None, &req),
            Err(AutoTuneError::NoCellAllowed)
        ));
    }

    /// The table is transcribed data, so the test is a transcription
    /// check: the two values the backend doc's own iso-time table was
    /// built from must reproduce it.
    #[test]
    fn measured_table_reproduces_the_published_iso_time_row() {
        // avif_backend_selection_2026-09-03.md §3.4, predicted encode
        // time at 1 MP: svt-rs speed 7 = 28.7 ms, zenravif speed 7 = 1,987 ms.
        let svt = wall_time_model(Av1Backend::Zenav1Svt, 7).expect("svt s7 row");
        let rav = wall_time_model(Av1Backend::Zenravif, 7).expect("zenravif s7 row");
        assert!(
            (svt.estimate_ms(1.0) - 28.7).abs() < 0.1,
            "svt s7 @1MP should be 28.7 ms, got {}",
            svt.estimate_ms(1.0)
        );
        assert!(
            (rav.estimate_ms(1.0) - 1987.0).abs() < 1.0,
            "zenravif s7 @1MP should be 1987 ms, got {}",
            rav.estimate_ms(1.0)
        );
    }

    #[test]
    fn negative_intercepts_are_preserved_not_sanitized() {
        // Six arms fit a negative intercept — the pooled-fit failure
        // showing through. Rewriting them to zero would hide the very
        // signal the instrument recorded, so the table keeps them and
        // only the ESTIMATE clamps.
        let svt1 = wall_time_model(Av1Backend::Zenav1Svt, 1).expect("svt s1 row");
        assert!(svt1.alpha_ms < 0.0, "svt s1 alpha is measured negative");
        assert_eq!(svt1.estimate_ms(0.0), 0.0, "the estimate clamps at zero");
        assert!(svt1.estimate_ms(1.0) > 0.0);
    }

    #[test]
    fn aom_has_no_measured_wall_time_and_says_so() {
        // Block A3 was never declared: zenav1-aom has no speed number
        // anywhere in the campaign. `None` is the honest answer.
        for speed in 0..=10u8 {
            assert_eq!(
                wall_time_model(Av1Backend::Zenav1Aom, speed),
                None,
                "zenav1-aom speed {speed} must read NOT MEASURED, never an analogy"
            );
        }
        let cell = TuneCell::parse("aom,chroma=420,speed=6").expect("parses");
        assert_eq!(estimated_wall_ms(&cell, 4.0), None);
    }

    #[test]
    fn measured_defaults_do_not_set_the_knobs_the_campaign_did_not_certify() {
        // Stage A measured `scm3` and `tune=0` as byte-identical to the
        // control at every reachable speed (zenav1-svt#17), and `svttune=3`
        // as a per-image decision that regresses 8/30 images. None of them
        // may appear as a blind default.
        let cell = default_cell(Av1Backend::Zenav1Svt).expect("svt default cell");
        assert_eq!(cell.svt_screen_content_mode(), None, "scm is inert at s6");
        assert_eq!(cell.svt_tune(), None, "svttune=3 regresses 8/30 images");
        assert_eq!(
            cell.enable_qm(),
            None,
            "the QM window is a per-image choice"
        );
        assert_eq!(cell.svt_sharpness(), None, "sharpness only pays with QM");
        assert!(
            !cell.declares_svt_knobs(),
            "the stub default must be applicable without the __expert surface"
        );
    }
}
