//! Automatic encoder-knob tuning via the rav1e knob predictor MLP.
//!
//! Given an input image and a quality target, predicts encoder knobs
//! (speed, quality, qm, vaq, …) using a zenanalyze feature vector run
//! through a baked ZNPR model.
//!
//! ## Quality target
//!
//! Today we support `QualityTarget::Zensim(score)` — pick the smallest
//! file whose predicted zensim ≥ score. Future variants: `BitsPerPixel`,
//! `MaxBytes`, `Butteraugli`.
//!
//! ## Time-budget and Pareto tradeoff
//!
//! The picker outputs a `bytes` regression head AND an `encode_ms`
//! regression head per cell. The user can constrain inference:
//!
//!   - `with_time_budget(Duration)` — mask out cells whose predicted
//!     encode_ms exceeds the budget, then argmin bytes over survivors.
//!   - `with_pareto_weight(α ∈ [0,1])` — combine bytes and encode_ms
//!     into a normalized weighted score: `α=0` ignores time (smallest
//!     bytes wins), `α=1` ignores bytes (fastest wins), `α=0.3` is a
//!     "fast enough, small enough" middle.
//!
//! Both constraints can be combined: time-budget masks first, then
//! pareto-weighted argmin runs over what remains.

use crate::EncoderConfig;
use std::time::Duration;

/// Target quality the picker should hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QualityTarget {
    /// Pick the smallest file whose predicted zensim ≥ this value.
    /// Range 0.0..=100.0; typical web targets 75..90.
    Zensim(f32),
}

/// User-configurable inference constraints.
#[derive(Debug, Clone)]
pub struct AutoTuneOptions {
    /// Reject cells whose predicted encode_ms exceeds this budget.
    /// `None` = no time constraint.
    pub time_budget: Option<Duration>,
    /// Restrict to a subset of speed presets.
    /// `None` = let predictor choose any speed in 1..=10.
    pub speed_range: Option<std::ops::RangeInclusive<u8>>,
    /// Pareto weight α ∈ [0, 1] between bytes (α=0) and encode_ms (α=1).
    /// `0.0` = optimize for size only (default).
    /// `0.3` = balanced (time matters, but byte cost dominates).
    /// `1.0` = optimize for speed only.
    pub pareto_weight: f32,
}

impl Default for AutoTuneOptions {
    fn default() -> Self {
        Self {
            time_budget: None,
            speed_range: None,
            pareto_weight: 0.0,
        }
    }
}

impl AutoTuneOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_time_budget(mut self, budget: Duration) -> Self {
        self.time_budget = Some(budget);
        self
    }

    pub fn with_speed_range(mut self, range: std::ops::RangeInclusive<u8>) -> Self {
        self.speed_range = Some(range);
        self
    }

    /// Pareto weight between bytes (0.0) and encode time (1.0).
    /// Clamped to `[0.0, 1.0]`.
    pub fn with_pareto_weight(mut self, w: f32) -> Self {
        self.pareto_weight = w.clamp(0.0, 1.0);
        self
    }
}

/// Errors raised by the auto-tune path.
#[derive(Debug, thiserror::Error)]
pub enum AutoTuneError {
    #[error("auto-tune model not yet baked — see docs/RAV1E_PICKER_PLAN.md")]
    ModelNotBaked,
    #[error("zenanalyze feature extraction failed: {0}")]
    FeatureExtraction(String),
    #[error("zenpredict inference failed: {0}")]
    Inference(String),
    #[error("no cell satisfies the constraints (time_budget too tight or speed_range empty?)")]
    NoCellAllowed,
}

impl EncoderConfig {
    /// Predict optimal encoder knobs for the given image and target.
    ///
    /// Runs zenanalyze on the supplied RGB pixels, feeds features
    /// through the baked rav1e knob predictor MLP, and applies the
    /// predicted speed/quality/qm/vaq/… knobs to `self`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::time::Duration;
    /// use zenavif::{EncoderConfig, QualityTarget, AutoTuneOptions};
    ///
    /// let config = EncoderConfig::new()
    ///     .auto_tune(
    ///         &rgb_pixels, width, height,
    ///         QualityTarget::Zensim(85.0),
    ///         AutoTuneOptions::new()
    ///             .with_time_budget(Duration::from_millis(500))
    ///             .with_pareto_weight(0.2),
    ///     )?;
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`AutoTuneError::ModelNotBaked`] until the predictor
    /// model has been trained + baked. See `docs/RAV1E_PICKER_PLAN.md`.
    #[cfg(feature = "auto-tune")]
    pub fn auto_tune(
        self,
        _rgb: &[u8],
        _width: u32,
        _height: u32,
        _target: QualityTarget,
        _opts: AutoTuneOptions,
    ) -> Result<Self, AutoTuneError> {
        // PHASE 5 SCAFFOLD — wired up in a follow-up commit once
        // train_hybrid.py + bake_picker.py have produced a ZNPR blob
        // for the v0.1 (speed×q baseline) picker. Pseudocode:
        //
        //   1. const MODEL: &[u8] = include_bytes!("models/rav1e_picker_v0_1.bin");
        //   2. let model = zenpredict::Model::from_bytes(MODEL)?;
        //   3. let feature_cols = model.metadata().get_utf8("zenpicker.feature_columns")?
        //                            .split(',').collect::<Vec<_>>();
        //   4. let query = build_zenanalyze_query(&feature_cols);
        //   5. let analysis = zenanalyze::analyze_features_rgb8(rgb, w, h, &query)?;
        //   6. let features = feature_cols.iter()
        //                       .map(|c| analysis.get_f32(parse_feat(c)).unwrap_or(0.0))
        //                       .collect::<Vec<f32>>();
        //   7. let mut p = Predictor::new(model);
        //   8. let n_cells = parse_n_cells_from_metadata(&p.model());
        //   9. p.predict(&features)?;
        //  10. let bytes_log: &[f32] = &p.output()[0..n_cells];
        //  11. let encode_ms: &[f32] = &p.output()[2*n_cells..3*n_cells];
        //  12. Build mask: cell c allowed iff
        //         (speed_range covers cell.speed) AND
        //         (encode_ms[c] <= time_budget_ms.unwrap_or(INF)) AND
        //         (predicted zensim_at_cell >= target zensim)
        //  13. score[c] = (1 - α) * bytes_log_norm[c] + α * encode_ms_norm[c]
        //  14. cell = argmin score over masked cells
        //  15. parse cell.config_name → apply self.speed(...).quality(...). ...
        //
        // Returning ModelNotBaked until the bake completes; the
        // training pipeline is on track, see Phase 4 in the plan doc.
        Err(AutoTuneError::ModelNotBaked)
    }
}
