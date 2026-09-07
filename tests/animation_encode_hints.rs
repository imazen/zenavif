#![cfg(any(feature = "two-pass-butteraugli", feature = "two-pass-zensim"))]
use imgref::Img;
use rgb::Rgba;
use zenavif::*;

#[test]
fn animation_quantizer_hints_change_color_and_preserve_alpha() {
    let frames = [20, 30].map(|duration_ms| AnimationFrameRgba {
        pixels: Img::new(
            (0..129 * 67)
                .map(|i| Rgba::new((i * 17) as u8, (i * 31) as u8, (i * 7) as u8, 128))
                .collect::<Vec<_>>(),
            129,
            67,
        ),
        duration_ms,
    });
    for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
        let base = EncoderConfig::new()
            .backend(Av1Backend::Zenravif)
            .speed(8)
            .quality(60.0)
            .threads(Some(1))
            .bit_depth(depth);
        let maps = [
            None,
            Some(vec![1.0; 6].into_boxed_slice()),
            Some(vec![0.5, 2.0, 0.5, 2.0, 0.5, 2.0].into_boxed_slice()),
        ];
        let mut files = Vec::new();
        let mut alphas = Vec::new();
        for map in maps {
            let encoded = encode_animation_rgba8(
                &frames,
                &base.clone().with_sb_q_scale(map),
                almost_enough::StopToken::new(almost_enough::Unstoppable),
            )
            .unwrap();
            let decoded = decode_animation(&encoded.avif_file).unwrap();
            assert_eq!(decoded.frames.len(), 2);
            assert!(decoded.info.has_alpha);
            let mut alpha = Vec::new();
            for frame in decoded.frames {
                if let Some(img) = frame.pixels.try_as_imgref::<Rgba<u8>>() {
                    alpha.extend(img.pixels().map(|p| u16::from(p.a)));
                } else {
                    alpha.extend(
                        frame
                            .pixels
                            .try_as_imgref::<Rgba<u16>>()
                            .unwrap()
                            .pixels()
                            .map(|p| p.a),
                    );
                }
            }
            alphas.push(alpha);
            files.push(encoded.avif_file);
        }
        assert_eq!(files[0], files[1], "neutral hints changed bytes");
        assert!(
            files[0] != files[2],
            "non-neutral hints ignored at {depth:?}"
        );
        assert_eq!(alphas[0], alphas[2], "color hints changed alpha");
    }
}
