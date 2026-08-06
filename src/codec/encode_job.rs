//! [`AvifEncodeJob`] — the per-operation encode job. It captures metadata,
//! policy and limits, then lowers all of it (resolved colour carrier, HDR
//! CLL/MDCV, orientation boxes, threading policy) onto the native
//! [`crate::EncoderConfig`] when building an encoder or animation encoder.

use std::sync::Arc;

use whereat::At;
use zencodec::{CodecError, Metadata, ResourceLimits};
use zenpixels::ColorAuthority;

use super::anim_encoder::AvifAnimationFrameEncoder;
use super::encode_config::{AVIF_ENCODE_CAPABILITIES, AvifEncoderConfig};
use super::encoder::AvifEncoder;
use super::orientation::orientation_to_avif;
use super::threads::policy_to_threads;

/// Per-operation AVIF encode job.
#[cfg(feature = "encode")]
pub struct AvifEncodeJob {
    pub(super) config: AvifEncoderConfig,
    pub(super) stop: Option<zencodec::StopToken>,
    pub(super) exif: Option<Arc<[u8]>>,
    pub(super) icc_profile: Option<Arc<[u8]>>,
    pub(super) xmp: Option<Arc<[u8]>>,
    pub(super) limits: ResourceLimits,
    pub(super) cicp: Option<zencodec::Cicp>,
    pub(super) content_light_level: Option<zencodec::ContentLightLevel>,
    pub(super) mastering_display: Option<zencodec::MasteringDisplay>,
    pub(super) rotation: Option<u8>,
    pub(super) mirror: Option<u8>,
    pub(super) policy: Option<zencodec::encode::EncodePolicy>,
    pub(super) canvas_size: Option<(u32, u32)>,
    pub(super) loop_count: Option<Option<u32>>,
}

#[cfg(feature = "encode")]
impl AvifEncodeJob {
    /// Set EXIF metadata to embed in the encoded AVIF.
    #[must_use]
    pub fn with_exif(mut self, exif: impl Into<Arc<[u8]>>) -> Self {
        self.exif = Some(exif.into());
        self
    }
}

#[cfg(feature = "encode")]
impl zencodec::encode::EncodeJob for AvifEncodeJob {
    type Error = At<CodecError>;
    type Enc = AvifEncoder;
    type AnimationFrameEnc = AvifAnimationFrameEncoder;

    fn with_stop(mut self, stop: zencodec::StopToken) -> Self {
        self.stop = Some(stop);
        self
    }

