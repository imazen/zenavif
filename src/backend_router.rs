//! Backend-owned support queries intersected with the real AVIF adapter.
//!
//! A codec's ability to encode raw planes does not imply that its AVIF
//! wrapper can prepare or mux them. Both layers must accept a candidate.
//! These queries allocate no image planes and perform no trial encodes.

#[cfg(feature = "routing-replay")]
#[path = "route_replay.rs"]
mod replay;

#[cfg(feature = "zenav1-svt")]
use crate::EncodeChromaSubsampling;
use crate::{Av1Backend, EncoderConfig, PlanInput};
#[cfg(feature = "zenav1-svt")]
pub use svtav1::avif::ZenEnhancement as SvtEnhancement;

#[cfg(feature = "zenav1-svt")]
pub use svtav1::encoder::film_grain_config::FilmGrainConfig as SvtFilmGrainConfig;
#[cfg(feature = "zenav1-svt")]
pub use svtav1::entropy::obu::FilmGrainParams as SvtFilmGrainTable;

/// Exact pixel entry point, kept separate from the legacy RGB-only `PlanInput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "routing-replay",
    derive(serde::Serialize, serde::Deserialize)
)]
pub enum StillInput {
    Rgb8 { width: u32, height: u32 },
    Rgba8 { width: u32, height: u32 },
    Rgb16 { width: u32, height: u32 },
    Rgba16 { width: u32, height: u32 },
    Gray8 { width: u32, height: u32 },
}
impl StillInput {
    pub fn plan_input(self) -> PlanInput {
        let (width, height, input_is_16bit, input_has_alpha) = match self {
            Self::Rgb8 { width, height } | Self::Gray8 { width, height } => {
                (width, height, false, false)
            }
            Self::Rgba8 { width, height } => (width, height, false, true),
            Self::Rgb16 { width, height } => (width, height, true, false),
            Self::Rgba16 { width, height } => (width, height, true, true),
        };
        PlanInput {
            width,
            height,
            input_is_16bit,
            input_has_alpha,
        }
    }
    pub fn is_monochrome(self) -> bool {
        matches!(self, Self::Gray8 { .. })
    }
}
impl From<PlanInput> for StillInput {
    fn from(p: PlanInput) -> Self {
        match (p.input_is_16bit, p.input_has_alpha) {
            (false, false) => Self::Rgb8 {
                width: p.width,
                height: p.height,
            },
            (false, true) => Self::Rgba8 {
                width: p.width,
                height: p.height,
            },
            (true, false) => Self::Rgb16 {
                width: p.width,
                height: p.height,
            },
            (true, true) => Self::Rgba16 {
                width: p.width,
                height: p.height,
            },
        }
    }
}

/// Configuration support for one built or unavailable backend.
#[derive(Debug, Clone)]
pub struct BackendSupportReport {
    /// The actual implementation queried.
    pub backend: Av1Backend,
    /// Why this backend or its adapter cannot serve the unchanged request.
    pub refusal: Option<String>,
    pub suitability: Option<SuitabilityReport>,
}

impl BackendSupportReport {
    /// Whether both the encoder and its AVIF adapter accept the request.
    pub fn supported(&self) -> bool {
        self.refusal.is_none()
    }
}

/// Query all three backends without changing the requested pixel format,
/// precision, range, alpha or coding settings. An unavailable Cargo feature
/// produces a refusal instead of disappearing from the report.
pub fn query_still_backends(
    config: &EncoderConfig,
    input: impl Into<StillInput>,
) -> Vec<BackendSupportReport> {
    let input = input.into();
    [
        Av1Backend::Zenravif,
        Av1Backend::Zenav1Svt,
        Av1Backend::Zenav1Aom,
    ]
    .into_iter()
    .map(|backend| match query_one(config, input, backend) {
        Ok(suitability) => BackendSupportReport {
            backend,
            refusal: None,
            suitability: Some(suitability),
        },
        Err(refusal) => BackendSupportReport {
            backend,
            refusal: Some(refusal),
            suitability: None,
        },
    })
    .collect()
}

