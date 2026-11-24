//! Preferred-pixel-format negotiation for decoder output, including the
//! byte-verified, ICC-signalling-aware gray collapse.

use crate::pixel::ImageSpecExt as _;
use crate::pixel::PixelBufExt as _;
use zenpixels::{ImageSpec, PixelBuf, SampleType};

// The `negotiate_gray` tests below reach these colour helpers through `super`.
#[cfg(test)]
use super::color::{attach_color_context_class_gated, icc_class_matches_layout};

/// Check if two descriptors match on pixel format (channel type + alpha),
/// ignoring transfer function, primaries, and signal range metadata.
pub(super) fn format_matches(a: &ImageSpec, b: &ImageSpec) -> bool {
    a.layout == b.layout
}

/// Apply preferred format negotiation to decoder output.
///
/// If `preferred` is empty, returns `pixels` unchanged (native format).
/// If `preferred` is non-empty, finds the first descriptor we can satisfy:
/// - Same or lower bit depth: downconvert (caller explicitly asked for it)
/// - Higher bit depth than native: skip (can't upscale losslessly)
///
/// Transfer function and color primaries on the native descriptor are preserved
/// (set from CICP metadata). Negotiation only considers channel type and alpha.
/// Whether negotiation selects native grayscale output for an alpha-free
/// monochrome source: yes with no preference (gray IS the native format,
/// per the `native_gray` capability), or when the caller's first
/// preference is a Gray layout. A leading RGB preference keeps the
/// classic expanded decode.
pub(super) fn wants_gray_output(preferred: &[ImageSpec]) -> bool {
    match preferred.first() {
        None => true,
        Some(p) => p.layout.order == zenpixels::ChannelOrder::Gray,
    }
}

/// The descriptor negotiation converts *to*: the caller's preference,
/// verbatim.
///
/// **Why not the native colour description with only the layout/depth
/// swapped?** That reading is what the module header implies ("negotiation
/// only considers channel type and alpha"), it is semantically tidier, and it
/// would let an HDR PQ source satisfy an `[Rgb8]` ask as a plain depth narrow
/// instead of declining it. It was implemented and MEASURED, and rejected on
/// two counts (2026-08-13, `weld_sato_12B_8B_q0.avif`, 1024x684):
///
/// 1. **It loses a bit of precision.** With target and source transfer equal,
///    zenpixels-convert 0.2.16 takes an integer U16→U8 path that *truncates*
///    `v * 255 / 65535`; with the transfer differing it goes through f32 and
///    *rounds*. Measured 407 of 2,101,248 bytes differing by exactly 1, every
///    one of them a value landing on a `.5019` tie (native 39193 → exact
///    152.5019 → rounds to 153, truncates to 152). Rounding is the correct
///    answer, so preserving the native transfer would have introduced a
///    1-LSB regression on every 10/12-bit narrow.
/// 2. **8-bit PQ is not a rendition worth producing.** PQ needs 10 bits to
///    avoid visible banding; handing a caller a banded 8-bit PQ buffer and
///    calling it success is a worse answer than declining, which is what
///    [`zencodec::decode::DecodeJob::decoder`] specifies anyway — "the decoder
///    picks the first it can produce **without lossy conversion**".
///
/// Do not "simplify" this back to a native-preserving target without
/// re-measuring both; the tie-rounding difference is invisible in a spot check
/// (a flat row compares equal) and only shows up over a whole image.
///
/// **Layout-only changes keep the source's colour description.** Adding or
/// dropping alpha at the same depth converts no sample values, so neither
/// point above applies; labelling the result with the preference's transfer
/// instead would relabel the pixels. A source whose transfer is unknown (an
/// ICC profile that cannot be parsed) then reached zenpixels-convert as an
/// `Unknown -> sRGB` request, which it rightly refuses (`NeedsCms`), and the
/// add-alpha preference was declined.
fn target_descriptor(native: &ImageSpec, pref: &ImageSpec) -> ImageSpec {
    if pref.layout.sample != native.layout.sample {
        return pref.clone();
    }
    let color = if native.layout.order.is_gray_family() && pref.layout.order.is_rgb_family() {
        zenpixels::ColorSpace::Rgb {
            primaries: native.primaries(),
            transfer: native.transfer(),
            range: native.range(),
            alpha: pref.color.alpha(),
            white: native.color.white(),
        }
    } else {
        native.color.clone().with_alpha(pref.color.alpha())
    };
    ImageSpec::new(pref.layout, color)
}

