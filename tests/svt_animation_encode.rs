#![cfg(feature = "zenav1-svt")]
#[path = "support/animation.rs"]
mod animation_support;

use imgref::Img;
use rgb::{Rgb, Rgba};
use zenavif::*;

fn config(depth: EncodeBitDepth, speed: u8) -> EncoderConfig {
    EncoderConfig::new()
        .backend(Av1Backend::Zenav1Svt)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
        .bit_depth(depth)
        .quality(85.0)
        .speed(speed)
}
fn stop() -> almost_enough::StopToken {
    almost_enough::StopToken::new(almost_enough::Unstoppable)
}

#[test]
fn svt_public_animation_encodes_all_four_input_types() {
    let (w, h) = (65, 67);
    for speed in [1, 6, 10] {
        for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
            let cfg = config(depth, speed);
            let rgb8: Vec<_> = [17, 33, 101]
                .into_iter()
                .enumerate()
                .map(|(i, duration_ms)| AnimationFrame {
                    pixels: Img::new(
                        (0..w * h)
                            .map(|n| Rgb {
                                r: ((n % w * 2 + i * 31) % 256) as u8,
                                g: ((n / w * 2 + i * 47) % 256) as u8,
                                b: (i * 83) as u8,
                            })
                            .collect::<Vec<_>>(),
                        w,
                        h,
                    ),
                    duration_ms,
                })
                .collect();
            let rgba8: Vec<_> = rgb8
                .iter()
                .enumerate()
                .map(|(i, f)| AnimationFrameRgba {
                    pixels: Img::new(
                        f.pixels
                            .buf()
                            .iter()
                            .enumerate()
                            .map(|(n, p)| Rgba {
                                r: p.r,
                                g: p.g,
                                b: p.b,
                                a: if i == 0 { 255 } else { (64 + n % 192) as u8 },
                            })
                            .collect::<Vec<_>>(),
                        w,
                        h,
                    ),
                    duration_ms: f.duration_ms,
                })
                .collect();
            let rgb16: Vec<_> = rgb8
                .iter()
                .map(|f| AnimationFrame16 {
                    pixels: Img::new(
                        f.pixels
                            .buf()
                            .iter()
                            .map(|p| Rgb {
                                r: u16::from(p.r) * 257,
                                g: u16::from(p.g) * 257,
                                b: u16::from(p.b) * 257,
                            })
                            .collect::<Vec<_>>(),
                        w,
                        h,
                    ),
                    duration_ms: f.duration_ms,
                })
                .collect();
            let rgba16: Vec<_> = rgba8
                .iter()
                .map(|f| AnimationFrameRgba16 {
                    pixels: Img::new(
                        f.pixels
                            .buf()
                            .iter()
                            .map(|p| Rgba {
                                r: u16::from(p.r) * 257,
                                g: u16::from(p.g) * 257,
                                b: u16::from(p.b) * 257,
                                a: u16::from(p.a) * 257,
                            })
                            .collect::<Vec<_>>(),
                        w,
                        h,
                    ),
                    duration_ms: f.duration_ms,
                })
                .collect();
            let outputs = [
                (encode_animation_rgb8(&rgb8, &cfg, stop()).unwrap(), false),
                (encode_animation_rgba8(&rgba8, &cfg, stop()).unwrap(), true),
                (encode_animation_rgb16(&rgb16, &cfg, stop()).unwrap(), false),
                (
                    encode_animation_rgba16(&rgba16, &cfg, stop()).unwrap(),
                    true,
                ),
            ];
            for (encoded, alpha) in outputs {
                assert_eq!(encoded.frame_count, 3);
                assert_eq!(encoded.total_duration_ms, 151);
                let parser = zenavif_parse::AvifParser::from_bytes(&encoded.avif_file).unwrap();
                assert_eq!(
                    parser.av1_config().unwrap().bit_depth,
                    if depth == EncodeBitDepth::Eight {
                        8
                    } else {
                        10
                    }
                );
                for frame in parser.frames() {
                    let frame = frame.unwrap();
                    assert_ne!(
                        frame.data.first(),
                        Some(&0x12),
                        "track samples omit temporal delimiters"
                    );
                    #[cfg(feature = "zenav1-aom")]
                    for payload in
                        std::iter::once(frame.data.as_ref()).chain(frame.alpha_data.as_deref())
                    {
                        let safe = decode_av1_obu_yuv(payload, DecodeBackend::Rav1dSafe).unwrap();
                        let aom = decode_av1_obu_yuv(payload, DecodeBackend::Zenav1Aom).unwrap();
                        assert_eq!((safe.y, safe.u, safe.v), (aom.y, aom.u, aom.v));
                    }
                    let meta =
                        zenavif_parse::AV1Metadata::parse_av1_bitstream(&frame.data).unwrap();
                    assert!(!meta.still_picture, "animations need full sequence headers");
                    assert_eq!(frame.alpha_data.is_some(), alpha);
                    if let Some(a) = frame.alpha_data {
                        let alpha_meta =
                            zenavif_parse::AV1Metadata::parse_av1_bitstream(&a).unwrap();
                        assert!(alpha_meta.monochrome);
                        assert!(!alpha_meta.still_picture);
                        assert_eq!(alpha_meta.bit_depth, meta.bit_depth);
                    }
                }
                let decoded = decode_animation(&encoded.avif_file).unwrap();
                assert_eq!(decoded.frames.len(), 3);
                assert_eq!(decoded.info.has_alpha, alpha);
                assert_eq!(decoded.info.timescale, 1000);
                for (f, duration) in decoded.frames.iter().zip([17, 33, 101]) {
                    assert_eq!((f.pixels.width(), f.pixels.height()), (w as u32, h as u32));
                    assert_eq!(f.duration_ms, duration);
                }
                assert_ne!(
                    decoded.frames[0].pixels.as_slice().row(0),
                    decoded.frames[1].pixels.as_slice().row(0)
                );
            }
        }
    }
}

