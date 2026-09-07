#![cfg(feature = "encode")]
use imgref::Img;
use rgb::{Rgb, Rgba};
use zenavif::*;
fn stop() -> almost_enough::StopToken {
    almost_enough::StopToken::new(almost_enough::Unstoppable)
}
fn storage<P: Copy>(pixels: &[P], width: usize, height: usize, padded: bool) -> imgref::ImgVec<P> {
    if !padded {
        return Img::new(pixels.to_vec(), width, height);
    }
    let stride = width + 5;
    let mut out = vec![pixels[0]; stride * height];
    for y in 0..height {
        out[y * stride..y * stride + width].copy_from_slice(&pixels[y * width..(y + 1) * width]);
    }
    Img::new_stride(out, width, height, stride)
}
#[test]
fn animation_honors_depth_independently_of_input_storage() {
    let (w, h) = (31, 33);
    let rgb: Vec<_> = (0..w * h)
        .map(|n| Rgb::new((n * 7) as u8, (n * 13) as u8, (n * 19) as u8))
        .collect();
    let rgba: Vec<_> = rgb
        .iter()
        .enumerate()
        .map(|(n, p)| Rgba::new(p.r, p.g, p.b, (64 + n % 192) as u8))
        .collect();
    let rgb16: Vec<_> = rgb
        .iter()
        .map(|p| {
            Rgb::new(
                u16::from(p.r) * 257,
                u16::from(p.g) * 257,
                u16::from(p.b) * 257,
            )
        })
        .collect();
    let rgba16: Vec<_> = rgba
        .iter()
        .map(|p| {
            Rgba::new(
                u16::from(p.r) * 257,
                u16::from(p.g) * 257,
                u16::from(p.b) * 257,
                u16::from(p.a) * 257,
            )
        })
        .collect();
    for (depth, requested) in [
        (EncodeBitDepth::Eight, Some(8)),
        (EncodeBitDepth::Ten, Some(10)),
        (EncodeBitDepth::Auto, None),
    ] {
        let mut packed = None;
        for padded in [false, true] {
            let cfg = EncoderConfig::new()
                .backend(Av1Backend::Zenravif)
                .speed(10)
                .bit_depth(depth)
                .chroma_subsampling(EncodeChromaSubsampling::Yuv420);
            let files = [
                encode_animation_rgb8(
                    &[AnimationFrame {
                        pixels: storage(&rgb, w, h, padded),
                        duration_ms: 17,
                    }]
                    .iter()
                    .cycle()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>(),
                    &cfg,
                    stop(),
                )
                .unwrap(),
                encode_animation_rgb16(
                    &[AnimationFrame16 {
                        pixels: storage(&rgb16, w, h, padded),
                        duration_ms: 17,
                    }]
                    .iter()
                    .cycle()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>(),
                    &cfg,
                    stop(),
                )
                .unwrap(),
                encode_animation_rgba8(
                    &[AnimationFrameRgba {
                        pixels: storage(&rgba, w, h, padded),
                        duration_ms: 17,
                    }]
                    .iter()
                    .cycle()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>(),
                    &cfg,
                    stop(),
                )
                .unwrap(),
                encode_animation_rgba16(
                    &[AnimationFrameRgba16 {
                        pixels: storage(&rgba16, w, h, padded),
                        duration_ms: 17,
                    }]
                    .iter()
                    .cycle()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>(),
                    &cfg,
                    stop(),
                )
                .unwrap(),
            ];
            for (index, file) in files.iter().enumerate() {
                let want = requested.unwrap_or(if index % 2 == 0 { 8 } else { 10 });
                assert_eq!(file.frame_count, 2);
                assert_eq!(file.total_duration_ms, 34);
                let p = zenavif_parse::AvifParser::from_bytes(&file.avif_file).unwrap();
                assert_eq!(p.av1_config().unwrap().bit_depth, want);
                assert_eq!(p.frame_timing(1).unwrap().pts_in_timescales, 17);
                if let Some(dir) = std::env::var_os("ZENAVIF_DEPTH_ARTIFACTS") {
                    std::fs::create_dir_all(&dir).unwrap();
                    std::fs::write(
                        std::path::Path::new(&dir)
                            .join(format!("depth-{want}-{index}-{padded}.avif")),
                        &file.avif_file,
                    )
                    .unwrap();
                }
                let frame = p.frame(0).unwrap();
                let header = zenavif_parse::AV1Metadata::parse_av1_bitstream(&frame.data).unwrap();
                assert_eq!(header.bit_depth, want);
                if let Some(alpha) = frame.alpha_data {
                    assert_eq!(
                        zenavif_parse::AV1Metadata::parse_av1_bitstream(&alpha)
                            .unwrap()
                            .bit_depth,
                        want
                    );
                }
            }
            let bytes: Vec<_> = files.iter().map(|file| file.avif_file.clone()).collect();
            if let Some(ref reference) = packed {
                assert_eq!(&bytes, reference, "row padding changed encoded pixels");
            } else {
                packed = Some(bytes);
            }
            if requested.is_some() {
                assert_eq!(
                    files[0].avif_file, files[1].avif_file,
                    "equivalent RGB storage must encode identically"
                );
                assert_eq!(
                    files[2].avif_file, files[3].avif_file,
                    "equivalent RGBA storage must encode identically"
                );
            }
        }
    }
}

