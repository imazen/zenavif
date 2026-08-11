//! [`AvifDecoder`] — the buffered single-image decode pipeline (limits,
//! native-gray opt-in, CICP + colour-context attach, HDR reconstruction,
//! format negotiation, orientation bake, gain-map/depth extras) behind the
//! [`zencodec::decode::Decode`] boundary.

use std::borrow::Cow;

use enough::Stop;
use whereat::{At, at};
use zencodec::decode::DecodeOutput;
use zencodec::{CodecError, ResourceLimits};
use zenpixels::PixelDescriptor;

use super::color::{attach_source_color_context, icc_allows_native_gray, set_cicp_on_pixels};
use super::gain_map::{convert_gain_map_info, reconstruct_hdr_pixels};
use super::info::{apply_decode_policy, convert_native_info};
use super::negotiate::{negotiate_format, wants_gray_output};
use super::orientation::{apply_reported_orientation, bake_orientation};
use crate::error::Error;

/// Single-image AVIF decoder.
pub struct AvifDecoder<'a> {
    pub(super) config: crate::DecoderConfig,
    pub(super) stop: Option<zencodec::StopToken>,
    pub(super) data: Cow<'a, [u8]>,
    pub(super) preferred: Vec<PixelDescriptor>,
    pub(super) limits: ResourceLimits,
    pub(super) policy: Option<zencodec::decode::DecodePolicy>,
    pub(super) extract_gain_map: bool,
    pub(super) gain_map_render: zencodec::GainMapRender,
    /// How to handle the image's stored orientation (`irot`/`imir`).
    /// Default [`OrientationHint::Preserve`](zencodec::OrientationHint::Preserve).
    pub(super) orientation: zencodec::OrientationHint,
}

impl zencodec::decode::Decode for AvifDecoder<'_> {
    type Error = At<CodecError>;

    fn decode(self) -> Result<DecodeOutput, At<CodecError>> {
        self.decode_inner().map_err(zencodec::CodecError::of)
    }
}

