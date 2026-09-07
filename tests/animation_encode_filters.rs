#![cfg(feature = "__expert")]

use imgref::Img;
use rgb::Rgba;
use zenavif::*;

#[test]
fn animation_color_filter_overrides_reach_the_bitstream() {
    let frames = [20, 30].map(|duration_ms| AnimationFrameRgba {
        pixels: Img::new(
            (0..65 * 67)
                .map(|i| Rgba::new((i * 17) as u8, (i * 31) as u8, (i * 7) as u8, 128))
                .collect::<Vec<_>>(),
            65,
            67,
        ),
        duration_ms,
    });
    for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
        let mut files = Vec::new();
        for (cdef, lrf) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut params = expert::InternalParams::default();
            params.lrf = Some(lrf);
            let config = EncoderConfig::new()
                .backend(Av1Backend::Zenravif)
                .speed(10)
                .quality(35.0)
                .threads(Some(1))
                .bit_depth(depth)
                .with_cdef(Some(cdef))
                .with_internal_params(params);
            let encoded = encode_animation_rgba8(
                &frames,
                &config,
                almost_enough::StopToken::new(almost_enough::Unstoppable),
            )
            .unwrap();
            let decoded = decode_animation(&encoded.avif_file).unwrap();
            assert_eq!(decoded.frames.len(), 2);
            files.push(encoded.avif_file);
        }
        assert!(files[0] != files[1], "CDEF override ignored at {depth:?}");
        assert!(
            files[0] != files[2],
            "restoration override ignored at {depth:?}"
        );
    }
}
