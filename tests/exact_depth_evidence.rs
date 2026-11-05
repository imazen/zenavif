//! Canonical U16 samples widened from AV1 codes say so: `ExactDepth` is what a
//! later step reads to know a 10- or 12-bit source sits on that grid.
use std::borrow::Cow;
use zenavif::AvifDecoderConfig;
use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
use zenpixels::{ChannelType, PixelDescriptor, PixelSlice};

fn vector(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/vectors/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Every sample must be exactly the nearest widening of a `bits`-bit code.
fn assert_on_grid(pixels: &PixelSlice<'_>, bits: u32, label: &str) {
    let max = (1u32 << bits) - 1;
    for y in 0..pixels.rows() {
        for sample in pixels.row(y).as_chunks::<2>().0 {
            let sample = u32::from(u16::from_ne_bytes(*sample));
            let code = (sample * max + 32767) / 65535;
            let widened = (code * 65535 + max / 2) / max;
            assert_eq!(widened, sample, "{label}: row {y}");
        }
    }
}

fn check_widened(data: &[u8], bits: u8, label: &str) {
    let output = AvifDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(data), &[])
        .unwrap()
        .decode()
        .unwrap();
    let pixels = output.pixels();
    assert_eq!(
        pixels.descriptor().channel_type(),
        ChannelType::U16,
        "{label}"
    );
    assert_eq!(output.info().source_color.bit_depth, Some(bits), "{label}");
    let exact = pixels
        .exact_depth()
        .unwrap_or_else(|| panic!("{label}: no depth"));
    assert!(exact.color_exact_at(bits), "{label}");
    assert_on_grid(&pixels, u32::from(bits), label);
}

#[test]
fn widened_decodes_state_their_depth_and_keep_to_it() {
    check_widened(
        &vector("zenavif/mono_gradient_10b_full.avif"),
        10,
        "mono 10",
    );
}

#[test]
fn eight_bit_results_state_nothing() {
    // A narrowed exit from a deep source, and an ordinary eight-bit source.
    for (name, preferred) in [
        (
            "zenavif/mono_gradient_10b_full.avif",
            vec![PixelDescriptor::RGB8_SRGB],
        ),
        ("libavif/kodim03_yuv420_8bpc.avif", vec![]),
    ] {
        let data = vector(name);
        let output = AvifDecoderConfig::new()
            .job()
            .decoder(Cow::Borrowed(&data), &preferred)
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(
            output.pixels().descriptor().channel_type(),
            ChannelType::U8,
            "{name}"
        );
        assert_eq!(output.pixels().exact_depth(), None, "{name}");
    }
}

#[cfg(feature = "encode")]
#[test]
fn color_widened_from_ten_and_twelve_bits_states_it() {
    use imgref::Img;
    use rgb::Rgb;
    use zenavif::{EncodeBitDepth, EncoderConfig};

    let pixels: Vec<Rgb<u16>> = (0..64u16 * 64)
        .map(|i| Rgb::new(i * 15, 60000 - i * 11, i * 3))
        .collect();
    for (depth, bits) in [(EncodeBitDepth::Ten, 10), (EncodeBitDepth::Twelve, 12)] {
        let config = EncoderConfig::new()
            .speed(10)
            .quality(90.0)
            .bit_depth(depth);
        let stop = almost_enough::StopToken::new(almost_enough::Unstoppable);
        let encoded =
            zenavif::encode_rgb16(Img::new(pixels.as_slice(), 64, 64), &config, stop).unwrap();
        check_widened(&encoded.avif_file, bits, "rgb");
    }
}

/// Unpremultiplying takes color off the code grid, so nothing is stated for it.
#[cfg(feature = "encode")]
#[test]
fn only_straight_alpha_states_a_depth() {
    use imgref::Img;
    use rgb::Rgba;
    use zenavif::{EncodeAlphaMode, EncodeBitDepth, EncoderConfig};

    let pixels: Vec<Rgba<u16>> = (0..64u16 * 64)
        .map(|i| Rgba::new(i * 15, 60000 - i * 11, i * 3, 20000 + i * 9))
        .collect();
    for (mode, expected_bits) in [
        (EncodeAlphaMode::UnassociatedDirty, Some(10u8)),
        (EncodeAlphaMode::Premultiplied, None),
    ] {
        let config = EncoderConfig::new()
            .speed(10)
            .quality(90.0)
            .bit_depth(EncodeBitDepth::Ten)
            .alpha_color_mode(mode);
        let stop = almost_enough::StopToken::new(almost_enough::Unstoppable);
        let encoded =
            zenavif::encode_rgba16(Img::new(pixels.as_slice(), 64, 64), &config, stop).unwrap();
        let output = AvifDecoderConfig::new()
            .job()
            .decoder(Cow::Borrowed(&encoded.avif_file), &[])
            .unwrap()
            .decode()
            .unwrap();
        let exact = output.pixels().exact_depth();
        match expected_bits {
            Some(bits) => {
                let exact = exact.expect("straight alpha states a depth");
                assert!(exact.color_exact_at(bits));
                assert_on_grid(&output.pixels(), u32::from(bits), "straight");
            }
            None => assert_eq!(exact, None, "premultiplied"),
        }
    }
}

#[cfg(all(feature = "encode", feature = "zenav1-aom"))]
#[test]
fn both_decode_backends_state_the_same_depth() {
    use almost_enough::Unstoppable;
    use imgref::Img;
    use rgb::Rgb;
    use zenavif::{DecodeBackend, DecoderConfig, EncodeBitDepth, EncoderConfig};

    let pixels: Vec<Rgb<u16>> = (0..64u16 * 64)
        .map(|i| Rgb::new(i * 15, 60000 - i * 11, i * 3))
        .collect();
    let config = EncoderConfig::new()
        .speed(10)
        .quality(90.0)
        .bit_depth(EncodeBitDepth::Ten);
    let stop = almost_enough::StopToken::new(Unstoppable);
    let encoded =
        zenavif::encode_rgb16(Img::new(pixels.as_slice(), 64, 64), &config, stop).unwrap();
    for backend in [DecodeBackend::Rav1dSafe, DecodeBackend::Zenav1Aom] {
        let decoded = zenavif::decode_with(
            &encoded.avif_file,
            &DecoderConfig::new().decode_backend(backend),
            &Unstoppable,
        )
        .unwrap();
        let exact = decoded
            .as_slice()
            .exact_depth()
            .unwrap_or_else(|| panic!("{backend:?}: no depth"));
        assert!(exact.color_exact_at(10), "{backend:?}");
        let narrowed = zenavif::decode_with(
            &encoded.avif_file,
            &DecoderConfig::new()
                .decode_backend(backend)
                .prefer_8bit(true),
            &Unstoppable,
        )
        .unwrap();
        assert_eq!(narrowed.as_slice().exact_depth(), None, "{backend:?}");
    }
}
