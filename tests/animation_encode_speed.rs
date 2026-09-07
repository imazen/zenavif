#![cfg(feature = "__expert")]
use imgref::Img;
use rgb::Rgba;
use zenavif::*;

#[test]
fn animation_speed_overrides_encode_and_decode() {
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
        let mut partition_files = Vec::new();
        for axis in 0..8 {
            for value in [false, true] {
                let mut params = expert::InternalParams::default();
                // Keep restoration enabled while exercising its search controls.
                params.lrf = Some(true);
                let mut config = EncoderConfig::new()
                    .backend(Av1Backend::Zenravif)
                    .speed(10)
                    .quality(35.0)
                    .threads(Some(1))
                    .bit_depth(depth);
                match axis {
                    0 => params.partition_range = value.then_some((4, 4)),
                    1 => params.complex_prediction_modes = Some(value),
                    2 => params.fast_deblock = Some(value),
                    3 => config = config.with_rdo_tx_decision(Some(value)),
                    4 => config = config.with_sgr_full(Some(value)),
                    5 => config = config.with_lru_on_skip(Some(value)),
                    6 => config = config.with_segmentation_complex(Some(value)),
                    7 => config = config.with_encode_bottomup(Some(value)),
                    _ => unreachable!(),
                }
                config = config.with_internal_params(params);
                let encoded = encode_animation_rgba8(
                    &frames,
                    &config,
                    almost_enough::StopToken::new(almost_enough::Unstoppable),
                )
                .unwrap();
                let decoded = decode_animation(&encoded.avif_file).unwrap();
                assert_eq!(
                    decoded.frames.len(),
                    2,
                    "axis={axis} value={value} depth={depth:?}"
                );
                assert!(decoded.info.has_alpha);
                if axis == 0 {
                    partition_files.push(encoded.avif_file);
                }
            }
        }
        assert!(
            partition_files[0] != partition_files[1],
            "partition override ignored at {depth:?}"
        );
    }
}
