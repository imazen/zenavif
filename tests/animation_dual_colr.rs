#[path = "support/animation.rs"]
mod animation_support;

use std::borrow::Cow;
use zenavif::{AvifDecoderConfig, DecoderConfig, ManagedAvifDecoder, Unstoppable};
use zenavif_serialize::animated::{AnimFrame, AnimatedImage};
use zencodec::decode::{AnimationFrameDecoder as _, DecodeJob as _, DecoderConfig as _};

// SVT 150x150 RGB(75,125,175) fixture. Only the three CICP bytes were changed
// to unspecified, permitting sample-entry nclx to define the conversion.
const SAMPLE: &[u8] = &[
    10, 14, 2, 0, 0, 44, 29, 229, 101, 64, 39, 145, 2, 2, 2, 132, 50, 28, 16, 0, 136, 0, 6, 24, 32,
    130, 8, 36, 0, 173, 64, 89, 124, 214, 69, 97, 11, 21, 193, 8, 150, 63, 234, 231, 8, 96,
];

#[derive(Default)]
struct CollectRows(Option<zenpixels::PixelBuffer>);

impl zencodec::decode::DecodeRowSink for CollectRows {
    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: zenpixels::PixelDescriptor,
    ) -> Result<(), zencodec::decode::SinkError> {
        self.0 = Some(zenpixels::PixelBuffer::new(width, height, descriptor));
        Ok(())
    }
    fn provide_next_buffer(
        &mut self,
        y: u32,
        height: u32,
        width: u32,
        descriptor: zenpixels::PixelDescriptor,
    ) -> Result<zenpixels::PixelSliceMut<'_>, zencodec::decode::SinkError> {
        let pixels = self.0.as_mut().unwrap();
        assert_eq!(pixels.width(), width);
        assert_eq!(pixels.descriptor(), descriptor);
        Ok(pixels.rows_mut(y, height))
    }
    fn finish(&mut self) -> Result<(), zencodec::decode::SinkError> {
        Ok(())
    }
}

#[test]
fn icc_and_nclx_survive_together_in_both_box_orders() {
    let fixture = include_bytes!("vectors/libavif/paris_icc_exif_xmp.avif");
    let p = zenavif_parse::AvifParser::from_bytes(fixture).unwrap();
    let Some(zenavif_parse::ColorInformation::IccProfile(icc)) = p.color_info() else {
        panic!("ICC fixture lost profile")
    };
    let mut mux = AnimatedImage::new();
    mux.set_color_description(9, 16, 9, true);
    let serialize = |mux: &AnimatedImage| {
        mux.try_serialize(
            150,
            150,
            &[AnimFrame::new(SAMPLE, 1).with_sync(true)],
            &SAMPLE[..16],
            None,
        )
        .unwrap()
    };
    let control = serialize(&mux);
    let expected = zenavif::decode_animation(&control).unwrap();
    mux.set_icc_profile(icc.clone());
    let original = serialize(&mux);
    // Reordering adjacent colr boxes preserves total sizes, sample offsets,
    // and the primary item's association with both color properties.
    let positions: Vec<_> = original
        .windows(4)
        .enumerate()
        .filter_map(|(i, b)| (b == b"colr").then(|| i - 4))
        .collect();
    assert_eq!(positions.len(), 4);
    for reverse in [false, true] {
        let mut data = original.clone();
        if reverse {
            for pair in positions.chunks_exact(2) {
                let a = pair[0];
                let b = pair[1];
                let len_a = u32::from_be_bytes(data[a..a + 4].try_into().unwrap()) as usize;
                let len_b = u32::from_be_bytes(data[b..b + 4].try_into().unwrap()) as usize;
                assert_eq!(a + len_a, b);
                data[a..b + len_b].rotate_left(len_a);
            }
        }
        for parser in [
            zenavif_parse::AvifParser::from_bytes(&data).unwrap(),
            zenavif_parse::AvifParser::from_owned(data.clone()).unwrap(),
        ] {
            for color in [parser.color_info(), parser.animation_color_info()] {
                assert_eq!(
                    color,
                    Some(&zenavif_parse::ColorInformation::IccProfile(icc.clone()))
                );
            }
            for nclx in [parser.nclx_color_info(), parser.animation_nclx_color_info()] {
                assert_eq!(
                    nclx,
                    Some(&zenavif_parse::ColorInformation::Nclx {
                        color_primaries: 9,
                        transfer_characteristics: 16,
                        matrix_coefficients: 9,
                        full_range: true
                    })
                );
            }
        }
        let detected = zenavif::detect::probe(&data).unwrap();
        assert!(detected.has_icc_profile);
        assert_eq!(
            (
                detected.color_primaries,
                detected.transfer_characteristics,
                detected.matrix_coefficients
            ),
            (Some(9), Some(16), Some(9))
        );
        #[cfg(feature = "unsafe-asm")]
        {
            let mut legacy = zenavif::AvifDecoder::new(&data, &DecoderConfig::new()).unwrap();
            assert_eq!(legacy.info().icc_profile.as_deref(), Some(icc.as_slice()));
            let pixels = legacy.decode(&Unstoppable).unwrap();
            for row in 0..150 {
                assert_eq!(
                    pixels.as_slice().row(row),
                    expected.frames[0].pixels.as_slice().row(row)
                );
            }
        }
        let cfg = AvifDecoderConfig::new();
        let info = cfg.clone().job().probe(&data).unwrap();
        assert_eq!(
            info.source_color.icc_profile.as_deref(),
            Some(icc.as_slice())
        );
        let mut native = ManagedAvifDecoder::new(&data, &DecoderConfig::new()).unwrap();
        let native_info = native.probe_info().unwrap();
        assert_eq!(native_info.icc_profile.as_deref(), Some(icc.as_slice()));
        let (poster, poster_info) = native.decode_full(&Unstoppable).unwrap();
        assert_eq!(poster_info.icc_profile.as_deref(), Some(icc.as_slice()));
        let mut streaming = ManagedAvifDecoder::new(&data, &DecoderConfig::new()).unwrap();
        let mut sink = CollectRows::default();
        let streaming_info = streaming.decode_to_sink(&Unstoppable, &mut sink).unwrap();
        assert_eq!(streaming_info.icc_profile.as_deref(), Some(icc.as_slice()));
        assert_eq!(streaming_info.color_primaries.0, 9);
        let streamed = sink.0.unwrap();
        for row in 0..150 {
            assert_eq!(
                streamed.as_slice().row(row),
                expected.frames[0].pixels.as_slice().row(row)
            );
        }
        let eager = zenavif::decode_animation(&data).unwrap();
        for row in 0..150 {
            assert_eq!(
                poster.as_slice().row(row),
                expected.frames[0].pixels.as_slice().row(row)
            );
            assert_eq!(
                eager.frames[0].pixels.as_slice().row(row),
                expected.frames[0].pixels.as_slice().row(row)
            );
        }
        for backend in [
            zenavif::DecodeBackend::Rav1dSafe,
            #[cfg(feature = "zenav1-aom")]
            zenavif::DecodeBackend::Zenav1Aom,
        ] {
            let mut cfg = AvifDecoderConfig::new();
            *cfg.inner_mut() = DecoderConfig::new().decode_backend(backend);
            let mut dec = cfg
                .job()
                .animation_frame_decoder(Cow::Borrowed(&data), &[])
                .unwrap();
            assert_eq!(
                dec.info().source_color.icc_profile.as_deref(),
                Some(icc.as_slice())
            );
            let frame = dec.render_next_frame(None).unwrap().unwrap();
            for row in 0..150 {
                assert_eq!(
                    frame.pixels().row(row),
                    expected.frames[0].pixels.as_slice().row(row)
                );
            }
        }
    }
}

