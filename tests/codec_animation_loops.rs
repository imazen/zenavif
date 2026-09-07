#![cfg(feature = "encode")]
use imgref::Img;
use rgb::Rgba;
use zenavif::*;
use zencodec::encode::{AnimationFrameEncoder as _, EncodeJob as _, EncoderConfig as _};

fn encode(backend: Av1Backend, count: Option<u32>) -> Vec<u8> {
    let mut cfg = AvifEncoderConfig::new();
    *cfg.inner_mut() = EncoderConfig::new()
        .backend(backend)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
        .speed(10)
        .threads(Some(1));
    let mut anim = cfg
        .job()
        .with_loop_count(count)
        .animation_frame_encoder()
        .unwrap();
    for (i, duration) in [11, 23].into_iter().enumerate() {
        let image = Img::new(
            vec![
                Rgba {
                    r: (32 + i * 20) as u8,
                    g: 64,
                    b: 96,
                    a: 128
                };
                32 * 32
            ],
            32,
            32,
        );
        anim.push_frame(
            zenpixels::PixelSlice::from(image.as_ref()).erase(),
            duration,
            None,
        )
        .unwrap();
    }
    anim.finish(None).unwrap().data().to_vec()
}
#[test]
fn codec_loop_counts_preserve_samples_timing_and_alpha() {
    let backends = [
        Av1Backend::Zenravif,
        #[cfg(feature = "zenav1-svt")]
        Av1Backend::Zenav1Svt,
    ];
    for backend in backends {
        let original = encode(backend, None);
        let original_parser = zenavif_parse::AvifParser::from_bytes(&original).unwrap();
        for count in [0, 1, 3, u32::MAX] {
            let data = encode(backend, Some(count));
            if count == 0 {
                assert_eq!(data, original, "explicit infinity preserves all bytes");
            }
            let parser = zenavif_parse::AvifParser::from_bytes(&data).unwrap();
            let decoded = decode_animation(&data).unwrap();
            assert_eq!(decoded.info.loop_count, u64::from(count), "{backend:?}");
            assert!(decoded.info.has_alpha);
            assert_eq!(decoded.frames.len(), 2);
            for (i, (before, after)) in original_parser.frames().zip(parser.frames()).enumerate() {
                let (before, after) = (before.unwrap(), after.unwrap());
                assert_eq!(before.data, after.data);
                assert_eq!(before.alpha_data, after.alpha_data);
                assert_eq!(
                    original_parser.frame_timing(i).unwrap(),
                    parser.frame_timing(i).unwrap()
                );
            }
        }
    }
}