/// Whether negotiation is willing to produce `pref` from `native`, judged on
/// layout and depth alone. Content-dependent decisions (the RGB→gray
/// collapse) are the caller's, and are handled before this is reached.
///
/// Kept as a predicate separate from the conversion so the strip-based decode
/// paths can make the same decision from a descriptor, before any pixels
/// exist (zenavif#36).
pub(super) fn reduction_is_offered(native: &ImageSpec, pref: &ImageSpec) -> bool {
    // Can't upscale bit depth losslessly.
    if pref.layout.sample.byte_size() > native.layout.sample.byte_size() {
        return false;
    }
    if pref.layout.order == zenpixels::ChannelOrder::Gray {
        // Collapsing *colour* to gray is content-verified (R==G==B at the
        // byte level) and therefore never decided from a descriptor. Gray to
        // gray is a plain depth narrow with no such question, and is offered
        // — that is the `mono10 + [Gray8]` cell the strip paths used to miss.
        //
        // In `negotiate_format` this branch is unreachable: its own gray arm
        // intercepts every Gray preference first (and runs the collapse).
        // Only the strip paths, which have no collapse, get here.
        return native.layout.order == zenpixels::ChannelOrder::Gray
            && pref.layout.sample == SampleType::U8
            && native.layout.sample == SampleType::U16;
    }
    // Gray native, color preference at the same depth: expand.
    if native.layout.order == zenpixels::ChannelOrder::Gray
        && pref.layout.sample == native.layout.sample
        && native.layout.sample == SampleType::U8
    {
        return true;
    }
    // Caller wants 8-bit and we have 16-bit: downconvert.
    if pref.layout.sample == SampleType::U8 && native.layout.sample == SampleType::U16 {
        return true;
    }
    // Same bit depth, different layout (e.g. RGB vs RGBA). Only at 8-bit —
    // the 16-bit layout changes have never been offered and adding them is a
    // separate, unmeasured widening of the contract.
    pref.layout.sample == native.layout.sample
        && native.layout.sample == SampleType::U8
        && pref.layout.order.has_alpha_slot() != native.layout.order.has_alpha_slot()
}

