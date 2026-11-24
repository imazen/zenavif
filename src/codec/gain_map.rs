//! Gain maps: ISO 21496-1 metadata to [`zencodec::GainMapInfo`] /
//! [`zencodec::GainMapPresence`] conversion, the ultrahdr-core
//! [`ReconstructHdr`](zencodec::GainMapRender::ReconstructHdr) apply shared by
//! the buffered and streaming paths, and the MaxCLL/MaxFALL measurement of the
//! reconstructed linear pixels.

use crate::pixel::PixelBufExt as _;
use enough::Stop;
use whereat::{At, at};
use zencodec::GainMapPresence;

use crate::error::Error;

/// Convert native AVIF gain map metadata to zencodec's `GainMapPresence`.
///
/// Parses the AV1 sequence header from the gain map data to extract dimensions,
/// then converts the ISO 21496-1 metadata to zencodec's canonical representation.
pub(super) fn convert_gain_map_presence(native: &crate::image::ImageInfo) -> GainMapPresence {
    let gm = match native.gain_map.as_ref() {
        Some(gm) => gm,
        None => return GainMapPresence::Absent,
    };

    match convert_gain_map_info(gm) {
        Some(info) => GainMapPresence::Available(Box::new(info)),
        // If we can't parse the OBU, we know a gain map exists but can't
        // extract its dimensions — report as Unknown rather than lying.
        None => GainMapPresence::Unknown,
    }
}

/// Convert an [`AvifGainMap`](crate::image::AvifGainMap) to zencodec's
/// [`GainMapInfo`](zencodec::GainMapInfo).
///
/// Parses the AV1 sequence header to extract dimensions, converts the
/// ISO 21496-1 metadata fields, and optionally converts alt color info
/// to a [`Cicp`](zencodec::Cicp). Returns `None` if the AV1 bitstream
/// cannot be parsed.
/// Apply the gain map to a decoded SDR base, producing linear f32 RGBA
/// HDR pixels (1.0 = SDR white / 203 nits, base image's primaries) plus
/// the measured (MaxCLL, MaxFALL). Shared by the buffered and streaming
/// decode paths so both honor [`zencodec::GainMapRender::ReconstructHdr`]
/// identically. Call only when `native_info.gain_map` is `Some`.
pub(super) fn reconstruct_hdr_pixels(
    pixels: zenpixels::PixelBuf,
    native_info: &crate::image::ImageInfo,
    target_headroom: Option<f32>,
    decode_config: &crate::DecoderConfig,
    stop: &dyn Stop,
) -> Result<(zenpixels::PixelBuf, (u16, u16)), At<Error>> {
    let gm = native_info
        .gain_map
        .as_ref()
        .expect("reconstruct_hdr_pixels: gain map presence checked by caller");
    // Honest-capability gates: the apply kernels read 8-bit RGB(A) bases
    // and emit constant alpha = 1.0, so a real alpha channel or a >8-bit
    // base cannot be reconstructed without corruption. The zencodec
    // contract demands a loud refusal over silent degradation (use
    // Components + apply downstream for those).
    if native_info.has_alpha {
        return Err(at!(Error::Unsupported(
            "ReconstructHdr with an alpha channel is unsupported \
                  (apply emits opaque); use GainMapRender::Components",
        )));
    }
    if !matches!(
        pixels.spec().layout,
        zenpixels::SampleLayout::RGB8 | zenpixels::SampleLayout::RGBA8
    ) {
        return Err(at!(Error::Unsupported(
            "ReconstructHdr requires an 8-bit base (10/12-bit not yet supported); use GainMapRender::Components",
        )));
    }
    let metadata = convert_gain_map_info(gm).ok_or_else(|| {
        at!(Error::Malformed(
            "gain map present but its ISO 21496-1 metadata failed to parse"
        ))
    })?;
    let params = &metadata.params;
    let (gpx, gw, gh, gch) =
        crate::decode_av1::decode_av1_obu_with_config(&gm.gain_map_data, decode_config)?;
    let gainmap = ultrahdr_core::GainMap {
        width: gw,
        height: gh,
        channels: gch,
        data: gpx,
    };
    // None = full reconstruction at the gain map's encoded maximum
    // headroom; Some(h) renders for a display with h× SDR-white
    // capability (clamped inside ultrahdr-core's weight calculation).
    let boost = target_headroom.unwrap_or_else(|| {
        (params.alternate_hdr_headroom.max(params.base_hdr_headroom) as f32).exp2()
    });
    let hdr = ultrahdr_core::gainmap::apply_gainmap(
        &pixels,
        &gainmap,
        params,
        boost,
        ultrahdr_core::HdrOutputFormat::LinearFloat,
        stop,
    )
    .map_err(|_e| {
        at!(Error::Malformed(
            "gain-map apply failed (see ultrahdr-core validation rules)"
        ))
    })?;
    let spec = hdr.spec().clone().with_alpha(zenpixels::AlphaMode::Opaque);
    let cll = measure_cll_linear(&hdr);
    let mut metadata = pixels.hdr();
    metadata.cll = Some(zenpixels::ContentLightLevel::new(cll.0, cll.1));
    let hdr = hdr
        .retag(spec)
        .with_origin(pixels.origin().clone())
        .with_hdr(metadata);
    Ok((hdr, cll))
}

