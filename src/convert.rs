//! Alpha channel handling and premultiply conversion

use crate::error::{Error, Result};
use crate::image::{ColorRange, DecodedImage};
use rgb::Rgba;
use rgb::prelude::*;
use whereat::at;

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

/// Add 8-bit alpha channel to an image from Y plane data
pub fn add_alpha8<'a>(
    img: &mut DecodedImage,
    alpha_rows: impl Iterator<Item = &'a [u8]>,
    width: usize,
    height: usize,
    alpha_range: ColorRange,
    premultiplied: bool,
) -> Result<()> {
    match img {
        DecodedImage::Rgba8(img) => {
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

/// Add 16-bit alpha channel to an image from Y plane data
pub fn add_alpha16<'a>(
    img: &mut DecodedImage,
    alpha_rows: impl Iterator<Item = &'a [u16]>,
    width: usize,
    height: usize,
    alpha_range: ColorRange,
    bit_depth: u8,
    premultiplied: bool,
) -> Result<()> {
    match img {
        DecodedImage::Rgba16(img) => {
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
                        ColorRange::Limited => limited_to_full_16(y, bit_depth),
                    };
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
