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

#[test]
fn native_animation_hdr_metadata_reaches_decoded_info_without_changing_pixels() {
    let mut image = AnimatedImage::new();
    let mut config = Av1CBox::default();
    config.seq_level_idx_0 = 0;
    image.set_color_config(config);
    let frames = [AnimFrame::new(SAMPLE, 1).with_sync(true)];
    let untagged = image
        .try_serialize(150, 150, &frames, &SAMPLE[2..18], None)
        .unwrap();
    let expected = zenavif::decode_animation(&untagged).unwrap();
    assert_eq!(expected.info.hdr, zenavif::AnimationHdrMetadata::default());
    let mut cclv = zenavif_serialize::CclvBox::new();
    cclv.primaries = Some([(-1234, 45678), (7500, 3000), (34000, 16000)]);
    cclv.min_luminance = Some(0);
    cclv.max_luminance = Some(10_000_000);
    cclv.avg_luminance = Some(1_000_000);
    image
        .set_amve(zenavif_serialize::AmveBox::new(100_000, 15635, 16450))
        .set_cclv(cclv);
    let tagged = image
        .try_serialize(150, 150, &frames, &SAMPLE[2..18], None)
        .unwrap();
    let actual = zenavif::decode_animation(&tagged).unwrap();
    let mut lazy = zenavif::AnimationDecoder::new(&tagged, &zenavif::DecoderConfig::new()).unwrap();
    assert_eq!(lazy.info().hdr, actual.info.hdr);
    assert_eq!(
        actual.info.hdr.ambient_viewing.unwrap().ambient_illuminance,
        100_000
    );
    assert_eq!(
        actual.info.hdr.content_colour_volume.unwrap().primaries,
        cclv.primaries
    );
    assert_eq!(
        actual.info.hdr.content_colour_volume.unwrap().min_luminance,
        Some(0)
    );
    assert_eq!(
        actual.info.hdr.content_colour_volume.unwrap().max_luminance,
        cclv.max_luminance
    );
    assert_eq!(
        actual.info.hdr.content_colour_volume.unwrap().avg_luminance,
        cclv.avg_luminance
    );
    assert!(actual.info.hdr.content_light_level.is_none());
    assert!(actual.info.hdr.mastering_display.is_none());
    let lazy_frame = lazy.next_frame(&zenavif::Unstoppable).unwrap().unwrap();
    for row in 0..150 {
        assert_eq!(
            actual.frames[0].pixels.as_slice().row(row),
            expected.frames[0].pixels.as_slice().row(row)
        );
        assert_eq!(
            lazy_frame.pixels.as_slice().row(row),
            expected.frames[0].pixels.as_slice().row(row)
        );
    }
}

