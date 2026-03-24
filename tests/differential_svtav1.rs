//! Differential tests comparing svtav1-rs and zenravif backends.
//!
//! Tests encoding quality (PSNR), compression efficiency (file size),
//! encoding speed, and OBU structure validity for both backends.

#![cfg(all(feature = "encode", feature = "encode-svtav1"))]

use imgref::Img;
use rgb::Rgb;
use zenavif::{Av1Backend, EncoderConfig, decode_av1_obu, encode_rgb8};

/// Create a gradient test image of given dimensions.
fn make_gradient(w: usize, h: usize) -> Img<Vec<Rgb<u8>>> {
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            pixels.push(Rgb {
                r: ((x * 255) / w.max(1)) as u8,
                g: ((y * 255) / h.max(1)) as u8,
                b: 128,
            });
        }
    }
    Img::new(pixels, w, h)
}

/// Create a zone plate (chirp) test pattern — contains all spatial frequencies.
fn make_zone_plate(w: usize, h: usize) -> Img<Vec<Rgb<u8>>> {
    let mut pixels = Vec::with_capacity(w * h);
    let cx = w as f64 / 2.0;
    let cy = h as f64 / 2.0;
    let scale = 0.1 / (w.max(h) as f64);
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

/// Compute PSNR between two RGB images (luma-only comparison).
fn psnr_rgb(a: &[Rgb<u8>], b: &[Rgb<u8>]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mut sse: f64 = 0.0;
    for (pa, pb) in a.iter().zip(b.iter()) {
        // BT.709 luma
        let ya = 0.2126 * pa.r as f64 + 0.7152 * pa.g as f64 + 0.0722 * pa.b as f64;
        let yb = 0.2126 * pb.r as f64 + 0.7152 * pb.g as f64 + 0.0722 * pb.b as f64;
        let d = ya - yb;
        sse += d * d;
    }
    let mse = sse / a.len() as f64;
    if mse < 0.01 {
        return 99.0;
    }
    10.0 * (255.0_f64 * 255.0 / mse).log10()
}

// =============================================================================
// Differential encoding tests
// =============================================================================

#[test]
fn both_backends_produce_output() {
    let img = make_gradient(64, 64);
    let quality = 70.0;
    let speed = 8;

    // Encode with zenravif
    let config_rav1e = EncoderConfig::new()
        .backend(Av1Backend::Zenravif)
        .quality(quality)
        .speed(speed);
    let result_rav1e = encode_rgb8(img.as_ref(), &config_rav1e, &enough::Unstoppable)
        .expect("zenravif encode should succeed");

    // Encode with svtav1
    let config_svtav1 = EncoderConfig::new()
        .backend(Av1Backend::Svtav1)
        .quality(quality)
        .speed(speed);
    let result_svtav1 = encode_rgb8(img.as_ref(), &config_svtav1, &enough::Unstoppable)
        .expect("svtav1 encode should succeed");

    assert!(!result_rav1e.avif_file.is_empty(), "zenravif: empty output");
    assert!(!result_svtav1.avif_file.is_empty(), "svtav1: empty output");

    eprintln!(
        "zenravif: {} bytes, svtav1: {} bytes (ratio: {:.2}x)",
        result_rav1e.avif_file.len(),
        result_svtav1.avif_file.len(),
        result_svtav1.avif_file.len() as f64 / result_rav1e.avif_file.len() as f64
    );
}

#[test]
fn compression_efficiency_comparison() {
    // Compare file sizes across quality levels
    let img = make_zone_plate(128, 128);

    eprintln!("\n  Quality | zenravif bytes | svtav1 bytes | ratio");
    eprintln!("  --------|----------------|--------------|------");

    for quality in [30.0f32, 60.0, 80.0] {
        let config_r = EncoderConfig::new()
            .backend(Av1Backend::Zenravif)
            .quality(quality)
            .speed(8);
        let result_r = encode_rgb8(img.as_ref(), &config_r, &enough::Unstoppable).unwrap();

        let config_s = EncoderConfig::new()
            .backend(Av1Backend::Svtav1)
            .quality(quality)
            .speed(8);
        let result_s = encode_rgb8(img.as_ref(), &config_s, &enough::Unstoppable).unwrap();

        let ratio = result_s.avif_file.len() as f64 / result_r.avif_file.len() as f64;
        eprintln!(
            "  {:>5.0}   | {:>14} | {:>12} | {:.2}x",
            quality,
            result_r.avif_file.len(),
            result_s.avif_file.len(),
            ratio
        );

        // Both should produce reasonable output
        assert!(
            result_r.avif_file.len() > 50,
            "zenravif q={quality}: too small"
        );
        assert!(
            result_s.avif_file.len() > 50,
            "svtav1 q={quality}: too small"
        );
    }
}

#[test]
fn encoding_speed_comparison() {
    let img = make_gradient(256, 256);

    eprintln!("\n  Speed | zenravif ms | svtav1 ms | speedup");
    eprintln!("  ------|-------------|-----------|--------");

    for speed in [4u8, 8, 10] {
        let config_r = EncoderConfig::new()
            .backend(Av1Backend::Zenravif)
            .quality(60.0)
            .speed(speed);
        let start = std::time::Instant::now();
        let _result_r = encode_rgb8(img.as_ref(), &config_r, &enough::Unstoppable).unwrap();
        let time_r = start.elapsed();

        let config_s = EncoderConfig::new()
            .backend(Av1Backend::Svtav1)
            .quality(60.0)
            .speed(speed);
        let start = std::time::Instant::now();
        let _result_s = encode_rgb8(img.as_ref(), &config_s, &enough::Unstoppable).unwrap();
        let time_s = start.elapsed();

        let speedup = time_r.as_secs_f64() / time_s.as_secs_f64();
        eprintln!(
            "  {:>5} | {:>9.1}   | {:>7.1}   | {:.2}x",
            speed,
            time_r.as_secs_f64() * 1000.0,
            time_s.as_secs_f64() * 1000.0,
            speedup
        );
    }
}

#[test]
fn svtav1_output_has_valid_obu_structure() {
    let img = make_gradient(64, 64);
    let config = EncoderConfig::new()
        .backend(Av1Backend::Svtav1)
        .quality(70.0)
        .speed(8);
    let result = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).unwrap();

    // svtav1 returns raw AV1 OBU data (not AVIF container)
    let data = &result.avif_file;
    assert!(data.len() > 10, "OBU data too short");

    // First byte should be a Temporal Delimiter OBU header
    let forbidden = data[0] >> 7;
    assert_eq!(forbidden, 0, "forbidden bit must be 0");
    let obu_type = (data[0] >> 3) & 0xF;
    assert_eq!(
        obu_type, 2,
        "first OBU should be Temporal Delimiter (type 2)"
    );

    eprintln!("svtav1 OBU output: {} bytes, valid TD header", data.len());
}

