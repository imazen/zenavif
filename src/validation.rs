//! Configuration validation.
//!
//! Provides [`ValidationError`] plus `validate()` methods on every
//! public `Config` type. Existing encode/decode entry points keep
//! their clamping behaviour — `validate()` is a fail-fast option
//! callers can opt into for batch jobs that want hard rejection on
//! out-of-range values rather than silent clamping.
//!
//! ```no_run
//! # #[cfg(feature = "encode")] {
//! use zenavif::EncoderConfig;
//!
//! let cfg = EncoderConfig::new().quality(150.0);
//! assert!(cfg.validate().is_err()); // out of 0.0..=100.0
//! # }
//! ```
//!
//! Validation never mutates the config and never reports a "fixed"
//! value — it reports the issue and lets the caller decide.

use core::ops::RangeInclusive;

/// Reasons a [`crate::EncoderConfig`], [`crate::DecoderConfig`], or
/// [`crate::expert::InternalParams`] fails validation.
///
/// `#[non_exhaustive]` — variants may be added in any patch release as
/// new configuration knobs are introduced.
#[non_exhaustive]
#[derive(Debug, Clone, thiserror::Error)]
pub enum ValidationError {
    // --- EncoderConfig ---
    /// Encoder `quality` must be within `0.0..=100.0`.
    #[error("encoder quality {value} out of valid range {valid:?}")]
    QualityOutOfRange {
        /// The offending value.
        value: f32,
        /// The valid range.
        valid: RangeInclusive<f32>,
    },

    /// Encoder `alpha_quality` must be within `0.0..=100.0`.
    #[error("encoder alpha_quality {value} out of valid range {valid:?}")]
    AlphaQualityOutOfRange {
        /// The offending value.
        value: f32,
        /// The valid range.
        valid: RangeInclusive<f32>,
    },

    /// Encoder `speed` must be within `1..=10`.
    #[error("encoder speed {value} out of valid range {valid:?}")]
    SpeedOutOfRange {
        /// The offending value.
        value: u8,
        /// The valid range.
        valid: RangeInclusive<u8>,
    },

    /// Encoder `threads`, when `Some`, must be greater than zero.
    /// Use `None` for the rayon default.
    #[error("encoder threads must be > 0 when Some(_); got Some(0)")]
    EncoderThreadsZero,

    /// Encoder `rotation` must be one of `0`, `90`, `180`, `270`.
    #[error("encoder rotation {value} invalid: must be one of {{0, 90, 180, 270}}")]
    RotationInvalid {
        /// The offending value.
        value: u8,
    },

    /// Encoder `mirror` axis must be `0` (vertical) or `1` (horizontal).
    #[error("encoder mirror {value} invalid: must be 0 (vertical) or 1 (horizontal)")]
    MirrorInvalid {
        /// The offending value.
        value: u8,
    },

    /// CICP code-point fields (color_primaries, transfer_characteristics,
    /// matrix_coefficients) must fit ITU-T H.273. The validator rejects
    /// the reserved value `3`.
    #[error("CICP {field} value {value} is reserved (3 is reserved per ITU-T H.273)")]
    CicpReserved {
        /// Which CICP field.
        field: &'static str,
        /// The offending value.
        value: u8,
    },

    /// VAQ strength must be within `0.0..=4.0`.
    #[error("VAQ strength {value} out of valid range {valid:?}")]
    VaqStrengthOutOfRange {
        /// The offending value.
        value: f64,
        /// The valid range.
        valid: RangeInclusive<f64>,
    },

    /// Segmentation boost out of valid range. zenravif accepts
    /// `0.5..=4.0`; `1.0` is "off" and `>1.0` widens deltas. Values
    /// `<0.5` or `>4.0` are rejected.
    #[error("seg_boost {value} out of valid range {valid:?}")]
    SegBoostOutOfRange {
        /// The offending value.
        value: f64,
        /// The valid range.
        valid: RangeInclusive<f64>,
    },

    /// Two parameters that cannot both be set / both be true.
    #[error("mutually exclusive: {a} and {b} cannot both be set")]
    MutuallyExclusive {
        /// First parameter name.
        a: &'static str,
        /// Second parameter name.
        b: &'static str,
    },

