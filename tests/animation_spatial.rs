#![cfg(feature = "zenav1-svt")]
#[path = "support/animation.rs"]
mod animation_support;

use std::borrow::Cow;
use zenavif::*;
use zenavif_serialize::{
    Av1CBox,
    animated::{AnimFrame, AnimatedImage, CropRect},
};
use zencodec::decode::{AnimationFrameDecoder as _, DecodeJob as _, DecoderConfig as _};

fn sequence_header(sample: &[u8]) -> &[u8] {
    assert_eq!(sample[0], 0x0a);
    assert!(sample[1] < 128);
    &sample[..2 + usize::from(sample[1])]
}

#[test]
fn animation_crop_and_orientation_use_track_and_transform_every_pixel() {
    let (width, height) = (65usize, 67usize);
    for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
        let input: Vec<_> = (0..2)
            .map(|f| AnimationFrameRgba {
                pixels: imgref::Img::new(
                    (0..width * height)
                        .map(|n| {
                            rgb::Rgba::new(
                                (n % width * 3 + f * 17) as u8,
                                (n / width * 3 + f * 23) as u8,
                                ((n % width + n / width) * 2) as u8,
                                (80 + n % 176) as u8,
                            )
                        })
                        .collect::<Vec<_>>(),
                    width,
                    height,
                ),
                duration_ms: 17 + f as u32 * 16,
            })
            .collect();
        let encoded = encode_animation_rgba8(
            &input,
            &EncoderConfig::new()
                .backend(Av1Backend::Zenav1Svt)
                .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
                .bit_depth(depth)
                .speed(6),
            almost_enough::StopToken::new(almost_enough::Unstoppable),
        )
        .unwrap();
        let control = decode_animation(&encoded.avif_file).unwrap();
        let parser = zenavif_parse::AvifParser::from_bytes(&encoded.avif_file).unwrap();
        let config = parser.av1_config().unwrap();
        let mut color = Av1CBox::default();
        color.seq_profile = config.profile;
        color.seq_level_idx_0 = config.level;
        color.seq_tier_0 = config.tier != 0;
        color.high_bitdepth = config.bit_depth > 8;
        color.chroma_sample_position = config.chroma_sample_position;
        let mut alpha = color.clone();
        alpha.monochrome = true;
        let refs: Vec<_> = (0..2).map(|i| parser.frame(i).unwrap()).collect();
        let frames: Vec<_> = refs
            .iter()
            .map(|f| {
                AnimFrame::new(&f.data, f.duration_ms)
                    .with_alpha(f.alpha_data.as_ref().unwrap())
                    .with_sync(true)
            })
            .collect();
        for rotation in [1u8, 0, 2, 3] {
            for mirror in [Some(1u8), None, Some(0)] {
                let mut mux = AnimatedImage::new();
                mux.set_color_config(color.clone())
                    .set_alpha_config(alpha.clone())
                    .set_color_description(1, 13, 6, true)
                    .set_crop(CropRect::new(3, 5, 49, 53))
                    .set_rotation(rotation)
                    .set_pixel_aspect_ratio(1, 1);
                if let Some(m) = mirror {
                    mux.set_mirror(m);
                }
                let original = mux
                    .try_serialize(
                        width as u32,
                        height as u32,
                        &frames,
                        sequence_header(&refs[0].data),
                        Some(sequence_header(refs[0].alpha_data.as_ref().unwrap())),
                    )
                    .unwrap();
                for scenario in 0..3 {
                    let mut data = original.clone();
                    let positions = |d: &[u8], kind: &[u8; 4]| {
                        d.windows(4)
                            .enumerate()
                            .filter_map(|(i, b)| (b == kind).then_some(i))
                            .collect::<Vec<_>>()
                    };
                    if scenario == 0 {
                        let rot = positions(&data, b"irot");
                        assert_eq!(rot.len(), 2);
                        data[rot[0] + 4] = (rotation + 1) % 4;
                        let clap = positions(&data, b"clap");
                        assert_eq!(clap.len(), 2);
                        data[clap[0] + 4..clap[0] + 8].copy_from_slice(&47u32.to_be_bytes());
                        data[clap[0] + 12..clap[0] + 16].copy_from_slice(&51u32.to_be_bytes());
                    } else if scenario == 1 {
                        animation_support::remove_poster(&mut data);
                    } else {
                        for kind in [b"irot", b"imir", b"clap", b"pasp"] {
                            let found = positions(&data, kind);
                            if let Some(&pos) = found.last() {
                                data[pos..pos + 4].copy_from_slice(b"free");
                            }
                        }
                    }
                    let parsed = zenavif_parse::AvifParser::from_bytes(&data).unwrap();
                    let spatial = parsed.animation_info().unwrap().spatial;
                    if scenario == 2 {
                        assert_eq!(spatial, AnimationSpatialMetadata::default());
                    } else {
                        assert_eq!(spatial.rotation.unwrap().angle, u16::from(rotation) * 90);
                        assert_eq!(spatial.mirror.map(|m| m.axis), mirror);
                        assert_eq!(spatial.clean_aperture.unwrap().width_n, 49);
                        assert_eq!(spatial.pixel_aspect_ratio.unwrap().h_spacing, 1);
                    }
                    let mut native = AnimationDecoder::new(&data, &DecoderConfig::new()).unwrap();
                    assert_eq!(native.info().spatial, spatial);
                    for expected in &control.frames {
                        let frame = native.next_frame(&Unstoppable).unwrap().unwrap();
                        assert_eq!((frame.pixels.width(), frame.pixels.height()), (65, 67));
                        for row in 0..67 {
                            assert_eq!(
                                frame.pixels.as_slice().row(row),
                                expected.pixels.as_slice().row(row)
                            );
                        }
                    }
                    for bake in [true, false] {
                        let (x0, y0, w, h, r, m) = if scenario == 2 {
                            (0, 0, 65, 67, 0, None)
                        } else {
                            (
                                3,
                                5,
                                49,
                                53,
                                if bake { rotation } else { 0 },
                                if bake { mirror } else { None },
                            )
                        };
                        let (ow, oh) = if r % 2 == 0 { (w, h) } else { (h, w) };
                        for backend in [
                            DecodeBackend::Rav1dSafe,
                            #[cfg(feature = "zenav1-aom")]
                            DecodeBackend::Zenav1Aom,
                        ] {
                            let mut cfg = AvifDecoderConfig::new();
                            *cfg.inner_mut() = DecoderConfig::new().decode_backend(backend);
                            let hint = if bake {
                                zencodec::OrientationHint::Correct
                            } else {
                                zencodec::OrientationHint::Preserve
                            };
                            let probe = cfg
                                .clone()
                                .job()
                                .with_orientation(hint)
                                .probe(&data)
                                .unwrap();
                            let mut dec = cfg
                                .job()
                                .with_orientation(hint)
                                .animation_frame_decoder(Cow::Borrowed(&data), &[])
                                .unwrap();
                            assert_eq!(dec.spatial_metadata(), spatial);
                            assert_eq!((probe.width, probe.height), (ow, oh));
                            assert_eq!((dec.info().width, dec.info().height), (ow, oh));
                            for expected in &control.frames {
                                let frame = dec.render_next_frame(None).unwrap().unwrap();
                                let pixels = frame.pixels();
                                assert_eq!((pixels.width(), pixels.rows()), (ow, oh));
                                let bpp = pixels.descriptor().bytes_per_pixel();
                                for y in 0..h {
                                    for x in 0..w {
                                        let (mut ox, mut oy) = match r {
                                            0 => (x, y),
                                            1 => (y, w - 1 - x),
                                            2 => (w - 1 - x, h - 1 - y),
                                            _ => (h - 1 - y, x),
                                        };
                                        if m == Some(0) {
                                            oy = oh - 1 - oy;
                                        }
                                        if m == Some(1) {
                                            ox = ow - 1 - ox;
                                        }
                                        assert_eq!(
                                            &pixels.row(oy)
                                                [ox as usize * bpp..(ox as usize + 1) * bpp],
                                            &expected.pixels.as_slice().row(y0 + y)[(x0 + x)
                                                as usize
                                                * bpp
                                                ..(x0 + x + 1) as usize * bpp]
                                        );
                                    }
                                }
                            }
                        }
                    }
                    if matches!(depth, EncodeBitDepth::Eight)
                        && rotation == 1
                        && mirror == Some(1)
                        && scenario < 2
                    {
                        if let Ok(dir) = std::env::var("ZENAVIF_SPATIAL_ARTIFACTS") {
                            std::fs::write(format!("{dir}/spatial-{scenario}.avif"), &data)
                                .unwrap();
                            std::fs::write(format!("{dir}/control.avif"), &encoded.avif_file)
                                .unwrap();
                        }
                    }
                }
            }
        }
    }
}