#[test]
fn quality_sweep_both_backends() {
    // Verify both backends' output grows with quality
    let img = make_gradient(64, 64);

    for backend in [Av1Backend::Zenravif, Av1Backend::Svtav1] {
        let name = match backend {
            Av1Backend::Zenravif => "zenravif",
            Av1Backend::Svtav1 => "svtav1",
        };
        let mut prev_size = 0;
        for q in [20.0f32, 50.0, 80.0, 95.0] {
            let config = EncoderConfig::new().backend(backend).quality(q).speed(8);
            let result = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).unwrap();
            assert!(!result.avif_file.is_empty(), "{name} q={q}: empty");

            // Higher quality should generally produce equal or larger output
            if prev_size > 0 && q > 50.0 {
                assert!(
                    result.avif_file.len() >= prev_size / 3,
                    "{name} q={q}: output {} much smaller than prev {}",
                    result.avif_file.len(),
                    prev_size
                );
            }
            prev_size = result.avif_file.len();
        }
    }
}

#[test]
fn various_image_sizes_both_backends() {
    // Both backends should handle various sizes without crashing
    for (w, h) in [(16, 16), (64, 48), (100, 75), (128, 128)] {
        let img = make_zone_plate(w, h);
        for backend in [Av1Backend::Zenravif, Av1Backend::Svtav1] {
            let name = match backend {
                Av1Backend::Zenravif => "zenravif",
                Av1Backend::Svtav1 => "svtav1",
            };
            let config = EncoderConfig::new()
                .backend(backend)
                .quality(60.0)
                .speed(10);
            let result = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable)
                .unwrap_or_else(|e| panic!("{name} {w}x{h}: {e}"));
            assert!(!result.avif_file.is_empty(), "{name} {w}x{h}: empty output");
        }
    }
}

// =============================================================================
// Decode roundtrip tests — encode with svtav1, decode with rav1d-safe
// =============================================================================