#[test]
fn codec_animation_depth_request_reaches_color_and_alpha_streams() {
    use zencodec::encode::{AnimationFrameEncoder as _, EncodeJob as _, EncoderConfig as _};
    for (depth, want) in [(EncodeBitDepth::Eight, 8), (EncodeBitDepth::Ten, 10)] {
        for input16 in [false, true] {
            let mut cfg = AvifEncoderConfig::new();
            *cfg.inner_mut() = EncoderConfig::new()
                .backend(Av1Backend::Zenravif)
                .speed(10)
                .bit_depth(depth)
                .chroma_subsampling(EncodeChromaSubsampling::Yuv420);
            let mut encoder = cfg
                .job()
                .with_canvas_size(31, 33)
                .animation_frame_encoder()
                .unwrap();
            let rgba = Img::new(vec![Rgba::new(75u8, 125, 175, 128); 31 * 33], 31, 33);
            let rgba16 = Img::new(
                vec![Rgba::new(75u16 * 257, 125 * 257, 175 * 257, 128 * 257); 31 * 33],
                31,
                33,
            );
            for duration in [17, 33] {
                let pixels = if input16 {
                    zenpixels::PixelSlice::from(rgba16.as_ref()).erase()
                } else {
                    zenpixels::PixelSlice::from(rgba.as_ref()).erase()
                };
                encoder.push_frame(pixels, duration, None).unwrap();
            }
            let encoded = encoder.finish(None).unwrap();
            let parser = zenavif_parse::AvifParser::from_bytes(encoded.data()).unwrap();
            let frame = parser.frame(0).unwrap();
            for sample in [
                frame.data.as_ref(),
                frame.alpha_data.as_ref().unwrap().as_ref(),
            ] {
                assert_eq!(
                    zenavif_parse::AV1Metadata::parse_av1_bitstream(sample)
                        .unwrap()
                        .bit_depth,
                    want
                );
            }
            for backend in [
                DecodeBackend::Rav1dSafe,
                #[cfg(feature = "zenav1-aom")]
                DecodeBackend::Zenav1Aom,
            ] {
                let mut decoder = ManagedAvifDecoder::new(
                    encoded.data(),
                    &DecoderConfig::new().decode_backend(backend),
                )
                .unwrap();
                assert_eq!(decoder.probe_animation_info().unwrap().bit_depth, want);
                let decoded = decoder.decode_animation(&Unstoppable).unwrap();
                assert_eq!(decoded.frames.len(), 2);
                assert_eq!(
                    decoded
                        .frames
                        .iter()
                        .map(|f| f.duration_ms)
                        .collect::<Vec<_>>(),
                    [17, 33]
                );
                assert!(decoded.info.has_alpha);
                for frame in decoded.frames {
                    assert_eq!((frame.pixels.width(), frame.pixels.height()), (31, 33));
                }
            }
        }
    }
}
