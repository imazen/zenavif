//! Alpha channel handling, premultiply conversion, and bit depth scaling

use crate::error::{Error, Result};
use crate::image::ColorRange;
use rgb::prelude::*;
use rgb::{Rgb, Rgba};
use whereat::at;
use zencodec_types::PixelData;

/// Scale a limited-range Y value to full range (8-bit)
#[inline]
fn limited_to_full_8(y: u8) -> u8 {
    // Limited range: Y ∈ [16, 235]
    // Full range: Y ∈ [0, 255]
    let y = y as i16;
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

/// Scale all channels in a 16-bit PixelData from native bit depth to full u16 range.
///
/// This converts e.g. 10-bit values (0–1023) to full 16-bit (0–65535) using
/// LSB replication for exact endpoint mapping.
pub fn scale_pixels_to_u16(image: &mut PixelData, bit_depth: u8) {
    if bit_depth >= 16 {
        return;
    }
    match image {
        PixelData::Rgb16(img) => {
            for px in img.buf_mut().iter_mut() {
                *px = Rgb {
                    r: scale_to_u16(px.r, bit_depth),
                    g: scale_to_u16(px.g, bit_depth),
                    b: scale_to_u16(px.b, bit_depth),
                };
            }
        }
        PixelData::Rgba16(img) => {
            for px in img.buf_mut().iter_mut() {
                *px = Rgba {
                    r: scale_to_u16(px.r, bit_depth),
                    g: scale_to_u16(px.g, bit_depth),
                    b: scale_to_u16(px.b, bit_depth),
                    a: scale_to_u16(px.a, bit_depth),
                };
            }
        }
        _ => {}
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
#[inline]
pub fn scale_from_u16(v: u16, bit_depth: u8) -> u16 {
    let shift = 16 - bit_depth;
    if shift == 0 {
        return v;
    }
    v >> shift
}

/// Add 8-bit alpha channel to an image from Y plane data
pub fn add_alpha8<'a>(
    img: &mut PixelData,
    alpha_rows: impl Iterator<Item = &'a [u8]>,
    width: usize,
    height: usize,
    alpha_range: ColorRange,
    premultiplied: bool,
) -> Result<()> {
    match img {
        PixelData::Rgba8(img) => {
            if img.width() != width || img.height() != height {
                return Err(at(Error::Unsupported("alpha size mismatch")));
            }

            for (alpha_row, img_row) in alpha_rows.zip(img.rows_mut()) {
                if alpha_row.len() < img_row.len() {
                    return Err(at(Error::Unsupported("alpha width mismatch")));
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
        }
        _ => {
            return Err(at(Error::Unsupported(
                "cannot add 8-bit alpha to this image type",
            )));
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
    img: &mut PixelData,
    alpha_rows: impl Iterator<Item = &'a [u16]>,
    width: usize,
    height: usize,
    alpha_range: ColorRange,
    bit_depth: u8,
    premultiplied: bool,
) -> Result<()> {
    match img {
        PixelData::Rgba16(img) => {
            if img.width() != width || img.height() != height {
                return Err(at(Error::Unsupported("alpha size mismatch")));
            }

            for (alpha_row, img_row) in alpha_rows.zip(img.rows_mut()) {
                if alpha_row.len() < img_row.len() {
                    return Err(at(Error::Unsupported("alpha width mismatch")));
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
        }
        _ => {
            return Err(at(Error::Unsupported(
                "cannot add 16-bit alpha to this image type",
            )));
        }
    }

    Ok(())
}

/// Convert premultiplied alpha to straight alpha for 8-bit RGBA
#[inline(never)]
pub fn unpremultiply8(img_row: &mut [Rgba<u8>]) {
    for px in img_row.iter_mut() {
        if px.a != 255 && px.a != 0 {
            *px.rgb_mut() = px
                .rgb()
                .map(|c| (c as u16 * 255 / px.a as u16).min(255) as u8);
        }
    }
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
