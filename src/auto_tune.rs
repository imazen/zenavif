//! Automatic encoder-knob tuning via the rav1e knob predictor MLP.
//!
//! Given an input image and a quality target, predicts encoder knobs
//! (speed, quality) using a zenanalyze feature vector run through a
//! baked ZNPR model, with optional time-budget and Pareto tradeoff
//! masks applied via baked-in lookup tables.
//!
//! # Architecture
//!
//! Three artifacts ship with the runtime, all `include_bytes!`'d:
//!
//! 1. **ZNPR model** (`src/models/rav1e_picker_v0_1_1.bin`) — the trained
//!    MLP. Inputs: zenanalyze feature vector + log_pixels. Outputs:
//!    `bytes` regression head per cell (one cell per speed preset).
//!    `argmin_masked_in_range(features, (0, n_cells), mask, ...)`
//!    selects the cell that minimizes predicted bytes for the user's
//!    target.
//!
//! 2. **encode_ms LUT** (`src/models/rav1e_encode_ms_lut_v0_1_1.json`) —
//!    median ms/MPx per (speed, size_class). Multiplied by the input's
//!    pixel_count to estimate encode time. Used to mask cells where
//!    predicted time > `with_time_budget`.
//!
//! 3. **quality LUT** (`src/models/rav1e_quality_lut_v0_1_1.json`) — per
//!    (cell, target_zq) → median q. Translates the picker's cell choice
//!    + user target_zq into the actual encoder `quality` parameter.
//!
//! # Pareto tradeoff
//!
//! With `with_pareto_weight(α)`:
//!   `score[c] = (1 - α) * bytes_norm[c] + α * encode_ms_norm[c]`
//! Argmin runs over `score[c]` instead of `bytes[c]`. `α=0` (default)
//! ignores time. `α=1` ignores bytes (pure speed pick).
//!
//! # Errors at compile vs runtime
//!
//! The `include_bytes!` calls reference files produced by the training
//! pipeline. Until `scripts/train_bake_pipeline.sh` completes once,
//! placeholder zero-byte files exist; the runtime returns
//! [`AutoTuneError::ModelNotBaked`] when the loaded blob fails to
//! parse. After the first bake lands, the placeholders are overwritten
//! with real artifacts and the runtime path lights up.

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
    #[error("auto-tune model not yet baked — run scripts/train_bake_pipeline.sh")]
    ModelNotBaked,
    #[error("zenanalyze feature extraction failed: {0}")]
    FeatureExtraction(String),
    #[error("zenpredict inference failed: {0}")]
    Inference(String),
    #[error("LUT JSON malformed: {0}")]
    LutMalformed(String),
    #[error("no cell satisfies the constraints (time_budget too tight or speed_range empty?)")]
    NoCellAllowed,
    #[error("target_zq {0:.1} is outside the LUT's covered range")]
    TargetOutOfRange(f32),
    #[error("internal: {0}")]
    Internal(&'static str),
}

// Baked artifacts. Replaced with real data by the bake pipeline.
// Until the first bake lands these are zero-byte placeholders, and
// every auto_tune call returns ModelNotBaked.
//
// `include_bytes!` doesn't guarantee alignment — it just emits a static
// `[u8; N]`. zenpredict needs 4-byte alignment for the f32 sections
// (scaler_mean, weights), so we route through a const-generic-aligned
// wrapper. The static reference gives the compiler a stable address
// with the chosen alignment.
#[repr(C, align(4))]
struct Align4<T: ?Sized>(T);

static MODEL_BYTES_ALIGNED: &Align4<[u8]> =
    &Align4(*include_bytes!("models/rav1e_picker_v0_1_1.bin"));
static MODEL_BYTES: &[u8] = &MODEL_BYTES_ALIGNED.0;
const ENCODE_MS_LUT_JSON: &str = include_str!("models/rav1e_encode_ms_lut_v0_1_1.json");
const QUALITY_LUT_JSON: &str = include_str!("models/rav1e_quality_lut_v0_1_1.json");

/// Per-(speed, size_class) median ms/MPx, parsed from the JSON LUT.
struct EncodeMsLut {
    /// `[speed_idx][size_class_idx]` — same order as `cells`.
    median_ms_per_mpx: Vec<[f32; 4]>, // 4 size classes: tiny, small, medium, large
}

