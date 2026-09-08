//! Wire format v1. Configuration fields are exhaustive: adding an EncoderConfig
//! field without adding it here fails compilation, rather than losing it on replay.
use super::*;
use crate::encoder::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

macro_rules! stored_config {
    ($( $(#[$cfg:meta])* $name:ident: $ty:ty $(=> $adapter:literal)?, )*) => {
        #[derive(Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct StoredConfig { $( $(#[$cfg])* $(#[serde(with = $adapter)])? $name: $ty, )* }
        impl From<EncoderConfig> for StoredConfig {
            fn from(value: EncoderConfig) -> Self {
                let EncoderConfig { $( $(#[$cfg])* $name, )* } = value;
                Self { $( $(#[$cfg])* $name, )* }
            }
        }
        impl From<StoredConfig> for EncoderConfig {
            fn from(value: StoredConfig) -> Self {
                let StoredConfig { $( $(#[$cfg])* $name, )* } = value;
                Self { $( $(#[$cfg])* $name, )* }
            }
        }
    }
}
stored_config! {
    backend: Av1Backend,
    #[cfg(feature = "zenav1-svt")]
    svt_route_policy: Option<svtav1::avif::EncodingPolicy> => "svt_policy",
    #[cfg(feature = "zenav1-svt")]
    svt_route_preset: Option<svtav1::avif::NativePreset> => "svt_preset",
    #[cfg(feature = "zenav1-svt")]
    svt_route_enhancements: svtav1::avif::ZenEnhancements => "svt_enhancements",
    #[cfg(feature = "zenav1-svt")]
    svt_film_grain: SvtFilmGrainConfig => "svt_grain",
    quality: f32,
    speed: u8,
    alpha_quality: Option<f32>,
    bit_depth: EncodeBitDepth,
    color_model: EncodeColorModel,
    chroma_subsampling: EncodeChromaSubsampling,
    alpha_color_mode: EncodeAlphaMode,
    threads: Option<usize>,
    exif: Option<Vec<u8>>,
    xmp: Option<Vec<u8>>,
    icc_profile: Option<Vec<u8>>,
    rotation: Option<u8>,
    mirror: Option<u8>,
    content_light_level: Option<(u16, u16)>,
    mastering_display: Option<MasteringDisplayConfig>,
    color_primaries: Option<u8>,
    transfer_characteristics: Option<u8>,
    matrix_coefficients: Option<u8>,
    pixel_range: Option<EncodePixelRange>,
    gain_map: Option<GainMapConfig>,
    gain_map_alt_colr: Option<(u8, u8, u8, bool)>,
    gain_map_alt_icc: Option<std::vec::Vec<u8>>,
    #[cfg(feature = "encode-imazen")]
    enable_qm: bool,
    #[cfg(feature = "encode-imazen")]
    enable_vaq: bool,
    #[cfg(feature = "encode-imazen")]
    vaq_strength: f64,
    #[cfg(feature = "encode-imazen")]
    tune_still_image: bool,
    #[cfg(feature = "encode-imazen")]
    lossless: bool,
    #[cfg(feature = "encode-imazen")]
    seg_boost: Option<f64>,
    #[cfg(feature = "encode-imazen")]
    override_cdef: Option<bool>,
    #[cfg(feature = "encode-imazen")]
    override_rdo_tx_decision: Option<bool>,
    #[cfg(feature = "encode-imazen")]
    override_sgr_full: Option<bool>,
    #[cfg(feature = "encode-imazen")]
    override_lru_on_skip: Option<bool>,
    #[cfg(feature = "encode-imazen")]
    override_segmentation_complex: Option<bool>,
    #[cfg(feature = "encode-imazen")]
    override_encode_bottomup: Option<bool>,
    #[cfg(feature = "encode-imazen")]
    override_partition_range: Option<(u8, u8)>,
    #[cfg(feature = "encode-imazen")]
    override_complex_prediction_modes: Option<bool>,
    #[cfg(feature = "encode-imazen")]
    override_lrf: Option<bool>,
    #[cfg(feature = "encode-imazen")]
    override_fast_deblock: Option<bool>,
    #[cfg(feature = "encode-imazen")]
    trellis: Option<bool>,
    #[cfg(feature = "encode-imazen")]
    palette_preference: Option<crate::palette_gate::PalettePreference>,
    #[cfg(feature = "encode-imazen")]
    fast_tier_budgets: Option<crate::fast_heads::FastTierBudgets>,
    #[cfg(any(feature = "two-pass-butteraugli", feature = "two-pass-zensim"))]
    sb_q_scale: Option<Box<[f32]>>,
    #[cfg(any(feature = "zenav1-svt", feature = "__expert"))]
    svt: crate::svt_params::SvtParams,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Replay {
    schema: u32,
    implementation_id: String,
    features: Vec<String>,
    config: StoredConfig,
    input: StillInput,
    policy: StillPolicy,
    selection: BackendSelection,
    effort: Option<f32>,
    requested_enhancements: u8,
    reason: RouteReason,
}
fn features() -> Vec<String> {
    [
        ("encode-imazen", cfg!(feature = "encode-imazen")),
        ("encode-mono", cfg!(feature = "encode-mono")),
        ("zenav1-svt", cfg!(feature = "zenav1-svt")),
        ("zenav1-aom-encode", cfg!(feature = "zenav1-aom-encode")),
        ("__expert", cfg!(feature = "__expert")),
        (
            "two-pass-butteraugli",
            cfg!(feature = "two-pass-butteraugli"),
        ),
        ("two-pass-zensim", cfg!(feature = "two-pass-zensim")),
    ]
    .into_iter()
    .filter(|(_, on)| *on)
    .map(|(name, _)| name.to_owned())
    .collect()
}
fn error(e: impl core::fmt::Display) -> RouteError {
    RouteError(e.to_string())
}
fn validate_portable(config: &EncoderConfig, id: &str) -> Result<(), RouteError> {
    if id.trim().is_empty() {
        return Err(error("replay requires an implementation/build identity"));
    }
    if !matches!(config.threads, Some(n) if n > 0) {
        return Err(error(
            "portable replay requires a pinned positive thread count",
        ));
    }
    if !config.quality.is_finite() || config.alpha_quality.is_some_and(|q| !q.is_finite()) {
        return Err(error("non-finite quality cannot be replayed"));
    }
    #[cfg(feature = "encode-imazen")]
    if !config.vaq_strength.is_finite() || config.seg_boost.is_some_and(|v| !v.is_finite()) {
        return Err(error("non-finite encoder controls cannot be replayed"));
    }
    #[cfg(any(feature = "zenav1-svt", feature = "__expert"))]
    if !config.svt.ac_bias.is_finite() {
        return Err(error("non-finite SVT bias cannot be replayed"));
    }
    #[cfg(any(feature = "two-pass-butteraugli", feature = "two-pass-zensim"))]
    if config
        .sb_q_scale
        .as_ref()
        .is_some_and(|v| v.iter().any(|x| !x.is_finite()))
    {
        return Err(error("non-finite superblock scales cannot be replayed"));
    }
    Ok(())
}
impl ResolvedRoute {
    /// Serialize all configuration, metadata and resolved selection, without pixel data.
    ///
    /// `implementation_id` must identify the caller's complete immutable build:
    /// zenavif and every backend/converter/muxer revision, dependency graph,
    /// compiler flags and target. Use a deployment artifact or build-manifest
    /// digest, not a package version or a mutable branch name. The caller owns
    /// this identity because a library cannot discover its consumer's actual
    /// dependency resolution or local dependency patches.
    ///
    /// Pin threads before resolution. Replay never guesses host-dependent work
    /// settings. Unknown schemas, fields, features and build identities fail closed.
    pub fn to_replay_json(&self, implementation_id: &str) -> Result<String, RouteError> {
        validate_portable(&self.config, implementation_id)?;
        self.config.validate_still_input(self.source)?;
        #[cfg(not(feature = "zenav1-svt"))]
        let requested_enhancements = 0;
        #[cfg(feature = "zenav1-svt")]
        let requested_enhancements = enhancement_bits(self.request.svt_enhancements);
        serde_json::to_string(&Replay {
            schema: 1,
            implementation_id: implementation_id.into(),
            features: features(),
            config: self.config.clone().into(),
            input: self.source,
            policy: self.request.policy,
            selection: self.request.backend,
            effort: self.request.effort.map(Effort::value),
            requested_enhancements,
            reason: self.reason,
        })
        .map_err(error)
    }
    /// Restore and revalidate the stored backend; never resolve a new selection.
    pub fn from_replay_json(json: &str, implementation_id: &str) -> Result<Self, RouteError> {
        let record: Replay = serde_json::from_str(json).map_err(error)?;
        if record.schema != 1 {
            return Err(error("unsupported route replay schema"));
        }
        if record.implementation_id != implementation_id || record.features != features() {
            return Err(error(
                "route replay build identity or feature envelope changed",
            ));
        }
        let config: EncoderConfig = record.config.into();
        validate_portable(&config, implementation_id)?;
        let mut request = RoutingRequest::default()
            .with_policy(record.policy)
            .with_backend(record.selection);
        request.effort = record.effort.map(Effort::new).transpose()?;
        #[cfg(feature = "zenav1-svt")]
        {
            request.svt_enhancements = enhancements_from_bits(record.requested_enhancements)?;
        }
        #[cfg(not(feature = "zenav1-svt"))]
        if record.requested_enhancements != 0 {
            return Err(error("SVT enhancements unavailable"));
        }
        if matches!(record.selection, BackendSelection::Explicit(b) if b != config.backend) {
            return Err(error("stored route contradicts explicit backend selection"));
        }
        match record.policy {
            StillPolicy::SvtParity(reference) => {
                if config.backend != Av1Backend::Zenav1Svt
                    || record.input.is_monochrome()
                    || record.input.plan_input().input_has_alpha
                {
                    return Err(error("stored route violates strict SVT parity policy"));
                }
                #[cfg(feature = "zenav1-svt")]
                if config.svt_route_policy
                    != Some(svtav1::avif::EncodingPolicy::SvtParity(reference.svt()))
                {
                    return Err(error("stored route reference conflicts with policy"));
                }
                #[cfg(not(feature = "zenav1-svt"))]
                {
                    let _ = reference;
                    return Err(error("SVT parity unavailable"));
                }
            }
            StillPolicy::Zen =>
            {
                #[cfg(feature = "zenav1-svt")]
                if config.svt_route_policy.is_some() {
                    return Err(error("stored route policy conflicts with configuration"));
                }
            }
        }
        if let Some(effort) = request.effort {
            if config.speed != 10 - (effort.value() * 9.0).floor() as u8 {
                return Err(error("stored speed conflicts with requested effort"));
            }
            #[cfg(feature = "zenav1-svt")]
            if config.svt_route_preset
                != Some(
                    svtav1::avif::Effort::new(effort.value())
                        .unwrap()
                        .native_preset(),
                )
            {
                return Err(error(
                    "stored native preset conflicts with requested effort",
                ));
            }
        }
        #[cfg(feature = "zenav1-svt")]
        if enhancement_bits(request.svt_enhancements)
            & !enhancement_bits(config.svt_route_enhancements)
            != 0
        {
            return Err(error("stored route drops a requested enhancement"));
        }
        let valid_reason = match (record.policy, record.selection) {
            (StillPolicy::SvtParity(_), _) => record.reason == RouteReason::StrictParity,
            (_, BackendSelection::Explicit(_)) => record.reason == RouteReason::ExplicitBackend,
            _ => matches!(
                record.reason,
                RouteReason::PreferredBackendSupported | RouteReason::CapabilityFallback
            ),
        };
        if !valid_reason {
            return Err(error("stored route reason conflicts with selection policy"));
        }
        config.validate_still_input(record.input)?;
        let reports = query_still_backends(&config, record.input);
        Ok(Self {
            config,
            input: record.input.plan_input(),
            source: record.input,
            request,
            reports,
            reason: record.reason,
        })
    }
    /// SHA-256 over the complete replay record and the caller's pixel digest.
    /// Hash logical samples in row order, excluding row padding, for `pixel_digest`.
    /// It must cover every input channel and precision bit. Metadata and gain-map
    /// bytes are already included in the record. This is an exact cache identity,
    /// not a promise to collapse different configurations that happen to alias.
    pub fn cache_key(
        &self,
        implementation_id: &str,
        pixel_digest: [u8; 32],
    ) -> Result<[u8; 32], RouteError> {
        let record = self.to_replay_json(implementation_id)?;
        let mut hash = Sha256::new();
        hash.update(b"zenavif-route-cache-v1\0");
        hash.update(pixel_digest);
        hash.update(record.as_bytes());
        Ok(hash.finalize().into())
    }
}
#[cfg(feature = "zenav1-svt")]
fn enhancement_bits(set: svtav1::avif::ZenEnhancements) -> u8 {
    u8::from(set.contains(SvtEnhancement::AomIntraEdgeFilter))
        | (u8::from(set.contains(SvtEnhancement::AomRestorationUnitSearch)) << 1)
}
#[cfg(feature = "zenav1-svt")]
fn enhancements_from_bits(bits: u8) -> Result<svtav1::avif::ZenEnhancements, RouteError> {
    if bits & !3 != 0 {
        return Err(error("unknown SVT enhancement bits"));
    }
    let mut set = svtav1::avif::ZenEnhancements::default();
    if bits & 1 != 0 {
        set = set.with(SvtEnhancement::AomIntraEdgeFilter);
    }
    if bits & 2 != 0 {
        set = set.with(SvtEnhancement::AomRestorationUnitSearch);
    }
    Ok(set)
}
#[cfg(feature = "zenav1-svt")]
mod svt_enhancements {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &svtav1::avif::ZenEnhancements,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        enhancement_bits(*value).serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<svtav1::avif::ZenEnhancements, D::Error> {
        enhancements_from_bits(u8::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
#[cfg(feature = "zenav1-svt")]
mod svt_preset {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &Option<svtav1::avif::NativePreset>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        value.map(|p| p.value()).serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Option<svtav1::avif::NativePreset>, D::Error> {
        Option::<i8>::deserialize(d)?
            .map(|v| {
                svtav1::avif::NativePreset::new(v)
                    .ok_or_else(|| serde::de::Error::custom("invalid native preset"))
            })
            .transpose()
    }
}
#[cfg(feature = "zenav1-svt")]
mod svt_policy {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &Option<svtav1::avif::EncodingPolicy>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .map(|p| match p {
                svtav1::avif::EncodingPolicy::Zen => 0u8,
                svtav1::avif::EncodingPolicy::SvtParity(
                    svtav1::avif::SvtReference::Mainline420,
                ) => 1,
                svtav1::avif::EncodingPolicy::SvtParity(svtav1::avif::SvtReference::Hybrid3115) => {
                    2
                }
            })
            .serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Option<svtav1::avif::EncodingPolicy>, D::Error> {
        Option::<u8>::deserialize(d)?
            .map(|v| match v {
                0 => Ok(svtav1::avif::EncodingPolicy::Zen),
                1 => Ok(svtav1::avif::EncodingPolicy::SvtParity(
                    svtav1::avif::SvtReference::Mainline420,
                )),
                2 => Ok(svtav1::avif::EncodingPolicy::SvtParity(
                    svtav1::avif::SvtReference::Hybrid3115,
                )),
                _ => Err(serde::de::Error::custom("unknown SVT policy")),
            })
            .transpose()
    }
}

#[cfg(feature = "zenav1-svt")]
mod svt_grain {
    use super::*;
    macro_rules! table {
        ($( $field:ident: $ty:ty, )*) => {
            #[derive(Serialize, Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Table { $( $field: $ty, )* }
            impl From<SvtFilmGrainTable> for Table {
                fn from(value: SvtFilmGrainTable) -> Self {
                    let SvtFilmGrainTable { $( $field, )* } = value;
                    Self { $( $field, )* }
                }
            }
            impl From<Table> for SvtFilmGrainTable {
                fn from(value: Table) -> Self {
                    let Table { $( $field, )* } = value;
                    Self { $( $field, )* }
                }
            }
        }
    }
    table! {
        apply_grain: bool,
        random_seed: u16,
        num_y_points: usize,
        scaling_points_y: [[u8; 2]; 14],
        chroma_scaling_from_luma: bool,
        num_cb_points: usize,
        scaling_points_cb: [[u8; 2]; 10],
        num_cr_points: usize,
        scaling_points_cr: [[u8; 2]; 10],
        scaling_shift: u8,
        ar_coeff_lag: u8,
        ar_coeffs_y: [i16; 24],
        ar_coeffs_cb: [i16; 25],
        ar_coeffs_cr: [i16; 25],
        ar_coeff_shift: u8,
        grain_scale_shift: u8,
        cb_mult: u8,
        cb_luma_mult: u8,
        cb_offset: u16,
        cr_mult: u8,
        cr_luma_mult: u8,
        cr_offset: u16,
        overlap_flag: bool,
        clip_to_restricted_range: bool,
    }
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Grain {
        denoise_strength: u8,
        denoise_apply: bool,
        adaptive: bool,
        table: Option<Table>,
        ignore_ref: bool,
    }
    pub fn serialize<S: serde::Serializer>(
        value: &SvtFilmGrainConfig,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let SvtFilmGrainConfig {
            denoise_strength,
            denoise_apply,
            adaptive,
            table,
            ignore_ref,
        } = value.clone();
        Grain {
            denoise_strength,
            denoise_apply,
            adaptive,
            table: table.map(Into::into),
            ignore_ref,
        }
        .serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<SvtFilmGrainConfig, D::Error> {
        let Grain {
            denoise_strength,
            denoise_apply,
            adaptive,
            table,
            ignore_ref,
        } = Grain::deserialize(d)?;
        let config = SvtFilmGrainConfig {
            denoise_strength,
            denoise_apply,
            adaptive,
            table: table.map(Into::into),
            ignore_ref,
        };
        config.validate().map_err(serde::de::Error::custom)?;
        Ok(config)
    }
}