    #[allow(deprecated)] // required trait method; callers use with_metadata_policy
    fn with_metadata(mut self, meta: Metadata) -> Self {
        if let Some(exif) = meta.exif {
            self.exif = Some(exif);
        }
        if let Some(icc) = meta.icc_profile {
            self.icc_profile = Some(icc);
        }
        if let Some(xmp) = meta.xmp {
            self.xmp = Some(xmp);
        }
        if let Some(cicp) = meta.cicp {
            self.cicp = Some(cicp);
        }
        if let Some(cll) = meta.content_light_level {
            self.content_light_level = Some(cll);
        }
        if let Some(mdcv) = meta.mastering_display {
            self.mastering_display = Some(mdcv);
        }
        // Map EXIF-style orientation to AVIF rotation/mirror boxes
        let (rotation, mirror) = orientation_to_avif(meta.orientation);
        self.rotation = rotation;
        self.mirror = mirror;
        self
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn with_policy(mut self, policy: zencodec::encode::EncodePolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    fn encoder(self) -> Result<AvifEncoder, At<CodecError>> {
        let mut config = self.config.inner.clone();
        // Resolve the color *description* (ICC vs CICP code points) through
        // zencodec's `resolve_color_emit` — the single source of truth for which
        // color carrier we emit. It reconciles a caller-supplied CICP / ICC
        // against AVIF's capabilities under the job's ColorEmitPolicy. The
        // returned CICP (if any) is lowered to AVIF's nclx carrier (all three
        // axes, so the matrix stays consistent); the ICC disposition picks the
        // bytes to embed. This subsumes the old "apply self.cicp verbatim".
        let (plan_cicp, plan_icc) =
            resolve_avif_color(self.cicp, self.icc_profile, self.policy.as_ref());
        if let Some(cicp) = plan_cicp {
            config = apply_cicp_to_config(config, cicp);
        }
        // Apply HDR metadata from Metadata
        if let Some(cll) = self.content_light_level {
            config = config.content_light_level(
                cll.max_content_light_level,
                cll.max_frame_average_light_level,
            );
        }
        if let Some(mdcv) = self.mastering_display {
            // ST 2086 (the mdcv box): f32 CIE xy (0.0–1.0) → 0.00002 units
            // (×50000), u16, stored verbatim by ravif/the box; the decoder
            // reads ×0.00002. (Was ×65535 — off by 1.31× vs spec, and broke
            // round-trip against our own decoder.)
            let xy_to_u16 = |v: f32| (v * 50000.0 + 0.5) as u16;
            config = config.mastering_display(crate::MasteringDisplayConfig {
                primaries: [
                    (
                        xy_to_u16(mdcv.primaries_xy[0][0]),
                        xy_to_u16(mdcv.primaries_xy[0][1]),
                    ),
                    (
                        xy_to_u16(mdcv.primaries_xy[1][0]),
                        xy_to_u16(mdcv.primaries_xy[1][1]),
                    ),
                    (
                        xy_to_u16(mdcv.primaries_xy[2][0]),
                        xy_to_u16(mdcv.primaries_xy[2][1]),
                    ),
                ],
                white_point: (
                    xy_to_u16(mdcv.white_point_xy[0]),
                    xy_to_u16(mdcv.white_point_xy[1]),
                ),
                // ST 2086: cd/m² → 0.0001 units (×10000). (Was ×256.)
                max_luminance: (mdcv.max_luminance * 10000.0 + 0.5) as u32,
                // ST 2086: cd/m² → 0.0001 units (×10000). (Was ×16384.)
                min_luminance: (mdcv.min_luminance * 10000.0 + 0.5) as u32,
            });
        }
        // Apply rotation/mirror from orientation metadata
        if let Some(rot) = self.rotation {
            config = config.rotation(rot);
        }
        if let Some(mir) = self.mirror {
            config = config.mirror(mir);
        }
        // Apply threading policy from ResourceLimits.
        // Skip Parallel — it means "use the ambient pool", so keep the codec's
        // own default rather than pinning a thread count. Dimensions are not
        // known yet here, so this only lowers the POLICY; the memory-budget
        // thread fit runs at encode time (`checked_config`), where it may pin
        // a lower count — including under Parallel — when the calibrated
        // estimate would exceed `max_memory_bytes` (or available RAM).
        if !matches!(self.limits.threading(), zencodec::ThreadingPolicy::Parallel) {
            let threads = policy_to_threads(self.limits.threading());
            if threads > 0 {
                config = config.threads(Some(threads as usize));
            }
            // threads == 0 only from future unknown variants; leave default
        }
        // Apply encode policy: suppress metadata the policy disallows.
        let exif = match self.policy {
            Some(ref p) if !p.resolve_exif(true) => None,
            _ => self.exif,
        };
        // `plan_icc` already encodes the keep/synthesize/drop decision from
        // resolve_color_emit. The coarse `embed_icc: Some(false)` gate is an
        // explicit caller override that can still suppress an otherwise-kept ICC.
        let icc_profile = match self.policy {
            Some(ref p) if !p.resolve_icc(true) => None,
            _ => plan_icc,
        };
        let xmp = match self.policy {
            Some(ref p) if !p.resolve_xmp(true) => None,
            _ => self.xmp,
        };
        Ok(AvifEncoder {
            config,
            stop: self.stop,
            exif,
            icc_profile,
            xmp,
            limits: self.limits,
            caller_cicp: plan_cicp,
            threads_note: None,
        })
    }

    fn with_canvas_size(mut self, width: u32, height: u32) -> Self {
        self.canvas_size = Some((width, height));
        self
    }

    fn with_loop_count(mut self, count: Option<u32>) -> Self {
        self.loop_count = Some(count);
        self
    }

    fn animation_frame_encoder(self) -> Result<AvifAnimationFrameEncoder, At<CodecError>> {
        let mut config = self.config.inner.clone();
        // Resolve color description the same way as the still path (single source
        // of truth): lower the resolved CICP to nclx and carry the resolved ICC.
        let (plan_cicp, plan_icc) =
            resolve_avif_color(self.cicp, self.icc_profile, self.policy.as_ref());
        if let Some(cicp) = plan_cicp {
            config = apply_cicp_to_config(config, cicp);
        }
        // Apply HDR metadata
        if let Some(cll) = self.content_light_level {
            config = config.content_light_level(
                cll.max_content_light_level,
                cll.max_frame_average_light_level,
            );
        }
        if let Some(mdcv) = self.mastering_display {
            let xy_to_u16 = |v: f32| (v * 65535.0 + 0.5) as u16;
            config = config.mastering_display(crate::MasteringDisplayConfig {
                primaries: [
                    (
                        xy_to_u16(mdcv.primaries_xy[0][0]),
                        xy_to_u16(mdcv.primaries_xy[0][1]),
                    ),
                    (
                        xy_to_u16(mdcv.primaries_xy[1][0]),
                        xy_to_u16(mdcv.primaries_xy[1][1]),
                    ),
                    (
                        xy_to_u16(mdcv.primaries_xy[2][0]),
                        xy_to_u16(mdcv.primaries_xy[2][1]),
                    ),
                ],
                white_point: (
                    xy_to_u16(mdcv.white_point_xy[0]),
                    xy_to_u16(mdcv.white_point_xy[1]),
                ),
                max_luminance: (mdcv.max_luminance * 256.0 + 0.5) as u32,
                min_luminance: (mdcv.min_luminance * 16384.0 + 0.5) as u32,
            });
        }
        if let Some(rot) = self.rotation {
            config = config.rotation(rot);
        }
        if let Some(mir) = self.mirror {
            config = config.mirror(mir);
        }
        // Apply threading policy (canvas dimensions are not known yet here;
        // the memory-budget thread fit runs in `finish_inner`, where it may
        // pin a lower count — including under Parallel).
        if !matches!(self.limits.threading(), zencodec::ThreadingPolicy::Parallel) {
            let threads = policy_to_threads(self.limits.threading());
            if threads > 0 {
                config = config.threads(Some(threads as usize));
            }
        }
        // Apply metadata
        let policy = self.policy.as_ref();
        if let Some(exif) = self.exif
            && policy.is_none_or(|p| p.resolve_exif(true))
        {
            config = config.exif(exif.to_vec());
        }
        if let Some(icc) = plan_icc
            && policy.is_none_or(|p| p.resolve_icc(true))
        {
            config = config.icc_profile(icc.to_vec());
        }
        if let Some(xmp) = self.xmp
            && policy.is_none_or(|p| p.resolve_xmp(true))
        {
            config = config.xmp(xmp.to_vec());
        }

        let (canvas_w, canvas_h) = match self.canvas_size {
            Some((w, h)) => (Some(w), Some(h)),
            None => (None, None),
        };

        Ok(AvifAnimationFrameEncoder {
            config,
            stop: self.stop,
            frames: Vec::new(),
            pixel_format: None,
            canvas_width: canvas_w,
            canvas_height: canvas_h,
            limits: self.limits,
            frame_count: 0,
        })
    }
}

/// Lower a [`zencodec::Cicp`] onto the native AVIF encoder config, writing all
/// three nclx axes (primaries, transfer, matrix) so the config carries a
/// coherent triple rather than a partial/stale one (the prior bug set only some
/// axes). Note: the *emitted* nclx matrix is determined by ravif's own YCbCr
/// conversion (BT.601), so `config.matrix_coefficients` is informational —
/// no available backend consults it (its only reader was the deprecated
/// svtav1 path); the coherent triple is kept for introspection and any
/// future backend.
#[cfg(feature = "encode")]
fn apply_cicp_to_config(
    config: crate::EncoderConfig,
    cicp: zencodec::Cicp,
) -> crate::EncoderConfig {
    config
        .color_primaries(cicp.color_primaries)
        .transfer_characteristics(cicp.transfer_characteristics)
        .matrix_coefficients(cicp.matrix_coefficients)
}

/// Resolve which color description to emit for an AVIF encode, the single source
/// of truth for the color carrier.
///
/// Feeds a [`zencodec::SourceColor`] (built from the caller's CICP and/or ICC)
/// and AVIF's `AVIF_ENCODE_CAPABILITIES`
/// through [`zencodec::resolve_color_emit`] under the job's
/// [`ColorEmitPolicy`](zencodec::ColorEmitPolicy) (defaulting to
/// [`Balanced`](zencodec::ColorEmitPolicy::Balanced)). Returns:
///
/// - the CICP to write to nclx (`None` ⇒ leave the descriptor / encoder default),
/// - the ICC bytes to embed, materialized from the plan's
///   [`IccDisposition`](zencodec::IccDisposition):
///   [`KeepSource`](zencodec::IccDisposition::KeepSource) keeps the caller's
///   bytes, [`SynthesizeFrom`](zencodec::IccDisposition::SynthesizeFrom) fetches
///   a bundled profile for the primaries (sRGB ⇒ `None`, so nothing is embedded),
///   and [`Drop`](zencodec::IccDisposition::Drop) emits no ICC.
///
/// Channel count is left unset here: pixels aren't known yet at job-build time,
/// and the resolver's grayscale path still fires off an ICC that declares gray.
#[cfg(feature = "encode")]
fn resolve_avif_color(
    cicp: Option<zencodec::Cicp>,
    icc: Option<Arc<[u8]>>,
    policy: Option<&zencodec::encode::EncodePolicy>,
) -> (Option<zencodec::Cicp>, Option<Arc<[u8]>>) {
    let mut src = zencodec::SourceColor::default();
    if let Some(c) = cicp {
        src = src.with_cicp(c).with_color_authority(ColorAuthority::Cicp);
    }
    if let Some(ref bytes) = icc {
        src = src
            .with_icc_profile(bytes.clone())
            .with_color_authority(ColorAuthority::Icc);
    }

    let emit_policy = policy
        .map(|p| p.resolve_color(zencodec::ColorEmitPolicy::Balanced))
        .unwrap_or(zencodec::ColorEmitPolicy::Balanced);

    let plan = zencodec::resolve_color_emit(&src, &AVIF_ENCODE_CAPABILITIES, emit_policy);

    let icc_out = match plan.icc {
        zencodec::IccDisposition::KeepSource => icc,
        zencodec::IccDisposition::SynthesizeFrom(c) => {
            // Transfer-aware lowering: `synthesize_icc_for_cicp` matches the TRC, so a
            // BT.2020-PQ source never gets the SDR-TRC Rec.2020 profile. `Profile`
            // → own a copy; `NotNeeded`/`NeedsCms`/`CmsUnsupported` → no ICC (nclx
            // still carries the color, and AVIF nclx is a sole-safe carrier).
            use zenpixels_convert::icc_profiles::SynthesizedIcc;
            match zenpixels_convert::icc_profiles::synthesize_icc_for_cicp(c) {
                SynthesizedIcc::Profile(bytes) => Some(Arc::<[u8]>::from(bytes.as_ref())),
                _ => None,
            }
        }
        zencodec::IccDisposition::Drop => None,
        // IccDisposition is #[non_exhaustive]; a future variant defaults to not
        // embedding an ICC (safe — nclx still carries the color).
        _ => None,
    };

    (plan.cicp, icc_out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use imgref::Img;
    use rgb::Rgb;
    use zencodec::ImageFormat;
    use zenpixels::PixelSlice;

    #[cfg(feature = "encode")]
    #[test]
    fn encoding_with_metadata() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        let enc = AvifEncoderConfig::new().with_quality(80.0);
        let pixels = vec![
            Rgb {
                r: 255u8,
                g: 0,
                b: 0
            };
            16
        ];
        let img = Img::new(pixels, 4, 4);

        let exif = b"fake exif data";
        let output = enc
            .job()
            .with_exif(&exif[..])
            .encoder()
            .unwrap()
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.data().is_empty());
    }

    #[cfg(feature = "encode")]
    #[test]
    fn four_layer_encode_flow() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200
            };
            8 * 8
        ];
        let img = imgref::ImgVec::new(pixels, 8, 8);

        let config = AvifEncoderConfig::new().with_quality(80.0);
        let output = config
            .job()
            .encoder()
            .unwrap()
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Avif);
    }
}