/// Per-(cell, target_zq) median quality value, parsed from JSON.
struct QualityLut {
    target_zqs: Vec<u32>,
    /// `[cell_idx][zq_idx]` — -1 if unreachable.
    median_q: Vec<Vec<i32>>,
}

fn size_class_idx(w: u32, h: u32) -> usize {
    let n = (w as u64) * (h as u64);
    if n < 64 * 64 {
        0 // tiny
    } else if n < 256 * 256 {
        1 // small
    } else if n < 1024 * 1024 {
        2 // medium
    } else {
        3 // large
    }
}

#[cfg(feature = "auto-tune")]
fn parse_encode_ms_lut(json: &str) -> Result<EncodeMsLut, AutoTuneError> {
    // Hand-roll a tiny parser to avoid a serde_json dep on the runtime
    // hot path. The schema is small and stable; if it grows we can
    // promote to serde_json later.
    if json.trim().is_empty() {
        return Err(AutoTuneError::ModelNotBaked);
    }
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| AutoTuneError::LutMalformed(format!("encode_ms_lut: {e}")))?;
    let median = v
        .get("median_ms_per_mpx")
        .and_then(|m| m.as_object())
        .ok_or_else(|| AutoTuneError::LutMalformed("missing median_ms_per_mpx".into()))?;

    let mut rows: Vec<(u32, [f32; 4])> = Vec::new();
    for (k, sub) in median {
        let speed: u32 = k
            .strip_prefix("speed")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| AutoTuneError::LutMalformed(format!("bad cell key {k}")))?;
        let sub = sub
            .as_object()
            .ok_or_else(|| AutoTuneError::LutMalformed("cell value not object".into()))?;
        let mut row = [f32::INFINITY; 4];
        for (sz_label, val) in sub {
            let idx = match sz_label.as_str() {
                "tiny" => 0,
                "small" => 1,
                "medium" => 2,
                "large" => 3,
                _ => continue,
            };
            row[idx] = val.as_f64().unwrap_or(f64::INFINITY) as f32;
        }
        rows.push((speed, row));
    }
    rows.sort_by_key(|(s, _)| *s);
    Ok(EncodeMsLut {
        median_ms_per_mpx: rows.into_iter().map(|(_, r)| r).collect(),
    })
}

