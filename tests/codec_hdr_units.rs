#![cfg(feature = "zenav1-svt")]
use imgref::Img;
use rgb::Rgb;
use zenavif::*;
use zencodec::encode::{
    AnimationFrameEncoder as _, EncodeJob as _, Encoder as _, EncoderConfig as _,
};

fn metadata() -> zencodec::MasteringDisplay {
    zencodec::MasteringDisplay::new(
        [[0.68, 0.32], [0.265, 0.69], [0.15, 0.06]],
        [0.3127, 0.3290],
        1000.0,
        0.005,
    )
}
fn config() -> EncoderConfig {
    EncoderConfig::new()
        .backend(Av1Backend::Zenav1Svt)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
        .speed(10)
}
fn expected() -> MasteringDisplayConfig {
    MasteringDisplayConfig {
        primaries: [(13250, 34500), (7500, 3000), (34000, 16000)],
        white_point: (15635, 16450),
        max_luminance: 10_000_000,
        min_luminance: 50,
    }
}
fn assert_wire(data: &[u8]) {
    let parser = zenavif_parse::AvifParser::from_bytes(data).unwrap();
    let actual = parser.mastering_display().unwrap();
    let expected = expected();
    assert_eq!(
        actual.primaries, expected.primaries,
        "mdcv uses GBR order and 50000 units"
    );
    assert_eq!(actual.white_point, expected.white_point);
    assert_eq!(actual.max_luminance, expected.max_luminance);
    assert_eq!(actual.min_luminance, expected.min_luminance);
}

#[test]
#[allow(deprecated)] // Directly exercise metadata ingestion on the public codec job.
fn codec_still_and_animation_write_st2086_units_and_gbr_order() {
    let pixels = Img::new(
        vec![
            Rgb {
                r: 32u8,
                g: 64,
                b: 96
            };
            32 * 32
        ],
        32,
        32,
    );
    let mut cfg = AvifEncoderConfig::new();
    *cfg.inner_mut() = config();
    let meta = zencodec::Metadata::none().with_mastering_display(metadata());
    let still = cfg
        .clone()
        .job()
        .with_metadata(meta.clone())
        .encoder()
        .unwrap()
        .encode(zenpixels::PixelSlice::from(pixels.as_ref()).erase())
        .unwrap();
    assert_wire(still.data());
    let mut anim = cfg
        .job()
        .with_metadata(meta)
        .animation_frame_encoder()
        .unwrap();
    for duration in [10, 20] {
        anim.push_frame(
            zenpixels::PixelSlice::from(pixels.as_ref()).erase(),
            duration,
            None,
        )
        .unwrap();
    }
    let encoded = anim.finish(None).unwrap();
    assert_wire(encoded.data());
    let decoded = decode_animation(encoded.data()).unwrap();
    let track = decoded.info.hdr.mastering_display.unwrap();
    assert_eq!(track.primaries, expected().primaries);
    assert_eq!(track.max_luminance, 10_000_000);
    assert_eq!(track.min_luminance, 50);
}

#[test]
fn codec_probe_reads_wire_gbr_as_rgb() {
    let pixels = Img::new(
        vec![
            Rgb {
                r: 32u8,
                g: 64,
                b: 96
            };
            32 * 32
        ],
        32,
        32,
    );
    let file = encode_rgb8(
        pixels.as_ref(),
        &config().mastering_display(expected()),
        almost_enough::StopToken::new(almost_enough::Unstoppable),
    )
    .unwrap();
    assert_wire(&file.avif_file);
    let actual = AvifDecoderConfig::new()
        .probe_full(&file.avif_file)
        .unwrap()
        .source_color
        .mastering_display
        .unwrap();
    let expected = metadata();
    for (actual, expected) in actual
        .primaries_xy
        .iter()
        .flatten()
        .zip(expected.primaries_xy.iter().flatten())
    {
        assert!(
            (actual - expected).abs() < 0.000021,
            "RGB primary {actual} vs {expected}"
        );
    }
    for (actual, expected) in actual
        .white_point_xy
        .iter()
        .zip(expected.white_point_xy.iter())
    {
        assert!((actual - expected).abs() < 0.000021);
    }
    assert!((actual.max_luminance - expected.max_luminance).abs() < 0.001);
    assert!((actual.min_luminance - expected.min_luminance).abs() < 0.0001);
}