/// Perform an offered reduction, or decline it.
///
/// `None` means the conversion library has no plan for this pair — the only
/// observed case is HDR (PQ/HLG) to a materially different colour description
/// without a peak luminance, which is a tone-mapping decision the negotiation
/// layer is not entitled to make up. Declining lets the next preference try,
/// and ultimately returns the native buffer, which is what
/// [`zencodec::decode::DecodeJob::decoder`] documents `preferred` to mean:
/// "the decoder picks the first it can produce **without lossy conversion**".
///
/// zenavif#39: this used to be an infallible `to_rgb8()` / `to_rgba8()`, whose
/// `RowConverter::new(..).expect("RowConverter: no conversion path")` unwound
/// the caller's thread. The descriptor that selects the arm is read out of the
/// decoded file, so the panic was reachable from untrusted input.
fn try_reduce(pixels: &PixelBuf, target: ImageSpec) -> Option<PixelBuf> {
    // Adding opaque alpha is a storage operation even for an unknown ICC.
    if pixels.spec().layout == zenpixels::SampleLayout::RGB8
        && target.layout == zenpixels::SampleLayout::RGBA8
        && pixels.spec().color.clone().with_alpha(target.color.alpha()) == target.color
    {
        let mut out = PixelBuf::new(pixels.width(), pixels.height(), target).ok()?;
        let src = pixels.as_view();
        let mut dst = out.as_view_mut();
        for y in 0..pixels.height() {
            for (rgb, rgba) in src
                .row(y)
                .as_chunks::<3>()
                .0
                .iter()
                .zip(dst.row_mut(y).as_chunks_mut::<4>().0)
            {
                *rgba = [rgb[0], rgb[1], rgb[2], 255];
            }
        }
        return Some(
            out.with_origin(pixels.origin().clone())
                .with_hdr(pixels.hdr()),
        );
    }
    // Unspecified CICP does not establish a transfer conversion. Honor a plain
    // storage narrowing of untagged RGB and keep its original interpretation.
    let src = pixels.spec();
    if src.layout == zenpixels::SampleLayout::RGB16
        && target.layout == zenpixels::SampleLayout::RGB8
        && src.transfer() == zenpixels::Transfer::Unknown
        && src.primaries() == zenpixels::Primaries::Unknown
        && src.range() == zenpixels::SignalRange::Full
        && !matches!(src.color, zenpixels::ColorSpace::Icc { .. })
    {
        let spec = ImageSpec::new(target.layout, src.color.clone());
        let mut out = PixelBuf::new(pixels.width(), pixels.height(), spec).ok()?;
        let src = pixels.as_view();
        let mut dst = out.as_view_mut();
        for y in 0..pixels.height() {
            let src_row = src.row_as::<u16>(y).ok()?;
            for (&code, byte) in src_row.iter().zip(dst.row_mut(y)) {
                *byte = ((u32::from(code) + 128) / 257) as u8;
            }
        }
        return Some(
            out.with_origin(pixels.origin().clone())
                .with_hdr(pixels.hdr()),
        );
    }
    pixels.convert_to(target).ok()
}

/// Collapse byte-verified neutral RGB (`R == G == B`) carrying
/// `Primaries::Unknown` (untagged monochrome decoded to RGB, no ICC) into
/// `Gray8`/`Gray16`, preserving transfer, range, white point, origin, and HDR.
/// Known non-D65 primaries are NOT collapsed here — `compact_to_buf` owns
/// white-point-aware compaction.
fn collapse_untagged_neutral_rgb(pixels: &PixelBuf) -> Option<PixelBuf> {
    use zenpixels::{AlphaMode, ColorSpace, Primaries, SampleLayout};
    let spec = pixels.spec();
    let ColorSpace::Rgb {
        primaries: Primaries::Unknown,
        transfer,
        range,
        alpha: AlphaMode::None,
        white,
    } = spec.color
    else {
        return None;
    };
    if pixels.origin().icc.is_some() {
        return None;
    }
    let view = pixels.as_view();
    let gray_color = ColorSpace::Gray {
        transfer,
        range,
        alpha: AlphaMode::None,
        white,
    };
    let (w, h) = (pixels.width(), pixels.height());
    let mut out = match spec.layout {
        SampleLayout::RGB8 => {
            PixelBuf::new(w, h, ImageSpec::new(SampleLayout::GRAY8, gray_color)).ok()?
        }
        SampleLayout::RGB16 => {
            PixelBuf::new(w, h, ImageSpec::new(SampleLayout::GRAY16, gray_color)).ok()?
        }
        _ => return None,
    };
    {
        let mut dst = out.as_view_mut();
        match spec.layout {
            SampleLayout::RGB8 => {
                for y in 0..h {
                    let src_row = view.row(y);
                    let dst_row = dst.row_mut(y);
                    for (rgb, g) in src_row.as_chunks::<3>().0.iter().zip(dst_row.iter_mut()) {
                        *g = rgb[0];
                    }
                }
            }
            SampleLayout::RGB16 => {
                for y in 0..h {
                    let src_row = view.row(y);
                    let dst_row = dst.row_mut(y);
                    for (rgb, g) in src_row
                        .as_chunks::<6>()
                        .0
                        .iter()
                        .zip(dst_row.as_chunks_mut::<2>().0.iter_mut())
                    {
                        *g = [rgb[0], rgb[1]];
                    }
                }
            }
            _ => unreachable!(),
        }
    }
    Some(
        out.with_origin(pixels.origin().clone())
            .with_hdr(pixels.hdr()),
    )
}

