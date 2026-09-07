//! Timing regression fixture: the 39-byte first keyframe from libavif 1.3.0
//! tests/data/colors-animated-8bpc.avif (150x150, 8-bit 4:2:0).
//! Repeating the keyframe changes timing without changing decoded pixels.
use std::borrow::Cow;
use zenavif::AvifDecoderConfig;
use zenavif_serialize::Av1CBox;
use zenavif_serialize::animated::{AnimFrame, AnimatedImage, RepetitionCount};
use zencodec::decode::{AnimationFrameDecoder as _, DecodeJob as _, DecoderConfig as _};

const SAMPLE: &[u8] = &[
    0x12, 0x00, 0x0a, 0x0e, 0x00, 0x00, 0x00, 0x03, 0xbc, 0xac, 0xa9, 0xb5, 0xf2, 0x20, 0x21, 0xa0,
    0xd0, 0x80, 0x32, 0x13, 0x10, 0x00, 0x83, 0x80, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0xeb, 0xc5,
    0xa6, 0x2e, 0x0c, 0x0d, 0xd1, 0x51, 0x40,
];

fn sequence(timescale: u32, durations: &[u32]) -> Vec<u8> {
    repeated_sequence(timescale, durations, RepetitionCount::Finite(0))
}

fn repeated_sequence(timescale: u32, durations: &[u32], repetition: RepetitionCount) -> Vec<u8> {
    let mut image = AnimatedImage::new();
    let mut config = Av1CBox::default();
    config.seq_level_idx_0 = 0;
    image
        .set_timescale(timescale)
        .set_color_config(config)
        .set_repetition_count(repetition);
    let frames: Vec<_> = durations
        .iter()
        .map(|&d| AnimFrame::new(SAMPLE, d).with_sync(true))
        .collect();
    image
        .try_serialize(150, 150, &frames, &SAMPLE[2..18], None)
        .unwrap()
}

#[test]
fn codec_limits_count_fractional_milliseconds() {
    let data = sequence(1001, &[1, 1]);
    let config = AvifDecoderConfig::new();
    let mut decoder = config
        .job()
        .with_limits(zencodec::ResourceLimits::none().with_max_animation_ms(1))
        .animation_frame_decoder(Cow::Borrowed(&data), &[])
        .unwrap();
    assert!(decoder.render_next_frame(None).unwrap().is_some());
    assert!(
        decoder.render_next_frame(None).is_err(),
        "two 1/1001-second frames exceed 1 ms"
    );
}

#[test]
fn codec_limits_count_durations_above_u32_milliseconds() {
    let data = sequence(1, &[u32::MAX]);
    let config = AvifDecoderConfig::new();
    let mut decoder = config
        .job()
        .with_limits(zencodec::ResourceLimits::none().with_max_animation_ms(u64::from(u32::MAX)))
        .animation_frame_decoder(Cow::Borrowed(&data), &[])
        .unwrap();
    assert!(
        decoder.render_next_frame(None).is_err(),
        "seconds must not saturate to u32 milliseconds"
    );
}

#[test]
fn native_frames_preserve_exact_timing_and_pixels() {
    for (timescale, durations) in [
        (1001, vec![1, 1, 999, 1000, 1]),
        (1, vec![u32::MAX, u32::MAX, 1]),
        (u32::MAX, vec![1, u32::MAX, u32::MAX - 1]),
        (1000, vec![u32::MAX, 1, 1]),
        (30_000, vec![1001, 1001, 1001]),
        (1001, vec![1; 257]),
    ] {
        let data = sequence(timescale, &durations);
        let config = zenavif::DecoderConfig::new();
        let mut lazy = zenavif::AnimationDecoder::new(&data, &config).unwrap();
        let mut managed = zenavif::ManagedAvifDecoder::new(&data, &config).unwrap();
        let eager = managed.decode_animation(&zenavif::Unstoppable).unwrap();
        assert_eq!(eager.frames.len(), durations.len());
        let codec_config = AvifDecoderConfig::new();
        let codec = codec_config
            .job()
            .animation_frame_decoder(Cow::Borrowed(&data), &[])
            .unwrap();
        let mut pts = 0;
        // Metadata lookup in reverse order must not advance the decoder.
        for index in (0..durations.len()).rev() {
            assert_eq!(
                lazy.frame_timing(index).unwrap().duration_in_timescales,
                durations[index]
            );
        }
        assert_eq!(lazy.frame_index(), 0);
        for (index, &duration) in durations.iter().enumerate() {
            let expected = zenavif::AnimationFrameTiming {
                timescale,
                pts_in_timescales: pts,
                duration_in_timescales: duration,
            };
            assert_eq!(managed.frame_timing(index).unwrap(), expected);
            assert_eq!(codec.frame_timing(index).unwrap(), expected);
            let frame = lazy.next_frame(&zenavif::Unstoppable).unwrap().unwrap();
            assert_eq!(frame.timing, expected);
            assert_eq!(eager.frames[index].timing, expected);
            let legacy_ms = ((u64::from(duration) * 1000) / u64::from(timescale))
                .min(u64::from(u32::MAX)) as u32;
            assert_eq!(frame.duration_ms, legacy_ms);
            assert_eq!((frame.pixels.width(), frame.pixels.height()), (150, 150));
            for row in 0..150 {
                assert_eq!(
                    frame.pixels.as_slice().row(row),
                    eager.frames[index].pixels.as_slice().row(row)
                );
                assert_eq!(
                    frame.pixels.as_slice().row(row),
                    eager.frames[0].pixels.as_slice().row(row)
                );
            }
            pts += u64::from(duration);
        }
        assert!(lazy.next_frame(&zenavif::Unstoppable).unwrap().is_none());
        assert!(lazy.frame_timing(durations.len()).is_err());
        assert!(managed.frame_timing(durations.len()).is_err());
        assert!(codec.frame_timing(durations.len()).is_err());
    }
}

