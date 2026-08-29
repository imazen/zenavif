//! `rd_tool` — the shared input/output half of the matched-wall-clock encode
//! RD harness (`scripts/encode_rd/`).
//!
//! The harness compares four AV1 encoders. For that comparison to mean
//! anything, everything *around* the encoder has to be byte-identical across
//! arms. This tool is that "everything":
//!
//! * `prep` — one source PNG becomes ONE canonical `src.y4m` (8-bit I420) plus
//!   the `ref.png` it is scored against. Every encoder reads the same y4m
//!   bytes, so no arm can win or lose on its private RGB->YUV.
//! * `decode` — one decoder (rav1d-safe, single-threaded) turns any arm's
//!   bitstream (IVF / bare OBU / AVIF) back into RGB, through one fixed
//!   YUV->RGB. No arm gets a friendlier decoder.
//! * `floor` — YUV->RGB of the *unencoded* `src.y4m`. This is the 4:2:0
//!   round-trip ceiling: no encoder can score above it, and reporting it is
//!   what makes the absolute numbers readable.
//!
//! ## The colour pair is FIXED, MATCHED and deliberately unfancy
//!
//! Forward is BT.601 limited-range integer RGB->I420 with a 2x2 box average
//! for chroma; inverse is the matching BT.601 limited-range integer
//! I420->RGB with *nearest* (box replication) chroma upsampling. Nearest is
//! chosen precisely because it is the inverse of the box average: a flat 2x2
//! block round-trips exactly, so the pair adds no directional blur. A
//! bilinear upsampler would look nicer and would NOT be the inverse.
//!
//! The exact coefficients do not need to match any particular spec — they
//! need to be fixed, and applied identically to every arm and to the floor.
//! They are byte-identical to the transform in zenav1-svt's
//! `svtav1/examples/identity_run.rs`, so that harness's `.yuv` and this
//! one's agree for the same PNG (`run_grid.py --verify-yuv` asserts it).
//!
//! Consequence to keep in mind when reading scores: absolute ssim2 is
//! depressed relative to a fancy-chroma pipeline, identically for all arms.
//! Compare arms to each other and to `floor`, never to numbers from another
//! harness.
//!
//! ## Usage
//!
//! ```text
//! rd_tool prep <src.png> <outdir> [max_dim]   # -> ref.png, src.y4m, src.yuv; prints "w h"
//! rd_tool floor <outdir>                      # -> floor.png (the 4:2:0 ceiling)
//! rd_tool decode <bitstream> <out.png>        # IVF | bare OBU | AVIF -> RGB PNG
//! ```
//!
//! `prep` downscales (Lanczos3) to `max_dim` on the long edge when the source
//! is larger, and NEVER upscales — a synthetic upscale has no high-frequency
//! detail and would misrepresent every encoder's intra tooling. Dimensions
//! are cropped to even; 4:2:0 with odd dims is well defined but adds an edge
//! class this harness has no reason to carry.

use enough::Unstoppable;
use rav1d_safe::src::managed::{Decoder, Frame, PixelLayout, Planes, Settings};
use std::io::Write;
use std::process::ExitCode;

// ---------------------------------------------------------------- colour --

fn clip8(x: i32) -> u8 {
    x.clamp(0, 255) as u8
}

/// Fixed BT.601 limited-range integer RGB -> I420, chroma by 2x2 box average.
/// Byte-identical to zenav1-svt `identity_run::rgb_to_i420_bt601`.
fn rgb_to_i420(rgb: &[u8], w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            let i = (r * w + c) * 3;
            let (rr, gg, bb) = (rgb[i] as i32, rgb[i + 1] as i32, rgb[i + 2] as i32);
            y[r * w + c] = clip8(((66 * rr + 129 * gg + 25 * bb + 128) >> 8) + 16);
        }
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    for cr in 0..ch {
        for cc in 0..cw {
            let (mut sr, mut sg, mut sb, mut n) = (0i32, 0i32, 0i32, 0i32);
            for dr in 0..2 {
                for dc in 0..2 {
                    let (sy, sx) = (cr * 2 + dr, cc * 2 + dc);
                    if sy >= h || sx >= w {
                        continue;
                    }
                    let i = (sy * w + sx) * 3;
                    sr += rgb[i] as i32;
                    sg += rgb[i + 1] as i32;
                    sb += rgb[i + 2] as i32;
                    n += 1;
                }
            }
            let half = n / 2;
            let (rr, gg, bb) = ((sr + half) / n, (sg + half) / n, (sb + half) / n);
            u[cr * cw + cc] = clip8(((-38 * rr - 74 * gg + 112 * bb + 128) >> 8) + 128);
            v[cr * cw + cc] = clip8(((112 * rr - 94 * gg - 18 * bb + 128) >> 8) + 128);
        }
    }
    (y, u, v)
}

