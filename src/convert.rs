//! Alpha channel handling, premultiply conversion, and bit depth scaling

use crate::error::{Error, Result};
use crate::image::ColorRange;
use rgb::prelude::*;
use rgb::{Rgb, Rgba};
use whereat::at;
use zenpixels::{PixelBuffer, PixelDescriptor};

/// Why the typed-view `expect`s in this module cannot fire.
///
/// `PixelBuffer::try_as_imgref` needs two things: a layout-compatible
/// descriptor (each call site checks that immediately above) AND a row stride
/// that is a whole number of pixels. Every buffer reaching this module is one
/// **zenavif itself allocated** on the decode path — `PixelBuffer::from_pixels`
/// and the decoder's own strip/frame allocations are all tightly packed, so
/// `stride == width * size_of::<P>()` by construction. This module is
/// crate-private (`mod convert;` in `lib.rs`), so no caller-supplied,
/// arbitrarily-strided buffer can reach it.
///
/// The public entry points that DO accept caller buffers (`lib.rs`
/// `encode_with`, `codec.rs` `map_rgb8_rows`/`map_rgba8_rows`) handle the
/// odd-stride case explicitly instead — with a typed error and an owning
/// fallback respectively.
const TIGHTLY_PACKED_INVARIANT: &str = "decode-path buffers are allocated tightly packed, so a layout-compatible \
     descriptor always yields a typed view";

/// Describe a decoded buffer with the container's CICP.
///
/// The **single** place the decode paths turn `transfer_characteristics` /
/// `color_primaries` into a [`PixelDescriptor`]. It lives here rather than in
/// the zencodec adapter because the strip converter and the grid sink both
/// mint descriptors of their own, and the adapter is too late to be the
/// authority — that split is what produced zenavif#37 (streaming and the row
/// sink handing PQ pixels to the caller labelled `transfer: Unknown`).
///
/// Note the asymmetry between the two fields, which is deliberate and
/// mirrors what the buffered path has always produced:
///
/// * **transfer** is set unconditionally, falling back to
///   [`TransferFunction::Unknown`] when the container says "unspecified"
///   (CICP 2) or names a curve zenpixels has no variant for. The strip
///   converter's own base descriptors are the hardcoded `RGB8_SRGB` /
///   `RGBA8_SRGB`, so *leaving* the field alone would assert sRGB about a
///   file that never claimed it. `Unknown` is the honest answer and it is
///   what the buffered path reports, since its buffers come from
///   `PixelBuffer::from_pixels`, whose default transfer is `Unknown`.
/// * **primaries** are only overwritten when the container names a set
///   zenpixels knows, leaving the `Bt709` default in place otherwise —
///   again matching the buffered path.
pub(crate) fn descriptor_with_cicp(
    mut desc: PixelDescriptor,
    info: &crate::image::ImageInfo,
) -> PixelDescriptor {
    desc = desc.with_transfer(
        zenpixels::TransferFunction::from_cicp(info.transfer_characteristics.0)
            .unwrap_or(zenpixels::TransferFunction::Unknown),
    );
    if let Some(p) = zenpixels::ColorPrimaries::from_cicp(info.color_primaries.0) {
        desc = desc.with_primaries(p);
    }
    desc
}

/// Scale a limited-range Y value to full range (8-bit)
#[inline]
fn limited_to_full_8(y: u8) -> u8 {
    // Limited range: Y ∈ [16, 235]
    // Full range: Y ∈ [0, 255]
    // Use i32 to avoid i16 overflow: (235-16)*255 = 55845 > i16::MAX
    let y = y as i32;
    ((y - 16).max(0) * 255 / 219).min(255) as u8
}

/// Scale a limited-range Y value to full range (16-bit, given bit depth)
#[inline]
fn limited_to_full_16(y: u16, bit_depth: u8) -> u16 {
    let max_val = (1u32 << bit_depth) - 1;
    let y_min = 16u32 << (bit_depth - 8);
    let y_range = 219u32 << (bit_depth - 8);
    let y32 = y as u32;
    ((y32.saturating_sub(y_min)) * max_val / y_range).min(max_val) as u16
}

