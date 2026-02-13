//! Tests for animated AVIF decoding
//!
//! Test vectors are at tests/vectors/libavif/colors-animated-*.avif

use enough::Unstoppable;
use std::fs;
use zenavif::{DecoderConfig, decode_animation, decode_animation_with};

fn animated_vector(name: &str) -> Vec<u8> {
    let path = format!("tests/vectors/libavif/{name}");
    fs::read(&path).unwrap_or_else(|e| panic!("Failed to read {path}: {e}"))
}

#[test]
fn decode_8bpc_no_alpha() {
    let data = animated_vector("colors-animated-8bpc.avif");
    let anim = decode_animation(&data).unwrap();

    assert!(anim.frames.len() > 1, "expected multiple frames");
    assert!(!anim.info.has_alpha, "8bpc no-alpha should not have alpha");
    assert_eq!(anim.info.loop_count, 1, "play-once = loop_count 1");

    // All frames should have consistent dimensions
    let first = &anim.frames[0];
    let w = first.pixels.width();
    let h = first.pixels.height();
    assert!(w > 0 && h > 0, "frame dimensions should be positive");
    for (i, frame) in anim.frames.iter().enumerate() {
        assert_eq!(frame.pixels.width(), w, "frame {i} width mismatch");
        assert_eq!(frame.pixels.height(), h, "frame {i} height mismatch");
        assert!(
            frame.duration_ms > 0,
            "frame {i} should have nonzero duration"
        );
    }

    eprintln!(
        "8bpc no-alpha: {} frames, {}x{}, loop={}",
        anim.frames.len(),
        w,
        h,
        anim.info.loop_count
    );
}

#[test]
fn decode_8bpc_with_alpha() {
    let data = animated_vector("colors-animated-8bpc-alpha-exif-xmp.avif");
    let anim = decode_animation(&data).unwrap();

    assert!(anim.frames.len() > 1, "expected multiple frames");
    assert!(anim.info.has_alpha, "should have alpha track");
    assert_eq!(anim.info.loop_count, 0, "infinite loop = loop_count 0");

    // All frames should be RGBA since we have alpha
    for (i, frame) in anim.frames.iter().enumerate() {
        assert!(
            frame.pixels.has_alpha(),
            "frame {i} should have alpha channel"
        );
    }

    eprintln!(
        "8bpc alpha: {} frames, {}x{}, loop={}",
        anim.frames.len(),
        anim.frames[0].pixels.width(),
        anim.frames[0].pixels.height(),
        anim.info.loop_count
    );
}

#[test]
fn decode_12bpc_keyframes() {
    let data = animated_vector("colors-animated-12bpc-keyframes-0-2-3.avif");
    let anim = decode_animation(&data).unwrap();

    assert!(anim.frames.len() > 1, "expected multiple frames");

    // 12bpc should produce 16-bit output
    for (i, frame) in anim.frames.iter().enumerate() {
        let is_16bit = matches!(
            &frame.pixels,
            zencodec_types::PixelData::Rgb16(_) | zencodec_types::PixelData::Rgba16(_)
        );
        assert!(is_16bit, "frame {i} should be 16-bit for 12bpc source");
    }

    eprintln!(
        "12bpc: {} frames, {}x{}, has_alpha={}",
        anim.frames.len(),
        anim.frames[0].pixels.width(),
        anim.frames[0].pixels.height(),
        anim.info.has_alpha,
    );
}

#[test]
fn decode_8bpc_audio_track_skipped() {
    // This file has color + audio tracks; audio should be skipped
    let data = animated_vector("colors-animated-8bpc-audio.avif");
    let anim = decode_animation(&data).unwrap();

    assert!(anim.frames.len() > 1, "expected multiple frames");
    // Audio track should not cause errors or appear as alpha
    eprintln!(
        "8bpc audio: {} frames, has_alpha={}",
        anim.frames.len(),
        anim.info.has_alpha,
    );
}

#[test]
fn decode_8bpc_depth() {
    let data = animated_vector("colors-animated-8bpc-depth-exif-xmp.avif");
    let anim = decode_animation(&data).unwrap();

    assert!(anim.frames.len() > 1, "expected multiple frames");
    eprintln!(
        "8bpc depth: {} frames, has_alpha={}",
        anim.frames.len(),
        anim.info.has_alpha,
    );
}

#[test]
fn still_image_returns_unsupported() {
    // A non-animated AVIF should return Error::Unsupported
    let data = fs::read("tests/vectors/libavif/kodim03_yuv420_8bpc.avif")
        .expect("need kodim03 test vector");
    let result = decode_animation(&data);
    assert!(
        result.is_err(),
        "still image should fail for animation decode"
    );
}

#[test]
fn animation_with_config_and_cancellation() {
    let data = animated_vector("colors-animated-8bpc.avif");
    let config = DecoderConfig::new().threads(1);
    let anim = decode_animation_with(&data, &config, &Unstoppable).unwrap();
    assert!(anim.frames.len() > 1);
}

#[test]
fn frame_durations_sum_positive() {
    let data = animated_vector("colors-animated-8bpc.avif");
    let anim = decode_animation(&data).unwrap();

    let total_ms: u64 = anim.frames.iter().map(|f| f.duration_ms as u64).sum();
    assert!(total_ms > 0, "total animation duration should be positive");
    eprintln!(
        "total duration: {}ms across {} frames",
        total_ms,
        anim.frames.len()
    );
}

#[test]
fn decode_12bpc_produces_16bit_with_full_range() {
    let data = animated_vector("colors-animated-12bpc-keyframes-0-2-3.avif");
    let anim = decode_animation(&data).unwrap();

    for (i, frame) in anim.frames.iter().enumerate() {
        match &frame.pixels {
            zencodec_types::PixelData::Rgba16(img) => {
                // Check that at least some pixels use values > 255 (proving 16-bit)
                let max_val = img.buf().iter().map(|p| p.r.max(p.g).max(p.b)).max().unwrap_or(0);
                eprintln!("frame {i}: {}x{} RGBA16, max channel value={max_val}", img.width(), img.height());
                assert!(max_val > 255, "12bpc should produce values > 255, got max={max_val}");
            }
            zencodec_types::PixelData::Rgb16(img) => {
                let max_val = img.buf().iter().map(|p| p.r.max(p.g).max(p.b)).max().unwrap_or(0);
                eprintln!("frame {i}: {}x{} RGB16, max channel value={max_val}", img.width(), img.height());
                assert!(max_val > 255, "12bpc should produce values > 255, got max={max_val}");
            }
            other => panic!("frame {i}: expected 16-bit, got {:?}", std::mem::discriminant(other)),
        }
    }
}
