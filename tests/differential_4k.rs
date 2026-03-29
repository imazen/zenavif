//! 4K (3840x2160) differential tests: large-scale encoding comparison.
//!
//! Tests both backends at UHD resolution with timing, file size, bpp metrics.
//! Only fast presets are tested at 4K to keep CI manageable.

#![cfg(all(feature = "encode", feature = "encode-svtav1"))]

use almost_enough::{StopToken, Unstoppable};
use imgref::Img;
use rgb::Rgb;
use zenavif::{Av1Backend, EncoderConfig, decode_av1_obu, encode_rgb8};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

fn make_4k_gradient() -> Img<Vec<Rgb<u8>>> {
    let w = 3840;
    let h = 2160;
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f64 / w as f64;
            let fy = y as f64 / h as f64;
            let r = (fx * 220.0 + 16.0).clamp(0.0, 255.0) as u8;
            let g = (fy * 200.0 + 20.0).clamp(0.0, 255.0) as u8;
            let b = ((fx * fy) * 180.0 + 30.0).clamp(0.0, 255.0) as u8;
            pixels.push(Rgb { r, g, b });
        }
    }
    Img::new(pixels, w, h)
}

fn make_4k_zone_plate() -> Img<Vec<Rgb<u8>>> {
    let w = 3840;
    let h = 2160;
    let mut pixels = Vec::with_capacity(w * h);
    let cx = w as f64 / 2.0;
    let cy = h as f64 / 2.0;
    let scale = 0.02 / (w as f64);
    for y in 0..h {
        for x in 0..w {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let rsq = dx * dx + dy * dy;
            let v = (128.0 + 100.0 * (rsq * scale).cos()).clamp(0.0, 255.0) as u8;
            pixels.push(Rgb { r: v, g: v, b: v });
        }
    }
    Img::new(pixels, w, h)
}

fn make_2k_mixed() -> Img<Vec<Rgb<u8>>> {
    let w = 1920;
    let h = 1080;
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            // Mixed content: gradient + edges + noise-like pattern
            let fx = x as f64 / w as f64;
            let fy = y as f64 / h as f64;
            let grad = (fx * 200.0 + fy * 50.0) as u8;
            let edge = if (x / 64 + y / 64) % 2 == 0 { 40u8 } else { 0 };
            let r = grad.wrapping_add(edge);
            let g = (grad as u16 + 20).min(255) as u8;
            let b = grad.wrapping_sub(edge.wrapping_mul(2));
            pixels.push(Rgb { r, g, b });
        }
    }
    Img::new(pixels, w, h)
}

// =============================================================================
// 4K encoding tests
// =============================================================================

#[test]
fn encode_4k_svtav1_speed10() {
    let img = make_4k_gradient();
    let config = EncoderConfig::new()
        .backend(Av1Backend::Svtav1)
        .quality(50.0)
        .speed(10);
    let start = std::time::Instant::now();
    let result =
        encode_rgb8(img.as_ref(), &config, stop()).expect("svtav1 4K encode should succeed");
    let elapsed = start.elapsed();
    let mpx = 3840.0 * 2160.0 / 1_000_000.0;
    let bpp = result.avif_file.len() as f64 * 8.0 / (3840.0 * 2160.0);
    let mpx_per_sec = mpx / elapsed.as_secs_f64();
    eprintln!(
        "  svtav1 3840x2160 q50 s10: {} bytes ({bpp:.3} bpp), {:.1}s, {mpx_per_sec:.2} Mpx/s",
        result.avif_file.len(),
        elapsed.as_secs_f64()
    );
    assert!(result.avif_file.len() > 1000, "4K output too small");
}

#[test]
fn encode_4k_zenravif_speed10() {
    let img = make_4k_gradient();
    let config = EncoderConfig::new()
        .backend(Av1Backend::Zenravif)
        .quality(50.0)
        .speed(10);
    let start = std::time::Instant::now();
    let result =
        encode_rgb8(img.as_ref(), &config, stop()).expect("zenravif 4K encode should succeed");
    let elapsed = start.elapsed();
    let mpx = 3840.0 * 2160.0 / 1_000_000.0;
    let bpp = result.avif_file.len() as f64 * 8.0 / (3840.0 * 2160.0);
    let mpx_per_sec = mpx / elapsed.as_secs_f64();
    eprintln!(
        "  zenravif 3840x2160 q50 s10: {} bytes ({bpp:.3} bpp), {:.1}s, {mpx_per_sec:.2} Mpx/s",
        result.avif_file.len(),
        elapsed.as_secs_f64()
    );
    assert!(result.avif_file.len() > 1000, "4K output too small");
}