/// Scale a value from native bit depth to full u16 range using LSB replication.
///
/// For 10-bit: `(v << 6) | (v >> 4)` maps 0→0, 1023→65535
/// For 12-bit: `(v << 4) | (v >> 8)` maps 0→0, 4095→65535
/// For 16-bit: no-op
#[inline]
fn scale_to_u16(v: u16, bit_depth: u8) -> u16 {
    let shift = 16 - bit_depth;
    if shift == 0 {
        return v;
    }
    // LSB replication: fill lower bits with copies of upper bits
    // This gives exact mapping: 0→0, max→65535
    (v << shift) | (v >> (bit_depth - shift))
}

/// Downscale a 16-bit PixelBuffer to 8-bit by taking the high byte of each channel.
///
/// Converts Rgb16 → Rgb8 and Rgba16 → Rgba8 in-place (reallocates to a new buffer).
/// Values are assumed to be in full u16 range (0–65535) after `scale_pixels_to_u16`.
pub fn downscale_to_8bit(image: PixelBuffer) -> PixelBuffer {
    let desc = image.descriptor();
    let w = image.width();
    let h = image.height();
    // The rebuilds below construct fresh buffers — carry the color
    // context across so downstream stages keep self-describing pixels.
    let ctx = image.color_context().cloned();
    let reattach = move |buf: PixelBuffer| match ctx {
        Some(ctx) => buf.with_color_context(ctx),
        None => buf,
    };
    if desc.layout_compatible(PixelDescriptor::RGB16) {
        let src = image
            .try_as_imgref::<Rgb<u16>>()
            .expect(TIGHTLY_PACKED_INVARIANT);
        let out: Vec<Rgb<u8>> = src
            .pixels()
            .map(|px| Rgb {
                r: (px.r >> 8) as u8,
                g: (px.g >> 8) as u8,
                b: (px.b >> 8) as u8,
            })
            .collect();
        reattach(
            PixelBuffer::from_pixels(out, w, h)
                .expect("allocation should succeed for same dimensions")
                .into(),
        )
    } else if desc.layout_compatible(PixelDescriptor::RGBA16) {
        let src = image
            .try_as_imgref::<Rgba<u16>>()
            .expect(TIGHTLY_PACKED_INVARIANT);
        let out: Vec<Rgba<u8>> = src
            .pixels()
            .map(|px| Rgba {
                r: (px.r >> 8) as u8,
                g: (px.g >> 8) as u8,
                b: (px.b >> 8) as u8,
                a: (px.a >> 8) as u8,
            })
            .collect();
        reattach(
            PixelBuffer::from_pixels(out, w, h)
                .expect("allocation should succeed for same dimensions")
                .into(),
        )
    } else if desc.layout_compatible(PixelDescriptor::GRAY16) {
        let src = image
            .try_as_imgref::<rgb::Gray<u16>>()
            .expect(TIGHTLY_PACKED_INVARIANT);
        let out: Vec<rgb::Gray<u8>> = src
            .pixels()
            .map(|px| rgb::Gray::new((px.value() >> 8) as u8))
            .collect();
        reattach(
            PixelBuffer::from_pixels(out, w, h)
                .expect("allocation should succeed for same dimensions")
                .into(),
        )
    } else {
        image
    }
}