#[test]
fn posterless_animation_retains_both_color_properties() {
    let fixture = include_bytes!("vectors/libavif/paris_icc_exif_xmp.avif");
    let p = zenavif_parse::AvifParser::from_bytes(fixture).unwrap();
    let Some(zenavif_parse::ColorInformation::IccProfile(icc)) = p.color_info() else {
        panic!("ICC fixture lost profile")
    };
    let mut mux = AnimatedImage::new();
    mux.set_color_description(9, 16, 9, true);
    let serialize = |mux: &AnimatedImage| {
        mux.try_serialize(
            150,
            150,
            &[AnimFrame::new(SAMPLE, 1).with_sync(true)],
            &SAMPLE[..16],
            None,
        )
        .unwrap()
    };
    let control = zenavif::decode_animation(&serialize(&mux)).unwrap();
    mux.set_icc_profile(icc.clone());
    let mut data = serialize(&mux);
    animation_support::remove_poster(&mut data);
    let p = zenavif_parse::AvifParser::from_bytes(&data).unwrap();
    assert!(p.primary_data().unwrap().is_empty());
    assert_eq!(p.color_info(), p.animation_color_info());
    assert_eq!(p.nclx_color_info(), p.animation_nclx_color_info());
    let native = ManagedAvifDecoder::new(&data, &DecoderConfig::new())
        .unwrap()
        .probe_info()
        .unwrap();
    assert_eq!(native.icc_profile.as_deref(), Some(icc.as_slice()));
    assert_eq!(
        (native.color_primaries.0, native.transfer_characteristics.0),
        (9, 16)
    );
    for backend in [
        zenavif::DecodeBackend::Rav1dSafe,
        #[cfg(feature = "zenav1-aom")]
        zenavif::DecodeBackend::Zenav1Aom,
    ] {
        let mut cfg = AvifDecoderConfig::new();
        *cfg.inner_mut() = DecoderConfig::new().decode_backend(backend);
        let mut dec = cfg
            .job()
            .animation_frame_decoder(Cow::Borrowed(&data), &[])
            .unwrap();
        assert_eq!(
            dec.info().source_color.icc_profile.as_deref(),
            Some(icc.as_slice())
        );
        let frame = dec.render_next_frame(None).unwrap().unwrap();
        for row in 0..150 {
            assert_eq!(
                frame.pixels().row(row),
                control.frames[0].pixels.as_slice().row(row)
            );
        }
    }
}