#[cfg(test)]
fn exact_metadata(md: &zenavif_parse::GainMapMetadata) -> Option<zenpixels::GainMapMetadata> {
    use zenpixels::{
        GainMapChannels, GainMapColorSpace, GainMapComponent, GainMapMetadata, SignedFraction,
        UnsignedFraction,
    };
    let component = |c: &zenavif_parse::GainMapChannel| {
        Some(GainMapComponent {
            min_gain_stops: SignedFraction::try_new(c.gain_map_min_n, c.gain_map_min_d)?,
            max_gain_stops: SignedFraction::try_new(c.gain_map_max_n, c.gain_map_max_d)?,
            gamma: UnsignedFraction::try_new(c.gamma_n, c.gamma_d)?,
            base_offset: SignedFraction::try_new(c.base_offset_n, c.base_offset_d)?,
            alternate_offset: SignedFraction::try_new(c.alternate_offset_n, c.alternate_offset_d)?,
        })
    };
    let channels = if md.is_multichannel {
        GainMapChannels::PerChannel([
            component(&md.channels[0])?,
            component(&md.channels[1])?,
            component(&md.channels[2])?,
        ])
    } else {
        GainMapChannels::Shared(component(&md.channels[0])?)
    };
    GainMapMetadata::try_new(
        channels,
        UnsignedFraction::try_new(md.base_hdr_headroom_n, md.base_hdr_headroom_d)?,
        UnsignedFraction::try_new(md.alternate_hdr_headroom_n, md.alternate_hdr_headroom_d)?,
        if md.use_base_colour_space {
            GainMapColorSpace::Base
        } else {
            GainMapColorSpace::Alternate
        },
    )
}

/// Measure (MaxCLL, MaxFALL) in nits from linear f32 RGBA pixels where
/// 1.0 = SDR white (203 nits): MaxCLL = peak of per-pixel max(R,G,B),
/// MaxFALL = frame average of the same, both scaled by 203.
fn measure_cll_linear(pixels: &zenpixels::PixelBuf) -> (u16, u16) {
    const SDR_WHITE_NITS: f32 = 203.0;
    let slice = pixels.as_view();
    let bytes = slice.as_strided_bytes();
    let stride = slice.stride();
    let (w, h) = (slice.width() as usize, slice.rows() as usize);
    let mut peak = 0.0f32;
    let mut sum = 0.0f64;
    for y in 0..h {
        let row = &bytes[y * stride..][..w * 16];
        let row_f32: &[f32] = rgb::bytemuck::cast_slice(row);
        for px in row_f32.as_chunks::<4>().0.iter() {
            let m = px[0].max(px[1]).max(px[2]).max(0.0);
            peak = peak.max(m);
            sum += f64::from(m);
        }
    }
    let fall = if w * h > 0 {
        (sum / (w * h) as f64) as f32
    } else {
        0.0
    };
    let to_nits = |v: f32| ((v * SDR_WHITE_NITS).round().clamp(0.0, 65535.0)) as u16;
    (to_nits(peak), to_nits(fall))
}