#[test]
fn codec_animation_hdr_comes_from_track_not_poster() {
    use zencodec::decode::AnimationFrameDecoder as _;
    let mut mux = AnimatedImage::new();
    let mut cfg = Av1CBox::default();
    cfg.seq_level_idx_0 = 0;
    mux.set_color_config(cfg)
        .set_clli(zenavif_serialize::ClliBox::new(1000, 400))
        .set_mdcv(zenavif_serialize::MdcvBox::new(
            [(13250, 34500), (7500, 3000), (34000, 16000)],
            (15635, 16450),
            10_000_000,
            50,
        ));
    let mut cclv = zenavif_serialize::CclvBox::new();
    cclv.primaries = Some([(-1234, 45678), (7500, 3000), (34000, 16000)]);
    cclv.max_luminance = Some(10_000_000);
    mux.set_amve(zenavif_serialize::AmveBox::new(100_000, 15635, 16450))
        .set_cclv(cclv);
    let original = mux
        .try_serialize(
            150,
            150,
            &[AnimFrame::new(SAMPLE, 1).with_sync(true)],
            &SAMPLE[2..18],
            None,
        )
        .unwrap();
    // The serializer writes poster properties before track properties. Check
    // both known payloads before changing only the poster or track's box type.
    let positions = |data: &[u8], kind: &[u8; 4]| -> Vec<usize> {
        data.windows(4)
            .enumerate()
            .filter_map(|(i, b)| (b == kind).then_some(i))
            .collect()
    };
    let clli = positions(&original, b"clli");
    let mdcv = positions(&original, b"mdcv");
    assert_eq!(clli.len(), 2);
    assert_eq!(mdcv.len(), 2);
    let expected_pixels = zenavif::decode_animation(&original).unwrap();
    for (remove_track, remove_poster) in [(false, false), (true, false), (false, true)] {
        let mut data = original.clone();
        // Give the poster different HDR values.
        data[clli[0] + 4..clli[0] + 8].copy_from_slice(&[0, 10, 0, 4]);
        data[mdcv[0] + 20..mdcv[0] + 24].copy_from_slice(&1_000_000u32.to_be_bytes());
        if remove_track {
            data[clli[1]..clli[1] + 4].copy_from_slice(b"free");
            data[mdcv[1]..mdcv[1] + 4].copy_from_slice(b"free");
        }
        if remove_poster {
            let mut pos = 0;
            while pos < data.len() {
                let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
                assert!(size >= 8 && pos + size <= data.len());
                if &data[pos + 4..pos + 8] == b"meta" {
                    data[pos + 4..pos + 8].copy_from_slice(b"free");
                }
                pos += size;
            }
        }
        let parser = zenavif_parse::AvifParser::from_bytes(&data).unwrap();
        assert_eq!(
            parser
                .content_light_level()
                .unwrap()
                .max_content_light_level,
            if remove_poster { 1000 } else { 10 }
        );
        let track = parser.animation_info().unwrap().hdr;
        assert_eq!(track.content_light_level.is_some(), !remove_track);
        let cfg = AvifDecoderConfig::new();
        let probe = cfg.clone().job().probe(&data).unwrap();
        let mut dec = cfg
            .job()
            .animation_frame_decoder(Cow::Borrowed(&data), &[])
            .unwrap();
        assert_eq!(dec.hdr_metadata(), track);
        assert_eq!(
            dec.hdr_metadata()
                .ambient_viewing
                .unwrap()
                .ambient_illuminance,
            100_000
        );
        assert_eq!(
            dec.hdr_metadata().content_colour_volume.unwrap().primaries,
            cclv.primaries
        );
        for info in [&probe, dec.info()] {
            if remove_track {
                assert!(info.source_color.content_light_level.is_none());
                assert!(info.source_color.mastering_display.is_none());
            } else {
                assert_eq!(
                    info.source_color
                        .content_light_level
                        .unwrap()
                        .max_content_light_level,
                    1000
                );
                assert_eq!(
                    info.source_color.mastering_display.unwrap().max_luminance,
                    1000.0
                );
            }
        }
        let frame = dec.render_next_frame(None).unwrap().unwrap();
        for row in 0..150 {
            assert_eq!(
                frame.pixels().row(row),
                expected_pixels.frames[0].pixels.as_slice().row(row)
            );
        }
    }
}

// Independently encoded SVT sample: 150x150 RGB(75,125,175), speed 6,
// default quality, 4:2:0. Unlike the flat timing fixture, this color gives
// distinct rendered pixels under BT.601 and BT.2020 matrices.
const COLOR_SAMPLE: &[u8] = &[
    10, 14, 2, 0, 0, 44, 29, 229, 101, 64, 39, 145, 1, 13, 6, 132, 50, 28, 16, 0, 136, 0, 6, 24,
    32, 130, 8, 36, 0, 173, 64, 89, 124, 214, 69, 97, 11, 21, 193, 8, 150, 63, 234, 231, 8, 96,
];

// The fixture's color-description fields start at bit 96. Make all three
// unspecified so differing item/track nclx values are permitted by AV1-ISOBMFF
// 2.3.4. Preserve every other bit, including the full-range flag.
fn unspecified_color_sample() -> Vec<u8> {
    let mut sample = COLOR_SAMPLE.to_vec();
    for (field, old) in [1u8, 13, 6].into_iter().enumerate() {
        for bit in 0..8 {
            let pos = 96 + field * 8 + bit;
            let shift = 7 - pos % 8;
            assert_eq!((sample[pos / 8] >> shift) & 1, (old >> (7 - bit)) & 1);
            sample[pos / 8] =
                (sample[pos / 8] & !(1 << shift)) | (((2u8 >> (7 - bit)) & 1) << shift);
        }
    }
    sample
}