#[test]
fn svt_animation_rejects_invalid_frame_layout_and_duration() {
    let cfg = config(EncodeBitDepth::Eight, 10);
    assert!(encode_animation_rgb8(&[], &cfg, stop()).is_err());
    let frame = |w, h, duration_ms| AnimationFrame {
        pixels: Img::new(
            vec![
                Rgb {
                    r: 64,
                    g: 128,
                    b: 192
                };
                w * h
            ],
            w,
            h,
        ),
        duration_ms,
    };
    assert!(encode_animation_rgb8(&[frame(64, 64, 0)], &cfg, stop()).is_err());
    assert!(encode_animation_rgb8(&[frame(64, 64, 1), frame(65, 64, 1)], &cfg, stop()).is_err());
    let result =
        encode_animation_rgb8(&[frame(64, 64, u32::MAX), frame(64, 64, 1)], &cfg, stop()).unwrap();
    assert_eq!(result.total_duration_ms, u64::from(u32::MAX) + 1);
    let parser = zenavif_parse::AvifParser::from_bytes(&result.avif_file).unwrap();
    assert_eq!(
        parser.frame_timing(1).unwrap().pts_in_timescales,
        u64::from(u32::MAX)
    );
}

#[test]
fn svt_animation_preserves_hdr_metadata_and_premultiplied_alpha() {
    let frame = AnimationFrameRgba {
        pixels: Img::new(
            vec![
                Rgba {
                    r: 32,
                    g: 64,
                    b: 96,
                    a: 128,
                };
                64 * 64
            ],
            64,
            64,
        ),
        duration_ms: 25,
    };
    let cfg = config(EncodeBitDepth::Eight, 10)
        .alpha_color_mode(EncodeAlphaMode::Premultiplied)
        .content_light_level(1000, 400)
        .xmp(b"<x:xmpmeta>animation</x:xmpmeta>".to_vec());
    let encoded = encode_animation_rgba8(&[frame.clone(), frame.clone()], &cfg, stop()).unwrap();
    let parser = zenavif_parse::AvifParser::from_bytes(&encoded.avif_file).unwrap();
    assert!(parser.premultiplied_alpha());
    assert_eq!(
        parser.xmp().unwrap().unwrap().as_ref(),
        b"<x:xmpmeta>animation</x:xmpmeta>"
    );
    let decoded = decode_animation(&encoded.avif_file).unwrap();
    let clli = decoded.info.hdr.content_light_level.unwrap();
    assert_eq!(
        (
            clli.max_content_light_level,
            clli.max_pic_average_light_level
        ),
        (1000, 400)
    );
    for f in &decoded.frames {
        let img = f.pixels.try_as_imgref::<Rgba<u8>>().unwrap();
        for p in img.pixels() {
            for (actual, expected) in [(p.r, 64u8), (p.g, 128), (p.b, 192), (p.a, 128)] {
                assert!(
                    actual.abs_diff(expected) <= 5,
                    "premultiplied channel: {actual} vs {expected}"
                );
            }
        }
    }
    let still = encode_rgba8(frame.pixels.as_ref(), &cfg, stop()).unwrap();
    assert!(
        zenavif_parse::AvifParser::from_bytes(&still.avif_file)
            .unwrap()
            .premultiplied_alpha()
    );
}

