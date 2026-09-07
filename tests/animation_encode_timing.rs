#![cfg(feature = "encode")]
use imgref::Img;
use rgb::{RGB8, RGBA8};
use zenavif::*;

fn boxes<'a>(data: &'a [u8], wanted: &[u8; 4], found: &mut Vec<&'a [u8]>) {
    let mut offset = 0;
    while offset < data.len() {
        assert!(data.len() - offset >= 8);
        let size = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        assert!(size >= 8 && size <= data.len() - offset);
        let kind = &data[offset + 4..offset + 8];
        let payload = &data[offset + 8..offset + size];
        if kind == wanted {
            found.push(payload);
        }
        if [b"moov", b"trak", b"mdia", b"minf", b"stbl"]
            .iter()
            .any(|k| kind == *k)
        {
            boxes(payload, wanted, found);
        }
        offset += size;
    }
}

fn verify(data: &[u8], timescale: u32, durations: &[u32], alpha: bool) {
    let mut media = Vec::new();
    boxes(data, b"mdhd", &mut media);
    assert_eq!(media.len(), if alpha { 2 } else { 1 });
    for payload in media {
        assert_eq!(payload[0], 1);
        assert_eq!(
            u32::from_be_bytes(payload[20..24].try_into().unwrap()),
            timescale
        );
        assert_eq!(
            u64::from_be_bytes(payload[24..32].try_into().unwrap()),
            durations.iter().map(|&d| u64::from(d)).sum::<u64>()
        );
    }
    let mut tables = Vec::new();
    boxes(data, b"stts", &mut tables);
    assert_eq!(tables.len(), if alpha { 2 } else { 1 });
    for payload in tables {
        let entries = u32::from_be_bytes(payload[4..8].try_into().unwrap()) as usize;
        assert_eq!(payload.len(), 8 + entries * 8);
        let mut actual = Vec::new();
        for entry in payload[8..].as_chunks::<8>().0 {
            let count = u32::from_be_bytes(entry[..4].try_into().unwrap());
            let delta = u32::from_be_bytes(entry[4..].try_into().unwrap());
            actual.extend(std::iter::repeat_n(delta, count as usize));
        }
        assert_eq!(actual, durations);
    }
}

#[test]
fn exact_timing_survives_depth_conversion_and_backend_dispatch() {
    let rgb: Vec<_> = (0..65 * 67)
        .map(|i| RGB8::new((i * 17) as u8, (i * 31) as u8, (i * 7) as u8))
        .collect();
    let rgba: Vec<_> = rgb.iter().map(|p| RGBA8::new(p.r, p.g, p.b, 128)).collect();
    let high: Vec<_> = rgb
        .iter()
        .map(|p| {
            rgb::RGB::new(
                u16::from(p.r) * 257,
                u16::from(p.g) * 257,
                u16::from(p.b) * 257,
            )
        })
        .collect();
    let high_alpha: Vec<_> = high
        .iter()
        .map(|p| rgb::RGBA::new(p.r, p.g, p.b, 32768))
        .collect();
    let backends = [
        Av1Backend::Zenravif,
        #[cfg(feature = "zenav1-svt")]
        Av1Backend::Zenav1Svt,
    ];
    for backend in backends {
        for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
            let config = EncoderConfig::new()
                .backend(backend)
                .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
                .speed(10)
                .quality(60.0)
                .threads(Some(1))
                .bit_depth(depth);
            for (timescale, durations) in [
                (30000, [1001, 2002]),
                (1_000_000, [1, 7]),
                (u32::MAX, [u32::MAX, u32::MAX]),
                (1000, [20, 30]),
            ] {
                let a = durations.map(|duration_ticks| TimedAnimationFrame {
                    pixels: Img::new(rgb.clone(), 65, 67),
                    duration_ticks,
                });
                let b = durations.map(|duration_ticks| TimedAnimationFrame {
                    pixels: Img::new(rgba.clone(), 65, 67),
                    duration_ticks,
                });
                let c = durations.map(|duration_ticks| TimedAnimationFrame {
                    pixels: Img::new(high.clone(), 65, 67),
                    duration_ticks,
                });
                let d = durations.map(|duration_ticks| TimedAnimationFrame {
                    pixels: Img::new(high_alpha.clone(), 65, 67),
                    duration_ticks,
                });
                let stop = || almost_enough::StopToken::new(almost_enough::Unstoppable);
                let results = [
                    encode_animation_rgb8_timed(&a, timescale, &config, stop()),
                    encode_animation_rgba8_timed(&b, timescale, &config, stop()),
                    encode_animation_rgb16_timed(&c, timescale, &config, stop()),
                    encode_animation_rgba16_timed(&d, timescale, &config, stop()),
                ];
                for (index, result) in results.into_iter().enumerate() {
                    let result = result.unwrap();
                    assert_eq!(result.timescale, timescale);
                    assert_eq!(
                        result.total_duration_ticks,
                        durations.iter().map(|&v| u64::from(v)).sum::<u64>()
                    );
                    assert_eq!(
                        result.total_duration_ms,
                        (u128::from(result.total_duration_ticks) * 1000 / u128::from(timescale))
                            as u64
                    );
                    verify(&result.avif_file, timescale, &durations, index % 2 != 0);
                    let decoded = decode_animation(&result.avif_file).unwrap();
                    assert_eq!(decoded.frames.len(), 2);
                    assert_eq!(decoded.info.has_alpha, index % 2 != 0);
                    if let Some(dir) = std::env::var_os("ZENAVIF_TIMING_ARTIFACTS") {
                        let dir = std::path::PathBuf::from(dir);
                        std::fs::create_dir_all(&dir).unwrap();
                        std::fs::write(
                            dir.join(format!(
                                "timing-{backend:?}-{depth:?}-{timescale}-{index}.avif"
                            )),
                            result.avif_file,
                        )
                        .unwrap();
                    }
                }
                assert!(encode_animation_rgb8_timed(&a, 0, &config, stop()).is_err());
            }
        }
    }
}

