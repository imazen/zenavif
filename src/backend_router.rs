//! Backend-owned support queries intersected with the real AVIF adapter.
//!
//! A codec's ability to encode raw planes does not imply that its AVIF
//! wrapper can prepare or mux them. Both layers must accept a candidate.
//! These queries allocate no image planes and perform no trial encodes.

use crate::{Av1Backend, EncodeChromaSubsampling, EncoderConfig, PlanInput};
#[cfg(feature = "zenav1-svt")]
pub use svtav1::avif::ZenEnhancement as SvtEnhancement;

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
pub fn query_still_backends(config: &EncoderConfig, input: PlanInput) -> Vec<BackendSupportReport> {
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
    input: PlanInput,
    backend: Av1Backend,
) -> Result<SuitabilityReport, String> {
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
            let chroma = match candidate.chroma_subsampling {
                EncodeChromaSubsampling::Yuv420 => svtav1::avif::ChromaSubsampling::Yuv420,
                EncodeChromaSubsampling::Yuv444 => svtav1::avif::ChromaSubsampling::Yuv444,
            };
            let mut encoder = svtav1::avif::AvifEncoder::new()
                .with_quality(candidate.quality)
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
                .resolve_still_policy()
                .map(|plan| match plan.suitability {
                    svtav1::avif::StillSuitability::Uncalibrated => {
                        SuitabilityReport::BackendUncalibrated
                    }
                })
                .map_err(|e| e.to_string())
        }
        #[cfg(feature = "zenav1-aom-encode")]
        Av1Backend::Zenav1Aom => crate::encoder_aom::key_frame_config(
            &candidate,
            input.width as usize,
            input.height as usize,
            depth,
            false,
        )
        .validate_configuration()
        .map(|_| SuitabilityReport::NotExposedByBackend)
        .map_err(|e| e.to_string()),
        _ => Err("backend is not available in this build".into()),
    }
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
pub enum StillPolicy {
    SvtParity(ParityReference),
    Zen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// An immutable resolved configuration, executable through all four RGB/RGBA
/// still entry points. Cloning/reusing a plan does not rerun backend selection.
/// This is not yet a portable serialized replay/cache record.
#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    config: EncoderConfig,
    input: PlanInput,
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
        if width != self.input.width as usize
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
    /// Resolve a real executable route without changing pixel requirements.
    /// Unknown calibration preserves a supported preferred backend; format
    /// fallback is explicitly labelled and never presented as an RD win.
    pub fn resolve_route(
        &self,
        request: RoutingRequest,
        input: PlanInput,
    ) -> Result<ResolvedRoute, RouteError> {
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
            if input.input_has_alpha {
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
        let mut reports = query_still_backends(&base, input);
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