fn collapse_neutral_rgb(pixels: &PixelBuf) -> Option<PixelBuf> {
    use zenpixels::{AlphaMode, Cicp, ColorOrigin, ColorSpace, SampleLayout};
    use zenpixels_convert::{CompactionReport, analyze_compaction, compact_to_buf};

    let spec = pixels.spec();
    if !matches!(spec.layout, SampleLayout::RGB8 | SampleLayout::RGB16) {
        return None;
    }
    let origin = pixels.origin();
    let view = pixels.as_view();
    let report: CompactionReport = analyze_compaction(&view);
    if report.requires_chroma != Some(false) {
        return None;
    }

    // The source's nclx describes the coded planes. The compaction planner
    // needs the active ICC alone here, with no coded matrix/range companion.
    // Preserve the complete original carriers on the returned raster.
    let current = match &spec.color {
        ColorSpace::Icc { profile, .. } => ColorOrigin::from_icc(profile.clone()),
        _ => ColorOrigin::assumed(),
    };
    if let Ok(Some(gray)) = compact_to_buf(&view.clone().with_origin(&current))
        && gray.spec().layout.order.is_gray_family()
    {
        return Some(gray.with_origin(origin.clone()));
    }

    if let Some(gray) = collapse_untagged_neutral_rgb(pixels) {
        return Some(gray);
    }

    let ColorSpace::Icc { profile, .. } = &spec.color else {
        return None;
    };
    let cicp = super::color::derived_cicp_from_icc(profile).or_else(|| {
        // Compatibility fallback for a malformed profile with separately
        // supplied CICP. A real, unidentified RGB profile stays authoritative.
        (!super::color::icc_class_matches_layout(profile, spec.layout.order))
            .then_some(origin.cicp)
            .flatten()
    })?;
    let rgb = Cicp::new(cicp.color_primaries, cicp.transfer_characteristics, 0, true);
    let color = rgb.to_color_space(AlphaMode::None).ok()?;
    let fallback_spec = ImageSpec::new(spec.layout, color);
    let fallback = zenpixels::PixelView::new(
        view.as_strided_bytes(),
        view.width(),
        view.rows(),
        view.stride(),
        &fallback_spec,
    )
    .ok()?
    .with_hdr(view.hdr());
    compact_to_buf(&fallback)
        .ok()
        .flatten()
        .filter(|gray| gray.spec().layout.order.is_gray_family())
        .map(|gray| gray.with_origin(origin.clone()))
}

/// The descriptor a strip-based decode should emit, or `None` to emit
/// `native` unchanged.
///
/// This is [`negotiate_format`]'s decision without its pixels: the strip
/// paths must choose an output format once, up front, because they announce
/// it to the caller (`OutputInfo` / `begin()`) before the first strip exists.
/// The plan is probed on a 1x1 buffer so a pair the conversion library cannot
/// handle is declined here rather than failing halfway through a stream —
/// same outcome as the buffered path's decline, just decided earlier.
///
/// zenavif#36: neither strip path ran any of this. They honoured a Gray
/// *layout* preference (via `set_native_gray`, since #35) and dropped every
/// other reduction `preferred` can express, so a caller asking for `Rgb8` got
/// `Rgb16` or `Rgba8` — 2x or 1.33x the bytes per pixel, in a different
/// layout, with no error.
pub(super) fn negotiate_strip_descriptor(
    native: &ImageSpec,
    preferred: &[ImageSpec],
) -> Option<ImageSpec> {
    if preferred.is_empty() || preferred.iter().any(|p| format_matches(p, native)) {
        return None;
    }
    preferred
        .iter()
        .filter(|pref| reduction_is_offered(native, pref))
        .map(|pref| target_descriptor(native, pref))
        .find(|target| {
            // Probe the plan on one pixel. Cheap, and it means the descriptor
            // we announce is always one we can actually deliver.
            PixelBuf::new(1, 1, native.clone())
                .ok()
                .and_then(|pixels| apply_strip_reduction(pixels, target.clone()))
                .is_some()
        })
}

