//! AVIF's existing regression checks, using the score and delta classifier
//! from zensimx. The tolerance and report calculations here are adapted from
//! zensimx 273eff30, zensim-regress/src/testing.rs (MIT OR Apache-2.0).
//! Using the classifier directly keeps the PNG-dependent report renderer out
//! of the test graph until it adopts the same pixel API as zenpng.

use zensim::{ErrorCategory, ImageSource, Zensim, ZensimError};

pub struct RegressionTolerance {
    max_delta: u8,
    min_similarity: f64,
    max_alpha_delta: u8,
}

impl RegressionTolerance {
    pub fn off_by_one() -> Self {
        Self {
            max_delta: 1,
            min_similarity: 85.0,
            max_alpha_delta: 0,
        }
    }

    #[allow(dead_code)]
    pub fn with_max_delta(mut self, value: u8) -> Self {
        self.max_delta = value;
        self
    }

    pub fn with_min_similarity(mut self, value: f64) -> Self {
        self.min_similarity = value;
        self
    }

    #[allow(dead_code)]
    pub fn with_max_alpha_delta(mut self, value: u8) -> Self {
        self.max_alpha_delta = value;
        self
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct RegressionReport {
    passed: bool,
    score: f64,
    category: ErrorCategory,
    max_channel_delta: [u8; 3],
}

#[allow(dead_code)]
impl RegressionReport {
    pub fn passed(&self) -> bool {
        self.passed
    }

    pub fn score(&self) -> f64 {
        self.score
    }

    pub fn category(&self) -> ErrorCategory {
        self.category
    }

    pub fn max_channel_delta(&self) -> [u8; 3] {
        self.max_channel_delta
    }
}

pub fn check_regression(
    zensim: &Zensim,
    expected: &impl ImageSource,
    actual: &impl ImageSource,
    tolerance: &RegressionTolerance,
) -> Result<RegressionReport, ZensimError> {
    let classified = zensim.classify(expected, actual)?;
    let delta = &classified.delta_stats;
    let max_channel_delta = delta
        .max_abs_delta
        .map(|d| (d * 255.0).round().min(255.0) as u8);
    let score = classified.result.score();
    let identical = delta.pixels_differing == 0 && (!delta.has_alpha || delta.alpha_max_delta == 0);
    let effective_score = if identical { 100.0 } else { score };
    let passed = max_channel_delta.iter().all(|&d| d <= tolerance.max_delta)
        && effective_score >= tolerance.min_similarity
        && (!delta.has_alpha || delta.alpha_max_delta <= tolerance.max_alpha_delta);
    Ok(RegressionReport {
        passed,
        score,
        category: classified.classification.dominant,
        max_channel_delta,
    })
}

impl core::fmt::Display for RegressionReport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "score {:.4}, max delta {:?}, {:?}",
            self.score, self.max_channel_delta, self.category
        )
    }
}