#[test]
fn codec_adapter_rescales_mixed_clocks_without_rounding_or_partial_append() {
    use zencodec::encode::{AnimationFrameEncoder as _, EncodeJob as _, EncoderConfig as _};
    let rgb = Img::new(vec![RGB8::new(70, 110, 150); 32 * 32], 32, 32);
    let rgba = Img::new(vec![RGBA8::new(70, 110, 150, 128); 32 * 32], 32, 32);
    let high = Img::new(vec![rgb::RGB::new(18000u16, 28000, 38000); 32 * 32], 32, 32);
    let high_alpha = Img::new(
        vec![rgb::RGBA::new(18000u16, 28000, 38000, 32768); 32 * 32],
        32,
        32,
    );
    let backends = [
        Av1Backend::Zenravif,
        #[cfg(feature = "zenav1-svt")]
        Av1Backend::Zenav1Svt,
    ];
    for backend in backends {
        for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
            for kind in 0..4 {
                let mut cfg = AvifEncoderConfig::new();
                *cfg.inner_mut() = EncoderConfig::new()
                    .backend(backend)
                    .bit_depth(depth)
                    .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
                    .speed(10)
                    .threads(Some(1));
                let mut anim = cfg.job().animation_frame_encoder().unwrap();
                let pixels = || match kind {
                    0 => zenpixels::PixelSlice::from(rgb.as_ref()).erase(),
                    1 => zenpixels::PixelSlice::from(rgba.as_ref()).erase(),
                    2 => zenpixels::PixelSlice::from(high.as_ref()).erase(),
                    _ => zenpixels::PixelSlice::from(high_alpha.as_ref()).erase(),
                };
                assert!(anim.push_frame_ticks(pixels(), 1, 0, None).is_err());
                anim.push_frame_ticks(pixels(), 1001, 30000, None).unwrap();
                anim.push_frame(pixels(), 23, None).unwrap();
                anim.push_frame_ticks(pixels(), 1001, 60000, None).unwrap();
                assert!(anim.push_frame_ticks(pixels(), 1, u32::MAX, None).is_err());
                assert!(anim.push_frame_ticks(pixels(), u32::MAX, 1, None).is_err());
                let output = anim.finish(None).unwrap();
                verify(output.data(), 60000, &[2002, 1380, 1001], kind % 2 != 0);
                assert_eq!(decode_animation(output.data()).unwrap().frames.len(), 3);
                if let Some(dir) = std::env::var_os("ZENAVIF_TIMING_ARTIFACTS") {
                    std::fs::write(
                        std::path::PathBuf::from(dir)
                            .join(format!("codec-{backend:?}-{depth:?}-60000-{kind}.avif")),
                        output.data(),
                    )
                    .unwrap();
                }
            }
        }
    }
}
