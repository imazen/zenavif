//! Resource probe for AVIF encode/decode calibration
//! (`scripts/avif_resource_calibrate.py`).
//!
//! Measures the MARGINAL working set (`VmHWM` high-water delta), the WALL
//! time, and the USER+SYS CPU time of a single encode OR decode call —
//! everything isolated to the codec call, so the binary floor, the PNG
//! load, and (for decode) the prior encode don't contaminate it. Encode
//! and decode are separate invocations (separate processes) so `VmHWM`
//! gives a clean per-operation peak. Threads pinned to 1 for a clean
//! per-pixel CPU model; wall ≈ user single-threaded.
//!
//! Usage:
//!   avif_probe <png> encode <quality> <speed> <8|16> <rgb|rgba> <out.avif>
//!   avif_probe <png> decode <quality> <speed> <8|16> <rgb|rgba> <in.avif>
//! Prints: `delta_kb=<n> peak_kb=<n> wall_ms=<f> user_ms=<f> sys_ms=<f> bytes=<n>`
//! (decode prints `bytes=` = decoded pixel count and also `w= h=`).

use std::fs;
use std::time::Instant;

use almost_enough::{StopToken, Unstoppable};
use imgref::Img;
use rgb::{Rgb, Rgba};
use zenavif::{EncodeBitDepth, EncoderConfig};

fn vmhwm_kb() -> u64 {
    let s = fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest
                .trim()
                .trim_end_matches(" kB")
                .trim()
                .parse()
                .unwrap_or(0);
        }
    }
    0
}

/// (utime, stime) of THIS process in clock ticks, from /proc/self/stat.
/// Fields after the last ')' : state ppid pgrp session tty tpgid flags
/// minflt cminflt majflt cmajflt utime(=idx 11) stime(=idx 12).
fn cpu_ticks() -> (u64, u64) {
    let s = fs::read_to_string("/proc/self/stat").unwrap_or_default();
    if let Some(paren) = s.rfind(')') {
        let after: Vec<&str> = s[paren + 1..].split_whitespace().collect();
        if after.len() > 12 {
            let u = after[11].parse().unwrap_or(0);
            let st = after[12].parse().unwrap_or(0);
            return (u, st);
        }
    }
    (0, 0)
}

// Linux x86_64 USER_HZ is 100 (10 ms ticks). AV1 encodes are >> 10 ms;
// for sub-10ms decodes the wall (Instant) carries the fine resolution.
const TICK_MS: f64 = 10.0;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 8 {
        eprintln!(
            "usage: avif_probe <png> <encode|decode> <quality> <speed> <8|16> <rgb|rgba> <avif>"
        );
        std::process::exit(2);
    }
    let (path, mode, quality, speed, depth, alpha, avif) = (
        &a[1],
        &a[2],
        a[3].parse::<f32>().unwrap(),
        a[4].parse::<u8>().unwrap(),
        a[5].parse::<u8>().unwrap(),
        &a[6],
        &a[7],
    );

    if mode == "encode" {
        let img = image::open(path).expect("open png");
        let (w, h) = (img.width() as usize, img.height() as usize);
        let config = EncoderConfig::new()
            .quality(quality)
            .speed(speed)
            .threads(Some(1))
            .bit_depth(if depth == 16 {
                EncodeBitDepth::Ten
            } else {
                EncodeBitDepth::Eight
            });

        // Materialize input buffer BEFORE the baseline so the PNG load is
        // part of the floor, not the measured encode delta.
        let (b0, t0) = (vmhwm_kb(), Instant::now());
        let (cu0, cs0) = cpu_ticks();
        let encoded = match (depth, alpha.as_str()) {
            (16, "rgba") => {
                let src = img.to_rgba16();
                let px: Vec<Rgba<u16>> = src
                    .pixels()
                    .map(|p| Rgba {
                        r: p[0],
                        g: p[1],
                        b: p[2],
                        a: p[1],
                    })
                    .collect();
                zenavif::encode_rgba16(
                    Img::new(px, w, h).as_ref(),
                    &config,
                    StopToken::new(Unstoppable),
                )
            }
            (16, _) => {
                let src = img.to_rgb16();
                let px: Vec<Rgb<u16>> = src
                    .pixels()
                    .map(|p| Rgb {
                        r: p[0],
                        g: p[1],
                        b: p[2],
                    })
                    .collect();
                zenavif::encode_rgb16(
                    Img::new(px, w, h).as_ref(),
                    &config,
                    StopToken::new(Unstoppable),
                )
            }
            (_, "rgba") => {
                let src = img.to_rgba8();
                let px: Vec<Rgba<u8>> = src
                    .pixels()
                    .map(|p| Rgba {
                        r: p[0],
                        g: p[1],
                        b: p[2],
                        a: p[1],
                    })
                    .collect();
                zenavif::encode_rgba8(
                    Img::new(px, w, h).as_ref(),
                    &config,
                    StopToken::new(Unstoppable),
                )
            }
            _ => {
                let src = img.to_rgb8();
                let px: Vec<Rgb<u8>> = src
                    .pixels()
                    .map(|p| Rgb {
                        r: p[0],
                        g: p[1],
                        b: p[2],
                    })
                    .collect();
                zenavif::encode_rgb8(
                    Img::new(px, w, h).as_ref(),
                    &config,
                    StopToken::new(Unstoppable),
                )
            }
        };
        let wall = t0.elapsed();
        let (cu1, cs1) = cpu_ticks();
        let peak = vmhwm_kb();
        let enc = encoded.expect("encode failed");
        fs::write(avif, &enc.avif_file).expect("write avif");
        println!(
            "delta_kb={} peak_kb={} wall_ms={:.1} user_ms={:.1} sys_ms={:.1} bytes={}",
            peak.saturating_sub(b0),
            peak,
            wall.as_secs_f64() * 1000.0,
            (cu1 - cu0) as f64 * TICK_MS,
            (cs1 - cs0) as f64 * TICK_MS,
            enc.avif_file.len()
        );
    } else {
        let data = fs::read(avif).expect("read avif");
        let (b0, t0) = (vmhwm_kb(), Instant::now());
        let (cu0, cs0) = cpu_ticks();
        let decoded = zenavif::decode(&data).expect("decode failed");
        let wall = t0.elapsed();
        let (cu1, cs1) = cpu_ticks();
        let peak = vmhwm_kb();
        let (w, h) = (decoded.width(), decoded.height());
        println!(
            "delta_kb={} peak_kb={} wall_ms={:.1} user_ms={:.1} sys_ms={:.1} bytes={} w={} h={}",
            peak.saturating_sub(b0),
            peak,
            wall.as_secs_f64() * 1000.0,
            (cu1 - cu0) as f64 * TICK_MS,
            (cs1 - cs0) as f64 * TICK_MS,
            (w as u64) * (h as u64),
            w,
            h
        );
    }
}
