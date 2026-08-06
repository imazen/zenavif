//! Preferred-pixel-format negotiation for decoder output, including the
//! byte-verified, ICC-signalling-aware gray collapse.

use zenpixels::{ChannelType, PixelBuffer, PixelDescriptor};
use zenpixels_convert::PixelBufferConvertTypedExt as _;

// The `negotiate_gray` tests below reach these colour helpers through `super`.
#[cfg(test)]
use super::color::{attach_color_context_class_gated, icc_class_matches_layout};

/// Check if two descriptors match on pixel format (channel type + alpha),
/// ignoring transfer function, primaries, and signal range metadata.
pub(super) fn format_matches(a: PixelDescriptor, b: PixelDescriptor) -> bool {
    a.pixel_format() == b.pixel_format()
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
pub(super) fn wants_gray_output(preferred: &[PixelDescriptor]) -> bool {
    match preferred.first() {
        None => true,
        Some(p) => p.layout() == zenpixels::ChannelLayout::Gray,
    }
}

/// `source_is_gray`: the *coded* image is alpha-free monochrome, so a
/// gray preference can be satisfied exactly (an RGB-expanded mono buffer
/// is R=G=B; luma of equal channels is the channel).
pub(super) fn negotiate_format(
    mut pixels: PixelBuffer,
    preferred: &[PixelDescriptor],
    source_is_gray: bool,
) -> PixelBuffer {
    if preferred.is_empty() {
        return pixels;
    }

    let native = pixels.descriptor();

    // If the native pixel format matches any preferred descriptor, return as-is.
    // We compare pixel format only (ignoring transfer/primaries/signal range),
    // because CICP metadata enriches the descriptor but doesn't change the data.
    if preferred.iter().any(|p| format_matches(*p, native)) {
        return pixels;
    }

    // Find first preferred descriptor we can produce.
    for pref in preferred {
        // Can't upscale bit depth losslessly.
        if pref.channel_type().byte_size() > native.channel_type().byte_size() {
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
        if pref.layout() == zenpixels::ChannelLayout::Gray {
            if source_is_gray && pref.channel_type() == ChannelType::U8 {
                use zenpixels_convert::PixelBufferLoadBearingExt as _;
                pixels.reduce_to_load_bearing_format_in_place(true);
                if pixels.descriptor().layout() == zenpixels::ChannelLayout::Gray {
                    // 10/12-bit mono reduces to Gray16; honor the U8 ask.
                    if pixels.descriptor().channel_type() == ChannelType::U16 {
                        return crate::convert::downscale_to_8bit(pixels);
                    }
                    return pixels;
                }
                // Scan disagreed with the metadata, or an underivable
                // RGB-class ICC suppressed the collapse: never fake
                // gray — let the remaining preferences have their shot.
            }
            continue;
        }

        // Gray native, color preference at the same depth: expand.
        if native.layout() == zenpixels::ChannelLayout::Gray
            && pref.channel_type() == native.channel_type()
            && native.channel_type() == ChannelType::U8
        {
            if pref.layout().has_alpha() {
                return pixels.to_rgba8().into();
            }
            return pixels.to_rgb8().into();
        }

        // If caller wants 8-bit and we have 16-bit, downconvert.
        if pref.channel_type() == ChannelType::U8 && native.channel_type() == ChannelType::U16 {
            if pref.layout().has_alpha() {
                return pixels.to_rgba8().into();
            }
            return pixels.to_rgb8().into();
        }

        // Same bit depth but different layout (e.g., RGB vs RGBA).
        if pref.channel_type() == native.channel_type() {
            if pref.layout().has_alpha() && !native.layout().has_alpha() {
                if native.channel_type() == ChannelType::U8 {
                    return pixels.to_rgba8().into();
                }
                continue;
            }
            if !pref.layout().has_alpha() && native.layout().has_alpha() {
                if native.channel_type() == ChannelType::U8 {
                    return pixels.to_rgb8().into();
                }
                continue;
            }
        }
    }

    // No preferred descriptor matched — return native format.
    pixels
}

#[cfg(test)]
mod tests {
    /// Pin the negotiate-layer gray collapse in zenavif's exact feature
    /// configuration (zenpixels-convert with `default-features = false`,
    /// i.e. NO `icc-db`): the load-bearing reduction byte-verifies
    /// R==G==B instead of trusting metadata, and the ICC color-signaling
    /// rules decide whether the collapse may proceed.
    mod negotiate_gray {
        use super::super::{format_matches, negotiate_format, wants_gray_output};
        use alloc::sync::Arc;
        use zenpixels::{Cicp, ColorContext, PixelBuffer, PixelDescriptor, PixelFormat};

        extern crate alloc;

        fn gray_content_rgb8(w: u32, h: u32) -> PixelBuffer {
            let px: alloc::vec::Vec<rgb::Rgb<u8>> = (0..w * h)
                .map(|i| {
                    let g = (i * 7 % 256) as u8;
                    rgb::Rgb { r: g, g, b: g }
                })
                .collect();
            PixelBuffer::from_pixels(px, w, h).unwrap().into()
        }

        /// No color context (zenavif's decode reality today — ICC rides on
        /// `ImageInfo`, never the buffer): Carry plan, collapse proceeds,
        /// and the gray bytes equal the source channel exactly.
        #[test]
        fn collapses_without_context_and_matches_channel() {
            let buf = gray_content_rgb8(9, 4);
            let want: alloc::vec::Vec<u8> = (0..36).map(|i| (i * 7 % 256) as u8).collect();
            let out = negotiate_format(buf, &[PixelDescriptor::GRAY8_SRGB], true);
            assert_eq!(out.descriptor().pixel_format(), PixelFormat::Gray8);
            let s = out.as_slice();
            let got: alloc::vec::Vec<u8> = (0..4).flat_map(|y| s.row(y)[..9].to_vec()).collect();
            assert_eq!(got, want, "gray must be the exact channel value");
        }

        /// sRGB-described ICC: the collapse is allowed and the RGB-class
        /// ICC is dropped in favor of CICP-only signaling (an RGB profile
        /// cannot describe a Gray layout; sRGB needs no profile at all).
        #[test]
        fn srgb_icc_collapses_and_drops_profile() {
            let mut ctx = ColorContext::from_icc(alloc::vec![0u8; 16]);
            ctx.cicp = Some(Cicp::SRGB);
            let buf = gray_content_rgb8(8, 2).with_color_context(Arc::new(ctx));
            let out = negotiate_format(buf, &[PixelDescriptor::GRAY8_SRGB], true);
            assert_eq!(out.descriptor().pixel_format(), PixelFormat::Gray8);
            let new_ctx = out
                .as_slice()
                .color_context()
                .cloned()
                .expect("cicp-only context survives the collapse");
            assert!(
                new_ctx.icc.is_none(),
                "an RGB-class ICC must never ride on a Gray buffer"
            );
            assert_eq!(new_ctx.cicp, Some(Cicp::SRGB));
        }

        /// Underivable ICC (junk bytes, no cicp): the collapse is
        /// suppressed and negotiation falls through to the NEXT
        /// preference instead of mislabeling or faking gray. Without
        /// `icc-db` this is also the path every non-sRGB profile takes.
        #[test]
        fn unknown_icc_suppresses_and_falls_through() {
            let ctx = ColorContext::from_icc(alloc::vec![0xAAu8; 64]);
            let buf = gray_content_rgb8(8, 2).with_color_context(Arc::new(ctx.clone()));
            let out = negotiate_format(
                buf,
                &[PixelDescriptor::GRAY8_SRGB, PixelDescriptor::RGBA8_SRGB],
                true,
            );
            assert_eq!(
                out.descriptor().pixel_format(),
                PixelFormat::Rgba8,
                "suppressed collapse must fall through to the next preference"
            );
            assert!(
                out.as_slice()
                    .color_context()
                    .is_some_and(|c| c.icc.is_some()),
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
            let buf: PixelBuffer = PixelBuffer::from_pixels(px, 8, 2).unwrap().into();
            let out = negotiate_format(buf, &[PixelDescriptor::GRAY8_SRGB], true);
            assert_ne!(
                out.descriptor().pixel_format(),
                PixelFormat::Gray8,
                "colorful pixels must never collapse, whatever the metadata says"
            );
        }

        /// Class gate: an RGB-class ICC never rides a Gray buffer — it
        /// is stripped and the raw CICP restored as the fallback signal.
        #[test]
        fn class_gate_strips_rgb_icc_from_gray() {
            use super::super::attach_color_context_class_gated;
            let mut icc = alloc::vec![0u8; 132];
            icc[16..20].copy_from_slice(b"RGB ");
            let mut sc = zencodec::decode::SourceColor::default();
            sc.icc_profile = Some(Arc::<[u8]>::from(icc.as_slice()));
            sc.cicp = Some(Cicp::SRGB);
            // Icc authority (the default): to_color_context drops the cicp.
            let gray: PixelBuffer =
                PixelBuffer::from_pixels(alloc::vec![rgb::Gray::<u8>::new(7); 8], 4, 2)
                    .unwrap()
                    .into();
            let out = attach_color_context_class_gated(gray, &sc);
            let ctx = out.as_slice().color_context().cloned().expect("ctx");
            assert!(ctx.icc.is_none(), "RGB-class ICC stripped from gray");
            assert_eq!(
                ctx.cicp,
                Some(Cicp::SRGB),
                "raw CICP restored as the fallback after the strip"
            );

            // Same source on an RGB-layout buffer: the ICC rides.
            let rgbbuf: PixelBuffer =
                PixelBuffer::from_pixels(alloc::vec![rgb::Rgb::<u8> { r: 7, g: 7, b: 7 }; 8], 4, 2)
                    .unwrap()
                    .into();
            let out = attach_color_context_class_gated(rgbbuf, &sc);
            let ctx = out.as_slice().color_context().cloned().expect("ctx");
            assert!(ctx.icc.is_some(), "RGB-class ICC valid on RGB pixels");
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
                zenpixels::ChannelLayout::Gray
            ));
            assert!(!icc_class_matches_layout(
                &gray_icc,
                zenpixels::ChannelLayout::Rgb
            ));
            assert!(!icc_class_matches_layout(
                &gray_icc[..64],
                zenpixels::ChannelLayout::Gray
            ));
        }

        /// Sanity for the helpers this arm depends on.
        #[test]
        fn helper_contracts() {
            assert!(wants_gray_output(&[]));
            assert!(wants_gray_output(&[PixelDescriptor::GRAY8_SRGB]));
            assert!(!wants_gray_output(&[PixelDescriptor::RGB8_SRGB]));
            assert!(format_matches(
                PixelDescriptor::GRAY8_SRGB,
                PixelDescriptor::GRAY8_SRGB
            ));
        }
    }
}
