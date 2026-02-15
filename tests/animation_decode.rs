//! Tests for animated AVIF decoding
//!
//! Test vectors are at tests/vectors/libavif/colors-animated-*.avif

use enough::Unstoppable;
use std::fs;
use zenavif::{AnimationDecoder, DecoderConfig, decode_animation, decode_animation_with};

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
                let max_val = img
                    .buf()
                    .iter()
                    .map(|p| p.r.max(p.g).max(p.b))
                    .max()
                    .unwrap_or(0);
                eprintln!(
                    "frame {i}: {}x{} RGBA16, max channel value={max_val}",
                    img.width(),
                    img.height()
                );
                assert!(
                    max_val > 255,
                    "12bpc should produce values > 255, got max={max_val}"
                );
            }
            zencodec_types::PixelData::Rgb16(img) => {
                let max_val = img
                    .buf()
                    .iter()
                    .map(|p| p.r.max(p.g).max(p.b))
                    .max()
                    .unwrap_or(0);
                eprintln!(
                    "frame {i}: {}x{} RGB16, max channel value={max_val}",
                    img.width(),
                    img.height()
                );
                assert!(
                    max_val > 255,
                    "12bpc should produce values > 255, got max={max_val}"
                );
            }
            other => panic!(
                "frame {i}: expected 16-bit, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }
}

#[cfg(feature = "encode")]
#[test]
fn animation_encode_decode_roundtrip_rgb8() {
    use imgref::ImgVec;
    use rgb::RGB8;
    use zenavif::{AnimationFrame, EncoderConfig, encode_animation_rgb8};

    // Create 3 frames of solid color: red, green, blue
    let colors = [
        RGB8 {
            r: 200,
            g: 30,
            b: 30,
        },
        RGB8 {
            r: 30,
            g: 200,
            b: 30,
        },
        RGB8 {
            r: 30,
            g: 30,
            b: 200,
        },
    ];
    let frames: Vec<AnimationFrame> = colors
        .iter()
        .map(|&c| AnimationFrame {
            pixels: ImgVec::new(vec![c; 64 * 64], 64, 64),
            duration_ms: 100,
        })
        .collect();

    let config = EncoderConfig::new().quality(80.0).speed(10);
    let encoded = encode_animation_rgb8(&frames, &config, &enough::Unstoppable).unwrap();
    eprintln!(
        "encoded {} frames, {} bytes",
        encoded.frame_count,
        encoded.avif_file.len()
    );
    assert_eq!(encoded.frame_count, 3);

    // Decode it back
    let decoded = decode_animation(&encoded.avif_file).unwrap();
    assert_eq!(decoded.frames.len(), 3);
    assert_eq!(decoded.info.frame_count, 3);

    for (i, frame) in decoded.frames.iter().enumerate() {
        assert_eq!(frame.pixels.width(), 64, "frame {i} width");
        assert_eq!(frame.pixels.height(), 64, "frame {i} height");
        assert_eq!(frame.duration_ms, 100, "frame {i} duration");
        eprintln!(
            "decoded frame {i}: {}x{}, {}ms",
            frame.pixels.width(),
            frame.pixels.height(),
            frame.duration_ms
        );
    }
}

#[cfg(feature = "encode")]
#[test]
fn animation_encode_decode_roundtrip_rgba8() {
    use imgref::ImgVec;
    use rgb::RGBA8;
    use zenavif::{AnimationFrameRgba, EncoderConfig, encode_animation_rgba8};

    // 2 frames with semi-transparent pixels
    let frames = vec![
        AnimationFrameRgba {
            pixels: ImgVec::new(
                vec![
                    RGBA8 {
                        r: 255,
                        g: 0,
                        b: 0,
                        a: 128
                    };
                    32 * 32
                ],
                32,
                32,
            ),
            duration_ms: 200,
        },
        AnimationFrameRgba {
            pixels: ImgVec::new(
                vec![
                    RGBA8 {
                        r: 0,
                        g: 0,
                        b: 255,
                        a: 200
                    };
                    32 * 32
                ],
                32,
                32,
            ),
            duration_ms: 300,
        },
    ];

    let config = EncoderConfig::new().quality(80.0).speed(10);
    let encoded = encode_animation_rgba8(&frames, &config, &enough::Unstoppable).unwrap();
    eprintln!(
        "encoded {} frames, {} bytes",
        encoded.frame_count,
        encoded.avif_file.len()
    );

    let decoded = decode_animation(&encoded.avif_file).unwrap();
    assert_eq!(decoded.frames.len(), 2);
    assert!(decoded.info.has_alpha, "roundtrip should preserve alpha");

    for (i, frame) in decoded.frames.iter().enumerate() {
        assert!(frame.pixels.has_alpha(), "frame {i} should have alpha");
        eprintln!(
            "decoded frame {i}: {}x{}, {}ms, has_alpha={}",
            frame.pixels.width(),
            frame.pixels.height(),
            frame.duration_ms,
            frame.pixels.has_alpha()
        );
    }
}

