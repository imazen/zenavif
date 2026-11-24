//! Exact identity-matrix decoder qualification and separate zenrav1e
//! channel-routing checks with the existing two-code encoder tolerance.

#[path = "support/independent_av1.rs"]
mod independent;
#[cfg(feature = "encode-imazen")]
#[path = "support/pixels.rs"]
mod pixel_support;

#[test]
fn independent_lossless_gbr_routes_every_native_code_exactly() {
    use zenpixels::sample::ExactDepth;
    use zenpixels::{SampleLayout, SignalRange};

    for (bits, obu) in [
        (8, independent::GBR8),
        (10, independent::GBR10),
        (12, independent::GBR12),
    ] {
        use zenavif_serialize::constants::{
            ColorPrimaries, MatrixCoefficients, TransferCharacteristics,
        };
        let avif = zenavif_serialize::Aviffy::new()
            .set_color_primaries(ColorPrimaries::Bt709)
            .set_transfer_characteristics(TransferCharacteristics::Srgb)
            .set_matrix_coefficients(MatrixCoefficients::Rgb)
            .set_full_color_range(true)
            .set_strict_payload_agreement(true)
            .try_to_vec(obu, None, 16, 16, bits)
            .unwrap();
        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        let header = parser.primary_metadata().unwrap();
        assert_eq!(header.bit_depth, bits);
        // The 12-bit libavif encoder supplies the identity declaration in
        // nclx. The 8/10-bit aomenc inputs also declare it in the AV1 header.
        assert_eq!(header.matrix_coefficients, if bits == 12 { 2 } else { 0 });
        assert!(matches!(
            parser.nclx_color_info(),
            Some(zenavif_parse::ColorInformation::Nclx {
                color_primaries: 1,
                transfer_characteristics: 13,
                matrix_coefficients: 0,
                full_range: true,
            })
        ));
        assert!(header.full_range);
        assert!(!header.chroma_subsampling.horizontal && !header.chroma_subsampling.vertical);
        assert_eq!(header.lossless, Some(true));

        let pixels = zenavif::decode(&avif).unwrap();
        let layout = if bits == 8 {
            SampleLayout::RGB8
        } else {
            SampleLayout::RGB16
        };
        assert_eq!(pixels.spec().layout, layout);
        let view = pixels.as_view();
        let depth = ExactDepth::new(layout.sample, SignalRange::Full, bits, bits).unwrap();
        assert!(depth.holds_for(&view).unwrap());
        if bits > 8 {
            assert_eq!(view.facts().depth, Some(depth));
        }
        let max = (1u16 << bits) - 1;
        for y in 0..16 {
            for x in 0..16usize {
                let expected = if bits > 8 && x == 0 {
                    [1, 4, 17]
                } else {
                    [[max, 0, 0], [0, max, 0], [0, 0, max], [max, max, max]]
                        [usize::from(y >= 8) * 2 + usize::from(x >= 8)]
                };
                if bits == 8 {
                    assert_eq!(
                        view.row_as::<[u8; 3]>(y).unwrap()[x],
                        expected.map(|q| q as u8),
                        "{bits}-bit ({x}, {y})"
                    );
                } else {
                    let widened = expected.map(|q| {
                        ((u32::from(q) * 65535 + u32::from(max) / 2) / u32::from(max)) as u16
                    });
                    assert_eq!(
                        view.row_as::<[u16; 3]>(y).unwrap()[x],
                        widened,
                        "{bits}-bit ({x}, {y})"
                    );
                }
            }
        }
    }
}

#[cfg(feature = "encode-imazen")]
use almost_enough::{StopToken, Unstoppable};
#[cfg(feature = "encode-imazen")]
use imgref::ImgVec;
#[cfg(feature = "encode-imazen")]
use pixel_support::BufTestExt as _;
#[cfg(feature = "encode-imazen")]
use rgb::{RGB16, Rgb};
#[cfg(feature = "encode-imazen")]
use zenavif::{EncodeColorModel, EncoderConfig};

#[cfg(feature = "encode-imazen")]
fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

#[cfg(feature = "encode-imazen")]
fn mix(x: u32, y: u32, salt: u32) -> u8 {
    let mut h = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B) ^ salt;
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    (h >> 16) as u8
}

/// Expand a 10-bit value to 16 bits the way the decoder does, the canonical
/// mapping: these values round-trip the encoder's nearest 16→10 narrowing exactly.
#[cfg(feature = "encode-imazen")]
fn expand10(v10: u16) -> u16 {
    ((u32::from(v10) * 65535 + 511) / 1023) as u16
}

#[test]
#[cfg(feature = "encode-imazen")]
fn zenrav1e_identity_8bit_channel_routing_within_two_codes() {
    let (w, h) = (64usize, 64usize);
    let pixels: Vec<Rgb<u8>> = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| Rgb {
                r: mix(x as u32, y as u32, 1),
                g: mix(x as u32, y as u32, 2),
                b: mix(x as u32, y as u32, 3),
            })
        })
        .collect();
    let img = ImgVec::new(pixels, w, h);

    let cfg = EncoderConfig::new()
        .speed(6)
        .threads(Some(1))
        .color_model(EncodeColorModel::Rgb)
        .with_lossless(true);
    let enc = zenavif::encode_rgb8(img.as_ref(), &cfg, stop()).expect("encode");

    let dec_cfg = zenavif::DecoderConfig::new().prefer_8bit(true);
    let out = zenavif::decode_with(&enc.avif_file, &dec_cfg, &Unstoppable).expect("decode");
    let got = out.try_as_imgref::<Rgb<u8>>().expect("rgb8 view");

    assert_eq!(got.width(), w);
    assert_eq!(got.height(), h);
    for y in 0..h {
        for x in 0..w {
            let (e, g) = (img.buf()[y * w + x], got.buf()[y * got.stride() + x]);
            let d = (i32::from(e.r) - i32::from(g.r))
                .abs()
                .max((i32::from(e.g) - i32::from(g.g)).abs())
                .max((i32::from(e.b) - i32::from(g.b)).abs());
            assert!(
                d <= 2, // zenrav1e#9; rotation/matrix bugs produce d in the tens+
                "identity roundtrip delta {d} at ({x},{y}): encoded {e:?}, decoded {g:?} — \
             channel rotation or matrix math on the identity path"
            );
        }
    }
}