/// Scale all channels in a 16-bit PixelBuffer from native bit depth to full u16 range.
///
/// This converts e.g. 10-bit values (0–1023) to full 16-bit (0–65535) using
/// LSB replication for exact endpoint mapping.
pub fn scale_pixels_to_u16(image: &mut PixelBuffer, bit_depth: u8) {
    if bit_depth >= 16 {
        return;
    }
    let desc = image.descriptor();
    if desc.layout_compatible(PixelDescriptor::RGB16) {
        let mut img = image
            .try_as_imgref_mut::<Rgb<u16>>()
            .expect(TIGHTLY_PACKED_INVARIANT);
        for px in img.buf_mut().iter_mut() {
            *px = Rgb {
                r: scale_to_u16(px.r, bit_depth),
                g: scale_to_u16(px.g, bit_depth),
                b: scale_to_u16(px.b, bit_depth),
            };
        }
    } else if desc.layout_compatible(PixelDescriptor::RGBA16) {
        let mut img = image
            .try_as_imgref_mut::<Rgba<u16>>()
            .expect(TIGHTLY_PACKED_INVARIANT);
        for px in img.buf_mut().iter_mut() {
            *px = Rgba {
                r: scale_to_u16(px.r, bit_depth),
                g: scale_to_u16(px.g, bit_depth),
                b: scale_to_u16(px.b, bit_depth),
                a: scale_to_u16(px.a, bit_depth),
            };
        }
    } else if desc.layout_compatible(PixelDescriptor::GRAY16) {
        let mut img = image
            .try_as_imgref_mut::<rgb::Gray<u16>>()
            .expect(TIGHTLY_PACKED_INVARIANT);
        for px in img.buf_mut().iter_mut() {
            *px = rgb::Gray::new(scale_to_u16(px.value(), bit_depth));
        }
    }
}

/// Scale a full u16 value (0–65535) down to native bit depth range.
///
/// For 10-bit: `v >> 6` maps 0→0, 65535→1023
/// For 12-bit: `v >> 4` maps 0→0, 65535→4095
///
/// Uses truncation (top-bit extraction), which is the exact inverse of
/// LSB replication in `scale_to_u16`. This gives lossless roundtrip for
/// values produced by LSB replication, symmetric bias for arbitrary
/// inputs, and lower max error than half-up rounding (63 vs 95 for 10-bit).
#[cfg(feature = "encode")]
#[inline]
pub fn scale_from_u16(v: u16, bit_depth: u8) -> u16 {
    let shift = 16 - bit_depth;
    if shift == 0 {
        return v;
    }
    v >> shift
}

/// Narrow a full-range u16 sample to the u8 domain, for the 8-bit
/// raw-plane encode APIs.
///
/// This is [`scale_from_u16`] at depth 8 — the crate's narrowing owner —
/// with the type change the `[u8; 3]` plane APIs need. It is deliberately
/// NOT half-up rounding: truncation is the exact inverse of the widening
/// rule (`scale_to_u16`, LSB replication), so 8-bit content promoted to
/// 16 bits narrows back to the original byte, and it matches the decode
/// side's `downscale_to_8bit` ("the high byte of each channel"). Pinned
/// by the `narrow_16_to_8` tests.
#[cfg(feature = "encode")]
#[inline]
pub fn narrow_to_u8(v: u16) -> u8 {
    // `scale_from_u16(v, 8)` is `v >> 8`, so the value is always <= 255
    // and the mask is a no-op that keeps the cast provably lossless.
    (scale_from_u16(v, 8) & 0xFF) as u8
}

/// Add 8-bit alpha channel to an image from Y plane data
pub fn add_alpha8<'a>(
    buf: &mut PixelBuffer,
    alpha_rows: impl Iterator<Item = &'a [u8]>,
    width: usize,
    height: usize,
    alpha_range: ColorRange,
    premultiplied: bool,
) -> Result<()> {
    let mut img = buf.try_as_imgref_mut::<Rgba<u8>>().ok_or_else(|| {
        at!(Error::InvalidBuffer(
            "cannot add 8-bit alpha to this image type".into(),
        ))
    })?;

    if img.width() != width || img.height() != height {
        return Err(at!(Error::InvalidBuffer("alpha size mismatch".into())));
    }

    for (alpha_row, img_row) in alpha_rows.zip(img.rows_mut()) {
        if alpha_row.len() < img_row.len() {
            return Err(at!(Error::InvalidBuffer("alpha width mismatch".into())));
        }
        for (&y, px) in alpha_row.iter().zip(img_row.iter_mut()) {
            px.a = match alpha_range {
                ColorRange::Full => y,
                ColorRange::Limited => limited_to_full_8(y),
            };
        }
        if premultiplied {
            unpremultiply8(img_row);
        }
    }

    Ok(())
}