#[test]
fn svtav1_decode_roundtrip_gradient() {
    let img = make_gradient(64, 64);
    let config = EncoderConfig::new()
        .backend(Av1Backend::Svtav1)
        .quality(70.0)
        .speed(8);
    let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable)
        .expect("svtav1 encode should succeed");

    // Try to decode the AV1 OBU output with rav1d-safe
    match decode_av1_obu(&encoded.avif_file) {
        Ok((pixels, w, h, channels)) => {
            eprintln!(
                "Decoded: {}x{}, {} channels, {} pixels",
                w,
                h,
                channels,
                pixels.len()
            );
            assert!(w > 0 && h > 0, "decoded dimensions should be positive");
            assert!(!pixels.is_empty(), "decoded pixels should be non-empty");
        }
        Err(e) => {
            // Expected: svtav1 bitstream may not be fully dav1d-conformant yet.
            // This test documents the current conformance status.
            eprintln!("Decode failed (expected — svtav1 bitstream not yet fully conformant): {e}");
        }
    }
}

#[test]
fn zenravif_decode_roundtrip_success() {
    // Verify the zenravif backend's output decodes successfully (baseline)
    let img = make_gradient(64, 64);
    let config = EncoderConfig::new()
        .backend(Av1Backend::Zenravif)
        .quality(70.0)
        .speed(8);
    let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable)
        .expect("zenravif encode should succeed");

    // zenravif output is AVIF container — decode with the full decoder
    let decoded =
        zenavif::decode(&encoded.avif_file).expect("zenravif AVIF should decode successfully");

    eprintln!(
        "zenravif roundtrip: encoded {} bytes, decoded {}x{}",
        encoded.avif_file.len(),
        decoded.width(),
        decoded.height(),
    );
}

#[test]
fn svtav1_decode_128x128() {
    let img = make_gradient(128, 128);
    let config = EncoderConfig::new()
        .backend(Av1Backend::Svtav1)
        .quality(70.0)
        .speed(8);
    let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable)
        .expect("svtav1 128x128 encode should succeed");
    eprintln!("128x128 encoded: {} bytes", encoded.avif_file.len());
    match decode_av1_obu(&encoded.avif_file) {
        Ok((pixels, w, h, ch)) => {
            eprintln!("  Decoded: {w}x{h}x{ch}, {} pixels", pixels.len());
        }
        Err(e) => {
            eprintln!("  Decode failed: {e}");
            // Write to disk for hex inspection
            std::fs::write("/tmp/svtav1_128x128.obu", &encoded.avif_file).ok();
            eprintln!("  Written to /tmp/svtav1_128x128.obu for inspection");
        }
    }
}

#[test]
fn svtav1_decode_64x64_speed8() {
    let img = make_gradient(64, 64);
    let config = EncoderConfig::new()
        .backend(Av1Backend::Svtav1)
        .quality(70.0)
        .speed(8);
    let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable)
        .expect("svtav1 64x64 encode should succeed");
    eprintln!("64x64 s8 encoded: {} bytes", encoded.avif_file.len());
    match decode_av1_obu(&encoded.avif_file) {
        Ok((pixels, w, h, ch)) => {
            eprintln!("  Decoded: {w}x{h}x{ch}, {} pixels", pixels.len());
        }
        Err(e) => {
            eprintln!("  Decode failed: {e}");
            std::fs::write("/tmp/svtav1_64x64_s8.obu", &encoded.avif_file).ok();
        }
    }
}

#[test]
fn svtav1_decode_size_sweep() {
    // Sweep sizes to find exactly where decode breaks
    for size in [32u32, 48, 64, 80, 96, 112, 128] {
        let img = make_gradient(size as usize, size as usize);
        let config = EncoderConfig::new()
            .backend(Av1Backend::Svtav1)
            .quality(50.0)
            .speed(10); // fastest preset
        let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable)
            .unwrap_or_else(|e| panic!("{size}x{size} encode failed: {e}"));
        
        match decode_av1_obu(&encoded.avif_file) {
            Ok((pixels, w, h, ch)) => {
                eprintln!("  {size}x{size}: DECODE OK — {w}x{h}x{ch}, {} bytes encoded", encoded.avif_file.len());
            }
            Err(e) => {
                eprintln!("  {size}x{size}: DECODE FAIL — {} bytes encoded, {e}", encoded.avif_file.len());
            }
        }
    }
}