/// The matching inverse: BT.601 limited-range integer I420 -> RGB, chroma by
/// nearest (box replication). `cw`/`ch` are the chroma plane dims; pass
/// `(w, h)` for 4:4:4 and `(ceil(w/2), h)` for 4:2:2 — the sample lookup
/// scales by the plane ratio, so all three layouts share this one path.
fn yuv_to_rgb(y: &[u8], u: &[u8], v: &[u8], w: usize, h: usize, cw: usize, ch: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 3];
    let mono = cw == 0 || ch == 0;
    for r in 0..h {
        // Integer-floor mapping; identical to replicating each chroma sample
        // across the luma block it covers.
        let cr = if mono { 0 } else { (r * ch / h).min(ch - 1) };
        for c in 0..w {
            let cc = if mono { 0 } else { (c * cw / w).min(cw - 1) };
            let yy = 298 * (y[r * w + c] as i32 - 16);
            let (uu, vv) = if mono {
                (0, 0)
            } else {
                (u[cr * cw + cc] as i32 - 128, v[cr * cw + cc] as i32 - 128)
            };
            let o = (r * w + c) * 3;
            out[o] = clip8((yy + 409 * vv + 128) >> 8);
            out[o + 1] = clip8((yy - 100 * uu - 208 * vv + 128) >> 8);
            out[o + 2] = clip8((yy + 516 * uu + 128) >> 8);
        }
    }
    out
}

// ------------------------------------------------------------------- io --

fn write_y4m(path: &str, y: &[u8], u: &[u8], v: &[u8], w: usize, h: usize) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    // F25:1 / Ip / A1:1 are inert for a single still frame; C420jpeg matches
    // the tag zenav1-svt's harness uses.
    writeln!(f, "YUV4MPEG2 W{w} H{h} F25:1 Ip A1:1 C420jpeg")?;
    f.write_all(b"FRAME\n")?;
    f.write_all(y)?;
    f.write_all(u)?;
    f.write_all(v)?;
    f.flush()
}

fn save_png(path: &str, rgb: &[u8], w: usize, h: usize) {
    image::save_buffer(path, rgb, w as u32, h as u32, image::ColorType::Rgb8)
        .unwrap_or_else(|e| panic!("write {path}: {e}"));
}

fn read_y4m(path: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>, usize, usize) {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let nl = data
        .iter()
        .position(|&b| b == b'\n')
        .expect("y4m: no header newline");
    let hdr = std::str::from_utf8(&data[..nl]).expect("y4m: non-utf8 header");
    let mut w = 0usize;
    let mut h = 0usize;
    for tok in hdr.split_whitespace() {
        match tok.as_bytes().first() {
            Some(b'W') => w = tok[1..].parse().expect("y4m W"),
            Some(b'H') => h = tok[1..].parse().expect("y4m H"),
            _ => {}
        }
    }
    assert!(w > 0 && h > 0, "y4m: missing W/H in {hdr:?}");
    // Skip the "FRAME...\n" marker.
    let fstart = nl + 1;
    let fnl = fstart
        + data[fstart..]
            .iter()
            .position(|&b| b == b'\n')
            .expect("y4m: no FRAME newline");
    let p = fnl + 1;
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let ysz = w * h;
    let csz = cw * ch;
    assert!(data.len() >= p + ysz + 2 * csz, "y4m: truncated frame");
    (
        data[p..p + ysz].to_vec(),
        data[p + ysz..p + ysz + csz].to_vec(),
        data[p + ysz + csz..p + ysz + 2 * csz].to_vec(),
        w,
        h,
    )
}

// ---------------------------------------------------------------- decode --

