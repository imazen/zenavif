//! Backend-owned support queries intersected with the real AVIF adapter.
//!
//! A codec's ability to encode raw planes does not imply that its AVIF
//! wrapper can prepare or mux them. Both layers must accept a candidate.
//! These queries allocate no image planes and perform no trial encodes.

use crate::{Av1Backend, EncodeChromaSubsampling, EncoderConfig, PlanInput};

/// Configuration support for one built or unavailable backend.
#[derive(Debug, Clone)]
pub struct BackendSupportReport {
    /// The actual implementation queried.
    pub backend: Av1Backend,
    /// Why this backend or its adapter cannot serve the unchanged request.
    pub refusal: Option<String>,
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
    [Av1Backend::Zenravif, Av1Backend::Zenav1Svt, Av1Backend::Zenav1Aom]
        .into_iter()
        .map(|backend| BackendSupportReport {
            backend,
            refusal: query_one(config, input, backend).err(),
        })
        .collect()
}

fn query_one(config: &EncoderConfig, input: PlanInput, backend: Av1Backend) -> Result<(), String> {
    if input.width == 0 || input.height == 0 {
        return Err("still-image width and height must be nonzero".into());
    }
    // Backend-specific explicit knobs cannot migrate to a codec that ignores
    // them. Defaults remain defaults; callers can inspect every refusal.
    #[cfg(any(feature = "zenav1-svt", feature = "__expert"))]
    if backend != Av1Backend::Zenav1Svt && config.svt != crate::svt_params::SvtParams::default() {
        return Err("explicit SVT coding controls require zenav1-svt".into());
    }
    let candidate = config.clone().backend(backend);
    candidate.validate_for_input(input).map_err(|e| e.to_string())?;
    let depth = candidate.coded_bit_depth_bits(input.input_is_16bit);
    match backend {
        Av1Backend::Zenravif => {
            // zenravif exposes 8/10, even though its zenrav1e backend also
            // handles 12-bit raw planes. Query the actual consumer envelope.
            if !matches!(depth, 8 | 10) {
                return Err("zenravif's pixel API codes only 8 and 10 bits".into());
            }
            Ok(())
        }
        #[cfg(feature = "zenav1-svt")]
        Av1Backend::Zenav1Svt => {
            let chroma = match candidate.chroma_subsampling {
                EncodeChromaSubsampling::Yuv420 => svtav1::avif::ChromaSubsampling::Yuv420,
                EncodeChromaSubsampling::Yuv444 => svtav1::avif::ChromaSubsampling::Yuv444,
            };
            svtav1::avif::AvifEncoder::new()
                .with_bit_depth(depth)
                .with_chroma_subsampling(chroma)
                .with_speed(candidate.speed)
                .validate_configuration()
                .map_err(|e| e.to_string())
        }
        #[cfg(feature = "zenav1-aom-encode")]
        Av1Backend::Zenav1Aom => crate::encoder_aom::key_frame_config(
            &candidate, input.width as usize, input.height as usize, depth, false,
        ).validate_configuration().map_err(|e| e.to_string()),
        _ => Err("backend is not available in this build".into()),
    }
}
