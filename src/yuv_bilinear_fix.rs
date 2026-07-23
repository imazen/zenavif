//! Completion wrapper for the `yuv` crate's 4:2:0 bilinear converters.
//!
//! Upstream defect (verified against yuv 0.8.12 and 0.8.16, see the
//! reproduction in this module's tests): every `yuv420_*_bilinear` /
//! `i0xx_*_bilinear` function pairs luma row-pairs with overlapping chroma
//! row windows (`windows(u_stride * 2).step_by(u_stride)`). For an
//! even-height image whose chroma planes are exactly `ceil(h/2)` rows, that
//! iterator yields `ceil(h/2) - 1` windows for `h/2` luma pairs — the zip
//! silently drops the **last luma row pair**, leaving the bottom two output
//! rows unwritten (black/transparent). Odd heights are unaffected (the tail
//! row has a dedicated clamp path), as are the 4:2:2 variants (horizontal
//! interpolation only).
//!
//! [`yuv420_bilinear_complete`] runs the underlying converter, then repairs
//! the missing pair by re-running it on a tight 2-row sub-image whose
//! chroma is the last chroma row duplicated — the same edge-clamp semantics
//! the crate itself applies to odd-height tails. Rows the crate already
//! writes are byte-identical to an unwrapped call.

use yuv::{YuvError, YuvPlanarImage};