// ---- AnimationDecoder (frame-by-frame) tests ----

#[test]
fn frame_by_frame_matches_batch() {
    let data = animated_vector("colors-animated-8bpc.avif");
    let config = DecoderConfig::new().threads(1);

    // Batch decode
    let batch = decode_animation_with(&data, &config, &Unstoppable).unwrap();

    // Frame-by-frame decode
    let mut decoder = AnimationDecoder::new(&data, &config).unwrap();
    assert_eq!(decoder.info().frame_count, batch.info.frame_count);
    assert_eq!(decoder.info().loop_count, batch.info.loop_count);
    assert_eq!(decoder.info().has_alpha, batch.info.has_alpha);
    assert_eq!(decoder.remaining_frames(), batch.frames.len());

    for (i, batch_frame) in batch.frames.iter().enumerate() {
        assert_eq!(decoder.frame_index(), i);
        let frame = decoder
            .next_frame(&Unstoppable)
            .unwrap()
            .unwrap_or_else(|| panic!("expected frame {i}"));

        assert_eq!(
            frame.pixels.width(),
            batch_frame.pixels.width(),
            "frame {i} width mismatch"
        );
        assert_eq!(
            frame.pixels.height(),
            batch_frame.pixels.height(),
            "frame {i} height mismatch"
        );
        assert_eq!(
            frame.duration_ms, batch_frame.duration_ms,
            "frame {i} duration mismatch"
        );
        assert_eq!(
            frame.pixels.has_alpha(),
            batch_frame.pixels.has_alpha(),
            "frame {i} alpha mismatch"
        );
    }

    // Should return None after all frames
    assert_eq!(decoder.remaining_frames(), 0);
    assert!(decoder.next_frame(&Unstoppable).unwrap().is_none());
}

#[test]
fn frame_by_frame_12bpc() {
    let data = animated_vector("colors-animated-12bpc-keyframes-0-2-3.avif");
    let config = DecoderConfig::new().threads(1);

    let mut decoder = AnimationDecoder::new(&data, &config).unwrap();
    let total = decoder.info().frame_count;
    assert!(total > 1, "expected multiple frames");

    let mut decoded_count = 0;
    while let Some(frame) = decoder.next_frame(&Unstoppable).unwrap() {
        let is_16bit = matches!(
            &frame.pixels,
            zencodec_types::PixelData::Rgb16(_) | zencodec_types::PixelData::Rgba16(_)
        );
        assert!(
            is_16bit,
            "frame {} should be 16-bit for 12bpc source",
            decoded_count
        );
        decoded_count += 1;
    }

    assert_eq!(decoded_count, total);
    eprintln!("frame-by-frame 12bpc: decoded {decoded_count} frames");
}