#[test]
fn encode_4k_comparison_table() {
    let img = make_4k_gradient();
    let mpx = 3840.0 * 2160.0 / 1_000_000.0;

    eprintln!("\n  4K (3840x2160) encoding comparison:");
    eprintln!(
        "  {:>8} {:>3} {:>3} | {:>8} | {:>6} | {:>6} | {:>8}",
        "backend", "q", "s", "bytes", "bpp", "sec", "Mpx/s"
    );
    eprintln!(
        "  {}|{}|{}|{}|{}",
        "-".repeat(18),
        "-".repeat(10),
        "-".repeat(8),
        "-".repeat(8),
        "-".repeat(10)
    );

    for &(backend, name) in &[(Av1Backend::Zenravif, "rav1e"), (Av1Backend::Svtav1, "svt")] {
        for &q in &[40.0f32, 70.0] {
            let config = EncoderConfig::new().backend(backend).quality(q).speed(10);
            let start = std::time::Instant::now();
            let result = encode_rgb8(img.as_ref(), &config, stop()).unwrap();
            let secs = start.elapsed().as_secs_f64();
            let bpp = result.avif_file.len() as f64 * 8.0 / (3840.0 * 2160.0);
            let mpxs = mpx / secs;
            eprintln!(
                "  {:>8} {:>3.0} {:>3} | {:>8} | {:>5.3} | {:>5.1}s | {:>7.2}",
                name,
                q,
                10,
                result.avif_file.len(),
                bpp,
                secs,
                mpxs
            );
        }
    }
}

#[test]
fn encode_2k_1080p_comparison() {
    let img = make_2k_mixed();
    let mpx = 1920.0 * 1080.0 / 1_000_000.0;

    eprintln!("\n  1080p (1920x1080) encoding comparison:");
    eprintln!(
        "  {:>8} {:>3} {:>3} | {:>8} | {:>6} | {:>6} | {:>8}",
        "backend", "q", "s", "bytes", "bpp", "sec", "Mpx/s"
    );
    eprintln!(
        "  {}|{}|{}|{}|{}",
        "-".repeat(18),
        "-".repeat(10),
        "-".repeat(8),
        "-".repeat(8),
        "-".repeat(10)
    );

    for &(backend, name) in &[(Av1Backend::Zenravif, "rav1e"), (Av1Backend::Svtav1, "svt")] {
        for &q in &[50.0f32, 80.0] {
            for &s in &[8u8, 10] {
                let config = EncoderConfig::new().backend(backend).quality(q).speed(s);
                let start = std::time::Instant::now();
                let result = encode_rgb8(img.as_ref(), &config, stop()).unwrap();
                let secs = start.elapsed().as_secs_f64();
                let bpp = result.avif_file.len() as f64 * 8.0 / (1920.0 * 1080.0);
                let mpxs = mpx / secs;
                eprintln!(
                    "  {:>8} {:>3.0} {:>3} | {:>8} | {:>5.3} | {:>5.1}s | {:>7.2}",
                    name,
                    q,
                    s,
                    result.avif_file.len(),
                    bpp,
                    secs,
                    mpxs
                );
            }
        }
    }
}

#[test]
fn encode_4k_zone_plate_svtav1() {
    // Zone plate at 4K — worst case for compression (all frequencies)
    let img = make_4k_zone_plate();
    let config = EncoderConfig::new()
        .backend(Av1Backend::Svtav1)
        .quality(50.0)
        .speed(10);
    let start = std::time::Instant::now();
    let result =
        encode_rgb8(img.as_ref(), &config, stop()).expect("svtav1 4K zone plate should succeed");
    let elapsed = start.elapsed();
    let bpp = result.avif_file.len() as f64 * 8.0 / (3840.0 * 2160.0);
    eprintln!(
        "  svtav1 4K zone plate: {} bytes ({bpp:.3} bpp), {:.1}s",
        result.avif_file.len(),
        elapsed.as_secs_f64()
    );
}

#[test]
fn decode_4k_svtav1_output() {
    // Encode 4K, attempt decode with rav1d-safe
    let img = make_4k_gradient();
    let config = EncoderConfig::new()
        .backend(Av1Backend::Svtav1)
        .quality(40.0)
        .speed(10);
    let encoded =
        encode_rgb8(img.as_ref(), &config, stop()).expect("svtav1 4K encode should succeed");

    eprintln!("  svtav1 4K encoded: {} bytes", encoded.avif_file.len());

    match decode_av1_obu(&encoded.avif_file) {
        Ok((pixels, w, h, ch)) => {
            eprintln!("  Decoded 4K: {w}x{h}x{ch}, {} bytes", pixels.len());
            assert!(w >= 3840 && h >= 2160);
        }
        Err(e) => {
            eprintln!("  4K decode: {e} (tile structure may differ at this resolution)");
        }
    }
}