/// Run a `yuv` crate 4:2:0 bilinear converter and complete the bottom two
/// rows it drops on even-height input.
///
/// * `out` / `out_stride` — destination in `T` units (`u8` samples for the
///   8-bit converters, `u16` for the 10/12/16-bit ones), matching what `f`
///   itself expects.
/// * `channels` — output components per pixel (3 for RGB, 4 for RGBA).
/// * `f` — the underlying converter, e.g.
///   `|p, o, s| yuv::yuv420_to_rgba_bilinear(p, o, s, range, matrix)`.
#[cfg_attr(not(feature = "unsafe-asm"), allow(dead_code))]
pub(crate) fn yuv420_bilinear_complete<T: Copy + Default + core::fmt::Debug>(
    planar: &YuvPlanarImage<'_, T>,
    out: &mut [T],
    out_stride: u32,
    channels: usize,
    f: impl Fn(&YuvPlanarImage<'_, T>, &mut [T], u32) -> Result<(), YuvError>,
) -> Result<(), YuvError> {
    f(planar, out, out_stride)?;

    let w = planar.width as usize;
    let h = planar.height as usize;
    if h < 2 || !h.is_multiple_of(2) {
        // Odd heights (and degenerate ones) are fully written upstream.
        return Ok(());
    }

    // Tight copies of the last two luma rows and the (duplicated) last
    // chroma row — tight so the repair call is immune to however the
    // caller's planes are padded or trimmed.
    let ys = planar.y_stride as usize;
    let us = planar.u_stride as usize;
    let vs = planar.v_stride as usize;
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);

    let mut y2 = vec![T::default(); 2 * w];
    y2[..w].copy_from_slice(&planar.y_plane[(h - 2) * ys..][..w]);
    y2[w..].copy_from_slice(&planar.y_plane[(h - 1) * ys..][..w]);
    let mut u2 = vec![T::default(); 2 * cw];
    u2[..cw].copy_from_slice(&planar.u_plane[(ch - 1) * us..][..cw]);
    u2.copy_within(..cw, cw);
    let mut v2 = vec![T::default(); 2 * cw];
    v2[..cw].copy_from_slice(&planar.v_plane[(ch - 1) * vs..][..cw]);
    v2.copy_within(..cw, cw);

    let sub = YuvPlanarImage {
        y_plane: &y2,
        y_stride: w as u32,
        u_plane: &u2,
        u_stride: cw as u32,
        v_plane: &v2,
        v_stride: cw as u32,
        width: planar.width,
        height: 2,
    };
    let row_units = w * channels;
    let mut tail = vec![T::default(); 2 * row_units];
    f(&sub, &mut tail, row_units as u32)?;

    let stride = out_stride as usize;
    out[(h - 2) * stride..][..row_units].copy_from_slice(&tail[..row_units]);
    out[(h - 1) * stride..][..row_units].copy_from_slice(&tail[row_units..]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 4:2:0 planar gradient with flat mid chroma.
    fn gradient_planar(w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut y = vec![0u8; w * h];
        for yy in 0..h {
            for xx in 0..w {
                y[yy * w + xx] = (((xx + yy) * 255) / (w + h)) as u8;
            }
        }
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);
        (y, vec![128u8; cw * ch], vec![128u8; cw * ch])
    }

    /// The upstream defect this module exists for: without the wrapper, the
    /// bottom two rows of an even-height 4:2:0 bilinear conversion stay
    /// unwritten. Guards the reproduction so a fixed upstream is noticed.
    #[test]
    fn upstream_drops_last_row_pair_on_even_height() {
        let (y, u, v) = gradient_planar(64, 64);
        let planar = YuvPlanarImage {
            y_plane: &y,
            y_stride: 64,
            u_plane: &u,
            u_stride: 32,
            v_plane: &v,
            v_stride: 32,
            width: 64,
            height: 64,
        };
        let mut out = vec![0u8; 64 * 64 * 4];
        yuv::yuv420_to_rgba_bilinear(
            &planar,
            &mut out,
            64 * 4,
            yuv::YuvRange::Full,
            yuv::YuvStandardMatrix::Bt601,
        )
        .unwrap();
        let last = &out[63 * 64 * 4..];
        assert!(
            last.iter().all(|&b| b == 0),
            "upstream yuv fixed the dropped-last-pair bug — this wrapper \
             (and its call sites) can be retired"
        );
    }

    #[test]
    fn complete_writes_every_row_even_height() {
        for (w, h) in [(64usize, 64usize), (128, 128), (64, 2), (66, 4)] {
            let (y, u, v) = gradient_planar(w, h);
            let planar = YuvPlanarImage {
                y_plane: &y,
                y_stride: w as u32,
                u_plane: &u,
                u_stride: w.div_ceil(2) as u32,
                v_plane: &v,
                v_stride: w.div_ceil(2) as u32,
                width: w as u32,
                height: h as u32,
            };
            let mut out = vec![0u8; w * h * 4];
            yuv420_bilinear_complete(&planar, &mut out, (w * 4) as u32, 4, |p, o, s| {
                yuv::yuv420_to_rgba_bilinear(
                    p,
                    o,
                    s,
                    yuv::YuvRange::Full,
                    yuv::YuvStandardMatrix::Bt601,
                )
            })
            .unwrap();
            for yy in 0..h {
                for xx in 0..w {
                    let i = (yy * w + xx) * 4;
                    let expect = ((xx + yy) * 255 / (w + h)) as i32;
                    let got = out[i] as i32;
                    assert!(
                        (got - expect).abs() <= 2,
                        "{w}x{h} px ({xx},{yy}): luma {expect} decoded {got}"
                    );
                    assert_eq!(out[i + 3], 255, "{w}x{h} alpha unwritten at ({xx},{yy})");
                }
            }
        }
    }

    /// Rows the crate already writes must be untouched by the wrapper.
    #[test]
    fn complete_is_byte_identical_on_written_rows() {
        let (y, u, v) = gradient_planar(64, 64);
        let planar = YuvPlanarImage {
            y_plane: &y,
            y_stride: 64,
            u_plane: &u,
            u_stride: 32,
            v_plane: &v,
            v_stride: 32,
            width: 64,
            height: 64,
        };
        let call = |p: &YuvPlanarImage<'_, u8>, o: &mut [u8], s: u32| {
            yuv::yuv420_to_rgb_bilinear(p, o, s, yuv::YuvRange::Full, yuv::YuvStandardMatrix::Bt601)
        };
        let mut plain = vec![0u8; 64 * 64 * 3];
        call(&planar, &mut plain, 64 * 3).unwrap();
        let mut fixed = vec![0u8; 64 * 64 * 3];
        yuv420_bilinear_complete(&planar, &mut fixed, 64 * 3, 3, call).unwrap();
        assert_eq!(plain[..62 * 64 * 3], fixed[..62 * 64 * 3]);
        assert!(fixed[63 * 64 * 3..].iter().any(|&b| b != 0));
    }

    /// Odd heights are upstream-complete; the wrapper must not disturb them.
    #[test]
    fn odd_height_passthrough() {
        let (y, u, v) = gradient_planar(64, 63);
        let planar = YuvPlanarImage {
            y_plane: &y,
            y_stride: 64,
            u_plane: &u,
            u_stride: 32,
            v_plane: &v,
            v_stride: 32,
            width: 64,
            height: 63,
        };
        let call = |p: &YuvPlanarImage<'_, u8>, o: &mut [u8], s: u32| {
            yuv::yuv420_to_rgb_bilinear(p, o, s, yuv::YuvRange::Full, yuv::YuvStandardMatrix::Bt601)
        };
        let mut plain = vec![0u8; 64 * 63 * 3];
        call(&planar, &mut plain, 64 * 3).unwrap();
        let mut fixed = vec![0u8; 64 * 63 * 3];
        yuv420_bilinear_complete(&planar, &mut fixed, 64 * 3, 3, call).unwrap();
        assert_eq!(plain, fixed);
    }

    /// 16-bit (i010) variant goes through the same generic wrapper.
    #[test]
    fn complete_i010_writes_last_rows() {
        let w = 64usize;
        let h = 64usize;
        let mut y = vec![0u16; w * h];
        for yy in 0..h {
            for xx in 0..w {
                y[yy * w + xx] = (((xx + yy) * 1023) / (w + h)) as u16;
            }
        }
        let u = vec![512u16; (w / 2) * (h / 2)];
        let v = vec![512u16; (w / 2) * (h / 2)];
        let planar = YuvPlanarImage {
            y_plane: &y,
            y_stride: w as u32,
            u_plane: &u,
            u_stride: (w / 2) as u32,
            v_plane: &v,
            v_stride: (w / 2) as u32,
            width: w as u32,
            height: h as u32,
        };
        let mut out = vec![0u16; w * h * 3];
        yuv420_bilinear_complete(&planar, &mut out, (w * 3) as u32, 3, |p, o, s| {
            yuv::i010_to_rgb10_bilinear(p, o, s, yuv::YuvRange::Full, yuv::YuvStandardMatrix::Bt601)
        })
        .unwrap();
        let i = (63 * w + 63) * 3;
        assert!(out[i] > 900, "last pixel unwritten: {}", out[i]);
    }
}
