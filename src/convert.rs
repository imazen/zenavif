//! YUV to RGB conversion utilities and alpha channel handling

use crate::error::{Error, Result};
use crate::image::DecodedImage;
use rgb::Rgba;
use rgb::prelude::*;
use whereat::at;
use yuv::convert::RGBConvert;

/// Add 8-bit alpha channel to an image
pub fn add_alpha8<'a>(
    img: &mut DecodedImage,
    alpha_rows: impl Iterator<Item = &'a [u8]>,
    width: usize,
    height: usize,
    conv: RGBConvert<u8>,
    premultiplied: bool,
) -> Result<()> {
    if let RGBConvert::Matrix(_) = conv {
        return Err(at(Error::Unsupported("alpha image has color matrix")));
    }

    match img {
        DecodedImage::Rgba8(img) => {
            if img.width() != width || img.height() != height {
                return Err(at(Error::Unsupported("alpha size mismatch")));
            }

            for (alpha_row, img_row) in alpha_rows.zip(img.rows_mut()) {
                if alpha_row.len() != img_row.len() {
                    return Err(at(Error::Unsupported("alpha width mismatch")));
                }
                for (y, px) in alpha_row.iter().copied().zip(img_row.iter_mut()) {
                    px.a = conv.to_luma(y);
                }
                if premultiplied {
                    unpremultiply8(img_row);
                }
            }
        }
        DecodedImage::Rgba16(img) => {
            if img.width() != width || img.height() != height {
                return Err(at(Error::Unsupported("alpha size mismatch")));
            }

            for (alpha_row, img_row) in alpha_rows.zip(img.rows_mut()) {
                if alpha_row.len() != img_row.len() {
                    return Err(at(Error::Unsupported("alpha width mismatch")));
                }
                for (y, px) in alpha_row.iter().copied().zip(img_row.iter_mut()) {
                    let g = u16::from(conv.to_luma(y));
                    px.a = (g << 8) | g;
                }
                if premultiplied {
                    unpremultiply16(img_row);
                }
            }
        }
        _ => {
            return Err(at(Error::Unsupported(
                "cannot add alpha to this image type",
            )));
        }
    }

    Ok(())
}

/// Add 16-bit alpha channel to an image
pub fn add_alpha16<'a>(
    img: &mut DecodedImage,
    alpha_rows: impl Iterator<Item = &'a [u16]>,
    width: usize,
    height: usize,
    conv: RGBConvert<u16>,
    premultiplied: bool,
) -> Result<()> {
    if let RGBConvert::Matrix(_) = conv {
        return Err(at(Error::Unsupported("alpha image has color matrix")));
    }

    match img {
        DecodedImage::Rgba8(img) => {
            if img.width() != width || img.height() != height {
                return Err(at(Error::Unsupported("alpha size mismatch")));
            }

            for (alpha_row, img_row) in alpha_rows.zip(img.rows_mut()) {
                if alpha_row.len() != img_row.len() {
                    return Err(at(Error::Unsupported("alpha width mismatch")));
                }
                for (y, px) in alpha_row.iter().copied().zip(img_row.iter_mut()) {
                    px.a = (conv.to_luma(y) >> 8) as u8;
                }
                if premultiplied {
                    unpremultiply8(img_row);
                }
            }
        }
        DecodedImage::Rgba16(img) => {
            if img.width() != width || img.height() != height {
                return Err(at(Error::Unsupported("alpha size mismatch")));
            }

            for (alpha_row, img_row) in alpha_rows.zip(img.rows_mut()) {
                if alpha_row.len() != img_row.len() {
                    return Err(at(Error::Unsupported("alpha width mismatch")));
                }
                for (y, px) in alpha_row.iter().copied().zip(img_row.iter_mut()) {
                    px.a = conv.to_luma(y);
                }
                if premultiplied {
                    unpremultiply16(img_row);
                }
            }
        }
        _ => {
            return Err(at(Error::Unsupported(
                "cannot add alpha to this image type",
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
