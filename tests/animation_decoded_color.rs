#![cfg(feature = "encode-imazen")]

use std::borrow::Cow;
use zencodec::decode::{AnimationFrameDecoder, DecodeJob, DecoderConfig};
use zenpixels::{ColorPrimaries, PixelDescriptor, TransferFunction};

fn encoded(depth: u8, cp: u8, tc: u8, identity: bool) -> Vec<u8> {
    let frames = [0, 1].map(|n| zenavif::TimedAnimationFrame {
        pixels: imgref::Img::new(
            (0..17 * 13)
                .map(|i| rgb::RGBA8::new((i * 37 + n * 11) as u8, 127, 3, (i * 19) as u8))
                .collect(),
            17,
            13,
        ),
        duration_ticks: 1,
    });
    let config = zenavif::EncoderConfig::new()
        .with_lossless(true)
        .speed(10)
        .threads(Some(1))
        .color_primaries(cp)
        .transfer_characteristics(tc)
        .bit_depth(if depth == 8 {
            zenavif::EncodeBitDepth::Eight
        } else {
            zenavif::EncodeBitDepth::Twelve
        })
        .color_model(if identity {
            zenavif::EncodeColorModel::Rgb
        } else {
            zenavif::EncodeColorModel::YCbCr
        })
        .chroma_subsampling(zenavif::EncodeChromaSubsampling::Yuv444)
        .pixel_range(if identity {
            zenavif::EncodePixelRange::Full
        } else {
            zenavif::EncodePixelRange::Limited
        });
    zenavif::encode_animation_rgba8_timed(
        &frames,
        100,
        &config,
        almost_enough::StopToken::new(almost_enough::Unstoppable),
    )
    .unwrap()
    .avif_file
}

#[test]
fn native_and_shared_alpha_animation_describe_the_decoded_samples() {
    for depth in [8, 12] {
        for (cp, tc, primaries, transfer) in [
            (1, 13, ColorPrimaries::Bt709, TransferFunction::Srgb),
            (9, 16, ColorPrimaries::Bt2020, TransferFunction::Pq),
            (9, 18, ColorPrimaries::Bt2020, TransferFunction::Hlg),
            (1, 8, ColorPrimaries::Bt709, TransferFunction::Linear),
        ] {
            let bytes = encoded(depth, cp, tc, true);
            let native = zenavif::decode_animation(&bytes).unwrap();
            for frame in native.frames {
                assert_eq!(
                    frame.pixels.descriptor().transfer(),
                    transfer,
                    "native depth={depth} tc={tc}"
                );
                assert_eq!(frame.pixels.descriptor().primaries, primaries);
            }
            let config = zenavif::AvifDecoderConfig::new();
            let mut decoder = config
                .job()
                .animation_frame_decoder(Cow::Borrowed(&bytes), &[])
                .unwrap();
            for _ in 0..2 {
                let frame = decoder.render_next_frame(None).unwrap().unwrap();
                let pixels = frame.pixels();
                assert_eq!(pixels.descriptor().transfer(), transfer);
                assert_eq!(pixels.descriptor().primaries, primaries);
                assert_eq!(
                    pixels.color_context().unwrap().cicp,
                    Some(zenpixels::Cicp::new(cp, tc, 0, true))
                );
            }
        }
    }
}

#[test]
fn animation_rgb_context_does_not_repeat_the_consumed_yuv_matrix_and_range() {
    let bytes = encoded(8, 1, 13, false);
    let config = zenavif::AvifDecoderConfig::new();
    let mut decoder = config
        .job()
        .animation_frame_decoder(Cow::Borrowed(&bytes), &[PixelDescriptor::RGBA8_SRGB])
        .unwrap();
    let source = decoder.info().source_color.cicp.unwrap();
    assert_eq!(source.matrix_coefficients, 6);
    assert!(!source.full_range);
    let frame = decoder.render_next_frame(None).unwrap().unwrap();
    assert_eq!(
        frame.pixels().color_context().unwrap().cicp,
        Some(zenpixels::Cicp::new(1, 13, 0, true))
    );
}