    /// The selected encoder backend is not compiled into this build.
    /// Without this check the encode entry points silently fall back to
    /// the zenravif backend — a config asking for one encoder would be
    /// served by another.
    #[error("backend {backend} requires the `{feature}` cargo feature, which is not enabled")]
    BackendUnavailable {
        /// The requested backend.
        backend: &'static str,
        /// The cargo feature that would enable it.
        feature: &'static str,
    },

    /// The selected encoder backend is compiled in, but does not
    /// support a configured parameter value (e.g. the experimental
    /// svtav1-rs backend is 8-bit 4:2:0 only). The encode entry points
    /// reject the same combinations at encode time; this surfaces them
    /// at validation time for fail-fast callers.
    #[error("backend {backend} does not support {param}: {detail}")]
    BackendUnsupportedParam {
        /// The requested backend.
        backend: &'static str,
        /// The unsupported parameter (name or name=value).
        param: &'static str,
        /// Why / what to use instead.
        detail: &'static str,
    },

    // --- DecoderConfig ---
    /// Decoder `frame_size_limit` cannot use a sentinel reserved by
    /// the validator. The current decoder treats `0` as "no limit"
    /// at runtime, but for validation purposes a non-zero positive
    /// limit must be supplied if the caller wants a bound. Use
    /// `frame_size_limit(0)` plus skipping `validate()` to opt out.
    /// Reserved for future use; not currently emitted.
    #[error("decoder frame size limit {value} cannot be zero")]
    DecoderFrameSizeLimitZero {
        /// The offending value.
        value: u64,
    },

    // --- expert::InternalParams ---
    /// `partition_range` must satisfy `min <= max` and both bounds
    /// must be in `{4, 8, 16, 32, 64}`. zenrav1e debug-asserts on
    /// `128`, so it is rejected here.
    #[error(
        "partition_range {min}..{max} invalid: \
         must satisfy min <= max and both ∈ {{4, 8, 16, 32, 64}}"
    )]
    PartitionRangeInvalid {
        /// The offending lower bound.
        min: u8,
        /// The offending upper bound.
        max: u8,
    },
}

/// Returns true if `v` is a valid AV1 partition block-size bound.
/// zenrav1e accepts `{4, 8, 16, 32, 64}`; `128` is reserved for
/// future large-superblock support and triggers a debug-assert today.
#[cfg(feature = "__expert")]
fn partition_bound_ok(v: u8) -> bool {
    matches!(v, 4 | 8 | 16 | 32 | 64)
}

#[cfg(feature = "__expert")]
impl crate::expert::InternalParams {
    /// Validate this `InternalParams` value.
    ///
    /// Returns `Err` if any `Some(_)` field is outside its accepted
    /// range. The most relevant invariant is `partition_range`: both
    /// bounds must be in `{4, 8, 16, 32, 64}` and `min <= max`. The
    /// `128` superblock size is reserved for future AV1 large-superblock
    /// support and triggers a zenrav1e debug-assert today.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some((min, max)) = self.partition_range
            && (!partition_bound_ok(min) || !partition_bound_ok(max) || min > max)
        {
            return Err(ValidationError::PartitionRangeInvalid { min, max });
        }
        // complex_prediction_modes / lrf / fast_deblock are bool
        // overrides — every value of `Option<bool>` is well-formed.
        Ok(())
    }
}

