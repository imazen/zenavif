#![cfg(feature = "encode")]
use imgref::Img;
use rgb::Rgba;
use zenavif::*;

fn check(input16: bool) {
    for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
        for alpha in [255u8, 128, 0] {
            let config = EncoderConfig::new()
                .backend(Av1Backend::Zenravif)
                .speed(10)
                .quality(100.0)
                .bit_depth(depth)
                .alpha_color_mode(EncodeAlphaMode::Premultiplied);
            let stop = almost_enough::StopToken::new(almost_enough::Unstoppable);
            let encoded = if input16 {
                let pixels = Img::new(
                    vec![
                        Rgba::new(75 * 257, 125 * 257, 175 * 257, u16::from(alpha) * 257);
                        32 * 32
                    ],
                    32,
                    32,
                );
                encode_rgba16(pixels.as_ref(), &config, stop)
            } else {
                let pixels = Img::new(vec![Rgba::new(75, 125, 175, alpha); 32 * 32], 32, 32);
                encode_rgba8(pixels.as_ref(), &config, stop)
            }
            .unwrap();
            let parser = zenavif_parse::AvifParser::from_bytes(&encoded.avif_file).unwrap();
            if parser.alpha_data().is_some() {
                assert!(
                    parser.premultiplied_alpha(),
                    "requested alpha association must be signaled"
                );
            } else {
                assert_eq!(alpha, 255, "non-opaque pixels require an alpha item");
            }
            if let Some(dir) = std::env::var_os("ZENAVIF_STILL_PREM_ARTIFACTS") {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(
                    std::path::Path::new(&dir)
                        .join(format!("still-prem-{input16}-{depth:?}-{alpha}.avif")),
                    &encoded.avif_file,
                )
                .unwrap();
            }
            for backend in [
                DecodeBackend::Rav1dSafe,
                #[cfg(feature = "zenav1-aom")]
                DecodeBackend::Zenav1Aom,
            ] {
                let mut decoder = ManagedAvifDecoder::new(
                    &encoded.avif_file,
                    &DecoderConfig::new().decode_backend(backend),
                )
                .unwrap();
                let pixels = decoder.decode(&Unstoppable).unwrap_or_else(|e| {
                    panic!("{backend:?} {depth:?} input16={input16} alpha={alpha}: {e}")
                });
                // Opaque input may legitimately omit the alpha plane.
                if let Some(img) = pixels.try_as_imgref::<Rgba<u8>>() {
                    for p in img.pixels() {
                        assert!(
                            p.a.abs_diff(alpha) <= 1,
                            "{backend:?} {depth:?} input16={input16}: alpha {} vs {alpha}",
                            p.a
                        );
                        if alpha != 0 {
                            for (v, want) in [(p.r, 75), (p.g, 125), (p.b, 175)] {
                                assert!(
                                    v.abs_diff(want) <= 5,
                                    "{backend:?} {depth:?} input16={input16} alpha={alpha}: {v} vs {want}"
                                );
                            }
                        }
                    }
                } else if let Some(img) = pixels.try_as_imgref::<Rgba<u16>>() {
                    for p in img.pixels() {
                        assert!(
                            p.a.abs_diff(u16::from(alpha) * 257) <= 257,
                            "{backend:?} {depth:?} input16={input16}: alpha {} vs {alpha}",
                            p.a
                        );
                        if alpha != 0 {
                            for (v, want) in [(p.r, 75u16), (p.g, 125), (p.b, 175)] {
                                assert!(
                                    v.abs_diff(want * 257) <= 5 * 257,
                                    "{backend:?} {depth:?} input16={input16} alpha={alpha}: {v} vs {}",
                                    want * 257
                                );
                            }
                        }
                    }
                } else if let Some(img) = pixels.try_as_imgref::<rgb::Rgb<u8>>() {
                    assert_eq!(alpha, 255);
                    for p in img.pixels() {
                        for (v, want) in [(p.r, 75), (p.g, 125), (p.b, 175)] {
                            assert!(v.abs_diff(want) <= 5);
                        }
                    }
                } else {
                    let img = pixels.try_as_imgref::<rgb::Rgb<u16>>().unwrap();
                    assert_eq!(alpha, 255);
                    for p in img.pixels() {
                        for (v, want) in [(p.r, 75u16), (p.g, 125), (p.b, 175)] {
                            assert!(v.abs_diff(want * 257) <= 5 * 257);
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn rgba8_premultiplication_preserves_visible_pixels_and_alpha() {
    check(false);
}
#[test]
fn rgba16_premultiplication_preserves_visible_pixels_and_alpha() {
    check(true);
}