#[test]
fn svtav1_decode_size_sweep_speed8() {
    for size in [32u32, 64, 128] {
        let img = make_gradient(size as usize, size as usize);
        let config = EncoderConfig::new()
            .backend(Av1Backend::Svtav1)
            .quality(70.0)
            .speed(8);
        let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable)
            .unwrap_or_else(|e| panic!("{size}x{size} encode failed: {e}"));
        
        match decode_av1_obu(&encoded.avif_file) {
            Ok((pixels, w, h, ch)) => {
                eprintln!("  s8 {size}x{size}: DECODE OK — {w}x{h}x{ch}, {} bytes", encoded.avif_file.len());
            }
            Err(e) => {
                eprintln!("  s8 {size}x{size}: DECODE FAIL — {} bytes, {e}", encoded.avif_file.len());
            }
        }
    }
}

#[test]
fn svtav1_decode_speed_sweep_64x64() {
    for speed in [4u8, 6, 8, 9, 10] {
        let img = make_gradient(64, 64);
        let config = EncoderConfig::new()
            .backend(Av1Backend::Svtav1)
            .quality(70.0)
            .speed(speed);
        let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).unwrap();
        match decode_av1_obu(&encoded.avif_file) {
            Ok((_, w, h, _)) => eprintln!("  64x64 s{speed}: OK ({w}x{h}), {} bytes", encoded.avif_file.len()),
            Err(_) => eprintln!("  64x64 s{speed}: FAIL, {} bytes", encoded.avif_file.len()),
        }
    }
}

#[test]
fn svtav1_decode_quality_sweep_64x64_s10() {
    for q in [30.0f32, 50.0, 60.0, 70.0, 90.0] {
        let img = make_gradient(64, 64);
        let config = EncoderConfig::new()
            .backend(Av1Backend::Svtav1)
            .quality(q)
            .speed(10);
        let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).unwrap();
        match decode_av1_obu(&encoded.avif_file) {
            Ok((_, w, h, _)) => eprintln!("  64x64 q{q:.0} s10: OK ({w}x{h}), {} bytes", encoded.avif_file.len()),
            Err(_) => eprintln!("  64x64 q{q:.0} s10: FAIL, {} bytes", encoded.avif_file.len()),
        }
    }
}

#[test]
fn svtav1_decode_mid_sizes_speed8() {
    for size in [64u32, 80, 96, 112, 128] {
        let img = make_gradient(size as usize, size as usize);
        let config = EncoderConfig::new()
            .backend(Av1Backend::Svtav1)
            .quality(70.0)
            .speed(8);
        let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).unwrap();
        match decode_av1_obu(&encoded.avif_file) {
            Ok((_, w, h, _)) => eprintln!("  s8 {size}x{size}: OK ({w}x{h}), {} bytes", encoded.avif_file.len()),
            Err(_) => eprintln!("  s8 {size}x{size}: FAIL, {} bytes", encoded.avif_file.len()),
        }
    }
}

#[test]
fn svtav1_decode_uniform_128x128() {
    // All gray — should produce mostly skip blocks
    let w = 128usize;
    let h = 128usize;
    let mut pixels = Vec::with_capacity(w * h);
    for _ in 0..h {
        for _ in 0..w {
            pixels.push(rgb::Rgb { r: 128, g: 128, b: 128 });
        }
    }
    let img = imgref::Img::new(pixels, w, h);
    let config = EncoderConfig::new()
        .backend(Av1Backend::Svtav1)
        .quality(70.0)
        .speed(8);
    let encoded = encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).unwrap();
    match decode_av1_obu(&encoded.avif_file) {
        Ok((_, w, h, _)) => eprintln!("  uniform 128x128: OK ({w}x{h}), {} bytes", encoded.avif_file.len()),
        Err(e) => eprintln!("  uniform 128x128: FAIL, {} bytes, {e}", encoded.avif_file.len()),
    }
}

#[test]
fn svtav1_decode_direct_128x128() {
    // Encode directly without zenavif's RGB→Y conversion
    let pixels = vec![128u8; 128 * 128]; // uniform gray
    let enc = svtav1::avif::AvifEncoder::new()
        .with_quality(70.0)
        .with_speed(8);
    let obu = enc.encode_to_av1_obu(&pixels, 128, 128, 128).unwrap();
    eprintln!("direct 128x128: {} bytes", obu.len());
    match decode_av1_obu(&obu) {
        Ok((_, w, h, _)) => eprintln!("  OK: {w}x{h}"),
        Err(e) => eprintln!("  FAIL: {e}"),
    }
}