#[cfg(feature = "encode")]
impl crate::EncoderConfig {
    /// Validate this `EncoderConfig` value.
    ///
    /// Returns `Err` on the first failed invariant. Validation does
    /// not mutate the config; existing encode entry points still
    /// clamp out-of-range values silently. Use this method when you
    /// want hard rejection (batch jobs, calibration sweeps, public
    /// HTTP endpoints) instead of silent clamping.
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.validate_quality_fields()?;
        self.validate_speed_and_threads()?;
        self.validate_transforms()?;
        self.validate_cicp()?;
        self.validate_backend_and_color()?;
        #[cfg(feature = "encode-imazen")]
        self.validate_imazen_knobs()?;
        #[cfg(feature = "__expert")]
        self.validate_expert_overrides()?;
        Ok(())
    }

    /// Validate this config against a concrete input shape.
    ///
    /// Runs [`validate`](Self::validate) plus the config×input checks a
    /// config-only pass cannot make. Today that is one rule: the 16-bit
    /// entry points always encode identity-matrix RGB planes, and AV1
    /// has no defined 4:2:0 subsampling for identity — the encoder
    /// rejects the pair at encode time; this rejects it up front.
    pub fn validate_for_input(
        &self,
        input: crate::encode_plan::PlanInput,
    ) -> Result<(), ValidationError> {
        self.validate()?;
        // zenravif's 16-bit path is the identity-RGB (GBR) encode, which
        // has no 4:2:0. The svtav1-rs backend converts 16-bit input to
        // 10-bit YCbCr 4:2:0 instead, so the pair is exactly its shape.
        if input.input_is_16bit
            && self.chroma_subsampling == crate::EncodeChromaSubsampling::Yuv420
            && self.backend != crate::Av1Backend::Zenav1Svt
        {
            return Err(ValidationError::MutuallyExclusive {
                a: "chroma_subsampling=Yuv420",
                b: "16-bit input (identity-RGB encode path)",
            });
        }
        // The svtav1-rs bit-depth envelope (issue #33): 10-bit Cs400
        // alpha/gray streams need SVT preset >= 9 (speed >= 7). Same
        // predicate as the encode path (`encoder_svt_rs::svt_rs_depth_error`).
        #[cfg(feature = "zenav1-svt")]
        if self.backend == crate::Av1Backend::Zenav1Svt {
            let bit_depth = match self.bit_depth {
                crate::EncodeBitDepth::Eight => 8,
                crate::EncodeBitDepth::Ten => 10,
                crate::EncodeBitDepth::Auto => {
                    if input.input_is_16bit {
                        10
                    } else {
                        8
                    }
                }
            };
            if let Some(detail) = crate::encoder_svt_rs::svt_rs_depth_error(
                bit_depth,
                self.speed,
                input.input_has_alpha,
            ) {
                return Err(ValidationError::BackendUnsupportedParam {
                    backend: "Av1Backend::Zenav1Svt",
                    param: "bit_depth=Ten with alpha",
                    detail,
                });
            }
        }
        // The svtav1-rs dimension envelope (issue #32): the 4:2:0 colour
        // path takes arbitrary dimensions at any speed; a Cs400 alpha or
        // grayscale stream needs SVT preset >= 6 (speed >= 5) AND multiples
        // of 8, else multiples of 64. One predicate serves this check and
        // the encode path (`encoder_svt_rs::svt_rs_dims_error`).
        #[cfg(feature = "zenav1-svt")]
        if self.backend == crate::Av1Backend::Zenav1Svt
            && let Some(detail) = crate::encoder_svt_rs::svt_rs_dims_error(
                input.width as usize,
                input.height as usize,
                self.speed,
                input.input_has_alpha,
            )
        {
            return Err(ValidationError::BackendUnsupportedParam {
                backend: "Av1Backend::Zenav1Svt",
                param: "input dimensions",
                detail,
            });
        }
        Ok(())
    }

    fn validate_backend_and_color(&self) -> Result<(), ValidationError> {
        // The deprecated svtav1 backend exists in no build (the
        // encode-svtav1 feature was never shipped); without this check
        // the encode entry points would silently serve the request with
        // zenravif instead.
        #[allow(deprecated)]
        if self.backend == crate::Av1Backend::Svtav1 {
            return Err(ValidationError::BackendUnavailable {
                backend: "Av1Backend::Svtav1",
                feature: "encode-svtav1",
            });
        }
        // The experimental svtav1-rs backend: unavailable without its
        // feature; inside the feature, only the 8-bit 4:2:0 YCbCr slice
        // it implements is accepted (the encode path rejects the same
        // combinations — see `src/encoder_svt_rs.rs`).
        if self.backend == crate::Av1Backend::Zenav1Svt {
            #[cfg(not(feature = "zenav1-svt"))]
            return Err(ValidationError::BackendUnavailable {
                backend: "Av1Backend::Zenav1Svt",
                feature: "zenav1-svt",
            });
            #[cfg(feature = "zenav1-svt")]
            self.validate_svt_rs_scope()?;
        }
        // 4:2:0 has no defined meaning for the identity (RGB) matrix;
        // zenravif rejects the pair at encode time
        // (encode_raw_planes_internal: Error::Unsupported). Mirror of
        // zenravif 0.1.3 Encoder::validate.
        if self.chroma_subsampling == crate::EncodeChromaSubsampling::Yuv420
            && self.color_model == crate::EncodeColorModel::Rgb
        {
            return Err(ValidationError::MutuallyExclusive {
                a: "chroma_subsampling=Yuv420",
                b: "color_model=Rgb",
            });
        }
        Ok(())
    }

    /// The configuration slice the experimental svtav1-rs backend
    /// implements: 8/10-bit 4:2:0 YCbCr full-range stills, no gain map, no
    /// lossless. The dimension envelope (multiples of 64; arbitrary at
    /// speed >= 5; multiples of 8 at speed >= 5 with alpha) is a
    /// config×input concern checked by [`Self::validate_for_input`] and at
    /// encode time.
    #[cfg(feature = "zenav1-svt")]
    fn validate_svt_rs_scope(&self) -> Result<(), ValidationError> {
        const BACKEND: &str = "Av1Backend::Zenav1Svt";
        if self.chroma_subsampling != crate::EncodeChromaSubsampling::Yuv420 {
            return Err(ValidationError::BackendUnsupportedParam {
                backend: BACKEND,
                param: "chroma_subsampling=Yuv444",
                detail: "only Yuv420 is implemented (svtav1-rs 4:2:0 still pipeline); \
                         set .chroma_subsampling(EncodeChromaSubsampling::Yuv420)",
            });
        }
        if self.color_model != crate::EncodeColorModel::YCbCr {
            return Err(ValidationError::BackendUnsupportedParam {
                backend: BACKEND,
                param: "color_model=Rgb",
                detail: "only the YCbCr model is implemented (identity/RGB has no \
                         defined 4:2:0 subsampling)",
            });
        }
        if self.pixel_range == Some(crate::EncodePixelRange::Limited) {
            return Err(ValidationError::BackendUnsupportedParam {
                backend: BACKEND,
                param: "pixel_range=Limited",
                detail: "the svtav1-rs sequence header signals full range only",
            });
        }
        if self.gain_map.is_some() {
            return Err(ValidationError::BackendUnsupportedParam {
                backend: BACKEND,
                param: "gain_map",
                detail: "gain-map muxing is zenravif-only for now",
            });
        }
        #[cfg(feature = "encode-imazen")]
        if self.lossless {
            return Err(ValidationError::BackendUnsupportedParam {
                backend: BACKEND,
                param: "lossless",
                detail: "svtav1-rs has no lossless mode (QP 0 is not \
                         mathematically lossless)",
            });
        }
        Ok(())
    }

    fn validate_quality_fields(&self) -> Result<(), ValidationError> {
        if !QUALITY_RANGE.contains(&self.quality) || !self.quality.is_finite() {
            return Err(ValidationError::QualityOutOfRange {
                value: self.quality,
                valid: QUALITY_RANGE,
            });
        }
        if let Some(aq) = self.alpha_quality
            && (!QUALITY_RANGE.contains(&aq) || !aq.is_finite())
        {
            return Err(ValidationError::AlphaQualityOutOfRange {
                value: aq,
                valid: QUALITY_RANGE,
            });
        }
        Ok(())
    }

    fn validate_speed_and_threads(&self) -> Result<(), ValidationError> {
        // speed 1..=10: zenravif documents "1 = slowest/best, 10 = fastest/worst" and
        // SpeedSettings::from_preset clamps; we reject 0 and >10 here for fail-fast callers.
        let speed_range: RangeInclusive<u8> = 1..=10;
        if !speed_range.contains(&self.speed) {
            return Err(ValidationError::SpeedOutOfRange {
                value: self.speed,
                valid: speed_range,
            });
        }
        // threads: None = rayon default. Some(0) is meaningless.
        if let Some(0) = self.threads {
            return Err(ValidationError::EncoderThreadsZero);
        }
        Ok(())
    }

    fn validate_transforms(&self) -> Result<(), ValidationError> {
        // rotation: AVIF irot box stores the angle as a 2-bit quarter-turn code
        // (0=0°, 1=90°, 2=180°, 3=270°). The serializer masks input to `& 0x03`, so passing
        // degrees would silently map to wrong rotations. We require irot code-point form
        // {0..=3} matching zenravif's ROTATION_RANGE.
        if let Some(angle) = self.rotation
            && angle > 3
        {
            return Err(ValidationError::RotationInvalid { value: angle });
        }
        // mirror axis: AVIF imir spec — 0 = vertical, 1 = horizontal.
        if let Some(axis) = self.mirror
            && axis > 1
        {
            return Err(ValidationError::MirrorInvalid { value: axis });
        }
        Ok(())
    }

    fn validate_cicp(&self) -> Result<(), ValidationError> {
        // CICP code points per ITU-T H.273: value 3 is reserved across all three fields.
        check_cicp_reserved(self.color_primaries, "color_primaries")?;
        check_cicp_reserved(self.transfer_characteristics, "transfer_characteristics")?;
        check_cicp_reserved(self.matrix_coefficients, "matrix_coefficients")?;
        Ok(())
    }

    #[cfg(feature = "encode-imazen")]
    fn validate_imazen_knobs(&self) -> Result<(), ValidationError> {
        // VAQ strength matches zenravif/zenrav1e's accepted band: 0.0 (off) ..= 4.0 (aggressive).
        let vaq_range: RangeInclusive<f64> = 0.0..=4.0;
        if !vaq_range.contains(&self.vaq_strength) || !self.vaq_strength.is_finite() {
            return Err(ValidationError::VaqStrengthOutOfRange {
                value: self.vaq_strength,
                valid: vaq_range,
            });
        }
        // seg_boost: zenravif's SEG_BOOST_RANGE = 0.5..=4.0.
        let seg_range: RangeInclusive<f64> = 0.5..=4.0;
        if let Some(b) = self.seg_boost
            && (!b.is_finite() || !seg_range.contains(&b))
        {
            return Err(ValidationError::SegBoostOutOfRange {
                value: b,
                valid: seg_range,
            });
        }
        // Cross-param: lossless overrides quality and is incompatible with VAQ — zenravif
        // disables QM internally for lossless; VAQ on top of quantizer=0 has no defined meaning.
        // (lossless + tune_still_image is allowed; the still-image tuning is a no-op at q=0
        // but conceptually conflict-free, unlike VAQ which actively fights the quantizer.)
        if self.lossless && self.enable_vaq {
            return Err(ValidationError::MutuallyExclusive {
                a: "lossless",
                b: "vaq",
            });
        }
        Ok(())
    }

    // We re-validate forwarded expert::InternalParams bounds at the EncoderConfig level so
    // callers who set the field via `with_internal_params` get a single call site for validation.
    #[cfg(feature = "__expert")]
    fn validate_expert_overrides(&self) -> Result<(), ValidationError> {
        if let Some((min, max)) = self.override_partition_range
            && (!partition_bound_ok(min) || !partition_bound_ok(max) || min > max)
        {
            return Err(ValidationError::PartitionRangeInvalid { min, max });
        }
        Ok(())
    }
}