#[cfg(feature = "auto-tune")]
fn parse_quality_lut(json: &str) -> Result<QualityLut, AutoTuneError> {
    if json.trim().is_empty() {
        return Err(AutoTuneError::ModelNotBaked);
    }
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| AutoTuneError::LutMalformed(format!("quality_lut: {e}")))?;
    let target_zqs: Vec<u32> = v
        .get("target_zqs")
        .and_then(|a| a.as_array())
        .ok_or_else(|| AutoTuneError::LutMalformed("missing target_zqs".into()))?
        .iter()
        .filter_map(|n| n.as_u64().map(|x| x as u32))
        .collect();
    let median_q: Vec<Vec<i32>> = v
        .get("median_q")
        .and_then(|a| a.as_array())
        .ok_or_else(|| AutoTuneError::LutMalformed("missing median_q".into()))?
        .iter()
        .map(|row| {
            row.as_array()
                .map(|r| {
                    r.iter()
                        .filter_map(|n| n.as_i64().map(|x| x as i32))
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();
    Ok(QualityLut {
        target_zqs,
        median_q,
    })
}

/// Pick the closest target_zq index in the LUT for a user-supplied
/// fractional target. Returns None if the LUT is empty.
fn nearest_target_zq_idx(target_zqs: &[u32], target: f32) -> Option<usize> {
    if target_zqs.is_empty() {
        return None;
    }
    let target_i = target.round() as i32;
    let mut best = 0usize;
    let mut best_d = i32::MAX;
    for (i, &t) in target_zqs.iter().enumerate() {
        let d = (t as i32 - target_i).abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    Some(best)
}

/// Reuse a shared [`zenanalyze_api::Offer`]'s feature values for `feature_cols`
/// (in the bake's column order) iff the offer's reuse key matches `model`'s
/// stamps — same analyzer `major.minor`, `feature_defs_version`, and analysis
/// `config_hash`. `None` → the caller runs its own analysis pass. The bake's
/// columns carry a `feat_` prefix; the contract keys by the bare
/// `AnalysisFeature::name`, so strip it to line the two conventions up.
#[cfg(feature = "auto-tune")]
fn reuse_from_offer(
    offer: Option<&zenanalyze_api::Offer<'_>>,
    model: &zenpredict::Model,
    feature_cols: &[&str],
) -> Option<Vec<f32>> {
    let offer = offer?;
    let names: Vec<&str> = feature_cols
        .iter()
        .map(|c| c.strip_prefix("feat_").unwrap_or(c))
        .collect();
    let request = zenanalyze_api::Request::new(
        &names,
        model.analyzer_version().unwrap_or(""),
        model.feature_defs_version().unwrap_or(0),
        model.feature_config_hash().unwrap_or(0),
    );
    offer.reuse_for(&request)
}

#[cfg(feature = "auto-tune")]
impl EncoderConfig {
    /// Predict optimal encoder knobs for the given image and target.
    ///
    /// Runs zenanalyze on the supplied RGB pixels, feeds features
    /// through the baked rav1e knob predictor MLP, and applies the
    /// predicted speed/quality knobs to `self`.
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
    ///         None, // or Some(&offer) to reuse a shared zenanalyze-api Offer
    ///     )?;
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`AutoTuneError::ModelNotBaked`] if the bundled artifacts
    /// haven't been overwritten with a real bake yet (see
    /// `scripts/train_bake_pipeline.sh`). Returns
    /// [`AutoTuneError::NoCellAllowed`] when constraints (time_budget,
    /// speed_range) eliminate every cell.
    pub fn auto_tune(
        self,
        rgb: &[u8],
        width: u32,
        height: u32,
        target: QualityTarget,
        opts: AutoTuneOptions,
        offer: Option<&zenanalyze_api::Offer<'_>>,
    ) -> Result<Self, AutoTuneError> {
        // 1. Load model. Empty/zero-byte placeholder until bake lands.
        let model = zenpredict::Model::from_bytes(MODEL_BYTES).map_err(|e| {
            if MODEL_BYTES.len() < 16 {
                AutoTuneError::ModelNotBaked
            } else {
                AutoTuneError::Inference(format!("Model::from_bytes: {e}"))
            }
        })?;
        let n_cells = model.n_outputs().max(1);

        // 2. Read feature columns from baked metadata. The bake stores
        // them under `zentrain.feature_columns` as utf8 with newline
        // separators (one column per line).
        let feature_cols_str = model
            .metadata()
            .get_utf8("zentrain.feature_columns")
            .map_err(|e| {
                AutoTuneError::Internal(Box::leak(format!("metadata: {e}").into_boxed_str()))
            })?;
        let feature_cols: Vec<&str> = feature_cols_str
            .split(|c: char| c == '\n' || c == ',')
            .filter(|s| !s.is_empty())
            .collect();

        // 3. Resolve the feature values. Prefer reusing a shared zenanalyze-api
        // Offer (one upstream analysis pass shared across codecs) when its reuse
        // key matches this baked model; otherwise run our own analysis.
        let raw_feats: Vec<f32> =
            reuse_from_offer(offer, &model, &feature_cols).unwrap_or_else(|| {
                // Own-pass: run zenanalyze with exactly those features.
                // No `from_name` API — walk SUPPORTED and match snake_case.
                let supported = zenanalyze::feature::FeatureSet::SUPPORTED;
                let lookup = |col: &str| -> Option<zenanalyze::feature::AnalysisFeature> {
                    let target = col.strip_prefix("feat_")?;
                    supported.iter().find(|f| f.name() == target)
                };
                let mut feature_set = zenanalyze::feature::FeatureSet::new();
                for c in &feature_cols {
                    if let Some(f) = lookup(c) {
                        feature_set = feature_set.with(f);
                    }
                }
                let query = zenanalyze::feature::AnalysisQuery::new(feature_set);
                let analysis = zenanalyze::analyze_features_rgb8(rgb, width, height, &query);
                feature_cols
                    .iter()
                    .map(|c| lookup(c).and_then(|f| analysis.get_f32(f)).unwrap_or(0.0))
                    .collect()
            });

        // 4. Engineer the feature vector to match train_hybrid.py:
        //   raw_feats (n) + size_oh (4) +
        //   [log_px, log_px², zq_norm, zq_norm², zq_norm*log_px] (5) +
        //   zq_norm * raw_feats (n) + [icc_placeholder] (1)
        // = 2n + 10 dims.
        let target_zq = match target {
            QualityTarget::Zensim(z) => z,
        };
        let pixels = (width as f32) * (height as f32);
        let log_px = pixels.max(1.0).ln();
        let zq_norm = target_zq / 100.0;
        let size_oh = match (width as u64) * (height as u64) {
            n if n < 64 * 64 => [1.0_f32, 0.0, 0.0, 0.0],
            n if n < 256 * 256 => [0.0, 1.0, 0.0, 0.0],
            n if n < 1024 * 1024 => [0.0, 0.0, 1.0, 0.0],
            _ => [0.0, 0.0, 0.0, 1.0],
        };
        let n = raw_feats.len();
        let mut features = Vec::with_capacity(2 * n + 10);
        features.extend_from_slice(&raw_feats);
        features.extend_from_slice(&size_oh);
        features.extend_from_slice(&[
            log_px,
            log_px * log_px,
            zq_norm,
            zq_norm * zq_norm,
            zq_norm * log_px,
        ]);
        for f in &raw_feats {
            features.push(zq_norm * f);
        }
        features.push(0.0); // icc placeholder

        // 5. Forward pass.
        let mut predictor = zenpredict::Predictor::new(&model);
        let output = predictor
            .predict(&features)
            .map_err(|e| AutoTuneError::Inference(format!("{e}")))?;
        let bytes_log: Vec<f32> = output[..n_cells.min(output.len())].to_vec();

        // 6. Apply masks: speed_range, time_budget.
        let ms_lut = parse_encode_ms_lut(ENCODE_MS_LUT_JSON)?;
        let q_lut = parse_quality_lut(QUALITY_LUT_JSON)?;
        let zq_idx = nearest_target_zq_idx(&q_lut.target_zqs, target_zq)
            .ok_or(AutoTuneError::Internal("empty quality LUT"))?;

        let sz_idx = size_class_idx(width, height);
        let mpx = (width as f32) * (height as f32) / 1_000_000.0;
        let budget_ms = opts.time_budget.map(|d| d.as_secs_f32() * 1000.0);

        // Compute encode_ms estimate per cell + mask
        let mut allowed = vec![false; n_cells];
        let mut encode_ms_est = vec![f32::INFINITY; n_cells];
        for cell in 0..n_cells {
            let speed = (cell + 1) as u8;
            if let Some(ref range) = opts.speed_range {
                if !range.contains(&speed) {
                    continue;
                }
            }
            // Cell unreachable for this target_zq?
            if cell < q_lut.median_q.len()
                && zq_idx < q_lut.median_q[cell].len()
                && q_lut.median_q[cell][zq_idx] < 0
            {
                continue;
            }
            let ms_per_mpx = ms_lut
                .median_ms_per_mpx
                .get(cell)
                .and_then(|row| row.get(sz_idx).copied())
                .unwrap_or(f32::INFINITY);
            let est = ms_per_mpx * mpx;
            encode_ms_est[cell] = est;
            if let Some(b) = budget_ms {
                if est > b {
                    continue;
                }
            }
            allowed[cell] = true;
        }

        // 6. Score: blend bytes_log and encode_ms via pareto_weight.
        let alpha = opts.pareto_weight;
        let (bytes_min, bytes_max) = bytes_log
            .iter()
            .copied()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| {
                (lo.min(x), hi.max(x))
            });
        let (ms_min, ms_max) = encode_ms_est
            .iter()
            .filter(|x| x.is_finite())
            .copied()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| {
                (lo.min(x), hi.max(x))
            });
        let bytes_span = (bytes_max - bytes_min).max(1e-6);
        let ms_span = (ms_max - ms_min).max(1e-6);

        let mut best_cell: Option<usize> = None;
        let mut best_score = f32::INFINITY;
        for cell in 0..n_cells {
            if !allowed[cell] {
                continue;
            }
            let bytes_norm = (bytes_log[cell] - bytes_min) / bytes_span;
            let ms_norm = if encode_ms_est[cell].is_finite() {
                (encode_ms_est[cell] - ms_min) / ms_span
            } else {
                1.0
            };
            let score = (1.0 - alpha) * bytes_norm + alpha * ms_norm;
            if score < best_score {
                best_score = score;
                best_cell = Some(cell);
            }
        }

        let cell = best_cell.ok_or(AutoTuneError::NoCellAllowed)?;
        let speed = (cell + 1) as u8;
        let q = q_lut
            .median_q
            .get(cell)
            .and_then(|row| row.get(zq_idx).copied())
            .filter(|q| *q >= 0)
            .ok_or(AutoTuneError::TargetOutOfRange(target_zq))?;

        // 7. Apply.
        Ok(self.speed(speed).quality(q as f32))
    }
}

#[cfg(all(test, feature = "auto-tune"))]
mod tests {
    use super::*;

    #[test]
    fn auto_tune_returns_model_not_baked_with_empty_artifacts() {
        // Until the first bake lands, MODEL_BYTES is the placeholder
        // and from_bytes returns Err → we map that to ModelNotBaked.
        if MODEL_BYTES.len() < 16 {
            // Synthesize a 1x1 RGB pixel so the call doesn't fail
            // earlier (zenanalyze tolerates 1x1 fine).
            let rgb = [128u8, 128, 128];
            let cfg = EncoderConfig::new();
            let r = cfg.auto_tune(
                &rgb,
                1,
                1,
                QualityTarget::Zensim(85.0),
                AutoTuneOptions::new(),
                None,
            );
            assert!(matches!(r, Err(AutoTuneError::ModelNotBaked)));
        }
    }

    #[test]
    fn auto_tune_offer_reuse_matches_own_pass() {
        // The v3 model loads and predicts; reusing a shared zenanalyze-api Offer
        // (same features, same reuse key) yields the SAME tuned config as the
        // own-analysis path. 256x256 textured noise populates every feature.
        let (w, h) = (256u32, 256u32);
        let rgb: Vec<u8> = (0..(w * h * 3))
            .map(|i| (i.wrapping_mul(7) % 251) as u8)
            .collect();
        let target = QualityTarget::Zensim(85.0);

        // Build an Offer as an orchestrator would: the model's feature columns'
        // analyzed values at the model's reuse key (bare names — strip `feat_`).
        let model = zenpredict::Model::from_bytes(MODEL_BYTES).expect("v3 model loads");
        let feat_cols: Vec<&str> = model.feature_columns().collect();
        let bare: Vec<&str> = feat_cols
            .iter()
            .map(|c| c.strip_prefix("feat_").unwrap_or(c))
            .collect();
        let supported = zenanalyze::feature::FeatureSet::SUPPORTED;
        let lookup = |n: &str| supported.iter().find(|f| f.name() == n);
        let mut fs = zenanalyze::feature::FeatureSet::new();
        for n in &bare {
            if let Some(f) = lookup(n) {
                fs = fs.with(f);
            }
        }
        let analysis = zenanalyze::analyze_features_rgb8(
            &rgb,
            w,
            h,
            &zenanalyze::feature::AnalysisQuery::new(fs),
        );
        let values: Vec<f32> = bare
            .iter()
            .map(|n| lookup(n).and_then(|f| analysis.get_f32(f)).unwrap_or(0.0))
            .collect();
        let offer = zenanalyze_api::Offer::new(
            &bare,
            &values,
            model.analyzer_version().unwrap_or(""),
            model.feature_defs_version().unwrap_or(0),
            model.feature_config_hash().unwrap_or(0),
        );

        // The offer is reuse-key-compatible and carries every feature → the
        // reuse helper returns exactly the offered values, in column order.
        assert_eq!(
            reuse_from_offer(Some(&offer), &model, &feat_cols).as_deref(),
            Some(values.as_slice()),
            "a matching offer must be reused verbatim"
        );

        // End to end: own-pass and offer-reuse produce IDENTICAL results —
        // whether a tuned config or the same LUT-coverage error — so the reuse is
        // exactly equivalent to running our own analysis.
        let own = EncoderConfig::new().auto_tune(&rgb, w, h, target, AutoTuneOptions::new(), None);
        let via_offer = EncoderConfig::new().auto_tune(
            &rgb,
            w,
            h,
            target,
            AutoTuneOptions::new(),
            Some(&offer),
        );
        assert_eq!(
            format!("{own:?}"),
            format!("{via_offer:?}"),
            "offer reuse must produce the same result as own-pass"
        );
    }
}