fn query_one(
    config: &EncoderConfig,
    source: StillInput,
    backend: Av1Backend,
) -> Result<SuitabilityReport, String> {
    let input = source.plan_input();
    if source.is_monochrome() && !cfg!(feature = "encode-mono") {
        return Err("grayscale requires the encode-mono cargo feature".into());
    }
    if input.width == 0 || input.height == 0 {
        return Err("still-image width and height must be nonzero".into());
    }
    // Backend-specific explicit knobs cannot migrate to a codec that ignores
    // them. Defaults remain defaults; callers can inspect every refusal.
    #[cfg(any(feature = "zenav1-svt", feature = "__expert"))]
    if backend != Av1Backend::Zenav1Svt && config.svt != crate::svt_params::SvtParams::default() {
        return Err("explicit SVT coding controls require zenav1-svt".into());
    }
    #[cfg(feature = "zenav1-svt")]
    if backend != Av1Backend::Zenav1Svt && !config.svt_route_enhancements.is_empty() {
        return Err("explicit SVT enhancements cannot migrate to another backend".into());
    }
    let candidate = config.clone().backend(backend);
    candidate
        .validate_for_input(input)
        .map_err(|e| e.to_string())?;
    validate_adapter_controls(&candidate, source)?;
    let depth = candidate.coded_bit_depth_bits(input.input_is_16bit);
    match backend {
        Av1Backend::Zenravif => {
            // zenravif exposes 8/10, even though its zenrav1e backend also
            // handles 12-bit raw planes. Query the actual consumer envelope.
            if !matches!(depth, 8 | 10) {
                return Err("zenravif's pixel API codes only 8 and 10 bits".into());
            }
            Ok(SuitabilityReport::NotExposedByBackend)
        }
        #[cfg(feature = "zenav1-svt")]
        Av1Backend::Zenav1Svt => {
            crate::encoder_svt_rs::validate_still_controls(&candidate, source.is_monochrome())?;
            let chroma = match candidate.chroma_subsampling {
                EncodeChromaSubsampling::Yuv420 => svtav1::avif::ChromaSubsampling::Yuv420,
                EncodeChromaSubsampling::Yuv444 => svtav1::avif::ChromaSubsampling::Yuv444,
            };
            let mut encoder = svtav1::avif::AvifEncoder::new()
                .with_quality(candidate.quality.clamp(1.0, 100.0))
                .with_film_grain(candidate.svt_film_grain.clone())
                .with_bit_depth(depth)
                .with_chroma_subsampling(chroma)
                .with_native_preset(candidate.svt_route_preset.unwrap_or_else(|| {
                    svtav1::avif::NativePreset::new(crate::encoder_svt_rs::speed_to_svt_preset(
                        candidate.speed,
                    ) as i8)
                    .unwrap()
                }));
            if let Some(policy) = candidate.svt_route_policy {
                encoder = encoder.with_policy(policy);
            }
            for enhancement in [
                SvtEnhancement::AomIntraEdgeFilter,
                SvtEnhancement::AomRestorationUnitSearch,
            ] {
                if candidate.svt_route_enhancements.contains(enhancement) {
                    encoder = encoder.with_enhancement(enhancement);
                }
            }
            encoder
                .validate_configuration_for_input(
                    input.width,
                    input.height,
                    if source.is_monochrome() {
                        svtav1::avif::StillInputFormat::Monochrome
                    } else {
                        svtav1::avif::StillInputFormat::Yuv420
                    },
                )
                .map_err(|e| e.to_string())?;
            encoder
                .resolve_still_policy()
                .map(|plan| match plan.suitability {
                    svtav1::avif::StillSuitability::Uncalibrated => {
                        SuitabilityReport::BackendUncalibrated
                    }
                })
                .map_err(|e| e.to_string())
        }
        #[cfg(feature = "zenav1-aom-encode")]
        Av1Backend::Zenav1Aom => {
            if let Some(reason) = crate::encoder_aom::aom_depth_error(depth, source.is_monochrome())
            {
                return Err(reason.into());
            }
            crate::encoder_aom::key_frame_config(
                &candidate,
                input.width as usize,
                input.height as usize,
                depth,
                source.is_monochrome(),
            )
            .validate_configuration()
            .map(|_| SuitabilityReport::NotExposedByBackend)
            .map_err(|e| e.to_string())
        }
        _ => Err("backend is not available in this build".into()),
    }
}

