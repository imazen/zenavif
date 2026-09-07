#![cfg(feature = "encode-imazen")]
use imgref::Img;
use rgb::{RGB8, RGBA8};
use zenavif::*;

fn pixel(n: usize, i: usize) -> RGB8 {
    RGB8::new(
        (i * 17 + n * 13) as u8,
        (i * 31 + n * 7) as u8,
        (i * 7 + n * 19) as u8,
    )
}

#[test]
fn animation_color_settings_survive_public_dispatch_and_depth_conversion() {
    let rgb = [0, 1].map(|n| (0..65 * 67).map(|i| pixel(n, i)).collect::<Vec<_>>());
    let a = [0, 1].map(|n| TimedAnimationFrame {
        pixels: Img::new(rgb[n].clone(), 65, 67),
        duration_ticks: 1001,
    });
    let b = [0, 1].map(|n| TimedAnimationFrame {
        pixels: Img::new(
            rgb[n]
                .iter()
                .map(|p| RGBA8::new(p.r, p.g, p.b, 128))
                .collect::<Vec<_>>(),
            65,
            67,
        ),
        duration_ticks: 1001,
    });
    let c = [0, 1].map(|n| TimedAnimationFrame {
        pixels: Img::new(
            rgb[n]
                .iter()
                .map(|p| {
                    rgb::RGB::new(
                        u16::from(p.r) * 257,
                        u16::from(p.g) * 257,
                        u16::from(p.b) * 257,
                    )
                })
                .collect::<Vec<_>>(),
            65,
            67,
        ),
        duration_ticks: 1001,
    });
    let d = [0, 1].map(|n| TimedAnimationFrame {
        pixels: Img::new(
            rgb[n]
                .iter()
                .map(|p| {
                    rgb::RGBA::new(
                        u16::from(p.r) * 257,
                        u16::from(p.g) * 257,
                        u16::from(p.b) * 257,
                        128 * 257,
                    )
                })
                .collect::<Vec<_>>(),
            65,
            67,
        ),
        duration_ticks: 1001,
    });
    for (name, chroma, model, range) in [
        (
            "420full",
            EncodeChromaSubsampling::Yuv420,
            EncodeColorModel::YCbCr,
            EncodePixelRange::Full,
        ),
        (
            "420limited",
            EncodeChromaSubsampling::Yuv420,
            EncodeColorModel::YCbCr,
            EncodePixelRange::Limited,
        ),
        (
            "444full",
            EncodeChromaSubsampling::Yuv444,
            EncodeColorModel::YCbCr,
            EncodePixelRange::Full,
        ),
        (
            "444limited",
            EncodeChromaSubsampling::Yuv444,
            EncodeColorModel::YCbCr,
            EncodePixelRange::Limited,
        ),
        (
            "rgb",
            EncodeChromaSubsampling::Yuv444,
            EncodeColorModel::Rgb,
            EncodePixelRange::Full,
        ),
    ] {
        for depth in [8, 10] {
            let config = EncoderConfig::new()
                .backend(Av1Backend::Zenravif)
                .speed(10)
                .threads(Some(1))
                .with_lossless(true)
                .chroma_subsampling(chroma)
                .color_model(model)
                .pixel_range(range)
                .bit_depth(if depth == 8 {
                    EncodeBitDepth::Eight
                } else {
                    EncodeBitDepth::Ten
                });
            let stop = || almost_enough::StopToken::new(almost_enough::Unstoppable);
            let results = [
                encode_animation_rgb8_timed(&a, 30000, &config, stop()),
                encode_animation_rgba8_timed(&b, 30000, &config, stop()),
                encode_animation_rgb16_timed(&c, 30000, &config, stop()),
                encode_animation_rgba16_timed(&d, 30000, &config, stop()),
            ];
            for (kind, result) in results.into_iter().enumerate() {
                let encoded = result.unwrap();
                assert_eq!(
                    (encoded.timescale, encoded.total_duration_ticks),
                    (30000, 2002)
                );
                let parser = zenavif_parse::AvifParser::from_bytes(&encoded.avif_file).unwrap();
                let cfg = parser.av1_config().unwrap();
                let subsampled = u8::from(chroma == EncodeChromaSubsampling::Yuv420);
                assert_eq!(
                    (
                        cfg.profile,
                        cfg.bit_depth,
                        cfg.chroma_subsampling_x,
                        cfg.chroma_subsampling_y
                    ),
                    (1 - subsampled, depth, subsampled, subsampled)
                );
                for color in [parser.nclx_color_info(), parser.animation_nclx_color_info()] {
                    let Some(zenavif_parse::ColorInformation::Nclx {
                        matrix_coefficients,
                        full_range,
                        ..
                    }) = color
                    else {
                        panic!("missing color signaling")
                    };
                    assert_eq!(*matrix_coefficients, if name == "rgb" { 0 } else { 6 });
                    assert_eq!(*full_range, range == EncodePixelRange::Full);
                }
                let frames = parser.frames().map(Result::unwrap).collect::<Vec<_>>();
                assert_eq!(frames.len(), 2);
                assert!(
                    frames
                        .iter()
                        .all(|f| f.alpha_data.is_some() == (kind % 2 == 1))
                );
                let decoded = decode_animation(&encoded.avif_file).unwrap();
                assert_eq!(decoded.frames.len(), 2);
                assert_eq!(decoded.info.has_alpha, kind % 2 == 1);
                if let Some(dir) = std::env::var_os("ZENAVIF_COLOR_ARTIFACTS") {
                    let dir = std::path::PathBuf::from(dir);
                    std::fs::create_dir_all(&dir).unwrap();
                    let stem = format!("color-{name}-{depth}-{kind}");
                    std::fs::write(dir.join(format!("{stem}.avif")), &encoded.avif_file).unwrap();
                    let color: Vec<_> =
                        frames.iter().flat_map(|f| f.data.iter().copied()).collect();
                    std::fs::write(dir.join(format!("{stem}.obu")), color).unwrap();
                    let sample = |v: u8| -> Vec<u8> {
                        if depth == 8 {
                            vec![v]
                        } else {
                            ((u16::from(v) * 257) >> 6).to_le_bytes().to_vec()
                        }
                    };
                    if name == "rgb" {
                        let mut source = Vec::new();
                        for frame in &rgb {
                            for plane in 0..3 {
                                for p in frame {
                                    source.extend(sample([p.g, p.b, p.r][plane]));
                                }
                            }
                        }
                        std::fs::write(dir.join(format!("{stem}.source.yuv")), source).unwrap();
                    }
                    if kind % 2 == 1 {
                        let alpha: Vec<_> = frames
                            .iter()
                            .flat_map(|f| f.alpha_data.as_ref().unwrap().iter().copied())
                            .collect();
                        std::fs::write(dir.join(format!("{stem}.alpha.obu")), alpha).unwrap();
                        let source: Vec<_> = (0..2 * 65 * 67).flat_map(|_| sample(128)).collect();
                        std::fs::write(dir.join(format!("{stem}.alpha.source.yuv")), source)
                            .unwrap();
                    }
                }
            }
        }
    }
}
