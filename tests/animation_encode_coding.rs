#![cfg(feature = "encode-imazen")]
use imgref::Img;
use rgb::Rgba;
use zenavif::*;

#[test]
fn animation_quantization_and_tuning_encode_and_decode() {
    let frames = [20, 30].map(|duration_ms| AnimationFrameRgba {
        pixels: Img::new(
            (0..65 * 67)
                .map(|i| {
                    Rgba::new(
                        (i * 17) as u8,
                        (i * 31) as u8,
                        (i * 7) as u8,
                        if i % 3 == 0 { 0 } else { 255 },
                    )
                })
                .collect::<Vec<_>>(),
            65,
            67,
        ),
        duration_ms,
    });
    for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
        let base = EncoderConfig::new()
            .backend(Av1Backend::Zenravif)
            .speed(10)
            .quality(35.0)
            .alpha_quality(35.0)
            .threads(Some(1))
            .bit_depth(depth)
            .with_vaq(false, 1.0)
            .with_still_image_tuning(false)
            .with_trellis(Some(false));
        let mut files = Vec::new();
        for axis in 0..7 {
            let config = match axis {
                0 => base.clone(),
                1 => base.clone().with_lossless(true),
                2 => base.clone().with_vaq(true, 2.0),
                3 => base.clone().with_still_image_tuning(true),
                4 => base.clone().with_trellis(Some(true)),
                5 => base.clone().with_vaq(true, 2.0).with_seg_boost(Some(1.5)),
                _ => base
                    .clone()
                    .with_lossless(true)
                    .with_vaq(true, 2.0)
                    .with_seg_boost(Some(1.5))
                    .with_still_image_tuning(true)
                    .with_trellis(Some(true)),
            };
            let encoded = encode_animation_rgba8(
                &frames,
                &config,
                almost_enough::StopToken::new(almost_enough::Unstoppable),
            )
            .unwrap();
            let decoded = decode_animation(&encoded.avif_file).unwrap();
            assert_eq!(decoded.frames.len(), 2);
            if axis == 1 || axis == 6 {
                for frame in &decoded.frames {
                    if let Some(img) = frame.pixels.try_as_imgref::<Rgba<u8>>() {
                        for (i, p) in img.pixels().enumerate() {
                            assert_eq!(p.a, if i % 3 == 0 { 0 } else { 255 });
                        }
                    } else {
                        let img = frame.pixels.try_as_imgref::<Rgba<u16>>().unwrap();
                        for (i, p) in img.pixels().enumerate() {
                            assert_eq!(p.a, if i % 3 == 0 { 0 } else { 65535 });
                        }
                    }
                }
            }
            files.push(encoded.avif_file);
        }
        assert!(files[0] != files[1], "lossless ignored at {depth:?}");
    }
}