/// Settings that an adapter cannot represent must not be accepted by a query
/// or silently ignored by a public pixel entry point.
pub(crate) fn validate_adapter_controls(
    config: &EncoderConfig,
    source: StillInput,
) -> Result<(), String> {
    if config.gain_map.is_none()
        && (config.gain_map_alt_colr.is_some() || config.gain_map_alt_icc.is_some())
    {
        return Err("alternate gain-map color metadata requires a gain map".into());
    }
    #[cfg(any(feature = "zenav1-svt", feature = "__expert"))]
    if config.backend != Av1Backend::Zenav1Svt
        && config.svt != crate::svt_params::SvtParams::default()
    {
        return Err("explicit SVT coding controls require zenav1-svt".into());
    }
    #[cfg(feature = "zenav1-svt")]
    if config.backend != Av1Backend::Zenav1Svt
        && config.svt_film_grain != SvtFilmGrainConfig::default()
    {
        return Err("explicit SVT film-grain controls cannot migrate to another backend".into());
    }
    #[cfg(feature = "encode-imazen")]
    {
        // These preferences are stored but have no consumer in the pinned
        // zenravif adapter. Do not present them as enabled encoding tools.
        if config.palette_preference.is_some() || config.fast_tier_budgets.is_some() {
            return Err(
                "palette preferences and fast-tier budgets are not wired in the pinned adapter"
                    .into(),
            );
        }
        if config.backend != Av1Backend::Zenravif
            && (!config.enable_qm
                || config.enable_vaq
                || config.vaq_strength != 1.0
                || config.tune_still_image
                || config.seg_boost.is_some()
                || config.override_cdef.is_some()
                || config.override_rdo_tx_decision.is_some()
                || config.override_sgr_full.is_some()
                || config.override_lru_on_skip.is_some()
                || config.override_segmentation_complex.is_some()
                || config.override_encode_bottomup.is_some()
                || config.override_partition_range.is_some()
                || config.override_complex_prediction_modes.is_some()
                || config.override_lrf.is_some()
                || config.override_fast_deblock.is_some()
                || config.trellis.is_some())
        {
            return Err(
                "explicit zenravif coding controls cannot migrate to another backend".into(),
            );
        }
        #[cfg(not(feature = "__expert"))]
        if config.override_partition_range.is_some()
            || config.override_complex_prediction_modes.is_some()
            || config.override_lrf.is_some()
            || config.override_fast_deblock.is_some()
        {
            return Err(
                "these expert controls require the __expert feature to reach the encoder".into(),
            );
        }
    }
    #[cfg(any(feature = "two-pass-butteraugli", feature = "two-pass-zensim"))]
    if config.backend != Av1Backend::Zenravif && config.sb_q_scale.is_some() {
        return Err("superblock scale maps require zenravif".into());
    }
    if matches!(
        config.backend,
        Av1Backend::Zenav1Svt | Av1Backend::Zenav1Aom
    ) {
        // The aom seam's matrix is DERIVED from the seam's own rule rather
        // than restated here, because restating it is what made this refusal
        // wrong: `if mono { 2 } else { 6 }` refused CICP 0 with the message
        // below even at `color_model(Rgb)` + 4:4:4, which is precisely where
        // `mux_aom` writes CICP 0 itself — so a caller who asked for exactly
        // what the muxer was about to write was told the pixel conversion path
        // does not implement it. It does; it is the identity path.
        // `reject_unsupported_config` still refuses identity outside 4:4:4,
        // by name, which is the refusal that IS true.
        let matrix = match config.backend {
            #[cfg(feature = "zenav1-aom-encode")]
            Av1Backend::Zenav1Aom => {
                crate::encoder_aom::coded_matrix_coefficients(config, source.is_monochrome())
            }
            _ => {
                if source.is_monochrome() {
                    2
                } else {
                    6
                }
            }
        };
        if config.matrix_coefficients.is_some_and(|m| m != matrix) {
            return Err("requested matrix is not implemented by this pixel conversion path".into());
        }
        if config
            .color_primaries
            .is_some_and(|v| !matches!(v, 1 | 2 | 6 | 9 | 11 | 12))
        {
            return Err("requested color primaries cannot be preserved by this AVIF muxer".into());
        }
        if config
            .transfer_characteristics
            .is_some_and(|v| !matches!(v, 1 | 2 | 4..=18))
        {
            return Err(
                "requested transfer characteristics cannot be preserved by this AVIF muxer".into(),
            );
        }
    }
    Ok(())
}

