//! Decode-side colour description: building the authoritative
//! [`zencodec::decode::SourceColor`] from native info, the ICC device-class
//! gate, attaching a class-gated [`zenpixels::ColorContext`] to decoded
//! buffers, ICC to CICP derivation, and stamping CICP onto the descriptor.

use std::sync::Arc;

use zenpixels::{ColorAuthority, PixelBuffer};

/// Set transfer function and color primaries from native CICP on the pixel buffer.
/// Whether an ICC profile's device class (header bytes 16..20) is valid
/// for the buffer's channel layout: GRAY-class on Gray/GrayAlpha,
/// RGB-class on Rgb/Rgba/Bgra. Pairing them crosswise is invalid
/// signaling (libpng, among others, rejects it).
pub(super) fn icc_class_matches_layout(icc: &[u8], layout: zenpixels::ChannelLayout) -> bool {
    if icc.len() < 132 {
        return false;
    }
    let class = &icc[16..20];
    let buffer_gray = matches!(
        layout,
        zenpixels::ChannelLayout::Gray | zenpixels::ChannelLayout::GrayAlpha
    );
    if buffer_gray {
        class == b"GRAY"
    } else {
        class == b"RGB "
    }
}

/// Attach the authoritative source color to a decoded buffer as a
/// [`zenpixels::ColorContext`], making the pixels self-describing for
/// downstream stages (CMS, load-bearing reduction, re-encode).
///
/// The selection runs through zencodec's drop-dupe rules
/// ([`zencodec::decode::SourceColor::to_color_context`]: the
/// non-authoritative field is dropped — ICC > nclx per MIAF). The ICC is
/// then class-gated against the buffer layout: an RGB-class profile
/// never rides a Gray buffer (the raw CICP stays as the fallback signal
/// — it carries the raw H.273 code points the descriptor enums fold
/// away). The conversion/orientation/reduction stages all propagate the
/// context; the load-bearing gray collapse swaps or suppresses per its
/// own ICC rules.
pub(super) fn attach_color_context_class_gated(
    pixels: PixelBuffer,
    source_color: &zencodec::decode::SourceColor,
) -> PixelBuffer {
    match color_context_for_layout(source_color, pixels.descriptor().layout()) {
        Some(ctx) => pixels.with_color_context(ctx),
        None => pixels,
    }
}

/// The class-gated context [`attach_color_context_class_gated`] attaches,
/// computed for a known layout — shared with the streaming decoder, whose
/// strip scratch buffers are rebuilt per batch and need the context
/// re-applied on every emitted slice.
pub(super) fn color_context_for_layout(
    source_color: &zencodec::decode::SourceColor,
    layout: zenpixels::ChannelLayout,
) -> Option<Arc<zenpixels::ColorContext>> {
    let mut ctx = source_color.to_color_context();
    if let Some(icc) = ctx
        .icc
        .take_if(|icc| !icc_class_matches_layout(icc, layout))
    {
        // Class mismatch: the profile cannot ride this layout. Its
        // DERIVED CICP is the authoritative description (the profile
        // outranked the signaled nclx per MIAF), so prefer it over both
        // the drop-dupe survivor and the signaled fallback.
        ctx.cicp = derived_cicp_from_icc(&icc)
            .or(ctx.cicp)
            .or(source_color.cicp);
    }
    if ctx.icc.is_none() && ctx.cicp.is_none() {
        return None;
    }
    Some(Arc::new(ctx))
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
    sc
}

/// Derive an ICC profile's CICP description: an explicit embedded
/// `cICP` tag first, then normalized-hash identification of well-known
/// profiles. This is the same chain zenpixels-convert's load-bearing
/// reduction uses to decide whether a gray collapse keeps accurate
/// color signaling.
fn derived_cicp_from_icc(icc: &[u8]) -> Option<zencodec::Cicp> {
    zenpixels::icc::extract_cicp(icc)
        .or_else(|| zenpixels::icc::identify_common(icc).and_then(|id| id.to_cicp()))
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
    pixels: PixelBuffer,
    native: &crate::image::ImageInfo,
) -> PixelBuffer {
    attach_color_context_class_gated(pixels, &native_source_color(native))
}

pub(super) fn set_cicp_on_pixels(
    pixels: PixelBuffer,
    info: &crate::image::ImageInfo,
) -> PixelBuffer {
    let mut desc = pixels.descriptor();
    if let Some(tf) = zenpixels::TransferFunction::from_cicp(info.transfer_characteristics.0) {
        desc = desc.with_transfer(tf);
    }
    if let Some(p) = zenpixels::ColorPrimaries::from_cicp(info.color_primaries.0) {
        desc = desc.with_primaries(p);
    }
    pixels.with_descriptor(desc)
}
