#![cfg(feature = "encode")]
use imgref::Img;
use rgb::{Rgb, Rgba};
use zenavif::*;
const EXIF: &[u8] = b"II\x2a\0\x08\0\0\0\0\0\0\0\0\0";
const XMP: &[u8] = b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>";
fn stop() -> almost_enough::StopToken {
    almost_enough::StopToken::new(almost_enough::Unstoppable)
}
#[test]
fn zenravif_animation_preserves_requested_container_metadata() {
    let profile = zenavif_parse::AvifParser::from_bytes(include_bytes!(
        "vectors/libavif/paris_icc_exif_xmp.avif"
    ))
    .unwrap();
    let zenavif_parse::ColorInformation::IccProfile(icc) = profile.color_info().unwrap() else {
        panic!("ICC fixture")
    };
    for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
        let frames: Vec<_> = [17, 33]
            .into_iter()
            .map(|duration_ms| AnimationFrameRgba {
                pixels: Img::new(vec![Rgba::new(75, 125, 175, 128); 31 * 33], 31, 33),
                duration_ms,
            })
            .collect();
        let base = EncoderConfig::new()
            .backend(Av1Backend::Zenravif)
            .speed(10)
            .bit_depth(depth)
            .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
            .content_light_level(1000, 400);
        let md = MasteringDisplayConfig {
            primaries: [(8500, 39850), (6550, 2300), (35400, 14600)],
            white_point: (15635, 16450),
            max_luminance: 10_000_000,
            min_luminance: 50,
        };
        let base = base.mastering_display(md);
        let config = base
            .clone()
            .icc_profile(icc.clone())
            .exif(EXIF.to_vec())
            .xmp(XMP.to_vec())
            .rotation(1)
            .mirror(0);
        for kind in 0..4 {
            let encode = |cfg: &EncoderConfig| match kind {
                0 => encode_animation_rgb8(
                    &frames
                        .iter()
                        .map(|f| AnimationFrame {
                            pixels: Img::new(
                                f.pixels.pixels().map(|p| p.rgb()).collect::<Vec<_>>(),
                                31,
                                33,
                            ),
                            duration_ms: f.duration_ms,
                        })
                        .collect::<Vec<_>>(),
                    cfg,
                    stop(),
                ),
                1 => encode_animation_rgba8(&frames, cfg, stop()),
                2 => encode_animation_rgb16(
                    &frames
                        .iter()
                        .map(|f| AnimationFrame16 {
                            pixels: Img::new(
                                f.pixels
                                    .pixels()
                                    .map(|p| {
                                        Rgb::new(
                                            u16::from(p.r) * 257,
                                            u16::from(p.g) * 257,
                                            u16::from(p.b) * 257,
                                        )
                                    })
                                    .collect::<Vec<_>>(),
                                31,
                                33,
                            ),
                            duration_ms: f.duration_ms,
                        })
                        .collect::<Vec<_>>(),
                    cfg,
                    stop(),
                ),
                _ => encode_animation_rgba16(
                    &frames
                        .iter()
                        .map(|f| AnimationFrameRgba16 {
                            pixels: Img::new(
                                f.pixels
                                    .pixels()
                                    .map(|p| {
                                        Rgba::new(
                                            u16::from(p.r) * 257,
                                            u16::from(p.g) * 257,
                                            u16::from(p.b) * 257,
                                            u16::from(p.a) * 257,
                                        )
                                    })
                                    .collect::<Vec<_>>(),
                                31,
                                33,
                            ),
                            duration_ms: f.duration_ms,
                        })
                        .collect::<Vec<_>>(),
                    cfg,
                    stop(),
                ),
            };
            let control = encode(&base).unwrap();
            let encoded = encode(&config).unwrap();
            if let Some(dir) = std::env::var_os("ZENAVIF_ENCODE_META_ARTIFACTS") {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(std::path::Path::new(&dir).join("encode-meta.icc"), icc).unwrap();
                std::fs::write(
                    std::path::Path::new(&dir).join(format!("encode-meta-{depth:?}-{kind}.avif")),
                    &encoded.avif_file,
                )
                .unwrap();
            }
            let p = zenavif_parse::AvifParser::from_bytes(&encoded.avif_file).unwrap();
            assert_eq!(
                p.animation_exif().transpose().unwrap().as_deref(),
                Some(EXIF)
            );
            assert_eq!(p.animation_xmp().transpose().unwrap().as_deref(), Some(XMP));
            assert_eq!(p.exif().transpose().unwrap().as_deref(), Some(EXIF));
            assert_eq!(p.xmp().transpose().unwrap().as_deref(), Some(XMP));
            for color in [p.color_info(), p.animation_color_info()] {
                let Some(zenavif_parse::ColorInformation::IccProfile(actual)) = color else {
                    panic!("ICC lost")
                };
                assert_eq!(actual, icc);
            }
            let info = p.animation_info().unwrap();
            assert_eq!(info.spatial.rotation, Some(ImageRotation { angle: 90 }));
            assert_eq!(info.spatial.mirror, Some(ImageMirror { axis: 0 }));
            let mastering = info.hdr.mastering_display.unwrap();
            assert_eq!(mastering.primaries, md.primaries);
            assert_eq!(mastering.white_point, md.white_point);
            assert_eq!(mastering.max_luminance, md.max_luminance);
            assert_eq!(mastering.min_luminance, md.min_luminance);
            assert_eq!(p.mastering_display(), Some(&mastering));
            for backend in [
                DecodeBackend::Rav1dSafe,
                #[cfg(feature = "zenav1-aom")]
                DecodeBackend::Zenav1Aom,
            ] {
                let mut decoder = ManagedAvifDecoder::new(
                    &encoded.avif_file,
                    &DecoderConfig::new().decode_backend(backend),
                )
                .unwrap();
                let probe = decoder.probe_animation_info().unwrap();
                assert_eq!(probe.icc_profile.as_deref(), Some(icc.as_slice()));
                assert_eq!(probe.exif.as_deref(), Some(EXIF));
                assert_eq!(probe.xmp.as_deref(), Some(XMP));
                let decoded = decoder.decode_animation(&Unstoppable).unwrap();
                assert_eq!(decoded.frames.len(), 2);
                assert_eq!(decoded.info.exif.as_deref(), Some(EXIF));
                assert_eq!(decoded.info.xmp.as_deref(), Some(XMP));
                assert_eq!(decoded.info.spatial, info.spatial);
                assert_eq!(decoded.info.hdr, info.hdr);
            }
            let clli = info.hdr.content_light_level.unwrap();
            assert_eq!(
                (
                    clli.max_content_light_level,
                    clli.max_pic_average_light_level
                ),
                (1000, 400)
            );
            let reference = zenavif_parse::AvifParser::from_bytes(&control.avif_file).unwrap();
            for i in 0..2 {
                let frame = p.frame(i).unwrap();
                let original = reference.frame(i).unwrap();
                assert_eq!(frame.data, original.data);
                assert_eq!(frame.alpha_data, original.alpha_data);
                assert_eq!(
                    p.frame_timing(i).unwrap(),
                    reference.frame_timing(i).unwrap()
                );
            }
        }
    }
}