/// Convert one strip to the descriptor [`negotiate_strip_descriptor`] chose.
///
/// The Gray16→Gray8 special case is not an optimisation — it is what keeps
/// the strip paths byte-identical to the buffered path. `negotiate_format`'s
/// gray arm narrows depth with [`crate::convert::downscale_to_8bit`], which
/// truncates (`>> 8`), while `convert_to` rounds in f32. Routing gray through
/// `convert_to` here would make streaming and buffered disagree by 1 LSB on
/// every 10/12-bit mono image — a difference `tests/negotiation_matrix.rs`
/// catches, and the reason this function exists instead of a bare
/// `convert_to` at each call site.
pub(super) fn apply_strip_reduction(pixels: PixelBuf, target: ImageSpec) -> Option<PixelBuf> {
    let native = pixels.spec();
    if native.layout.order == zenpixels::ChannelOrder::Gray
        && target.layout.order == zenpixels::ChannelOrder::Gray
        && native.layout.sample == SampleType::U16
        && target.layout.sample == SampleType::U8
    {
        return Some(crate::convert::downscale_to_8bit(pixels));
    }
    try_reduce(&pixels, target)
}

/// `source_is_gray`: the *coded* image is alpha-free monochrome, so a
/// gray preference can be satisfied exactly (an RGB-expanded mono buffer
/// is R=G=B; luma of equal channels is the channel).
pub(super) fn negotiate_format(
    pixels: PixelBuf,
    preferred: &[ImageSpec],
    source_is_gray: bool,
) -> PixelBuf {
    if preferred.is_empty() {
        return pixels;
    }

    let native = pixels.spec();

    // If the native pixel format matches any preferred descriptor, return as-is.
    // We compare pixel format only (ignoring transfer/primaries/signal range),
    // because CICP metadata enriches the descriptor but doesn't change the data.
    if preferred.iter().any(|p| format_matches(p, native)) {
        return pixels;
    }

    // Find first preferred descriptor we can produce.
    for pref in preferred {
        // Can't upscale bit depth losslessly.
        if pref.layout.sample.byte_size() > native.layout.sample.byte_size() {
            continue;
        }

        // Grayscale preferences: satisfiable exactly only for monochrome
        // sources (never synthesize luma for color images here — that is
        // a CMS decision, not format negotiation). The collapse goes
        // through the load-bearing reduction, which VERIFIES R==G==B at
        // the byte level (instead of trusting container metadata),
        // rewrites in place with no allocation, and handles color
        // signaling (an RGB-class ICC profile cannot describe a Gray
        // layout — a gray-class variant is swapped in when derivable,
        // otherwise the collapse is suppressed and we fall through
        // honestly).
        if pref.layout.order == zenpixels::ChannelOrder::Gray {
            if source_is_gray && pref.layout.sample == SampleType::U8 {
                if native.layout.order == zenpixels::ChannelOrder::Gray {
                    return crate::convert::downscale_to_8bit(pixels);
                }
                if native.layout == zenpixels::SampleLayout::RGB8
                    && matches!(native.color, zenpixels::ColorSpace::Rgb { .. })
                    && pixels.origin().icc.is_none()
                    && zenpixels_convert::analyze_compaction(&pixels.as_view()).requires_chroma
                        == Some(false)
                {
                    let mut pixels = pixels;
                    zenpixels_convert::compact_in_place(
                        &mut pixels,
                        zenpixels_convert::AlphaCompaction::RetagOpaque,
                    );
                    if pixels.spec().layout.order == zenpixels::ChannelOrder::Gray {
                        return pixels;
                    }
                    if let Some(gray) = collapse_untagged_neutral_rgb(&pixels) {
                        return gray;
                    }
                    return pixels;
                }
                if let Some(gray) = collapse_neutral_rgb(&pixels) {
                    return crate::convert::downscale_to_8bit(gray);
                }
            }
            continue;
        }

        // Everything else negotiation offers — gray-native expansion to
        // colour, the 16→8 depth narrow, and the 8-bit add/drop-alpha layout
        // change — is one fallible conversion to the preferred layout and
        // depth, carrying the native colour description across.
        if !reduction_is_offered(native, pref) {
            continue;
        }
        match try_reduce(&pixels, target_descriptor(native, pref)) {
            Some(converted) => return converted,
            // No conversion plan for this pair: decline and let the next
            // preference have its shot (zenavif#39 — never unwind).
            None => continue,
        }
    }

    // No preferred descriptor matched — return native format.
    pixels
}

