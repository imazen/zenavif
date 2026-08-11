//! Native [`crate::image::ImageInfo`] to [`zencodec::ImageInfo`] conversion,
//! plus the [`zencodec::decode::DecodePolicy`] metadata stripping applied to
//! the result.

use zencodec::{GainMapPresence, ImageFormat, ImageInfo, Supplements};
use zenpixels::ColorAuthority;

use super::gain_map::convert_gain_map_presence;
use super::orientation::avif_to_orientation;

/// Convert zenavif's native `ImageInfo` to `zencodec::ImageInfo`.
pub(super) fn convert_native_info(native: &crate::image::ImageInfo) -> ImageInfo {
    let orientation = avif_to_orientation(native.rotation.as_ref(), native.mirror.as_ref());

    let cicp = zencodec::Cicp::new(
        native.color_primaries.0,
        native.transfer_characteristics.0,
        native.matrix_coefficients.0,
        native.color_range == crate::image::ColorRange::Full,
    );

    let channels: u8 = if native.monochrome {
        if native.has_alpha { 2 } else { 1 }
    } else if native.has_alpha {
        4
    } else {
        3
    };

    let mut info = ImageInfo::new(native.width, native.height, ImageFormat::Avif)
        .with_alpha(native.has_alpha)
        .with_bit_depth(native.bit_depth)
        .with_channel_count(channels)
        .with_cicp(cicp)
        .with_orientation(orientation);

    if let Some(ref icc) = native.icc_profile {
        info = info.with_icc_profile(icc.clone());
        // authority stays Icc (default) — ICC > nclx per MIAF spec
    } else {
        // No ICC → CICP (from nclx or AV1 SPS) is authoritative
        info = info.with_color_authority(ColorAuthority::Cicp);
    }
    if let Some(ref exif) = native.exif {
        info = info.with_exif(exif.clone());
    }
    if let Some(ref xmp) = native.xmp {
        info = info.with_xmp(xmp.clone());
    }
    if let Some(ref cll) = native.content_light_level {
        info = info.with_content_light_level(zencodec::ContentLightLevel::new(
            cll.max_content_light_level,
            cll.max_pic_average_light_level,
        ));
    }
    if let Some(ref mdcv) = native.mastering_display {
        // Convert from 0.00002 units (u16) to CIE 1931 xy (f32), and 0.0001 cd/m² (u32) to f32
        let xy = |v: u16| v as f32 * 0.00002;
        info = info.with_mastering_display(zencodec::MasteringDisplay::new(
            [
                [xy(mdcv.primaries[0].0), xy(mdcv.primaries[0].1)],
                [xy(mdcv.primaries[1].0), xy(mdcv.primaries[1].1)],
                [xy(mdcv.primaries[2].0), xy(mdcv.primaries[2].1)],
            ],
            [xy(mdcv.white_point.0), xy(mdcv.white_point.1)],
            mdcv.max_luminance as f32 * 0.0001,
            mdcv.min_luminance as f32 * 0.0001,
        ));
    }

    // Supplemental content flags: gain map, depth map.
    let has_gain_map = native.gain_map.is_some();
    let has_depth_map = native.depth_map.is_some();
    if has_gain_map || has_depth_map {
        let mut supplements = Supplements::default();
        supplements.gain_map = has_gain_map;
        supplements.depth_map = has_depth_map;
        info = info.with_supplements(supplements);
    }

    // Gain map presence: Absent when definitively none, Available when metadata
    // can be converted, Unknown otherwise (default).
    if native.gain_map.is_some() {
        info = info.with_gain_map(convert_gain_map_presence(native));
    } else {
        info = info.with_gain_map(GainMapPresence::Absent);
    }

    info
}

/// Strip metadata from [`ImageInfo`] according to a [`DecodePolicy`](zencodec::decode::DecodePolicy).
///
/// When a policy flag resolves to `false` (default is `true` = allow), the
/// corresponding metadata field is cleared so callers never see it.
pub(super) fn apply_decode_policy(info: &mut ImageInfo, policy: &zencodec::decode::DecodePolicy) {
    if !policy.resolve_icc(true) {
        info.source_color.icc_profile = None;
    }
    if !policy.resolve_exif(true) {
        info.embedded_metadata.exif = None;
    }
    if !policy.resolve_xmp(true) {
        info.embedded_metadata.xmp = None;
    }
}