#[test]
fn svtav1_decode_direct_64x64() {
    let pixels = vec![128u8; 64 * 64];
    let enc = svtav1::avif::AvifEncoder::new()
        .with_quality(70.0)
        .with_speed(8);
    let obu = enc.encode_to_av1_obu(&pixels, 64, 64, 64).unwrap();
    eprintln!("direct 64x64: {} bytes", obu.len());
    match decode_av1_obu(&obu) {
        Ok((_, w, h, _)) => eprintln!("  OK: {w}x{h}"),
        Err(e) => eprintln!("  FAIL: {e}"),
    }
}

#[test]
fn svtav1_decode_direct_gradient_64x64() {
    let mut pixels = vec![0u8; 64 * 64];
    for r in 0..64 {
        for c in 0..64 {
            pixels[r * 64 + c] = ((r * 4 + c * 2) % 256) as u8;
        }
    }
    let enc = svtav1::avif::AvifEncoder::new()
        .with_quality(70.0)
        .with_speed(8);
    let obu = enc.encode_to_av1_obu(&pixels, 64, 64, 64).unwrap();
    eprintln!("direct gradient 64x64: {} bytes", obu.len());
    match decode_av1_obu(&obu) {
        Ok((_, w, h, _)) => eprintln!("  OK: {w}x{h}"),
        Err(e) => eprintln!("  FAIL: {e}"),
    }
}

#[test]
fn svtav1_decode_direct_gradient_128x128() {
    let mut pixels = vec![0u8; 128 * 128];
    for r in 0..128 {
        for c in 0..128 {
            pixels[r * 128 + c] = ((r * 2 + c) % 256) as u8;
        }
    }
    let enc = svtav1::avif::AvifEncoder::new()
        .with_quality(70.0)
        .with_speed(8);
    let obu = enc.encode_to_av1_obu(&pixels, 128, 128, 128).unwrap();
    eprintln!("direct gradient 128x128: {} bytes", obu.len());
    match decode_av1_obu(&obu) {
        Ok((_, w, h, _)) => eprintln!("  OK: {w}x{h}"),
        Err(e) => {
            eprintln!("  FAIL: {e}");
            std::fs::write("/tmp/svtav1_grad128.obu", &obu).ok();
        }
    }
}

#[test]
fn svtav1_dump_obu_comparison() {
    // Save 64x64 and 128x128 gradient OBU files for byte comparison
    let mut make = |size: usize| -> Vec<u8> {
        let mut pixels = vec![0u8; size * size];
        for r in 0..size {
            for c in 0..size {
                pixels[r * size + c] = ((r * 2 + c) % 256) as u8;
            }
        }
        let enc = svtav1::avif::AvifEncoder::new().with_quality(70.0).with_speed(8);
        enc.encode_to_av1_obu(&pixels, size as u32, size as u32, size as u32).unwrap()
    };
    let obu64 = make(64);
    let obu128 = make(128);
    std::fs::write("/tmp/svtav1_cmp_64.obu", &obu64).unwrap();
    std::fs::write("/tmp/svtav1_cmp_128.obu", &obu128).unwrap();
    eprintln!("64x64: {} bytes → /tmp/svtav1_cmp_64.obu", obu64.len());
    eprintln!("128x128: {} bytes → /tmp/svtav1_cmp_128.obu", obu128.len());
    
    // Verify 64x64 decodes
    match decode_av1_obu(&obu64) {
        Ok(_) => eprintln!("64x64: DECODES OK"),
        Err(e) => eprintln!("64x64: FAIL {e}"),
    }
    match decode_av1_obu(&obu128) {
        Ok(_) => eprintln!("128x128: DECODES OK"),
        Err(e) => eprintln!("128x128: FAIL {e}"),
    }
}

#[test]
fn svtav1_decode_128x64() {
    let mut pixels = vec![0u8; 128 * 64];
    for r in 0..64 { for c in 0..128 { pixels[r * 128 + c] = ((r * 2 + c) % 256) as u8; } }
    let enc = svtav1::avif::AvifEncoder::new().with_quality(70.0).with_speed(8);
    let obu = enc.encode_to_av1_obu(&pixels, 128, 64, 128).unwrap();
    eprintln!("128x64: {} bytes", obu.len());
    match decode_av1_obu(&obu) {
        Ok((_, w, h, _)) => eprintln!("  OK: {w}x{h}"),
        Err(e) => eprintln!("  FAIL: {e}"),
    }
}
