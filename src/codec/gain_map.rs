//! Gain maps: ISO 21496-1 metadata to [`zencodec::GainMapInfo`] /
//! [`zencodec::GainMapPresence`] conversion, the ultrahdr-core
//! [`ReconstructHdr`](zencodec::GainMapRender::ReconstructHdr) apply shared by
//! the buffered and streaming paths, and the MaxCLL/MaxFALL measurement of the
//! reconstructed linear pixels.

use std::sync::Arc;

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
    pixels: zenpixels::PixelBuffer,
    native_info: &crate::image::ImageInfo,
    target_headroom: Option<f32>,
    decode_config: &crate::DecoderConfig,
    stop: &dyn Stop,
) -> Result<(zenpixels::PixelBuffer, (u16, u16)), At<Error>> {
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
    match pixels.descriptor().pixel_format() {
        zenpixels::PixelFormat::Rgb8 | zenpixels::PixelFormat::Rgba8 => {}
        _ => {
            return Err(at!(Error::Unsupported(
                "ReconstructHdr requires an 8-bit base (10/12-bit not yet \
                      supported); use GainMapRender::Components",
            )));
        }
    }
    let metadata = convert_gain_map_info(gm).ok_or_else(|| {
        at!(Error::Malformed(
            "gain map present but its ISO 21496-1 metadata failed to parse"
        ))
    })?;
    let (gpx, gw, gh, gch) =
        crate::decode_av1::decode_av1_obu_with_config(&gm.gain_map_data, decode_config)?;
    let gainmap = ultrahdr_core::GainMap {
        width: gw,
        height: gh,
        channels: gch,
        data: gpx,
    };
    let params = &metadata.params;
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
    // The apply kernels emit constant alpha = 1.0 (structural, not
    // scanned) — tag it Opaque so downstream encoders know the lane is
    // not load-bearing without rescanning.
    let desc = hdr
        .descriptor()
        .with_alpha(Some(zenpixels::AlphaMode::Opaque));
    let hdr = hdr.with_descriptor(desc);
    // The linear output IS describable: source primaries (raw code
    // point), H.273 transfer 8 (linear), identity matrix (RGB data),
    // full range. No SDR ICC or transfer may carry over, but a linear
    // CICP is strictly more self-describing than nothing — the enum
    // descriptor folds primaries the raw code point keeps.
    let linear_cicp = zencodec::Cicp::new(native_info.color_primaries.0, 8, 0, true);
    let hdr = hdr.with_color_context(Arc::new(zenpixels::ColorContext::from_cicp(linear_cicp)));
    let cll = measure_cll_linear(&hdr);
    Ok((hdr, cll))
}

/// Measure (MaxCLL, MaxFALL) in nits from linear f32 RGBA pixels where
/// 1.0 = SDR white (203 nits): MaxCLL = peak of per-pixel max(R,G,B),
/// MaxFALL = frame average of the same, both scaled by 203.
fn measure_cll_linear(pixels: &zenpixels::PixelBuffer) -> (u16, u16) {
    const SDR_WHITE_NITS: f32 = 203.0;
    let slice = pixels.as_slice();
    let bytes = slice.as_strided_bytes();
    let stride = slice.stride();
    let (w, h) = (slice.width() as usize, slice.rows() as usize);
    let mut peak = 0.0f32;
    let mut sum = 0.0f64;
    for y in 0..h {
        let row = &bytes[y * stride..][..w * 16];
        let row_f32: &[f32] = rgb::bytemuck::cast_slice(row);
        for px in row_f32.chunks_exact(4) {
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