/// Checked ordinal effort, independent of a backend's preset numbering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effort(f32);
impl Effort {
    pub fn new(value: f32) -> Result<Self, RouteError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(RouteError("effort must be finite and in 0..=1".into()));
        }
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }
    pub const fn value(self) -> f32 {
        self.0
    }
}

/// The exact source a strict parity request targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(
    feature = "routing-replay",
    derive(serde::Serialize, serde::Deserialize)
)]
pub enum ParityReference {
    #[default]
    Mainline420,
    Hybrid3115,
}
#[cfg(feature = "zenav1-svt")]
impl ParityReference {
    pub(crate) fn svt(self) -> svtav1::avif::SvtReference {
        match self {
            Self::Mainline420 => svtav1::avif::SvtReference::Mainline420,
            Self::Hybrid3115 => svtav1::avif::SvtReference::Hybrid3115,
        }
    }
}

/// Policy is independent of which backend an automatic request prefers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "routing-replay",
    derive(serde::Serialize, serde::Deserialize)
)]
pub enum StillPolicy {
    SvtParity(ParityReference),
    Zen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "routing-replay",
    derive(serde::Serialize, serde::Deserialize)
)]
pub enum BackendSelection {
    /// Use exactly this backend; an unsupported request is an error.
    Explicit(Av1Backend),
    /// Keep the config's preferred backend when supported but uncalibrated.
    Automatic,
}

/// Inputs to deterministic route resolution. Quality and pixel requirements
/// remain in EncoderConfig; effort is optional to preserve its existing speed.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RoutingRequest {
    pub policy: StillPolicy,
    pub backend: BackendSelection,
    pub effort: Option<Effort>,
    #[cfg(feature = "zenav1-svt")]
    svt_enhancements: svtav1::avif::ZenEnhancements,
}
impl RoutingRequest {
    /// Select an explicit beyond-native SVT experiment. It requires the native
    /// research tip and Zen policy; no other backend may ignore this request.
    #[cfg(feature = "zenav1-svt")]
    pub fn with_svt_enhancement(mut self, enhancement: SvtEnhancement) -> Self {
        self.svt_enhancements = self.svt_enhancements.with(enhancement);
        self
    }

