//! Retirement gate for zenavif's `src/yuv_bilinear_fix.rs`.
//!
//! Two rounds, one binary:
//!   MODE=wrapper  — run the converter through a verbatim copy of zenavif's
//!                   `yuv420_bilinear_complete`, as `src/decoder.rs` does today.
//!   MODE=direct   — call the converter straight, as retirement would.
//!
//! Run round A on the pinned yuv (0.8.15) with MODE=wrapper, round B on the
//! candidate (0.8.17) with MODE=direct, and diff the TSVs. Identical
//! fingerprints across every cell mean the wrapper can be deleted without
//! moving a single output byte.
//!
//! Covers all EIGHT converter variants used at the eight call sites in
//! `src/decoder.rs`, crossed with geometry (both height parities, tiny→large),
//! range, matrix and content class.

use std::fmt::Write as _;
use yuv::{YuvError, YuvPlanarImage, YuvRange, YuvStandardMatrix};

// ---------------------------------------------------------------------------
// Verbatim copy of zenavif's wrapper (src/yuv_bilinear_fix.rs), so round A
// reproduces today's shipped behaviour exactly.
// ---------------------------------------------------------------------------
fn yuv420_bilinear_complete<T: Copy + Default + core::fmt::Debug>(
    planar: &YuvPlanarImage<'_, T>,
    out: &mut [T],
    out_stride: u32,
    channels: usize,
    f: impl Fn(&YuvPlanarImage<'_, T>, &mut [T], u32) -> Result<(), YuvError>,
) -> Result<(), YuvError> {
    f(planar, out, out_stride)?;

    let w = planar.width as usize;
    let h = planar.height as usize;
    if h < 2 || h % 2 != 0 {
        return Ok(());
    }

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

// ---------------------------------------------------------------------------

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn fnv_u8(v: &[u8]) -> u64 {
    fnv1a64(v)
}
fn fnv_u16(v: &[u16]) -> u64 {
    let mut bytes = Vec::with_capacity(v.len() * 2);
    for &x in v {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    fnv1a64(&bytes)
}

/// Deterministic, seed-varied content. `max` bounds the sample range so each
/// bit depth gets values in its real domain.
fn planes(w: usize, h: usize, content: &str, max: u32, seed: u64) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let mut y = vec![0u32; w * h];
    let mut u = vec![0u32; cw * ch];
    let mut v = vec![0u32; cw * ch];
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    match content {
        "gradient" => {
            for yy in 0..h {
                for xx in 0..w {
                    y[yy * w + xx] = (((xx + yy) as u64 * max as u64) / (w + h) as u64) as u32;
                }
            }
            for i in 0..u.len() {
                u[i] = max / 2;
                v[i] = max / 2;
            }
        }
        "noise" => {
            for e in y.iter_mut() {
                *e = (next() % (max as u64 + 1)) as u32;
            }
            for e in u.iter_mut() {
                *e = (next() % (max as u64 + 1)) as u32;
            }
            for e in v.iter_mut() {
                *e = (next() % (max as u64 + 1)) as u32;
            }
        }
        // Sharp vertical chroma edges — the class most sensitive to how the
        // bottom row pair's chroma is clamped.
        _ => {
            for yy in 0..h {
                for xx in 0..w {
                    y[yy * w + xx] = if (xx / 3) % 2 == 0 { max } else { 0 };
                }
            }
            for yy in 0..ch {
                for xx in 0..cw {
                    u[yy * cw + xx] = if yy % 2 == 0 { max } else { 0 };
                    v[yy * cw + xx] = if xx % 2 == 0 { 0 } else { max };
                }
            }
        }
    }
    (y, u, v)
}

macro_rules! cell8 {
    ($rows:expr, $wrapper:expr, $name:literal, $conv:path, $ch:expr,
     $w:expr, $h:expr, $content:expr, $range:expr, $matrix:expr, $seed:expr) => {{
        let (w, h) = ($w, $h);
        let (y32, u32v, v32v) = planes(w, h, $content, 255, $seed);
        let y: Vec<u8> = y32.iter().map(|&x| x as u8).collect();
        let u: Vec<u8> = u32v.iter().map(|&x| x as u8).collect();
        let v: Vec<u8> = v32v.iter().map(|&x| x as u8).collect();
        let cw = w.div_ceil(2);
        let planar = YuvPlanarImage {
            y_plane: &y,
            y_stride: w as u32,
            u_plane: &u,
            u_stride: cw as u32,
            v_plane: &v,
            v_stride: cw as u32,
            width: w as u32,
            height: h as u32,
        };
        let stride = (w * $ch) as u32;
        let mut out = vec![0u8; w * h * $ch];
        let call = |p: &YuvPlanarImage<'_, u8>, o: &mut [u8], s: u32| $conv(p, o, s, $range, $matrix);
        let r = if $wrapper {
            yuv420_bilinear_complete(&planar, &mut out, stride, $ch, call)
        } else {
            call(&planar, &mut out, stride)
        };
        let fp = match r {
            Ok(()) => format!("{:016x}", fnv_u8(&out)),
            Err(e) => format!("ERR:{e:?}"),
        };
        // Is the final row actually written? This is what the retirement rests on.
        let last_nonzero = out[(h - 1) * (w * $ch)..].iter().any(|&b| b != 0);
        $rows.push(format!(
            "{}\t{}\t{}\t{}\t{:?}\t{:?}\t{}\t{}",
            $name, w, h, $content, $range, $matrix, fp, last_nonzero
        ));
    }};
}

macro_rules! cell16 {
    ($rows:expr, $wrapper:expr, $name:literal, $conv:path, $ch:expr, $max:expr,
     $w:expr, $h:expr, $content:expr, $range:expr, $matrix:expr, $seed:expr) => {{
        let (w, h) = ($w, $h);
        let (y32, u32v, v32v) = planes(w, h, $content, $max, $seed);
        let y: Vec<u16> = y32.iter().map(|&x| x as u16).collect();
        let u: Vec<u16> = u32v.iter().map(|&x| x as u16).collect();
        let v: Vec<u16> = v32v.iter().map(|&x| x as u16).collect();
        let cw = w.div_ceil(2);
        let planar = YuvPlanarImage {
            y_plane: &y,
            y_stride: w as u32,
            u_plane: &u,
            u_stride: cw as u32,
            v_plane: &v,
            v_stride: cw as u32,
            width: w as u32,
            height: h as u32,
        };
        let stride = (w * $ch) as u32;
        let mut out = vec![0u16; w * h * $ch];
        let call =
            |p: &YuvPlanarImage<'_, u16>, o: &mut [u16], s: u32| $conv(p, o, s, $range, $matrix);
        let r = if $wrapper {
            yuv420_bilinear_complete(&planar, &mut out, stride, $ch, call)
        } else {
            call(&planar, &mut out, stride)
        };
        let fp = match r {
            Ok(()) => format!("{:016x}", fnv_u16(&out)),
            Err(e) => format!("ERR:{e:?}"),
        };
        let last_nonzero = out[(h - 1) * (w * $ch)..].iter().any(|&b| b != 0);
        $rows.push(format!(
            "{}\t{}\t{}\t{}\t{:?}\t{:?}\t{}\t{}",
            $name, w, h, $content, $range, $matrix, fp, last_nonzero
        ));
    }};
}

fn main() {
    let wrapper = std::env::var("MODE").unwrap_or_default() == "wrapper";
    eprintln!(
        "mode={} yuv={}",
        if wrapper { "wrapper" } else { "direct" },
        env!("CARGO_PKG_VERSION")
    );

    // Both height parities, tiny through large, odd and even widths.
    let geoms: &[(usize, usize)] = &[
        (2, 2),
        (4, 4),
        (64, 2),
        (66, 4),
        (63, 64),
        (64, 64),
        (65, 64),
        (64, 65),
        (65, 65),
        (256, 256),
        (257, 256),
        (256, 257),
        (320, 240),
        (1024, 1024),
        (1920, 1080),
        (2048, 2048),
    ];
    let ranges = [YuvRange::Full, YuvRange::Limited];
    let mats = [
        YuvStandardMatrix::Bt601,
        YuvStandardMatrix::Bt709,
        YuvStandardMatrix::Bt2020,
    ];
    let contents = ["gradient", "noise", "edges"];

    let mut rows: Vec<String> = Vec::new();
    let mut seed = 1u64;
    for &(w, h) in geoms {
        for &r in &ranges {
            for &m in &mats {
                for &c in &contents {
                    seed += 1;
                    // 8-bit: the two call sites in decoder.rs:1056 / :1093
                    cell8!(rows, wrapper, "yuv420_to_rgba", yuv::yuv420_to_rgba_bilinear, 4, w, h, c, r, m, seed);
                    cell8!(rows, wrapper, "yuv420_to_rgb", yuv::yuv420_to_rgb_bilinear, 3, w, h, c, r, m, seed);
                    // 10/12/16-bit RGBA: decoder.rs:1272 / :1279 / :1286
                    cell16!(rows, wrapper, "i010_to_rgba10", yuv::i010_to_rgba10_bilinear, 4, 1023, w, h, c, r, m, seed);
                    cell16!(rows, wrapper, "i012_to_rgba12", yuv::i012_to_rgba12_bilinear, 4, 4095, w, h, c, r, m, seed);
                    cell16!(rows, wrapper, "i016_to_rgba16", yuv::i016_to_rgba16_bilinear, 4, 65535, w, h, c, r, m, seed);
                    // 10/12/16-bit RGB: decoder.rs:1358 / :1365 / :1372
                    cell16!(rows, wrapper, "i010_to_rgb10", yuv::i010_to_rgb10_bilinear, 3, 1023, w, h, c, r, m, seed);
                    cell16!(rows, wrapper, "i012_to_rgb12", yuv::i012_to_rgb12_bilinear, 3, 4095, w, h, c, r, m, seed);
                    cell16!(rows, wrapper, "i016_to_rgb16", yuv::i016_to_rgb16_bilinear, 3, 65535, w, h, c, r, m, seed);
                }
            }
        }
    }

    let mut out = String::new();
    let _ = writeln!(out, "converter\twidth\theight\tcontent\trange\tmatrix\tfingerprint\tlast_row_written");
    for r in &rows {
        let _ = writeln!(out, "{r}");
    }
    print!("{out}");
    eprintln!("cells: {}", rows.len());
}