/// Feed a bitstream to rav1d-safe and return the last frame. Accepts IVF
/// (`DKIF`), a bare OBU stream, or an AVIF file (primary item is extracted
/// first, so an AVIF arm is decoded by the SAME decoder as every other arm).
fn decode_bitstream(data: &[u8]) -> Result<Frame, String> {
    // AVIF? `ftyp` box at offset 4.
    let payload: Vec<u8> = if data.len() > 12 && &data[4..8] == b"ftyp" {
        // Lenient on purpose: a corpus file with a container quirk should still
        // yield a measurement cell rather than dropping out of the sweep. Production
        // decode is strict -- see tests/parser_leniency_scope.rs.
        let cfg = zenavif_parse::DecodeConfig::default().lenient(true);
        let parser =
            zenavif_parse::AvifParser::from_owned_with_config(data.to_vec(), &cfg, &Unstoppable)
                .map_err(|e| format!("avif parse: {e}"))?;
        parser
            .primary_data()
            .map_err(|e| format!("avif primary: {e}"))?
            .as_ref()
            .to_vec()
    } else {
        data.to_vec()
    };

    let mut settings = Settings::default();
    settings.threads = 1;
    let mut dec = Decoder::with_settings(settings).map_err(|e| format!("decoder init: {e:?}"))?;
    let mut frames: Vec<Frame> = Vec::new();
    let mut feed = |p: &[u8], frames: &mut Vec<Frame>| -> Result<(), String> {
        match dec.decode(p) {
            Ok(Some(f)) => {
                frames.push(f);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(e) => Err(format!("decode: {e:?}")),
        }
    };

    if payload.len() >= 32 && &payload[0..4] == b"DKIF" {
        let mut off = 32usize;
        while off + 12 <= payload.len() {
            let sz = u32::from_le_bytes([
                payload[off],
                payload[off + 1],
                payload[off + 2],
                payload[off + 3],
            ]) as usize;
            off += 12;
            if off + sz > payload.len() {
                return Err("truncated IVF frame".into());
            }
            feed(&payload[off..off + sz], &mut frames)?;
            off += sz;
        }
    } else {
        feed(&payload, &mut frames)?;
    }
    if let Ok(mut fl) = dec.flush() {
        frames.append(&mut fl);
    }
    frames.into_iter().last().ok_or_else(|| "no frames".into())
}

/// A decoded frame flattened to 8-bit planes plus their dimensions.
struct Planes8 {
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    w: usize,
    h: usize,
    cw: usize,
    ch: usize,
}

/// Pull a decoded frame's planes down to 8-bit. High-bitdepth frames are
/// right-shifted to 8 bits; this harness is 8-bit only.
fn frame_to_planes8(f: &Frame) -> Result<Planes8, String> {
    let (w, h) = (f.width() as usize, f.height() as usize);
    let (cw, ch) = match f.pixel_layout() {
        PixelLayout::I400 => (0, 0),
        PixelLayout::I420 => (w.div_ceil(2), h.div_ceil(2)),
        PixelLayout::I422 => (w.div_ceil(2), h),
        PixelLayout::I444 => (w, h),
    };
    let mut y = Vec::with_capacity(w * h);
    let mut u = Vec::with_capacity(cw * ch);
    let mut v = Vec::with_capacity(cw * ch);
    match f.planes() {
        Planes::Depth8(p) => {
            for row in p.y().rows().take(h) {
                y.extend_from_slice(&row[..w]);
            }
            if cw > 0 {
                for (dst, view) in [(&mut u, p.u()), (&mut v, p.v())] {
                    let pl = view.ok_or("missing chroma plane")?;
                    for row in pl.rows().take(ch) {
                        dst.extend_from_slice(&row[..cw]);
                    }
                }
            }
        }
        Planes::Depth16(p) => {
            let bd = f.bit_depth() as i32;
            let sh = (bd - 8).max(0) as u32;
            for row in p.y().rows().take(h) {
                y.extend(row[..w].iter().map(|&s| (s >> sh).min(255) as u8));
            }
            if cw > 0 {
                for (dst, view) in [(&mut u, p.u()), (&mut v, p.v())] {
                    let pl = view.ok_or("missing chroma plane")?;
                    for row in pl.rows().take(ch) {
                        dst.extend(row[..cw].iter().map(|&s| (s >> sh).min(255) as u8));
                    }
                }
            }
        }
    }
    Ok(Planes8 {
        y,
        u,
        v,
        w,
        h,
        cw,
        ch,
    })
}

// ------------------------------------------------------------------ main --

fn cmd_prep(src: &str, outdir: &str, max_dim: Option<u32>) -> Result<(), String> {
    std::fs::create_dir_all(outdir).map_err(|e| format!("mkdir {outdir}: {e}"))?;
    let img = image::open(src)
        .map_err(|e| format!("open {src}: {e}"))?
        .to_rgb8();
    let (mut w, mut h) = (img.width(), img.height());

    // Downscale only. Lanczos3 keeps edge energy that a box filter would eat;
    // an upscale would fabricate detail that no encoder should be judged on.
    let img = match max_dim {
        Some(m) if w.max(h) > m => {
            let s = m as f64 / w.max(h) as f64;
            let (nw, nh) = (
                ((w as f64 * s).round() as u32).max(2),
                ((h as f64 * s).round() as u32).max(2),
            );
            w = nw;
            h = nh;
            image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3)
        }
        _ => img,
    };

    // Even dims: crop bottom/right. Cheaper than carrying an odd-4:2:0 edge
    // class through four encoders that each round it differently.
    let (cw_, ch_) = (w & !1, h & !1);
    let rgb: Vec<u8> = if (cw_, ch_) == (w, h) {
        img.into_raw()
    } else {
        let mut o = Vec::with_capacity((cw_ * ch_ * 3) as usize);
        let raw = img.as_raw();
        for r in 0..ch_ as usize {
            let s = r * w as usize * 3;
            o.extend_from_slice(&raw[s..s + cw_ as usize * 3]);
        }
        o
    };
    let (w, h) = (cw_ as usize, ch_ as usize);
    if w < 4 || h < 4 {
        return Err(format!(
            "{src}: {w}x{h} too small after crop (encoders need >= 4)"
        ));
    }

    save_png(&format!("{outdir}/ref.png"), &rgb, w, h);
    let (y, u, v) = rgb_to_i420(&rgb, w, h);
    write_y4m(&format!("{outdir}/src.y4m"), &y, &u, &v, w, h).map_err(|e| format!("y4m: {e}"))?;
    let mut raw = std::fs::File::create(format!("{outdir}/src.yuv")).map_err(|e| e.to_string())?;
    for p in [&y, &u, &v] {
        raw.write_all(p).map_err(|e| e.to_string())?;
    }
    println!("{w} {h}");
    Ok(())
}

