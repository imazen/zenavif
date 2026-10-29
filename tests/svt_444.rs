//! Full-resolution SVT stills must preserve format, planes, and source pixels.
#![cfg(feature = "zenav1-svt")]
use almost_enough::{StopToken, Unstoppable};
use imgref::ImgRef;
use rgb::{Rgb, Rgba};
use zenavif::{
    Av1Backend, EncodeBitDepth, EncodeChromaSubsampling, EncodeColorModel, EncoderConfig, PlanInput,
};

fn config() -> EncoderConfig {
    EncoderConfig::new()
        .backend(Av1Backend::Zenav1Svt)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv444)
        .threads(Some(1))
        .speed(10)
}
fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}
fn pixels(w: usize, h: usize, stride: usize) -> Vec<Rgb<u8>> {
    (0..(h - 1) * stride + w)
        .map(|i| Rgb {
            r: (i * 37) as u8,
            g: (i * 19 + i / 7) as u8,
            b: (i * 73 + i / 11) as u8,
        })
        .collect()
}

#[test]
fn ycbcr_444_retains_full_resolution_signaling_and_strided_input() {
    let (w, h, stride) = (65, 67, 69);
    let src = pixels(w, h, stride);
    let img = ImgRef::new_stride(&src, w, h, stride);
    let cfg = config().quality(83.0);
    cfg.validate_for_input(PlanInput::rgb8(w as u32, h as u32))
        .unwrap();
    assert!(
        zenavif::backend_router::query_still_backends(&cfg, PlanInput::rgb8(w as u32, h as u32))
            .iter()
            .find(|r| r.backend == Av1Backend::Zenav1Svt)
            .unwrap()
            .supported()
    );
    let out = zenavif::encode_rgb8(img, &cfg, stop()).unwrap();
    let decoder =
        zenavif::ManagedAvifDecoder::new(&out.avif_file, &zenavif::DecoderConfig::default())
            .unwrap();
    let info = decoder.probe_info().unwrap();
    assert_eq!(info.matrix_coefficients.0, 6);
    assert_eq!(info.chroma_sampling, zenavif::ChromaSampling::Cs444);
    let parser = zenavif_parse::AvifParser::from_owned_with_config(
        out.avif_file.clone(),
        &Default::default(),
        &Unstoppable,
    )
    .unwrap();
    let payload = parser.primary_data().unwrap();
    let metadata = zenavif_parse::AV1Metadata::parse_av1_bitstream(&payload).unwrap();
    assert_eq!(metadata.seq_profile, 1);
    assert_eq!(metadata.matrix_coefficients, 6);
    let decoded = zenavif::decode_av1_obu_yuv(&payload, zenavif::DecodeBackend::Rav1dSafe).unwrap();
    assert_eq!((decoded.subsampling_x, decoded.subsampling_y), (0, 0));
    assert_eq!((decoded.width_uv, decoded.height_uv), (w, h));
    let packed: Vec<_> = img.rows().flat_map(|r| r.iter().copied()).collect();
    let tight = zenavif::encode_rgb8(ImgRef::new(&packed, w, h), &cfg, stop()).unwrap();
    assert_eq!(out.avif_file, tight.avif_file);
}

#[test]
fn identity_444_lossless_preserves_all_rgb_and_rgba_samples() {
    for (w, h) in [(1, 1), (7, 9), (65, 67)] {
        let stride = w + 3;
        let src = pixels(w, h, stride);
        let image = ImgRef::new_stride(&src, w, h, stride);
        let expected: Vec<_> = image.rows().flat_map(|r| r.iter().copied()).collect();
        for speed in [1, 4, 6, 10] {
            let cfg = config()
                .color_model(EncodeColorModel::Rgb)
                .with_lossless(true)
                .speed(speed);
            cfg.validate_for_input(PlanInput::rgb8(w as u32, h as u32))
                .unwrap();
            let out = zenavif::encode_rgb8(image, &cfg, stop()).unwrap();
            let decoded = zenavif::decode(&out.avif_file).unwrap();
            let actual = decoded.try_as_imgref::<Rgb<u8>>().unwrap();
            assert_eq!(*actual.buf(), expected.as_slice(), "{w}x{h} speed {speed}");
            let rgba: Vec<_> = expected
                .iter()
                .enumerate()
                .map(|(i, p)| Rgba {
                    r: p.r,
                    g: p.g,
                    b: p.b,
                    a: (i * 41) as u8,
                })
                .collect();
            let out = zenavif::encode_rgba8(ImgRef::new(&rgba, w, h), &cfg, stop()).unwrap();
            let decoded = zenavif::decode(&out.avif_file).unwrap();
            assert_eq!(
                *decoded.try_as_imgref::<Rgba<u8>>().unwrap().buf(),
                rgba.as_slice(),
                "RGB including alpha-zero color and alpha must be exact: {w}x{h} speed {speed}"
            );
        }
    }
}

#[test]
fn unsupported_444_depth_and_identity_420_remain_explicit_errors() {
    let src = pixels(8, 8, 8);
    for depth in [EncodeBitDepth::Ten, EncodeBitDepth::Twelve] {
        let cfg = config().bit_depth(depth);
        assert!(cfg.validate_for_input(PlanInput::rgb8(8, 8)).is_err());
        assert!(zenavif::encode_rgb8(ImgRef::new(&src, 8, 8), &cfg, stop()).is_err());
    }
    let cfg = config()
        .color_model(EncodeColorModel::Rgb)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420);
    assert!(cfg.validate().is_err());
    assert!(zenavif::encode_rgb8(ImgRef::new(&src, 8, 8), &cfg, stop()).is_err());
}

#[test]
fn trait_lossless_keeps_the_svt_native_mode_and_source_samples() {
    use zencodec::encode::{EncoderConfig as _, Fidelity};
    let (w, h) = (17, 19);
    let source = pixels(w, h, w);
    let mut encoder = zenavif::AvifEncoderConfig::new();
    *encoder.inner_mut() = config().color_model(EncodeColorModel::Rgb);
    let encoder = encoder.with_fidelity(Fidelity::Lossless);
    assert_eq!(encoder.resolved_target_fidelity(), Some(Fidelity::Lossless));
    let output = encoder.encode_rgb8(ImgRef::new(&source, w, h)).unwrap();
    let decoded = zenavif::decode(output.data()).unwrap();
    assert_eq!(
        *decoded.try_as_imgref::<Rgb<u8>>().unwrap().buf(),
        source.as_slice()
    );
}

#[test]
fn high_precision_input_is_refused_before_lossless_conversion() {
    let input = vec![
        Rgb {
            r: 1u16,
            g: 127,
            b: 65535
        };
        8 * 8
    ];
    let config = config()
        .color_model(EncodeColorModel::Rgb)
        .with_lossless(true);
    assert!(
        config
            .validate_for_input(
                zenavif::backend_router::StillInput::Rgb16 {
                    width: 8,
                    height: 8
                }
                .plan_input()
            )
            .is_err()
    );
    let error = zenavif::encode_rgb16(ImgRef::new(&input, 8, 8), &config, stop()).unwrap_err();
    assert!(error.to_string().contains("8-bit"), "{error}");
}