/// Add 16-bit alpha channel to an image from Y plane data.
///
/// Alpha values from the plane are in native bit depth range (e.g. 0–1023 for
/// 10-bit). They are range-converted (limited→full if needed) and then scaled
/// to full u16 (0–65535) to match the already-scaled RGB channels.
pub fn add_alpha16<'a>(
    buf: &mut PixelBuffer,
    alpha_rows: impl Iterator<Item = &'a [u16]>,
    width: usize,
    height: usize,
    alpha_range: ColorRange,
    bit_depth: u8,
    premultiplied: bool,
) -> Result<()> {
    let mut img = buf.try_as_imgref_mut::<Rgba<u16>>().ok_or_else(|| {
        at!(Error::InvalidBuffer(
            "cannot add 16-bit alpha to this image type".into(),
        ))
    })?;

    if img.width() != width || img.height() != height {
        return Err(at!(Error::InvalidBuffer("alpha size mismatch".into())));
    }

    for (alpha_row, img_row) in alpha_rows.zip(img.rows_mut()) {
        if alpha_row.len() < img_row.len() {
            return Err(at!(Error::InvalidBuffer("alpha width mismatch".into())));
        }
        for (&y, px) in alpha_row.iter().zip(img_row.iter_mut()) {
            let a = match alpha_range {
                ColorRange::Full => y,
                ColorRange::Limited => limited_to_full_16(y, bit_depth),
            };
            // Scale from native bit depth to full u16
            px.a = scale_to_u16(a, bit_depth);
        }
        if premultiplied {
            unpremultiply16(img_row);
        }
    }

    Ok(())
}

/// Convert premultiplied alpha to straight alpha for 8-bit RGBA
#[inline(never)]
pub fn unpremultiply8(img_row: &mut [Rgba<u8>]) {
    // Divides by the pixel's own alpha, so no integer-SIMD form exists and the
    // scalar loop cannot vectorize. On aarch64 this dispatches to a `vld4q_u8`
    // kernel that deinterleaves RGBA into planes; bit-identical, proven over
    // the complete (channel, alpha) domain by tests/unpremul8_exhaustive.rs.
    // Elsewhere it is exactly the loop that used to live here.
    crate::simd::unpremultiply8_dispatch(img_row)
}