fn cmd_floor(outdir: &str) -> Result<(), String> {
    let (y, u, v, w, h) = read_y4m(&format!("{outdir}/src.y4m"));
    let rgb = yuv_to_rgb(&y, &u, &v, w, h, w.div_ceil(2), h.div_ceil(2));
    save_png(&format!("{outdir}/floor.png"), &rgb, w, h);
    Ok(())
}

fn cmd_decode(input: &str, output: &str) -> Result<(), String> {
    let data = std::fs::read(input).map_err(|e| format!("read {input}: {e}"))?;
    let frame = decode_bitstream(&data)?;
    let p = frame_to_planes8(&frame)?;
    let rgb = yuv_to_rgb(&p.y, &p.u, &p.v, p.w, p.h, p.cw, p.ch);
    save_png(output, &rgb, p.w, p.h);
    println!(
        "{} {} {:?} {}",
        p.w,
        p.h,
        frame.pixel_layout(),
        frame.bit_depth()
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["prep", src, outdir] => cmd_prep(src, outdir, None),
        ["prep", src, outdir, m] => cmd_prep(src, outdir, Some(m.parse().unwrap_or(u32::MAX))),
        ["floor", outdir] => cmd_floor(outdir),
        ["decode", input, output] => cmd_decode(input, output),
        _ => {
            eprintln!(
                "usage:\n  rd_tool prep <src.png> <outdir> [max_dim]\n  \
                 rd_tool floor <outdir>\n  rd_tool decode <bitstream> <out.png>"
            );
            return ExitCode::FAILURE;
        }
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("FAIL: {e}");
            ExitCode::FAILURE
        }
    }
}