    pub fn with_policy(mut self, policy: StillPolicy) -> Self {
        self.policy = policy;
        self
    }
    pub fn with_backend(mut self, backend: BackendSelection) -> Self {
        self.backend = backend;
        self
    }
    pub fn with_effort(mut self, effort: Effort) -> Self {
        self.effort = Some(effort);
        self
    }
}
impl Default for RoutingRequest {
    fn default() -> Self {
        Self {
            policy: StillPolicy::Zen,
            backend: BackendSelection::Automatic,
            effort: None,
            #[cfg(feature = "zenav1-svt")]
            svt_enhancements: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[cfg_attr(
    feature = "routing-replay",
    derive(serde::Serialize, serde::Deserialize)
)]
pub enum RouteReason {
    ExplicitBackend,
    StrictParity,
    PreferredBackendSupported,
    CapabilityFallback,
}

/// All current broad-corpus RD/time calibration is incomplete. This is an
/// absence of an estimate, never an estimate of zero cost or a preset ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RouteCalibration {
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteError(pub String);
impl core::fmt::Display for RouteError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for RouteError {}

/// An immutable resolved configuration for RGB/RGBA and feature-gated Gray8.
/// Cloning/reusing it never reruns backend selection. The `routing-replay`
/// feature adds versioned portable records and complete cache identities.
#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    config: EncoderConfig,
    input: PlanInput,
    source: StillInput,
    request: RoutingRequest,
    reports: Vec<BackendSupportReport>,
    reason: RouteReason,
}
impl ResolvedRoute {
    pub const fn version(&self) -> u32 {
        1
    }
    pub fn backend(&self) -> Av1Backend {
        self.config.backend
    }
    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }
    pub fn input(&self) -> PlanInput {
        self.input
    }
    pub fn input_kind(&self) -> StillInput {
        self.source
    }
    pub fn request(&self) -> RoutingRequest {
        self.request
    }
    pub fn reports(&self) -> &[BackendSupportReport] {
        &self.reports
    }
    pub fn reason(&self) -> RouteReason {
        self.reason
    }
    #[cfg(feature = "zenav1-svt")]
    pub fn svt_enhancements(&self) -> svtav1::avif::ZenEnhancements {
        self.config.svt_route_enhancements
    }
    pub fn calibration(&self) -> RouteCalibration {
        RouteCalibration::Unavailable
    }

    fn check_input(
        &self,
        width: usize,
        height: usize,
        deep: bool,
        alpha: bool,
    ) -> crate::Result<()> {
        if self.source.is_monochrome()
            || width != self.input.width as usize
            || height != self.input.height as usize
            || deep != self.input.input_is_16bit
            || alpha != self.input.input_has_alpha
        {
            return Err(whereat::at!(crate::Error::InvalidParameters(
                "resolved route input kind or dimensions changed".into()
            )));
        }
        Ok(())
    }
    #[cfg(feature = "encode-mono")]
    pub fn encode_gray8(
        &self,
        img: imgref::ImgRef<'_, u8>,
        stop: almost_enough::StopToken,
    ) -> crate::Result<crate::EncodedImage> {
        if !self.source.is_monochrome()
            || img.width() != self.input.width as usize
            || img.height() != self.input.height as usize
        {
            return Err(whereat::at!(crate::Error::InvalidParameters(
                "resolved route input kind or dimensions changed".into()
            )));
        }
        crate::encode_gray8(img, &self.config, stop)
    }
    pub fn encode_rgb8(
        &self,
        img: imgref::ImgRef<'_, rgb::Rgb<u8>>,
        stop: almost_enough::StopToken,
    ) -> crate::Result<crate::EncodedImage> {
        self.check_input(img.width(), img.height(), false, false)?;
        crate::encode_rgb8(img, &self.config, stop)
    }
    pub fn encode_rgba8(
        &self,
        img: imgref::ImgRef<'_, rgb::Rgba<u8>>,
        stop: almost_enough::StopToken,
    ) -> crate::Result<crate::EncodedImage> {
        self.check_input(img.width(), img.height(), false, true)?;
        crate::encode_rgba8(img, &self.config, stop)
    }
    pub fn encode_rgb16(
        &self,
        img: imgref::ImgRef<'_, rgb::Rgb<u16>>,
        stop: almost_enough::StopToken,
    ) -> crate::Result<crate::EncodedImage> {
        self.check_input(img.width(), img.height(), true, false)?;
        crate::encode_rgb16(img, &self.config, stop)
    }
    pub fn encode_rgba16(
        &self,
        img: imgref::ImgRef<'_, rgb::Rgba<u16>>,
        stop: almost_enough::StopToken,
    ) -> crate::Result<crate::EncodedImage> {
        self.check_input(img.width(), img.height(), true, true)?;
        crate::encode_rgba16(img, &self.config, stop)
    }
}

impl EncoderConfig {
    /// Validate the actual pixel entry point against the same checks as routing.
    pub fn validate_still_input(&self, input: impl Into<StillInput>) -> Result<(), RouteError> {
        query_one(self, input.into(), self.backend)
            .map(|_| ())
            .map_err(RouteError)
    }
    /// Resolve a real executable route without changing pixel requirements.
    /// Unknown calibration preserves a supported preferred backend; format
    /// fallback is explicitly labelled and never presented as an RD win.
    pub fn resolve_route(
        &self,
        request: RoutingRequest,
        source: impl Into<StillInput>,
    ) -> Result<ResolvedRoute, RouteError> {
        let source = source.into();
        let input = source.plan_input();
        let mut base = self.clone();
        if let Some(effort) = request.effort {
            // Legacy wrapper speed buckets. SVT additionally resolves the native
            // research tip through its own checked resolver below.
            base.speed = 10 - (effort.value() * 9.0).floor() as u8;
        }
        if let StillPolicy::SvtParity(_) = request.policy {
            if matches!(request.backend, BackendSelection::Explicit(b) if b != Av1Backend::Zenav1Svt)
            {
                return Err(RouteError("SvtParity cannot select another backend".into()));
            }
            if input.input_has_alpha || source.is_monochrome() {
                return Err(RouteError(
                    "SvtParity cannot code the Rust-only monochrome alpha extension".into(),
                ));
            }
        }
        let preferred = match (request.policy, request.backend) {
            (StillPolicy::SvtParity(_), _) => Av1Backend::Zenav1Svt,
            (_, BackendSelection::Explicit(b)) => b,
            _ => base.backend,
        };
        #[cfg(feature = "zenav1-svt")]
        {
            for enhancement in [
                SvtEnhancement::AomIntraEdgeFilter,
                SvtEnhancement::AomRestorationUnitSearch,
            ] {
                if request.svt_enhancements.contains(enhancement) {
                    base.svt_route_enhancements = base.svt_route_enhancements.with(enhancement);
                }
            }
            base.svt_route_policy = match request.policy {
                StillPolicy::SvtParity(reference) => {
                    Some(svtav1::avif::EncodingPolicy::SvtParity(reference.svt()))
                }
                StillPolicy::Zen => None,
            };
            if let Some(e) = request.effort {
                base.svt_route_preset = Some(
                    svtav1::avif::Effort::new(e.value())
                        .unwrap()
                        .native_preset(),
                );
            }
        }
        let mut reports = query_still_backends(&base, source);
        // Explicit legacy/unknown backend identities must also get a refusal.
        if !reports.iter().any(|r| r.backend == preferred) {
            reports.push(BackendSupportReport {
                backend: preferred,
                refusal: Some("backend is unavailable for routing".into()),
                suitability: None,
            });
        }
        let preferred_supported = reports
            .iter()
            .any(|r| r.backend == preferred && r.supported());
        let strict = matches!(request.policy, StillPolicy::SvtParity(_))
            || matches!(request.backend, BackendSelection::Explicit(_));
        let chosen = if preferred_supported {
            Some(preferred)
        } else if strict {
            None
        } else {
            reports.iter().find(|r| r.supported()).map(|r| r.backend)
        };
        let Some(chosen) = chosen else {
            return Err(RouteError(format!(
                "no permitted backend supports the request: {reports:?}"
            )));
        };
        let reason = if matches!(request.policy, StillPolicy::SvtParity(_)) {
            RouteReason::StrictParity
        } else if matches!(request.backend, BackendSelection::Explicit(_)) {
            RouteReason::ExplicitBackend
        } else if chosen == preferred {
            RouteReason::PreferredBackendSupported
        } else {
            RouteReason::CapabilityFallback
        };
        let config = base.backend(chosen);
        Ok(ResolvedRoute {
            config,
            input,
            source,
            request,
            reports,
            reason,
        })
    }
}

/// Provenance of a supported backend's suitability response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SuitabilityReport {
    /// The backend reports that calibration is incomplete.
    BackendUncalibrated,
    /// Support validation exists but no suitability API is exposed yet.
    NotExposedByBackend,
}