#[test]
fn codec_duration_limit_keeps_fractional_precision_across_skipped_frames() {
    let data = sequence(1001, &[1, 1, 1]);
    let config = AvifDecoderConfig::new();
    let mut decoder = config
        .job()
        .with_start_frame_index(1)
        .with_limits(zencodec::ResourceLimits::none().with_max_animation_ms(1))
        .animation_frame_decoder(Cow::Borrowed(&data), &[])
        .unwrap();
    let err = decoder.render_next_frame(None).unwrap_err();
    assert!(matches!(
        err.error()
            .detail()
            .and_then(|d| d.downcast_ref::<zenavif::Error>()),
        Some(zenavif::Error::ResourceLimit(_))
    ));
}

#[test]
fn codec_duration_limit_accepts_exact_boundary() {
    let data = sequence(1001, &[1001]);
    let config = AvifDecoderConfig::new();
    let mut decoder = config
        .job()
        .with_limits(zencodec::ResourceLimits::none().with_max_animation_ms(1000))
        .animation_frame_decoder(Cow::Borrowed(&data), &[])
        .unwrap();
    let frame = decoder.render_next_frame(None).unwrap().unwrap();
    assert_eq!(frame.duration_ms(), 1000);
    assert!(decoder.render_next_frame(None).unwrap().is_none());
}

#[test]
fn finite_count_survives_native_pixel_decode_and_codec_boundary() {
    for (repeat, expected) in [
        (RepetitionCount::Finite(0), 1),
        (RepetitionCount::Finite(u32::MAX - 1), u64::from(u32::MAX)),
        (RepetitionCount::Finite(u32::MAX), u64::from(u32::MAX) + 1),
        (RepetitionCount::Infinite, 0),
    ] {
        let data = repeated_sequence(1001, &[1, 2], repeat);
        let config = zenavif::DecoderConfig::new();
        let eager = zenavif::decode_animation(&data).unwrap();
        assert_eq!(eager.info.loop_count, expected);
        assert_eq!(eager.frames.len(), 2);
        let mut lazy = zenavif::AnimationDecoder::new(&data, &config).unwrap();
        assert_eq!(lazy.info().loop_count, expected);
        assert!(lazy.next_frame(&zenavif::Unstoppable).unwrap().is_some());
        assert!(lazy.next_frame(&zenavif::Unstoppable).unwrap().is_some());
        assert!(lazy.next_frame(&zenavif::Unstoppable).unwrap().is_none());
        let codec_config = AvifDecoderConfig::new();
        let probe = codec_config.clone().job().probe(&data);
        let codec = codec_config
            .job()
            .animation_frame_decoder(Cow::Borrowed(&data), &[]);
        if expected <= u64::from(u32::MAX) {
            assert!(probe.unwrap().is_animation());
            assert_eq!(codec.unwrap().loop_count(), Some(expected as u32));
        } else {
            let errors = [probe.err().unwrap(), codec.err().unwrap()];
            for err in errors {
                assert!(matches!(
                    err.error()
                        .detail()
                        .and_then(|d| d.downcast_ref::<zenavif::Error>()),
                    Some(zenavif::Error::Unsupported(_))
                ));
            }
        }
    }
}