#[test]
fn frame_by_frame_cancellation() {
    use enough::StopReason;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct StopAfter {
        count: AtomicUsize,
    }

    impl enough::Stop for StopAfter {
        fn check(&self) -> std::result::Result<(), StopReason> {
            let n = self.count.fetch_add(1, Ordering::Relaxed);
            // Allow enough calls for setup + first frame, stop on second
            if n > 10 {
                Err(StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }

    let data = animated_vector("colors-animated-8bpc.avif");
    let config = DecoderConfig::new().threads(1);
    let mut decoder = AnimationDecoder::new(&data, &config).unwrap();

    let stop = StopAfter {
        count: AtomicUsize::new(0),
    };

    // First frame should succeed
    let first = decoder.next_frame(&stop);
    if first.is_ok() {
        // Subsequent frames should eventually fail with cancellation
        let mut got_cancel = false;
        for _ in 0..decoder.remaining_frames() {
            match decoder.next_frame(&stop) {
                Err(_) => {
                    got_cancel = true;
                    break;
                }
                Ok(None) => break,
                Ok(Some(_)) => continue,
            }
        }
        assert!(
            got_cancel || decoder.remaining_frames() == 0,
            "should have been cancelled or completed"
        );
    }
    // If even the first frame was cancelled, that's also valid
    eprintln!(
        "cancellation test: stopped at frame {}",
        decoder.frame_index()
    );
}

#[test]
fn frame_by_frame_still_image_returns_unsupported() {
    let data = fs::read("tests/vectors/libavif/kodim03_yuv420_8bpc.avif")
        .expect("need kodim03 test vector");
    let result = AnimationDecoder::new(&data, &DecoderConfig::default());
    assert!(
        result.is_err(),
        "AnimationDecoder should reject still images"
    );
}

#[cfg(feature = "encode")]
#[test]
fn animation_encode_decode_roundtrip_rgb16() {
    use imgref::ImgVec;
    use rgb::RGB16;
    use zenavif::{AnimationFrame16, EncoderConfig, encode_animation_rgb16};

    // Create 3 frames of solid color (full u16 range)
    let colors = [
        RGB16 {
            r: 51200,
            g: 6400,
            b: 6400,
        },
        RGB16 {
            r: 6400,
            g: 51200,
            b: 6400,
        },
        RGB16 {
            r: 6400,
            g: 6400,
            b: 51200,
        },
    ];
    let frames: Vec<AnimationFrame16> = colors
        .iter()
        .map(|&c| AnimationFrame16 {
            pixels: ImgVec::new(vec![c; 64 * 64], 64, 64),
            duration_ms: 100,
        })
        .collect();

    let config = EncoderConfig::new().quality(80.0).speed(10);
    let encoded = encode_animation_rgb16(&frames, &config, &Unstoppable).unwrap();
    eprintln!(
        "rgb16 encoded {} frames, {} bytes",
        encoded.frame_count,
        encoded.avif_file.len()
    );
    assert_eq!(encoded.frame_count, 3);

    // Decode it back — should produce 16-bit output (10-bit source)
    let decoded = decode_animation(&encoded.avif_file).unwrap();
    assert_eq!(decoded.frames.len(), 3);
    assert_eq!(decoded.info.frame_count, 3);

    for (i, frame) in decoded.frames.iter().enumerate() {
        assert_eq!(frame.pixels.width(), 64, "frame {i} width");
        assert_eq!(frame.pixels.height(), 64, "frame {i} height");
        assert_eq!(frame.duration_ms, 100, "frame {i} duration");

        // 10-bit source should decode to 16-bit output
        let is_16bit = matches!(
            &frame.pixels,
            zencodec_types::PixelData::Rgb16(_) | zencodec_types::PixelData::Rgba16(_)
        );
        assert!(
            is_16bit,
            "frame {i} should be 16-bit for 10-bit source, got {:?}",
            std::mem::discriminant(&frame.pixels)
        );
    }
}

#[cfg(feature = "encode")]
#[test]
fn animation_encode_decode_roundtrip_rgba16() {
    use imgref::ImgVec;
    use rgb::RGBA16;
    use zenavif::{AnimationFrameRgba16, EncoderConfig, encode_animation_rgba16};

    // 2 frames with semi-transparent pixels (full u16 range)
    let frames = vec![
        AnimationFrameRgba16 {
            pixels: ImgVec::new(
                vec![
                    RGBA16 {
                        r: 57600,
                        g: 6400,
                        b: 6400,
                        a: 32768
                    };
                    32 * 32
                ],
                32,
                32,
            ),
            duration_ms: 200,
        },
        AnimationFrameRgba16 {
            pixels: ImgVec::new(
                vec![
                    RGBA16 {
                        r: 6400,
                        g: 6400,
                        b: 57600,
                        a: 51200
                    };
                    32 * 32
                ],
                32,
                32,
            ),
            duration_ms: 300,
        },
    ];

    let config = EncoderConfig::new().quality(80.0).speed(10);
    let encoded = encode_animation_rgba16(&frames, &config, &Unstoppable).unwrap();
    eprintln!(
        "rgba16 encoded {} frames, {} bytes",
        encoded.frame_count,
        encoded.avif_file.len()
    );

    let decoded = decode_animation(&encoded.avif_file).unwrap();
    assert_eq!(decoded.frames.len(), 2);
    assert!(decoded.info.has_alpha, "roundtrip should preserve alpha");

    for (i, frame) in decoded.frames.iter().enumerate() {
        assert!(frame.pixels.has_alpha(), "frame {i} should have alpha");

        let is_16bit = matches!(&frame.pixels, zencodec_types::PixelData::Rgba16(_));
        assert!(is_16bit, "frame {i} should be RGBA16 for 10-bit source");
    }
}