pub(super) fn convert_gain_map_info(
    gm: &crate::image::AvifGainMap,
) -> Option<zencodec::GainMapInfo> {
    // Parse AV1 sequence header to get gain map image dimensions.
    let (width, height, gm_channels_from_av1) =
        match zenavif_parse::AV1Metadata::parse_av1_bitstream(&gm.gain_map_data) {
            Ok(meta) => (
                meta.max_frame_width.get(),
                meta.max_frame_height.get(),
                if meta.monochrome { 1u8 } else { 3u8 },
            ),
            Err(_) => return None,
        };

    let md = &gm.metadata;
    let channels = if md.is_multichannel {
        3u8
    } else {
        gm_channels_from_av1.min(1)
    };

    let params = zencodec::GainMapParams::from(md);

    let mut gm_info = zencodec::GainMapInfo::new(params, width, height, channels);

    // Convert alternate rendition color info to CICP / ICC.
    match &gm.alt_color_info {
        Some(zenavif_parse::ColorInformation::Nclx {
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            full_range,
        }) => {
            gm_info = gm_info.with_alternate_cicp(zencodec::Cicp::new(
                *color_primaries as u8,
                *transfer_characteristics as u8,
                *matrix_coefficients as u8,
                *full_range,
            ));
        }
        Some(zenavif_parse::ColorInformation::IccProfile(icc)) => {
            gm_info = gm_info.with_alternate_icc(icc.clone());
        }
        None => {}
    }

    Some(gm_info)
}

#[test]
fn tmap_rational_pairs_survive_exact_pixel_metadata() {
    let channel = zenavif_parse::GainMapChannel {
        gain_map_min_n: -2,
        gain_map_min_d: 8,
        gain_map_max_n: 6,
        gain_map_max_d: 3,
        gamma_n: 2,
        gamma_d: 2,
        base_offset_n: 2,
        base_offset_d: 128,
        alternate_offset_n: 3,
        alternate_offset_d: 192,
    };
    let source = zenavif_parse::GainMapMetadata {
        is_multichannel: true,
        use_base_colour_space: false,
        backward_direction: true,
        base_hdr_headroom_n: 8,
        base_hdr_headroom_d: 4,
        alternate_hdr_headroom_n: 0,
        alternate_hdr_headroom_d: 7,
        channels: [channel; 3],
    };
    let wire = source.to_bytes();
    let parsed = zenavif_parse::GainMapMetadata::parse_tmap_bytes(&wire).unwrap();
    assert_eq!(parsed, source);
    let exact = exact_metadata(&parsed).unwrap();
    assert_eq!(
        exact.direction(),
        Some(zenpixels::GainMapDirection::Reverse)
    );
    assert!(exact.channels.is_multichannel());
    assert_eq!(
        exact.application_space,
        zenpixels::GainMapColorSpace::Alternate
    );
    assert_eq!(exact.base_hdr_headroom.num, 8);
    assert_eq!(exact.base_hdr_headroom.den.get(), 4);
    assert_eq!(exact.alternate_hdr_headroom.den.get(), 7);
    let component = exact.channel(0);
    assert_eq!(
        (component.base_offset.num, component.base_offset.den.get()),
        (2, 128)
    );
    assert_eq!(
        (
            component.alternate_offset.num,
            component.alternate_offset.den.get()
        ),
        (3, 192)
    );
    let float = zencodec::GainMapParams::from(&parsed);
    let canonical = zenavif_parse::GainMapMetadata::from(&float);
    assert_eq!(float.channels[0].base_offset, 1.0 / 64.0);
    assert_ne!(canonical.to_bytes(), wire);
}