#[test]
fn zenravif_animation_premultiplication_matches_decoded_pixels() {
    for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
        let frames: Vec<_> = [0, 128, 255]
            .into_iter()
            .map(|a| AnimationFrameRgba {
                pixels: Img::new(vec![Rgba::new(75, 125, 175, a); 32 * 32], 32, 32),
                duration_ms: 17,
            })
            .collect();
        let config = EncoderConfig::new()
            .backend(Av1Backend::Zenravif)
            .speed(10)
            .quality(100.0)
            .bit_depth(depth)
            .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
            .alpha_color_mode(EncodeAlphaMode::Premultiplied);
        let encoded = encode_animation_rgba8(&frames, &config, stop()).unwrap();
        if let Some(dir) = std::env::var_os("ZENAVIF_ENCODE_META_ARTIFACTS") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                std::path::Path::new(&dir).join(format!("encode-prem-{depth:?}.avif")),
                &encoded.avif_file,
            )
            .unwrap();
        }
        let parser = zenavif_parse::AvifParser::from_bytes(&encoded.avif_file).unwrap();
        assert_eq!(parser.animation_premultiplied_alpha(), Some(true));
        assert!(parser.premultiplied_alpha());
        for backend in [
            DecodeBackend::Rav1dSafe,
            #[cfg(feature = "zenav1-aom")]
            DecodeBackend::Zenav1Aom,
        ] {
            let mut decoder = ManagedAvifDecoder::new(
                &encoded.avif_file,
                &DecoderConfig::new().decode_backend(backend),
            )
            .unwrap();
            let decoded = decoder.decode_animation(&Unstoppable).unwrap();
            assert_eq!(decoded.frames.len(), 3);
            for (frame, alpha) in decoded.frames.iter().zip([0u8, 128, 255]) {
                if let Some(img) = frame.pixels.try_as_imgref::<Rgba<u8>>() {
                    for p in img.pixels() {
                        assert!(p.a.abs_diff(alpha) <= 1);
                        if alpha != 0 {
                            for (v, want) in [(p.r, 75), (p.g, 125), (p.b, 175)] {
                                assert!(
                                    v.abs_diff(want) <= 5,
                                    "{backend:?} {depth:?} alpha={alpha}: {v} vs {want}"
                                );
                            }
                        }
                    }
                } else {
                    let img = frame.pixels.try_as_imgref::<Rgba<u16>>().unwrap();
                    for p in img.pixels() {
                        assert!(p.a.abs_diff(u16::from(alpha) * 257) <= 257);
                        if alpha != 0 {
                            for (v, want) in [(p.r, 75u16), (p.g, 125), (p.b, 175)] {
                                assert!(
                                    v.abs_diff(want * 257) <= 5 * 257,
                                    "{backend:?} {depth:?} alpha={alpha}: {v} vs {}",
                                    want * 257
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn invalid_animation_spatial_metadata_returns_a_serialization_error() {
    let frames = [AnimationFrameRgba {
        pixels: Img::new(vec![Rgba::new(75, 125, 175, 128); 32 * 32], 32, 32),
        duration_ms: 17,
    }];
    let base = EncoderConfig::new().backend(Av1Backend::Zenravif).speed(10);
    for config in [base.clone().rotation(4), base.mirror(2)] {
        let error = match encode_animation_rgba8(&frames, &config, stop()) {
            Ok(_) => panic!("invalid spatial metadata silently accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("rotation must be"), "{error}");
    }
}