/// Actual native speed control for the primary color stream, not auxiliary
/// planes or a complete encode/cache fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeSettings {
    pub control: &'static str,
    pub value: i16,
    pub reference: Option<&'static str>,
}
impl ResolvedRoute {
    pub fn primary_native_settings(&self) -> NativeSettings {
        match self.config.backend {
            #[cfg(feature = "zenav1-svt")]
            Av1Backend::Zenav1Svt => {
                let reference = match self.config.svt_route_policy {
                    Some(svtav1::avif::EncodingPolicy::SvtParity(reference)) => reference,
                    _ => svtav1::avif::SvtReference::Hybrid3115,
                };
                NativeSettings {
                    control: "svt-native-preset",
                    value: self
                        .config
                        .svt_route_preset
                        .map(|p| p.value() as i16)
                        .unwrap_or_else(|| {
                            crate::encoder_svt_rs::speed_to_svt_preset(self.config.speed) as i16
                        }),
                    reference: Some(reference.id()),
                }
            }
            #[cfg(feature = "zenav1-aom-encode")]
            Av1Backend::Zenav1Aom => NativeSettings {
                control: "aom-cpu-used",
                value: crate::encoder_aom::speed_to_cpu_used(self.config.speed) as i16,
                reference: None,
            },
            _ => NativeSettings {
                control: "zenravif-speed",
                value: self.config.speed_effective() as i16,
                reference: None,
            },
        }
    }
}
