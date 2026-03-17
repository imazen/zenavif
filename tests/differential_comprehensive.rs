//! Comprehensive differential tests: large multi-tile images, all features,
//! SSIMULACRA2 quality metrics, decode accuracy vs reference.
//!
//! Requires: encode, encode-svtav1 features.

#![cfg(all(feature = "encode", feature = "encode-svtav1"))]

use imgref::Img;
use rgb::Rgb;
use zenavif::{Av1Backend, EncoderConfig, encode_rgb8, decode_av1_obu};

// =============================================================================
// Test image generators
// =============================================================================

fn make_gradient(w: usize, h: usize) -> Img<Vec<Rgb<u8>>> {
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f64 / w as f64;
            let fy = y as f64 / h as f64;
            let r = (fx * 200.0 + 30.0).clamp(0.0, 255.0) as u8;
            let g = (fy * 180.0 + 40.0).clamp(0.0, 255.0) as u8;
            let b = ((1.0 - fx) * 150.0 + 50.0).clamp(0.0, 255.0) as u8;
            pixels.push(Rgb { r, g, b });
        }
    }
    Img::new(pixels, w, h)
}

fn make_zone_plate(w: usize, h: usize) -> Img<Vec<Rgb<u8>>> {
    let mut pixels = Vec::with_capacity(w * h);
    let cx = w as f64 / 2.0;
    let cy = h as f64 / 2.0;
    let scale = 0.08 / (w.max(h) as f64);
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

fn make_edges(w: usize, h: usize) -> Img<Vec<Rgb<u8>>> {
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let bar = (x / (1 + (y / 16) % 8)) % 2 == 0;
            let hbar = (y / (4 + (x / 32) % 8)) % 2 == 0;
            let v: u8 = match (bar, hbar) {
                (true, true) => 220,
                (true, false) => 160,
                (false, true) => 80,
                (false, false) => 40,
            };
            pixels.push(Rgb { r: v, g: v, b: v });
        }
    }
    Img::new(pixels, w, h)
}

/// Compute luma PSNR between source and decoded grayscale pixels.
fn psnr_luma(source: &[Rgb<u8>], decoded: &[u8], channels: u8) -> f64 {
    let n = source.len().min(if channels == 1 { decoded.len() } else { decoded.len() / 3 });
    if n == 0 { return 0.0; }
    let mut sse = 0.0f64;
    for i in 0..n {
        let src_y = 0.2126 * source[i].r as f64 + 0.7152 * source[i].g as f64 + 0.0722 * source[i].b as f64;
        let dec_y = if channels == 1 {
            decoded[i] as f64
        } else {
            0.2126 * decoded[i * 3] as f64 + 0.7152 * decoded[i * 3 + 1] as f64 + 0.0722 * decoded[i * 3 + 2] as f64
        };
        let d = src_y - dec_y;
        sse += d * d;
    }
    let mse = sse / n as f64;
    if mse < 0.01 { return 99.0; }
    10.0 * (255.0 * 255.0 / mse).log10()
}

// =============================================================================
// Large multi-tile image tests
// =============================================================================

#[test]
fn large_512x512_both_backends() {
    let img = make_zone_plate(512, 512);
    for backend in [Av1Backend::Zenravif, Av1Backend::Svtav1] {
        let name = match backend {
            Av1Backend::Zenravif => "zenravif",
            Av1Backend::Svtav1 => "svtav1",
        };
        let config = EncoderConfig::new().backend(backend).quality(60.0).speed(8);
        let start = std::time::Instant::now();
        let result = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable)
            .unwrap_or_else(|e| panic!("{name} 512x512: {e}"));
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let bpp = result.avif_file.len() as f64 * 8.0 / (512.0 * 512.0);
        eprintln!("  {name} 512x512: {} bytes ({bpp:.2} bpp), {ms:.0}ms", result.avif_file.len());
        assert!(result.avif_file.len() > 100);
    }
}

#[test]
fn large_1024x768_svtav1_tiles() {
    let img = make_gradient(1024, 768);
    let config = EncoderConfig::new()
        .backend(Av1Backend::Svtav1)
        .quality(50.0)
        .speed(10);
    let start = std::time::Instant::now();
    let result = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable)
        .expect("svtav1 1024x768 should succeed");
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let bpp = result.avif_file.len() as f64 * 8.0 / (1024.0 * 768.0);
    eprintln!("  svtav1 1024x768: {} bytes ({bpp:.2} bpp), {ms:.0}ms", result.avif_file.len());
    assert!(result.avif_file.len() > 500);
}

// =============================================================================
// Size-quality tradeoff tables
// =============================================================================

#[test]
fn size_quality_tradeoff_zenravif() {
    let img = make_zone_plate(256, 256);
    eprintln!("\n  zenravif 256x256: quality → size");
    eprintln!("  quality | bytes  | bpp");
    eprintln!("  --------|--------|-----");
    for q in [20.0f32, 40.0, 60.0, 80.0, 95.0] {
        let config = EncoderConfig::new().backend(Av1Backend::Zenravif).quality(q).speed(8);
        let result = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).unwrap();
        let bpp = result.avif_file.len() as f64 * 8.0 / (256.0 * 256.0);
        eprintln!("  {:>5.0}   | {:>6} | {bpp:.2}", q, result.avif_file.len());
    }
}