#[cfg(test)]
mod tests {
    /// Exercise the shared byte-verified reduction in the workspace test graph.
    /// The PNG dev dependency can enable `icc-db`; the downstream probe
    /// qualifies feature-isolated gray signaling.
    mod negotiate_gray {
        use super::super::{format_matches, negotiate_format, wants_gray_output};
        use alloc::sync::Arc;
        use zenpixels::{
            Cicp, ColorOrigin, ColorSpace, ImageSpec, PixelBuf, SampleLayout, Transfer,
        };

        extern crate alloc;

        fn with_icc(pixels: PixelBuf, profile: Arc<[u8]>) -> PixelBuf {
            use crate::pixel::PixelBufExt as _;
            pixels.retag(ImageSpec::new(
                SampleLayout::RGB8,
                ColorSpace::Icc {
                    profile,
                    channels: 3,
                    alpha: zenpixels::AlphaMode::None,
                    intent: zenpixels::RenderingIntent::Perceptual,
                },
            ))
        }

        fn gray_content_rgb8(w: u32, h: u32) -> PixelBuf {
            let px: alloc::vec::Vec<rgb::Rgb<u8>> = (0..w * h)
                .map(|i| {
                    let g = (i * 7 % 256) as u8;
                    rgb::Rgb { r: g, g, b: g }
                })
                .collect();
            crate::pixel::from_pixels(px, w, h).unwrap()
        }

        /// No color context (zenavif's decode reality today — ICC rides on
        /// `ImageInfo`, never the buffer): Carry plan, collapse proceeds,
        /// and the gray bytes equal the source channel exactly.
        #[test]
        fn collapses_without_context_and_matches_channel() {
            let buf = gray_content_rgb8(9, 4);
            let want: alloc::vec::Vec<u8> = (0..36).map(|i| (i * 7 % 256) as u8).collect();
            let out = negotiate_format(buf, &[ImageSpec::GRAY8_SRGB], true);
            assert_eq!(out.spec().layout, SampleLayout::GRAY8);
            let s = out.as_view();
            let got: alloc::vec::Vec<u8> = (0..4).flat_map(|y| s.row(y)[..9].to_vec()).collect();
            assert_eq!(got, want, "gray must be the exact channel value");
        }

