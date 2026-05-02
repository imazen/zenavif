//! Expert-only knobs for codec calibration and picker training.
//!
//! Anything in this module is **unstable**: it may change in any patch
//! release without semver justification, and is **not part of the
//! public API contract**. Reach for it only when:
//!
//! 1. Sweeping parameter combinations to feed a picker / regression /
//!    calibration training pipeline.
//! 2. Diagnosing codec behaviour by overriding speed-preset defaults.
//! 3. Wiring a future `predict` feature that selects [`InternalParams`]
//!    via a baked MLP.
//!
//! Everything in here lives behind the `__expert` cargo feature, whose
//! double-underscore signals "private — do not depend on this in
//! production code." Default builds expose only stable public knobs
//! ([`crate::EncoderConfig::with_quality`], `with_speed`, etc.).

/// Expert override knobs for the AVIF encoder.
///
/// Each field is `Option<T>`: `None` (the [`Default`]) keeps the speed
/// preset's value, `Some(_)` overrides it. Apply via
/// [`crate::EncoderConfig::with_internal_params`].
///
/// `#[non_exhaustive]` — fields may be added in any patch release.
/// Construct via [`Default::default`] and field-by-field assignment.
#[non_exhaustive]
#[derive(Default, Clone, Debug)]
pub struct InternalParams {
    /// Partition block-size range `(min, max)` in pixels. Each must be
    /// one of `{4, 8, 16, 32, 64, 128}` and `min <= max`. Smaller mins
    /// help text/screen content; larger maxes help smooth photo content.
    pub partition_range: Option<(u8, u8)>,

    /// Override prediction-modes setting. `Some(true)` = ComplexAll
    /// (slowest). `Some(false)` = Simple (fastest).
    pub complex_prediction_modes: Option<bool>,

    /// Override loop restoration filter (Wiener / SGR). Helps
    /// smooth/noisy content; can over-soften line art and text.
    pub lrf: Option<bool>,

    /// Override fast vs full deblock. `Some(true)` = fast. `Some(false)`
    /// = full (better edge preservation).
    pub fast_deblock: Option<bool>,
}