#[test]
fn svt_animation_is_wired_through_codec_traits() {
    use zencodec::encode::{AnimationFrameEncoder as _, EncodeJob as _, EncoderConfig as _};
    let mut cfg = AvifEncoderConfig::new();
    *cfg.inner_mut() = config(EncodeBitDepth::Ten, 6);
    let mut enc = cfg
        .job()
        .with_canvas_size(65, 67)
        .animation_frame_encoder()
        .unwrap();
    for (i, duration) in [25, 50, 75].into_iter().enumerate() {
        let img = Img::new(
            vec![
                Rgba {
                    r: (i * 40) as u8,
                    g: 80,
                    b: 120,
                    a: 128
                };
                65 * 67
            ],
            65,
            67,
        );
        enc.push_frame(
            zenpixels::PixelSlice::from(img.as_ref()).erase(),
            duration,
            None,
        )
        .unwrap();
    }
    let encoded = enc.finish(None).unwrap();
    let decoded = decode_animation(encoded.data()).unwrap();
    assert_eq!(decoded.frames.len(), 3);
    assert!(decoded.info.has_alpha);
    assert_eq!(
        decoded
            .frames
            .iter()
            .map(|f| f.duration_ms)
            .collect::<Vec<_>>(),
        [25, 50, 75]
    );
}

#[test]
fn animation_alpha_uses_track_reference_with_or_without_poster() {
    use std::borrow::Cow;
    use zencodec::decode::{AnimationFrameDecoder as _, DecodeJob as _, DecoderConfig as _};
    for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
        let frame = AnimationFrameRgba {
            pixels: Img::new(
                vec![
                    Rgba {
                        r: 32,
                        g: 64,
                        b: 96,
                        a: 128
                    };
                    65 * 67
                ],
                65,
                67,
            ),
            duration_ms: 25,
        };
        let cfg = config(depth, 6).alpha_color_mode(EncodeAlphaMode::Premultiplied);
        let encoded =
            encode_animation_rgba8(&[frame.clone(), frame.clone()], &cfg, stop()).unwrap();
        let straight = encode_animation_rgba8(
            &[frame.clone(), frame],
            &cfg.clone()
                .alpha_color_mode(EncodeAlphaMode::UnassociatedDirty),
            stop(),
        )
        .unwrap();
        let straight_pixels = decode_animation(&straight.avif_file).unwrap();
        let expected = decode_animation(&encoded.avif_file).unwrap();
        for scenario in 0..3 {
            let keep_poster = scenario != 0;
            let track_premultiplied = scenario != 2;
            let mut data = encoded.avif_file.clone();
            if keep_poster {
                let refs: Vec<_> = data
                    .windows(4)
                    .enumerate()
                    .filter_map(|(i, b)| (b == b"prem").then_some(i))
                    .collect();
                assert_eq!(refs.len(), 2);
                // The poster declares straight alpha while the track remains premultiplied.
                let removed = if track_premultiplied {
                    refs[0]
                } else {
                    refs[1]
                };
                data[removed..removed + 4].copy_from_slice(b"free");
            } else {
                animation_support::remove_poster(&mut data);
            }
            assert_eq!(
                zenavif_parse::AvifParser::from_bytes(&data)
                    .unwrap()
                    .animation_premultiplied_alpha(),
                Some(track_premultiplied)
            );
            assert_eq!(
                zenavif_parse::AvifParser::from_owned(data.clone())
                    .unwrap()
                    .animation_premultiplied_alpha(),
                Some(track_premultiplied)
            );
            let info = ManagedAvifDecoder::new(&data, &DecoderConfig::new())
                .unwrap()
                .probe_info()
                .unwrap();
            assert!(info.has_alpha);
            assert_eq!(info.premultiplied_alpha, scenario != 1);
            for backend in [
                DecodeBackend::Rav1dSafe,
                #[cfg(feature = "zenav1-aom")]
                DecodeBackend::Zenav1Aom,
            ] {
                let mut cfg = AvifDecoderConfig::new();
                *cfg.inner_mut() = DecoderConfig::new().decode_backend(backend);
                let mut decoder = cfg
                    .job()
                    .animation_frame_decoder(Cow::Borrowed(&data), &[])
                    .unwrap();
                assert!(decoder.info().has_alpha);
                let reference = if track_premultiplied {
                    &expected
                } else {
                    &straight_pixels
                };
                for expected in &reference.frames {
                    let actual = decoder.render_next_frame(None).unwrap().unwrap();
                    for row in 0..67 {
                        assert_eq!(
                            actual.pixels().row(row),
                            expected.pixels.as_slice().row(row),
                            "{backend:?} {depth:?}"
                        );
                    }
                }
                assert!(decoder.render_next_frame(None).unwrap().is_none());
            }
        }
    }
}