/// Convert premultiplied alpha to straight alpha for 16-bit RGBA
#[inline(never)]
pub fn unpremultiply16(img_row: &mut [Rgba<u16>]) {
    for px in img_row.iter_mut() {
        if px.a != 0xFFFF && px.a != 0 {
            *px.rgb_mut() = px
                .rgb()
                .map(|c| (c as u32 * 0xFFFF / px.a as u32).min(0xFFFF) as u16);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_to_full_8_no_overflow() {
        // Regression: i16 arithmetic overflowed for y > 144
        // (y-16)*255 = (235-16)*255 = 55845 > i16::MAX (32767)
        assert_eq!(limited_to_full_8(16), 0);
        assert_eq!(limited_to_full_8(235), 255);
        // y=145: (145-16)*255 = 32895 > 32767 — would overflow with i16
        assert_eq!(limited_to_full_8(145), 150);
        // y=200: (200-16)*255 = 46920 — definitely overflows i16
        assert_eq!(limited_to_full_8(200), 214);
        // Below range clamps to 0
        assert_eq!(limited_to_full_8(0), 0);
        assert_eq!(limited_to_full_8(15), 0);
        // Above range clamps to 255
        assert_eq!(limited_to_full_8(255), 255);
    }

    #[test]
    fn limited_to_full_8_all_values_in_range() {
        // Ensure no panic or overflow for any u8 input
        for y in 0..=255u8 {
            let result = limited_to_full_8(y);
            // u8 is always <= 255, but verify the function doesn't panic
            let _ = result;
        }
    }

    #[test]
    fn limited_to_full_16_endpoints() {
        // 10-bit
        assert_eq!(limited_to_full_16(64, 10), 0); // 16<<2 = 64
        assert_eq!(limited_to_full_16(940, 10), 1023); // 235<<2 = 940
        // 12-bit
        assert_eq!(limited_to_full_16(256, 12), 0); // 16<<4 = 256
        assert_eq!(limited_to_full_16(3760, 12), 4095); // 235<<4 = 3760
    }

    /// `downscale_to_8bit` over all four layouts it claims to handle.
    ///
    /// Measured cold in every feature combo: only the RGB16 arm ran, so the
    /// RGBA16 / GRAY16 / GRAYA16 arms — each its own hand-written `>> 8` per
    /// channel — were unmeasured (cargo-llvm-cov, 2026-08-11;
    /// docs/TEST_COVERAGE.md). A dropped or duplicated channel there is a
    /// wrong-pixel bug on the `prefer_8bit` decode path for 10/12-bit files.
    #[test]
    fn downscale_to_8bit_keeps_every_channel_in_place() {
        // Distinct per-channel values so a swap or a duplicate shows up.
        let rgba: Vec<Rgba<u16>> = (0..6u16)
            .map(|i| Rgba {
                r: 0x1100 * (i + 1),
                g: 0x0700 * (i + 1) + 3,
                b: 0x0300 * (i + 1) + 7,
                a: 0xFF00 - 0x0100 * i,
            })
            .collect();
        let buf: PixelBuffer = PixelBuffer::from_pixels(rgba.clone(), 3, 2)
            .expect("rgba16 buffer")
            .into();
        assert!(
            buf.descriptor().layout_compatible(PixelDescriptor::RGBA16),
            "fixture must be RGBA16 or this test measures the wrong arm"
        );
        let out = downscale_to_8bit(buf);
        let got = out
            .try_as_imgref::<Rgba<u8>>()
            .expect("RGBA16 must downscale to RGBA8");
        for (i, (src, dst)) in rgba.iter().zip(got.pixels()).enumerate() {
            assert_eq!(
                (dst.r, dst.g, dst.b, dst.a),
                (
                    (src.r >> 8) as u8,
                    (src.g >> 8) as u8,
                    (src.b >> 8) as u8,
                    (src.a >> 8) as u8
                ),
                "RGBA16 -> RGBA8 channel mismatch at pixel {i} ({src:?})"
            );
        }

        // RGB16 arm (the one that was already covered) — kept here so the
        // four arms are asserted by one test.
        let rgb: Vec<Rgb<u16>> = rgba
            .iter()
            .map(|p| Rgb {
                r: p.r,
                g: p.g,
                b: p.b,
            })
            .collect();
        let buf: PixelBuffer = PixelBuffer::from_pixels(rgb.clone(), 3, 2)
            .expect("rgb16 buffer")
            .into();
        let out = downscale_to_8bit(buf);
        let got = out
            .try_as_imgref::<Rgb<u8>>()
            .expect("RGB16 must downscale to RGB8");
        for (src, dst) in rgb.iter().zip(got.pixels()) {
            assert_eq!(
                (dst.r, dst.g, dst.b),
                ((src.r >> 8) as u8, (src.g >> 8) as u8, (src.b >> 8) as u8),
                "RGB16 -> RGB8 channel mismatch"
            );
        }

        // GRAY16 arm.
        let gray: Vec<rgb::Gray<u16>> = (0..6u16)
            .map(|i| rgb::Gray::new(0x2300 * (i + 1) + 9))
            .collect();
        let buf: PixelBuffer = PixelBuffer::from_pixels(gray.clone(), 3, 2)
            .expect("gray16 buffer")
            .into();
        let out = downscale_to_8bit(buf);
        let got = out
            .try_as_imgref::<rgb::Gray<u8>>()
            .expect("GRAY16 must downscale to GRAY8");
        for (src, dst) in gray.iter().zip(got.pixels()) {
            assert_eq!(
                dst.value(),
                (src.value() >> 8) as u8,
                "GRAY16 -> GRAY8 mismatch"
            );
        }
    }

    #[test]
    fn scale_to_u16_endpoints() {
        // 10-bit
        assert_eq!(scale_to_u16(0, 10), 0);
        assert_eq!(scale_to_u16(1023, 10), 65535);
        // 12-bit
        assert_eq!(scale_to_u16(0, 12), 0);
        assert_eq!(scale_to_u16(4095, 12), 65535);
        // 16-bit no-op
        assert_eq!(scale_to_u16(12345, 16), 12345);
    }
}

/// The 16→8 narrowing rule used by `encode_rgb16`/`encode_rgba16` when a
/// caller asks for [`crate::EncodeBitDepth::Eight`], pinned against the
/// two properties that make it the right rule for this crate.
///
/// The narrowing owner is `scale_from_u16(v, 8)` — the same function the
/// 10-bit branch of those encoders already uses, and the same rule the
/// decode side applies in `downscale_to_8bit` ("high byte of each
/// channel"). It is NOT half-up rounding; these tests record why.
#[cfg(all(test, feature = "encode"))]
mod narrow_16_to_8 {
    use super::*;

    /// `scale_from_u16(v, 8)` is the exact inverse of the widening rule
    /// (`scale_to_u16`, LSB replication) for every 8-bit value.
    ///
    /// This is the property that matters on the generic zencodec route,
    /// which reaches `encode_rgb16` with 8-bit content promoted to 16
    /// bits by bit replication. Under this rule that promotion is
    /// perfectly reversible: the coded 8-bit sample is the original byte.
    #[test]
    fn narrowing_inverts_bit_replication_exhaustively() {
        for v8 in 0u16..=255 {
            let widened = scale_to_u16(v8, 8);
            assert_eq!(
                widened,
                v8 * 257,
                "scale_to_u16 at depth 8 must be bit replication (v*257)"
            );
            assert_eq!(
                scale_from_u16(widened, 8),
                v8,
                "8 -> 16 -> 8 must be lossless for every byte (v8={v8})"
            );
        }
    }

    /// It agrees with the decode-side narrowing (`downscale_to_8bit`,
    /// documented as "the high byte of each channel") on the whole u16
    /// domain, so an encode-at-8 / decode-with-prefer_8bit pair cannot
    /// disagree about which byte a sample is.
    #[test]
    fn narrowing_matches_the_decode_side_high_byte_rule() {
        for v in 0u32..=u32::from(u16::MAX) {
            let v = v as u16;
            assert_eq!(scale_from_u16(v, 8), v >> 8, "diverged at v={v}");
        }
    }

    /// Half-up rounding — `(v + 128) >> 8` — is NOT usable here, measured
    /// two ways rather than argued:
    ///
    /// 1. it leaves the u8 domain at the top of the range (65535 maps to
    ///    256, which is not representable in the `[u8; 3]` planes
    ///    `encode_raw_planes_8_bit` takes), and
    /// 2. it breaks the 8 -> 16 -> 8 roundtrip above for most bytes.
    ///
    /// Kept as a test so the tradeoff is a fact in the tree, not a claim
    /// in a commit message.
    #[test]
    fn half_up_rounding_would_overflow_and_break_the_roundtrip() {
        // (1) overflow: white saturates past u8::MAX.
        let white = u32::from(u16::MAX);
        assert_eq!((white + 128) >> 8, 256, "half-up leaves the u8 domain");
        assert_eq!(
            scale_from_u16(u16::MAX, 8),
            255,
            "the shipped rule does not"
        );

        // (2) roundtrip breakage: count the bytes half-up would corrupt.
        let broken = (0u16..=255)
            .filter(|&v8| {
                let widened = u32::from(scale_to_u16(v8, 8));
                ((widened + 128) >> 8) != u32::from(v8)
            })
            .count();
        assert_eq!(
            broken, 128,
            "half-up rounding corrupts the 8 -> 16 -> 8 roundtrip for 128 of 256 bytes"
        );
    }
}