        /// Explicit CICP supplies active color after a malformed ICC claim.
        #[test]
        fn malformed_icc_with_explicit_srgb_cicp_collapses() {
            use super::super::attach_color_context_class_gated;
            let profile = Arc::<[u8]>::from(alloc::vec![0u8; 16]);
            let mut sc = zencodec::decode::SourceColor::default();
            sc.icc_profile = Some(profile.clone());
            sc.cicp = Some(Cicp::SRGB);
            let buf = attach_color_context_class_gated(gray_content_rgb8(8, 2), &sc);
            let out = negotiate_format(buf, &[ImageSpec::GRAY8_SRGB], true);
            assert_eq!(out.spec().layout, SampleLayout::GRAY8);
            assert!(matches!(
                out.spec().color,
                ColorSpace::Gray {
                    transfer: Transfer::Srgb,
                    ..
                }
            ));
            assert_eq!(out.origin().cicp, Some(Cicp::SRGB));
            assert_eq!(out.origin().icc, Some(profile));
        }

        #[test]
        fn real_rgb_profiles_reduce_equal_channels_and_keep_original_icc() {
            let profiles = [
                crate::codec::negotiate::profiles::SRGB,
                zenpixels_convert::icc_profiles::DISPLAY_P3_V4,
            ];
            for profile in profiles {
                assert_eq!(&profile[16..20], b"RGB ");
                assert_eq!(&profile[36..40], b"acsp");
                let profile = Arc::<[u8]>::from(profile);
                let input = with_icc(gray_content_rgb8(8, 2), profile.clone())
                    .with_origin(ColorOrigin::from_icc(profile.clone()));
                let out = negotiate_format(input, &[ImageSpec::GRAY8_SRGB], true);
                assert_eq!(out.spec().layout, SampleLayout::GRAY8);
                match &out.spec().color {
                    ColorSpace::Icc {
                        profile,
                        channels: 1,
                        ..
                    } => assert_eq!(&profile[16..20], b"GRAY"),
                    ColorSpace::Gray {
                        transfer: Transfer::Srgb,
                        ..
                    } => {}
                    wrong => panic!("incorrect active gray color {wrong:?}"),
                }
                let view = out.as_view();
                assert_eq!(view.origin().unwrap().icc, Some(profile));
                for y in 0..2 {
                    assert_eq!(
                        view.row(y),
                        (0..8).map(|x| ((8 * y + x) * 7) as u8).collect::<Vec<_>>()
                    );
                }
            }
        }

        /// Underivable ICC (unidentified RGB profile, no cicp): the collapse is
        /// suppressed and negotiation falls through to the NEXT
        /// preference and keeps its original color carrier.
        #[test]
        fn unknown_icc_suppresses_and_falls_through() {
            let buf = with_icc(
                gray_content_rgb8(8, 2),
                Arc::from(crate::codec::negotiate::profiles::UNIDENTIFIED_RGB),
            );
            let out = negotiate_format(buf, &[ImageSpec::GRAY8_SRGB, ImageSpec::RGBA8_SRGB], true);
            assert_eq!(
                out.spec().layout,
                SampleLayout::RGBA8,
                "suppressed collapse must fall through to the next preference"
            );
            assert!(
                matches!(out.spec().color, ColorSpace::Icc { .. }),
                "the original RGB-class context stays with the RGB-class pixels"
            );
        }

        /// Metadata claims mono but the pixels are NOT R==G==B: the
        /// byte-level verification refuses the collapse — this is the
        /// trust-nothing property the load-bearing reduction buys over
        /// `to_gray8()` (which would have averaged the lie into luma).
        #[test]
        fn lying_metadata_never_fakes_gray() {
            let px: alloc::vec::Vec<rgb::Rgb<u8>> = (0..16)
                .map(|i| rgb::Rgb {
                    r: 200,
                    g: (i * 3) as u8,
                    b: 10,
                })
                .collect();
            let buf: PixelBuf = crate::pixel::from_pixels(px, 8, 2).unwrap();
            let out = negotiate_format(buf, &[ImageSpec::GRAY8_SRGB], true);
            assert_ne!(
                out.spec().layout,
                SampleLayout::GRAY8,
                "colorful pixels must never collapse, whatever the metadata says"
            );
        }