#[test]
#[cfg(feature = "encode-imazen")]
fn zenrav1e_identity_10bit_channel_routing_within_two_codes() {
    let (w, h) = (64usize, 64usize);
    // Canonical 10-bit-derived values with independently varied lanes.
    // The assertion allows the documented zenrav1e channel-code error.
    let pixels: Vec<RGB16> = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| RGB16 {
                r: expand10(u16::from(mix(x as u32, y as u32, 1)) * 4 % 1024),
                g: expand10(u16::from(mix(x as u32, y as u32, 2)) * 4 % 1024),
                b: expand10(u16::from(mix(x as u32, y as u32, 3)) * 4 % 1024),
            })
        })
        .collect();
    let img = ImgVec::new(pixels, w, h);

    let cfg = EncoderConfig::new()
        .speed(6)
        .threads(Some(1))
        .with_lossless(true);
    let enc = zenavif::encode_rgb16(img.as_ref(), &cfg, stop()).expect("encode");

    let dec_cfg = zenavif::DecoderConfig::new().prefer_8bit(false);
    let out = zenavif::decode_with(&enc.avif_file, &dec_cfg, &Unstoppable).expect("decode");
    let got = out.try_as_imgref::<Rgb<u16>>().expect("rgb16 view");

    assert_eq!(got.width(), w);
    assert_eq!(got.height(), h);
    for y in 0..h {
        for x in 0..w {
            let (e, g) = (img.buf()[y * w + x], got.buf()[y * got.stride() + x]);
            // Compare in the native 10-bit domain (>>6 inverts expansion
            // on this canonical 10-bit source grid).
            let d10 = |a: u16, b: u16| (i32::from(a >> 6) - i32::from(b >> 6)).abs();
            let d = d10(e.r, g.r).max(d10(e.g, g.g)).max(d10(e.b, g.b));
            assert!(
                d <= 2, // zenrav1e#9 (±2 ten-bit steps); rotation = hundreds
                "16-bit identity roundtrip 10-bit delta {d} at ({x},{y}): encoded {e:?}, \
             decoded {g:?} — this is exactly the #14 plane-rotation failure mode"
            );
        }
    }
}

/// Lossy sanity for the same paths: dominant channels land in the
/// right slots without requiring exact equality (catches gross
/// rotations even if lossless ever regresses to near-lossless).
#[test]
#[cfg(feature = "encode-imazen")]
fn identity_lossy_channels_stay_in_their_slots() {
    let (w, h) = (64usize, 64usize);
    let red8: Vec<Rgb<u8>> = vec![
        Rgb {
            r: 200,
            g: 10,
            b: 30
        };
        w * h
    ];
    let img = ImgVec::new(red8, w, h);
    let cfg = EncoderConfig::new()
        .quality(95.0)
        .speed(8)
        .threads(Some(1))
        .color_model(EncodeColorModel::Rgb);
    let enc = zenavif::encode_rgb8(img.as_ref(), &cfg, stop()).expect("encode");
    let out = zenavif::decode_with(
        &enc.avif_file,
        &zenavif::DecoderConfig::new().prefer_8bit(true),
        &Unstoppable,
    )
    .expect("decode");
    let got = out.try_as_imgref::<Rgb<u8>>().expect("rgb8");
    let p = got.buf()[(h / 2) * got.stride() + w / 2];
    assert!(
        p.r > 150 && p.g < 80 && p.b < 80,
        "red must stay red on the identity path; got {p:?} (the pre-fix decoder \
     returned R=111 G=0 B=0 here via BT.601-on-GBR)"
    );

    let red16: Vec<RGB16> = vec![
        RGB16 {
            r: 200 * 257,
            g: 10 * 257,
            b: 30 * 257,
        };
        w * h
    ];
    let img16 = ImgVec::new(red16, w, h);
    let cfg16 = EncoderConfig::new().quality(95.0).speed(8).threads(Some(1));
    let enc16 = zenavif::encode_rgb16(img16.as_ref(), &cfg16, stop()).expect("encode16");
    let out16 = zenavif::decode_with(
        &enc16.avif_file,
        &zenavif::DecoderConfig::new().prefer_8bit(true),
        &Unstoppable,
    )
    .expect("decode16");
    let got16 = out16.try_as_imgref::<Rgb<u8>>().expect("rgb8 of 16");
    let p = got16.buf()[(h / 2) * got16.stride() + w / 2];
    assert!(
        p.r > 150 && p.g < 80 && p.b < 80,
        "red must stay red on the 16-bit path; got {p:?} (the pre-fix encoder \
     wrote RGB plane order, decoding as R=63 G=255 B=0 here)"
    );
}