#[test]
fn codec_animation_color_comes_from_track_not_poster() {
    let sample = unspecified_color_sample();
    let mut mux = AnimatedImage::new();
    mux.set_color_description(9, 16, 9, true);
    let original = mux
        .try_serialize(
            150,
            150,
            &[AnimFrame::new(&sample, 1).with_sync(true)],
            &sample[..16],
            None,
        )
        .unwrap();
    let colr: Vec<_> = original
        .windows(4)
        .enumerate()
        .filter_map(|(i, b)| (b == b"colr").then_some(i))
        .collect();
    assert_eq!(colr.len(), 2);
    let mut data = original.clone();
    assert_eq!(&data[colr[0] + 4..colr[0] + 8], b"nclx");
    data[colr[0] + 8..colr[0] + 14].copy_from_slice(&[0, 1, 0, 13, 0, 6]);
    let cfg = AvifDecoderConfig::new();
    let probe = cfg.clone().job().probe(&data).unwrap();
    let dec = cfg
        .job()
        .animation_frame_decoder(Cow::Borrowed(&data), &[])
        .unwrap();
    for info in [&probe, dec.info()] {
        let cicp = info.source_color.cicp.unwrap();
        assert_eq!(
            (cicp.color_primaries, cicp.transfer_characteristics),
            (9, 16)
        );
    }
    // This matrix choice changes actual pixels. Verify the witness first,
    // then require eager and both lazy backends to ignore the other matrix
    // signaled solely by the poster.
    let control = zenavif::decode_animation(&original).unwrap();
    let mut wrong = original.clone();
    for &pos in &colr {
        wrong[pos + 8..pos + 14].copy_from_slice(&[0, 1, 0, 13, 0, 6]);
    }
    let different = zenavif::decode_animation(&wrong).unwrap();
    assert!(
        (0..150).any(|row| control.frames[0].pixels.as_slice().row(row)
            != different.frames[0].pixels.as_slice().row(row))
    );
    let eager = zenavif::decode_animation(&data).unwrap();
    for row in 0..150 {
        assert_eq!(
            eager.frames[0].pixels.as_slice().row(row),
            control.frames[0].pixels.as_slice().row(row)
        );
    }
    for backend in [
        zenavif::DecodeBackend::Rav1dSafe,
        #[cfg(feature = "zenav1-aom")]
        zenavif::DecodeBackend::Zenav1Aom,
    ] {
        let mut cfg = AvifDecoderConfig::new();
        *cfg.inner_mut() = zenavif::DecoderConfig::new().decode_backend(backend);
        let mut dec = cfg
            .job()
            .animation_frame_decoder(Cow::Borrowed(&data), &[])
            .unwrap();
        let frame = dec.render_next_frame(None).unwrap().unwrap();
        for row in 0..150 {
            assert_eq!(
                frame.pixels().row(row),
                control.frames[0].pixels.as_slice().row(row)
            );
        }
    }
}

#[test]
fn animation_icc_absence_does_not_inherit_poster_profile() {
    let fixture = include_bytes!("vectors/zenavif/mono_gradient_8b_rgbicc.avif");
    let parser = zenavif_parse::AvifParser::from_bytes(fixture).unwrap();
    let Some(zenavif_parse::ColorInformation::IccProfile(icc)) = parser.color_info() else {
        panic!("ICC fixture lost profile")
    };
    let mut mux = AnimatedImage::new();
    mux.set_icc_profile(icc.clone());
    let original = mux
        .try_serialize(
            150,
            150,
            &[AnimFrame::new(SAMPLE, 1).with_sync(true)],
            &SAMPLE[2..18],
            None,
        )
        .unwrap();
    let prof: Vec<_> = original
        .windows(4)
        .enumerate()
        .filter_map(|(i, b)| (b == b"prof").then_some(i))
        .collect();
    assert_eq!(prof.len(), 2);
    for remove_track in [false, true] {
        let mut data = original.clone();
        let pos = prof[usize::from(remove_track)];
        assert_eq!(&data[pos - 4..pos], b"colr");
        data[pos - 4..pos].copy_from_slice(b"free");
        let parser = zenavif_parse::AvifParser::from_bytes(&data).unwrap();
        assert_eq!(
            matches!(
                parser.color_info(),
                Some(zenavif_parse::ColorInformation::IccProfile(_))
            ),
            remove_track
        );
        let cfg = AvifDecoderConfig::new();
        let probe = cfg.clone().job().probe(&data).unwrap();
        let dec = cfg
            .job()
            .animation_frame_decoder(Cow::Borrowed(&data), &[])
            .unwrap();
        for info in [&probe, dec.info()] {
            assert_eq!(
                info.source_color.icc_profile.as_deref(),
                if remove_track {
                    None
                } else {
                    Some(icc.as_slice())
                }
            );
        }
    }
}