#[cfg(feature = "encode")]
const QUALITY_RANGE: RangeInclusive<f32> = 0.0..=100.0;

#[cfg(feature = "encode")]
fn check_cicp_reserved(
    field_value: Option<u8>,
    field_name: &'static str,
) -> Result<(), ValidationError> {
    if let Some(v) = field_value
        && v == 3
    {
        return Err(ValidationError::CicpReserved {
            field: field_name,
            value: v,
        });
    }
    Ok(())
}

impl crate::DecoderConfig {
    /// Validate this `DecoderConfig` value.
    ///
    /// Returns `Err` on the first failed invariant. The decoder
    /// itself accepts `frame_size_limit = 0` as "no limit"; this
    /// method does **not** reject zero (no caller should be forced
    /// to pick an arbitrary cap). It validates positively-set
    /// numeric fields where a wrong value would silently misconfigure
    /// the decoder.
    ///
    /// `threads = 0` is the documented "auto-detect" sentinel and is
    /// accepted; positive values are also accepted. There is no
    /// invalid threads value at the moment.
    pub fn validate(&self) -> Result<(), ValidationError> {
        // Currently no DecoderConfig field has an invalid range:
        //   - threads: 0 = auto, any u32 accepted.
        //   - apply_grain: bool, every value valid.
        //   - frame_size_limit: 0 = no limit, any u32 accepted.
        //   - cpu_flags_mask: any u32 valid (0 = scalar-only).
        //   - parser_*_limit: Option<_>, every value valid.
        //   - prefer_8bit: bool, every value valid.
        //
        // The variant `DecoderFrameSizeLimitZero` is reserved for
        // future use if a stricter mode is added.
        Ok(())
    }
}