        /// Class gate: an RGB-class ICC never rides a Gray buffer — it
        /// is stripped and the raw CICP restored as the fallback signal.
        #[test]
        fn class_gate_strips_rgb_icc_from_gray() {
            use super::super::attach_color_context_class_gated;
            let mut sc = zencodec::decode::SourceColor::default();
            sc.icc_profile = Some(Arc::<[u8]>::from(
                crate::codec::negotiate::profiles::UNIDENTIFIED_RGB,
            ));
            sc.cicp = Some(Cicp::SRGB);
            // Icc authority (the default): to_color_context drops the cicp.
            let gray: PixelBuf =
                crate::pixel::from_pixels(alloc::vec![rgb::Gray::<u8>::new(7); 8], 4, 2).unwrap();
            let out = attach_color_context_class_gated(gray, &sc);
            assert!(matches!(
                out.spec().color,
                ColorSpace::Gray {
                    transfer: Transfer::Srgb,
                    ..
                }
            ));
            assert_eq!(out.origin().cicp, Some(Cicp::SRGB));

            // Same source on an RGB-layout buffer: the ICC rides.
            let rgbbuf: PixelBuf = crate::pixel::from_pixels(
                alloc::vec![rgb::Rgb::<u8> { r: 7, g: 7, b: 7 }; 8],
                4,
                2,
            )
            .unwrap();
            let out = attach_color_context_class_gated(rgbbuf, &sc);
            assert!(
                matches!(out.spec().color, ColorSpace::Icc { .. }),
                "RGB-class ICC valid on RGB pixels"
            );

            // Structurally malformed RGB-looking ICC falls back cleanly to CICP without panicking.
            let mut bad_icc = alloc::vec![0u8; 132];
            bad_icc[16..20].copy_from_slice(b"RGB ");
            let mut bad_sc = zencodec::decode::SourceColor::default();
            bad_sc.icc_profile = Some(Arc::<[u8]>::from(bad_icc.as_slice()));
            bad_sc.cicp = Some(Cicp::SRGB);
            let rgbbuf2: PixelBuf = crate::pixel::from_pixels(
                alloc::vec![rgb::Rgb::<u8> { r: 7, g: 7, b: 7 }; 8],
                4,
                2,
            )
            .unwrap();
            let out = attach_color_context_class_gated(rgbbuf2, &bad_sc);
            assert!(matches!(
                out.spec().color,
                ColorSpace::Rgb {
                    transfer: Transfer::Srgb,
                    ..
                }
            ));
            assert_eq!(out.origin().cicp, Some(Cicp::SRGB));
        }

        /// A GRAY-class ICC is allowed onto gray output (mono AVIFs with
        /// MIAF-correct profiles), and a truncated blob never passes.
        #[test]
        fn class_gate_accepts_gray_icc_and_rejects_short() {
            use super::super::icc_class_matches_layout;
            let mut gray_icc = alloc::vec![0u8; 132];
            gray_icc[16..20].copy_from_slice(b"GRAY");
            assert!(icc_class_matches_layout(
                &gray_icc,
                zenpixels::ChannelOrder::Gray
            ));
            assert!(!icc_class_matches_layout(
                &gray_icc,
                zenpixels::ChannelOrder::Rgb
            ));
            assert!(!icc_class_matches_layout(
                &gray_icc[..64],
                zenpixels::ChannelOrder::Gray
            ));
        }

        /// Sanity for the helpers this arm depends on.
        #[test]
        fn helper_contracts() {
            assert!(wants_gray_output(&[]));
            assert!(wants_gray_output(&[ImageSpec::GRAY8_SRGB]));
            assert!(!wants_gray_output(&[ImageSpec::RGB8_SRGB]));
            assert!(format_matches(
                &ImageSpec::GRAY8_SRGB,
                &ImageSpec::GRAY8_SRGB
            ));
        }
    }
}

#[cfg(test)]
#[path = "../../tests/support/icc_profiles.rs"]
mod profiles;