#[cfg(feature = "zenav1-svt")]
#[test]
fn codec_animation_geometry_and_limits_use_track_not_poster() {
    use zenavif::{AnimationFrame, Av1Backend, EncodeBitDepth, EncoderConfig};
    for (pw, ph) in [(32usize, 34usize), (220, 230)] {
        let poster = zenavif::encode_animation_rgb8(
            &[AnimationFrame {
                pixels: imgref::Img::new(vec![rgb::Rgb::new(30, 60, 90); pw * ph], pw, ph),
                duration_ms: 1,
            }],
            &EncoderConfig::new()
                .backend(Av1Backend::Zenav1Svt)
                .chroma_subsampling(zenavif::EncodeChromaSubsampling::Yuv420)
                .bit_depth(EncodeBitDepth::Ten)
                .speed(6),
            almost_enough::StopToken::new(almost_enough::Unstoppable),
        )
        .unwrap();
        let pp = zenavif_parse::AvifParser::from_bytes(&poster.avif_file).unwrap();
        let payload = pp.primary_data().unwrap();
        let mut mux = AnimatedImage::new();
        let mut config = Av1CBox::default();
        config.seq_level_idx_0 = 0;
        mux.set_color_config(config)
            .set_color_description(1, 13, 6, true);
        let mut data = mux.serialize(
            150,
            150,
            &[AnimFrame::new(SAMPLE, 1).with_sync(true)],
            b"",
            None,
        );
        let locate =
            |bytes: &[u8], name: &[u8; 4]| bytes.windows(4).position(|b| b == name).unwrap();
        // Redirect only the poster's sole file extent into a new mdat. Track
        // offsets still point at the original 150x150 8-bit sample.
        let iloc = locate(&data, b"iloc");
        assert_eq!(&data[iloc + 4..iloc + 12], &[1, 0, 0, 0, 0x44, 0, 0, 1]);
        assert_eq!(&data[iloc + 12..iloc + 20], &[0, 1, 0, 0, 0, 0, 0, 1]);
        let start = u32::try_from(data.len() + 8).unwrap();
        data[iloc + 20..iloc + 24].copy_from_slice(&start.to_be_bytes());
        data[iloc + 24..iloc + 28].copy_from_slice(&(payload.len() as u32).to_be_bytes());
        data.extend_from_slice(&(payload.len() as u32 + 8).to_be_bytes());
        data.extend_from_slice(b"mdat");
        data.extend_from_slice(&payload);
        let ispe = locate(&data, b"ispe");
        data[ispe + 8..ispe + 12].copy_from_slice(&(pw as u32).to_be_bytes());
        data[ispe + 12..ispe + 16].copy_from_slice(&(ph as u32).to_be_bytes());
        let av1c = locate(&data, b"av1C");
        let pc = locate(&poster.avif_file, b"av1C");
        assert_eq!(
            u32::from_be_bytes(data[av1c - 4..av1c].try_into().unwrap()),
            12
        );
        data[av1c + 4..av1c + 8].copy_from_slice(&poster.avif_file[pc + 4..pc + 8]);
        let pixi = locate(&data, b"pixi");
        assert_eq!(&data[pixi + 8..pixi + 12], &[3, 8, 8, 8]);
        data[pixi + 9..pixi + 12].copy_from_slice(&[10, 10, 10]);
        let mut native =
            zenavif::ManagedAvifDecoder::new(&data, &zenavif::DecoderConfig::new()).unwrap();
        let (_, poster_info) = native.decode_full(&zenavif::Unstoppable).unwrap();
        assert_eq!(
            (poster_info.width, poster_info.height, poster_info.bit_depth),
            (pw as u32, ph as u32, 10)
        );
        let eager = zenavif::decode_animation_with(
            &data,
            &zenavif::DecoderConfig::new().frame_size_limit(150 * 150),
            &zenavif::Unstoppable,
        )
        .unwrap();
        assert_eq!(
            (
                eager.frames[0].pixels.width(),
                eager.frames[0].pixels.height()
            ),
            (150, 150)
        );
        assert!(
            zenavif::decode_animation_with(
                &data,
                &zenavif::DecoderConfig::new().frame_size_limit(150 * 150 - 1),
                &zenavif::Unstoppable
            )
            .is_err()
        );
        let cfg = AvifDecoderConfig::new();
        let probe = cfg.clone().job().probe(&data).unwrap();
        assert_eq!(
            (probe.width, probe.height, probe.source_color.bit_depth),
            (150, 150, Some(8))
        );
        let limits = zencodec::ResourceLimits::none()
            .with_max_pixels(150 * 150)
            .with_max_width(150)
            .with_max_height(150);
        let mut decoder = cfg
            .clone()
            .job()
            .with_limits(limits)
            .animation_frame_decoder(Cow::Borrowed(&data), &[])
            .unwrap();
        assert_eq!((decoder.info().width, decoder.info().height), (150, 150));
        let frame = decoder.render_next_frame(None).unwrap().unwrap();
        assert_eq!((frame.pixels().width(), frame.pixels().rows()), (150, 150));
        assert!(
            cfg.job()
                .with_limits(zencodec::ResourceLimits::none().with_max_width(149))
                .animation_frame_decoder(Cow::Borrowed(&data), &[])
                .is_err()
        );
    }
}
