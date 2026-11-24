//! Decode-side colour description: building the authoritative
//! [`zencodec::decode::SourceColor`] from native info, the ICC device-class
//! gate, attaching a class-gated [`zenpixels::ColorContext`] to decoded
//! buffers, ICC to CICP derivation, and stamping CICP onto the descriptor.

use crate::pixel::PixelBufExt as _;
use std::sync::Arc;

use zencodec::ColorAuthority;
use zenpixels::PixelBuf;

/// Set transfer function and color primaries from native CICP on the pixel buffer.
/// Whether an ICC profile's device class (header bytes 16..20) is valid
/// for the buffer's channel layout: GRAY-class on Gray/GrayAlpha,
/// RGB-class on Rgb/Rgba/Bgra. Pairing them crosswise is invalid
/// signaling (libpng, among others, rejects it).
pub(super) fn icc_class_matches_layout(icc: &[u8], layout: zenpixels::ChannelOrder) -> bool {
    if icc.len() < 132 {
        return false;
    }
    let class = &icc[16..20];
    let buffer_gray = matches!(
        layout,
        zenpixels::ChannelOrder::Gray | zenpixels::ChannelOrder::GrayAlpha
    );
    if buffer_gray {
        class == b"GRAY"
    } else {
        class == b"RGB "
    }
}

/// Preserve source signaling independently of the reconstructed RGB/gray color.
pub(super) fn attach_color_context_class_gated(
    pixels: PixelBuf,
    source_color: &zencodec::decode::SourceColor,
) -> PixelBuf {
    let origin = source_color.color_origin();
    let spec = spec_for_source(pixels.spec().clone(), source_color);
    let hdr = hdr_for_source(source_color);
    pixels.retag(spec).with_origin(origin).with_hdr(hdr)
}

pub(super) fn spec_for_source(
    spec: zenpixels::ImageSpec,
    source: &zencodec::decode::SourceColor,
) -> zenpixels::ImageSpec {
    use zenpixels::{ColorSpace, ImageSpec, RenderingIntent};
    let active = source.authoritative_origin();
    if let Some(icc) = active.icc {
        if icc_class_matches_layout(&icc, spec.layout.order) {
            let channels = if spec.layout.order.is_gray_family() {
                1
            } else {
                3
            };
            return ImageSpec::new(
                spec.layout,
                ColorSpace::Icc {
                    profile: icc,
                    channels,
                    intent: RenderingIntent::Perceptual,
                    alpha: spec.color.alpha(),
                },
            );
        }
        if let Some(cicp) = derived_cicp_from_icc(&icc).or(source.cicp) {
            return with_rendered_cicp(spec, cicp);
        }
    }
    if let Some(cicp) = active.cicp {
        return with_rendered_cicp(spec, cicp);
    }
    spec
}

pub(super) fn hdr_for_source(source: &zencodec::decode::SourceColor) -> zenpixels::HdrMetadata {
    zenpixels::HdrMetadata {
        cll: source.content_light_level,
        mastering: source.mastering_display,
    }
}

fn with_rendered_cicp(spec: zenpixels::ImageSpec, cicp: zenpixels::Cicp) -> zenpixels::ImageSpec {
    let spec = spec.with_primaries(
        zenpixels::Primaries::from_cicp(cicp.color_primaries)
            .unwrap_or(zenpixels::Primaries::Unknown),
    );
    spec.with_transfer(
        zenpixels::Transfer::from_cicp(cicp.transfer_characteristics)
            .unwrap_or(zenpixels::Transfer::Unknown),
    )
}

/// Build the [`zencodec::decode::SourceColor`] for the native info the
/// way [`convert_native_info`] does (raw CICP + full-range; ICC
/// authority when ICC bytes are present, per MIAF).
pub(super) fn native_source_color(
    native: &crate::image::ImageInfo,
) -> zencodec::decode::SourceColor {
    let mut sc = zencodec::decode::SourceColor::default();
    sc.cicp = Some(zencodec::Cicp::new(
        native.color_primaries.0,
        native.transfer_characteristics.0,
        native.matrix_coefficients.0,
        native.color_range == crate::image::ColorRange::Full,
    ));
    if let Some(ref icc) = native.icc_profile {
        sc.icc_profile = Some(Arc::<[u8]>::from(icc.as_slice()));
        // authority stays Icc (SourceColor's default) — ICC > nclx per MIAF
    } else {
        sc.color_authority = ColorAuthority::Cicp;
    }
    let hdr = super::info::native_hdr(native);
    sc.content_light_level = hdr.cll;
    sc.mastering_display = hdr.mastering;
    sc
}

/// Derive an ICC profile's CICP description: an explicit embedded
/// `cICP` tag first, then normalized-hash identification of well-known
/// profiles. This is the same chain zenpixels-convert's load-bearing
/// reduction uses to decide whether a gray collapse keeps accurate
/// color signaling.
fn derived_cicp_from_icc(icc: &[u8]) -> Option<zencodec::Cicp> {
    zenpixels::Cicp::from_icc(icc).or_else(|| {
        zenpixels::icc::IccIdentification::from_icc(icc).and_then(|profile| profile.to_cicp())
    })
}

/// Whether native-gray output keeps accurate color for this file.
///
/// Gray files carrying RGB-class ICC profiles are common in the wild.
/// When the profile's CICP is derivable, native gray is fine: the gray
/// pixels get a CICP-only context (white point + transfer remain fully
/// meaningful for single-channel data), so there is no need to expand
/// to RGB just to honor the profile. Only an underivable RGB-class (or
/// unclassifiable) profile declines native gray — the profile is then
/// the sole accurate description and must stay on a layout it
/// describes; a gray preference resolves through the load-bearing
/// reduction's ICC rules instead.
pub(super) fn icc_allows_native_gray(native: &crate::image::ImageInfo) -> bool {
    match &native.icc_profile {
        None => true,
        Some(icc) => {
            (icc.len() >= 132 && &icc[16..20] == b"GRAY") || derived_cicp_from_icc(icc).is_some()
        }
    }
}

/// [`attach_color_context_class_gated`] from zenavif's native info: build
/// the [`zencodec::decode::SourceColor`] exactly the way
/// [`convert_native_info`] does (raw CICP code points + full-range flag;
/// ICC authority when ICC bytes are present, per MIAF).
pub(super) fn attach_source_color_context(
    pixels: PixelBuf,
    native: &crate::image::ImageInfo,
) -> PixelBuf {
    attach_color_context_class_gated(pixels, &native_source_color(native))
}

pub(super) fn set_cicp_on_pixels(pixels: PixelBuf, info: &crate::image::ImageInfo) -> PixelBuf {
    let desc = crate::convert::descriptor_with_cicp(pixels.spec(), info);
    pixels.retag(desc)
}