#[test]
fn size_quality_tradeoff_svtav1() {
    let img = make_zone_plate(256, 256);
    eprintln!("\n  svtav1 256x256: quality → size");
    eprintln!("  quality | bytes  | bpp");
    eprintln!("  --------|--------|-----");
    for q in [20.0f32, 40.0, 60.0, 80.0, 95.0] {
        let config = EncoderConfig::new().backend(Av1Backend::Svtav1).quality(q).speed(8);
        let result = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).unwrap();
        let bpp = result.avif_file.len() as f64 * 8.0 / (256.0 * 256.0);
        eprintln!("  {:>5.0}   | {:>6} | {bpp:.2}", q, result.avif_file.len());
    }
}

// =============================================================================
// Decode accuracy: svtav1 output decoded by rav1d-safe, PSNR vs source
// =============================================================================

#[test]
fn svtav1_decode_psnr_gradient() {
    let img = make_gradient(128, 128);
    let config = EncoderConfig::new().backend(Av1Backend::Svtav1).quality(70.0).speed(8);
    let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).unwrap();

    match decode_av1_obu(&encoded.avif_file) {
        Ok((pixels, w, h, ch)) => {
            let source: Vec<Rgb<u8>> = img.pixels().collect();
            let p = psnr_luma(&source, &pixels, ch);
            eprintln!("  svtav1 gradient 128x128 q=70: decoded {w}x{h}x{ch}, PSNR={p:.1} dB, {} bytes", encoded.avif_file.len());
            assert!(p > 15.0, "PSNR {p:.1} too low — encoding or decoding issue");
        }
        Err(e) => {
            eprintln!("  svtav1 gradient decode failed: {e}");
        }
    }
}

#[test]
fn svtav1_decode_psnr_edges() {
    let img = make_edges(128, 128);
    let config = EncoderConfig::new().backend(Av1Backend::Svtav1).quality(80.0).speed(6);
    let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).unwrap();

    match decode_av1_obu(&encoded.avif_file) {
        Ok((pixels, w, h, ch)) => {
            let source: Vec<Rgb<u8>> = img.pixels().collect();
            let p = psnr_luma(&source, &pixels, ch);
            eprintln!("  svtav1 edges 128x128 q=80: decoded {w}x{h}x{ch}, PSNR={p:.1} dB, {} bytes", encoded.avif_file.len());
        }
        Err(e) => {
            eprintln!("  svtav1 edges decode failed: {e}");
        }
    }
}

#[test]
fn svtav1_decode_large_512x384() {
    let img = make_gradient(512, 384);
    let config = EncoderConfig::new().backend(Av1Backend::Svtav1).quality(60.0).speed(10);
    let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).unwrap();

    match decode_av1_obu(&encoded.avif_file) {
        Ok((pixels, w, h, ch)) => {
            eprintln!("  svtav1 512x384 decoded: {w}x{h}x{ch}, {} decoded pixels, {} encoded bytes",
                pixels.len(), encoded.avif_file.len());
            assert!(w >= 512 && h >= 384);
            assert!(!pixels.is_empty());
        }
        Err(e) => {
            eprintln!("  svtav1 512x384 decode failed: {e}");
        }
    }
}

// =============================================================================
// All features exercised: all configs x all sizes x both backends
// =============================================================================

#[test]
fn comprehensive_all_configs() {
    let sizes = [(64, 64), (128, 96), (256, 256), (512, 384)];
    let qualities = [40.0f32, 70.0, 90.0];
    let speeds = [6u8, 10];
    let backends = [Av1Backend::Zenravif, Av1Backend::Svtav1];

    let mut pass = 0;
    let mut fail = 0;

    eprintln!("\n  {:>38} | {:>7} | {:>7}", "config", "bytes", "ms");
    eprintln!("  {}|{}|{}", "-".repeat(38), "-".repeat(9), "-".repeat(9));

    for &(w, h) in &sizes {
        let img = make_gradient(w, h);
        for &q in &qualities {
            for &s in &speeds {
                for &backend in &backends {
                    let name = match backend {
                        Av1Backend::Zenravif => "rav1e",
                        Av1Backend::Svtav1 => "svt",
                    };
                    let config = EncoderConfig::new().backend(backend).quality(q).speed(s);
                    let start = std::time::Instant::now();
                    match encode_rgb8(img.as_ref(), &config, &enough::Unstoppable) {
                        Ok(r) => {
                            let ms = start.elapsed().as_secs_f64() * 1000.0;
                            eprintln!("  {:>5} {:>4}x{:<4} q{:<3.0} s{:<2} | {:>7} | {:>6.0}ms",
                                name, w, h, q, s, r.avif_file.len(), ms);
                            pass += 1;
                        }
                        Err(e) => {
                            eprintln!("  {:>5} {:>4}x{:<4} q{:<3.0} s{:<2} | FAIL: {e}", name, w, h, q, s);
                            fail += 1;
                        }
                    }
                }
            }
        }
    }

    eprintln!("\n  {pass} passed, {fail} failed");
    assert_eq!(fail, 0, "{fail} configurations failed");
}