impl AvifDecoder<'_> {
    fn decode_inner(self) -> Result<DecodeOutput, At<Error>> {
        let stop: &dyn Stop = match &self.stop {
            Some(s) => s,
            None => &enough::Unstoppable,
        };
        let mut decoder = crate::ManagedAvifDecoder::new(&self.data, &self.config)?;
        let native_info = decoder.probe_info()?;

        // Check dimensions and memory limits before the expensive pixel decode.
        self.limits
            .check_dimensions(native_info.width, native_info.height)
            .map_err(|_| {
                at!(Error::ImageTooLarge {
                    width: native_info.width,
                    height: native_info.height,
                })
            })?;
        let bpp: u64 = if native_info.bit_depth > 8 {
            if native_info.has_alpha { 8 } else { 6 }
        } else if native_info.has_alpha {
            4
        } else {
            3
        };
        let estimated_mem = native_info.width as u64 * native_info.height as u64 * bpp;
        self.limits
            .check_memory(estimated_mem)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;

        // Native grayscale opt-in (zenavif#5): alpha-free monochrome
        // decodes straight to Gray8/Gray16 (1-2 bytes/pixel) when
        // negotiation selects it. Grid composition stitches RGB tiles and
        // HDR reconstruction needs an RGB base, so both stay expanded
        // (a gray preference is then satisfied post-hoc in
        // `negotiate_format` — exact, since mono RGB is R=G=B).
        let mono_source = native_info.monochrome && !native_info.has_alpha;
        let reconstructing = matches!(
            self.gain_map_render,
            zencodec::GainMapRender::ReconstructHdr { .. }
        ) && native_info.gain_map.is_some();
        if mono_source
            && !reconstructing
            && !decoder.is_grid()
            && icc_allows_native_gray(&native_info)
            && wants_gray_output(&self.preferred)
        {
            decoder.set_native_gray(true);
        }

        let (pixels, native_info) = decoder.decode_full(stop)?;

        // Set transfer function and primaries from CICP on the pixel descriptor.
        let pixels = set_cicp_on_pixels(pixels, &native_info);
        // Self-describing pixels: attach the authoritative source color
        // (class-gated). Conversions, orientation, and the load-bearing
        // reduction all propagate it; the HDR reconstruction below
        // replaces the buffer and re-tags it with a linear CICP (no SDR
        // ICC/transfer may carry onto linear f32).
        let pixels = attach_source_color_context(pixels, &native_info);
        // HDR reconstruction (GainMapRender::ReconstructHdr): apply the
        // gain map to the SDR base via ultrahdr-core, BEFORE orientation
        // bake (base and gain map share stored orientation) and before
        // SDR format negotiation (the output is linear f32 RGBA, 1.0 =
        // SDR white / 203 nits). MaxCLL/MaxFALL are MEASURED from the
        // reconstructed pixels per the zencodec contract.
        let mut reconstructed_cll: Option<(u16, u16)> = None;
        let reconstruct_target = match self.gain_map_render {
            zencodec::GainMapRender::ReconstructHdr { target_headroom }
                if native_info.gain_map.is_some() =>
            {
                Some(target_headroom)
            }
            _ => None,
        };
        let pixels = if let Some(target_headroom) = reconstruct_target {
            let (hdr, cll) =
                reconstruct_hdr_pixels(pixels, &native_info, target_headroom, &self.config, stop)?;
            reconstructed_cll = Some(cll);
            hdr
        } else {
            // BaseOnly / Components — or ReconstructHdr on a file with
            // no gain map, where the base IS the only rendition and an
            // honest SDR output is the correct rendering.
            negotiate_format(pixels, &self.preferred, mono_source)
        };
        // Orientation policy: `Correct` bakes the intrinsic `irot`/`imir`
        // orientation into the pixels and reports display dims + `Identity`;
        // `Preserve` (default) keeps stored orientation and reports the
        // intrinsic tag + stored dims. `convert_native_info` already reports the
        // preserve view, so only the bake path rewrites it.
        let (pixels, _orientation, _w, _h) =
            bake_orientation(pixels, &native_info, self.orientation);
        let mut info = apply_reported_orientation(
            convert_native_info(&native_info),
            &native_info,
            self.orientation,
        );
        if let Some(ref policy) = self.policy {
            apply_decode_policy(&mut info, policy);
        }
        if let Some((max_cll, max_fall)) = reconstructed_cll {
            // Measured envelope of the reconstructed pixels — the
            // signaled CLL described the alternate rendition, this
            // describes what we actually produced (zencodec contract:
            // MaxCLL/MaxFALL are measured; mastering display passes
            // through unchanged).
            info =
                info.with_content_light_level(zencodec::ContentLightLevel::new(max_cll, max_fall));
        }
        let mut output = DecodeOutput::new(pixels, info);
        if let Ok(probe) = crate::detect::probe(&self.data) {
            output = output.with_source_encoding_details(probe);
        }
        // Gain-map rendition intent. Components decodes the gain-map AV1
        // payload into a DecodedGainMap; ReconstructHdr ADDITIONALLY
        // applies it to the base via ultrahdr-core (above) — the output
        // pixels are linear f32 RGBA with 1.0 = SDR white, and the
        // components are still surfaced for transcode use. Unknown
        // future modes are refused, never mis-rendered.
        let surface_components = match self.gain_map_render {
            zencodec::GainMapRender::BaseOnly => false,
            zencodec::GainMapRender::Components
            | zencodec::GainMapRender::ReconstructHdr { .. } => true,
            _ => {
                return Err(at!(Error::InvalidParameters(
                    "unrecognized GainMapRender mode".into()
                )));
            }
        };

        // Attach gain map / depth map as typed extras only when opted in.
        // Metadata (`ImageInfo.supplements`, `GainMapPresence`) is always
        // populated regardless — only the heavy data blobs are gated.
        if (self.extract_gain_map || surface_components)
            && let Some(gm) = native_info.gain_map
            && let Some(metadata) = convert_gain_map_info(&gm)
        {
            // Components: decode the AV1-coded gain-map image into pixels.
            // Errors only when a present gain map is malformed.
            if surface_components {
                let (px, gw, gh, channels) =
                    crate::decode_av1::decode_av1_obu_with_config(&gm.gain_map_data, &self.config)?;
                let desc = if channels == 1 {
                    PixelDescriptor::GRAY8_SRGB
                } else {
                    PixelDescriptor::RGB8_SRGB
                };
                let pixels = zenpixels::PixelBuffer::from_vec(px, gw, gh, desc).map_err(|_| {
                    at!(Error::Decode {
                        code: -1,
                        msg: "gain-map pixel buffer creation failed",
                    })
                })?;
                output = output.with_extras(zencodec::decode::DecodedGainMap::new(
                    pixels,
                    metadata.clone(),
                ));
            }
            let source = zencodec::gainmap::GainMapSource::new(
                gm.gain_map_data,
                zencodec::ImageFormat::Avif,
                metadata,
            );
            output = output.with_extras(source);
        }
        if self.extract_gain_map
            && let Some(dm) = native_info.depth_map
        {
            output = output.with_extras(dm);
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "encode")]
    use super::*;
    #[cfg(feature = "encode")]
    use crate::codec::decode_config::AvifDecoderConfig;
    #[cfg(feature = "encode")]
    use crate::codec::encode_config::AvifEncoderConfig;
    #[cfg(feature = "encode")]
    use imgref::Img;
    #[cfg(feature = "encode")]
    use rgb::Rgb;
    #[cfg(feature = "encode")]
    use zencodec::ImageFormat;
    #[cfg(feature = "encode")]
    use zenpixels::PixelSlice;

    #[cfg(feature = "encode")]
    #[test]
    fn decode_roundtrip() {
        let enc = AvifEncoderConfig::new()
            .with_quality(80.0)
            .with_effort_u32(10);
        let pixels = vec![
            Rgb {
                r: 200u8,
                g: 100,
                b: 50
            };
            64
        ];
        let img = Img::new(pixels, 8, 8);
        let encoded = enc.encode_rgb8(img.as_ref()).unwrap();

        let dec = AvifDecoderConfig::new();
        let output = dec.decode(encoded.data()).unwrap();
        assert_eq!(output.info().width, 8);
        assert_eq!(output.info().height, 8);
        assert_eq!(output.info().format, ImageFormat::Avif);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn single_thread_encode_decode_roundtrip() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        // Encode with SingleThread threading policy
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            16 * 16
        ];
        let img = imgref::ImgVec::new(pixels, 16, 16);
        let config = AvifEncoderConfig::new().with_quality(80.0);
        let limits = ResourceLimits::none().with_threading(zencodec::ThreadingPolicy::Sequential);
        let encoder = config.job().with_limits(limits).encoder().unwrap();
        let encoded = encoder
            .encode(PixelSlice::from(img.as_ref()).erase())
            .unwrap();
        assert!(!encoded.is_empty());

        // Decode with SingleThread threading policy
        let dec_config = AvifDecoderConfig::new();
        let dec_limits =
            ResourceLimits::none().with_threading(zencodec::ThreadingPolicy::Sequential);
        let decoded = dec_config
            .job()
            .with_limits(dec_limits)
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(decoded.info().width, 16);
        assert_eq!(decoded.info().height, 16);
    }
}
