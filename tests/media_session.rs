//! zenrav1e and rav1d-safe through the `zencodec-media` sessions: lossless frames decode
//! sample-exact through `Av1VideoDecoder`, timing survives, and the session
//! meets the encoder contract.

use std::sync::Arc;

use enough::Unstoppable;
use zenavif::media::{Av1EncodeSettings, Av1SessionError, Av1VideoDecoder, Av1VideoEncoder};
use zencodec::time::{TimeBase, Timestamp};
use zencodec_media::frame::{
    ChromaSiting, ColorClaim, Frame, FrameGeometry, FrameInterpretation, FrameTiming, OwnedPlane,
    OwnedPlanes, PlaneData, PlaneRole, Presentation, SampleEncoding, Subsampling,
};
use zencodec_media::packet::{Packet, RandomAccess};
use zencodec_media::session::CodecConfig;
use zencodec_media::session::{
    ProtocolViolation, Receive, SendStatus, SessionError, VideoDecoder, VideoEncoder,
};
use zencodec_media::track::CodecId;
use zencodec_media_testkit::{check_video_decoder_contract, check_video_encoder_contract};
use zenpixels::{ChannelType, Cicp};

const TB: TimeBase = TimeBase::MILLIS;

fn code(frame: usize, component: usize, x: usize, y: usize, bits: u8) -> u16 {
    let max = (1u32 << bits) - 1;
    (((17 * x + 29 * y + 43 * frame + 71 * component) as u32 * 997) % (max + 1)) as u16
}

fn dims(sub: Subsampling, w: u32, h: u32) -> Vec<(usize, usize)> {
    let (w, h) = (w as usize, h as usize);
    match sub {
        Subsampling::Mono => vec![(w, h)],
        Subsampling::Yuv420 => vec![
            (w, h),
            (w.div_ceil(2), h.div_ceil(2)),
            (w.div_ceil(2), h.div_ceil(2)),
        ],
        Subsampling::Yuv422 => vec![(w, h), (w.div_ceil(2), h), (w.div_ceil(2), h)],
        _ => vec![(w, h); 3],
    }
}

fn frame(i: usize, bits: u8, sub: Subsampling, w: u32, h: u32, pts: i64) -> Frame {
    frame_sited(i, bits, sub, ChromaSiting::Unknown, w, h, pts)
}

fn frame_sited(
    i: usize,
    bits: u8,
    sub: Subsampling,
    siting: ChromaSiting,
    w: u32,
    h: u32,
    pts: i64,
) -> Frame {
    frame_with(i, bits, sub, siting, 1, w, h, pts)
}

/// A frame shaped exactly as `s` encodes.
fn frame_for(s: &Av1EncodeSettings, i: usize, pts: i64) -> Frame {
    frame_with(
        i,
        s.bit_depth,
        s.subsampling,
        s.siting,
        s.color.matrix_coefficients,
        s.width,
        s.height,
        pts,
    )
}

/// The planes AV1 carries: G, B, R under the identity matrix.
fn roles(sub: Subsampling, matrix: u8) -> Vec<PlaneRole> {
    match (sub, matrix) {
        (Subsampling::Mono, _) => vec![PlaneRole::Luma],
        (_, 0) => vec![PlaneRole::Green, PlaneRole::Blue, PlaneRole::Red],
        _ => vec![PlaneRole::Luma, PlaneRole::Cb, PlaneRole::Cr],
    }
}

#[allow(clippy::too_many_arguments)]
fn frame_with(
    i: usize,
    bits: u8,
    sub: Subsampling,
    siting: ChromaSiting,
    matrix: u8,
    w: u32,
    h: u32,
    pts: i64,
) -> Frame {
    let planes: Vec<OwnedPlane> = dims(sub, w, h)
        .into_iter()
        .enumerate()
        .map(|(c, (pw, ph))| {
            let codes = (0..ph).flat_map(|y| (0..pw).map(move |x| code(i, c, x, y, bits)));
            if bits == 8 {
                OwnedPlane {
                    data: PlaneData::U8(codes.map(|v| v as u8).collect()),
                    width: pw,
                    height: ph,
                    stride_bytes: pw,
                }
            } else {
                OwnedPlane {
                    data: PlaneData::U16(codes.collect()),
                    width: pw,
                    height: ph,
                    stride_bytes: pw * 2,
                }
            }
        })
        .collect();
    let enc = if bits == 8 {
        SampleEncoding::U8
    } else {
        SampleEncoding::new(ChannelType::U16, bits, 0).unwrap()
    };
    let interp = FrameInterpretation::new(
        FrameGeometry::full(w, h),
        sub,
        siting,
        enc,
        roles(sub, matrix),
    );
    Frame::new(
        Arc::new(OwnedPlanes::new(planes, enc).unwrap()),
        Arc::new(interp),
        FrameTiming::at(Timestamp::new(pts, TB)).with_duration_ticks(40),
        Presentation::default(),
    )
}

fn encode(enc: &mut Av1VideoEncoder, frames: &[Frame]) -> Vec<Packet> {
    let mut out = Vec::new();
    for f in frames {
        loop {
            match enc.send(f, &Unstoppable).unwrap() {
                SendStatus::Accepted => break,
                _ => match enc.receive(&Unstoppable).unwrap() {
                    Receive::Ready(p) => out.push(p),
                    other => panic!("ReceivePending, then {other:?}"),
                },
            }
        }
        while let Receive::Ready(p) = enc.receive(&Unstoppable).unwrap() {
            out.push(p);
        }
    }
    enc.end_input().unwrap();
    loop {
        match enc.receive(&Unstoppable).unwrap() {
            Receive::Ready(p) => out.push(p),
            Receive::EndOfStream => break,
            other => panic!("drain: {other:?}"),
        }
    }
    out
}

fn decode(enc: &Av1VideoEncoder, packets: &[Packet]) -> Vec<Frame> {
    let mut dec = Av1VideoDecoder::new().unwrap();
    dec.configure(enc.output_config().unwrap(), &Unstoppable)
        .unwrap();
    let mut frames = Vec::new();
    for p in packets {
        let mut p = p.clone();
        loop {
            match dec.send(&mut p, &Unstoppable).unwrap() {
                SendStatus::Accepted => break,
                _ => match dec.receive(&Unstoppable).unwrap() {
                    Receive::Ready(f) => frames.push(f),
                    other => panic!("{other:?}"),
                },
            }
        }
        while let Receive::Ready(f) = dec.receive(&Unstoppable).unwrap() {
            frames.push(f);
        }
    }
    dec.end_input().unwrap();
    while let Receive::Ready(f) = dec.receive(&Unstoppable).unwrap() {
        frames.push(f);
    }
    frames
}

fn settings(bits: u8, sub: Subsampling, w: u32, h: u32) -> Av1EncodeSettings {
    Av1EncodeSettings::new(w, h)
        .with_bit_depth(bits)
        .with_subsampling(sub)
        .with_color(Cicp::new(
            1,
            13,
            if matches!(sub, Subsampling::Yuv444) {
                0
            } else {
                1
            },
            true,
        ))
        .with_quantizer(0)
        .with_speed(10)
        .with_key_frame_interval(4)
        .with_threads(1)
}

#[test]
fn lossless_frames_decode_sample_exact() {
    let cases = [
        (8, Subsampling::Yuv420, 33, 17),
        (8, Subsampling::Mono, 16, 16),
        (10, Subsampling::Yuv422, 40, 9),
        (10, Subsampling::Yuv444, 7, 13),
        (12, Subsampling::Yuv420, 24, 24),
        (12, Subsampling::Yuv444, 18, 30),
    ];
    for (bits, sub, w, h) in cases {
        let name = format!("{bits}-bit {sub:?} {w}x{h}");
        let pts: Vec<i64> = vec![0, 40, 80, 125, 160, 200, 240];
        let s = settings(bits, sub, w, h);
        let frames: Vec<Frame> = pts
            .iter()
            .enumerate()
            .map(|(i, &t)| frame_for(&s, i, t))
            .collect();
        let mut enc = Av1VideoEncoder::new(s.clone(), TB).unwrap();
        let cfg = enc.output_config().unwrap().clone();
        assert_eq!(
            cfg.private.as_deref().map(|p| p[0]),
            Some(0x81),
            "{name}: av1C marker"
        );
        let packets = encode(&mut enc, &frames);
        assert_eq!(packets.len(), frames.len(), "{name}");
        assert_eq!(packets[0].random_access(), RandomAccess::Yes, "{name}");
        let got_pts: Vec<i64> = packets.iter().map(|p| p.pts().unwrap().ticks()).collect();
        assert_eq!(
            got_pts, pts,
            "{name}: decode order equals presentation order at speed 10"
        );
        assert!(
            packets.iter().all(|p| p.duration_ticks() == Some(40)),
            "{name}"
        );
        let decoded = decode(&enc, &packets);
        assert_eq!(decoded.len(), frames.len(), "{name}");
        for (i, f) in decoded.iter().enumerate() {
            let it = f.interpretation();
            assert_eq!(it.subsampling, sub, "{name}");
            assert_eq!(it.encoding.code_bits(), bits, "{name}");
            assert_eq!(it.color.bitstream, Some(s.color), "{name}");
            // The planes come back as what they were: G, B, R under the
            // identity matrix.
            assert_eq!(it.roles, frames[i].interpretation().roles, "{name}");
            assert_eq!(f.timing().pts.unwrap().ticks(), pts[i], "{name}");
            let m = f.map().unwrap();
            for (c, (pw, ph)) in dims(sub, w, h).into_iter().enumerate() {
                let p = m.plane(c).unwrap();
                assert_eq!((p.width(), p.height()), (pw, ph), "{name}");
                for y in 0..ph {
                    for x in 0..pw {
                        assert_eq!(
                            p.code(x, y),
                            Some(code(i, c, x, y, bits)),
                            "{name} f{i} c{c} ({x},{y})"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn the_session_meets_the_encoder_contract() {
    for queue in [1, 3, 64] {
        let frames: Vec<Frame> = (0..9)
            .map(|i| frame(i, 8, Subsampling::Yuv420, 32, 32, i as i64 * 40))
            .collect();
        check_video_encoder_contract(
            || {
                Av1VideoEncoder::new(
                    Av1EncodeSettings::new(32, 32)
                        .with_speed(10)
                        .with_threads(1)
                        .with_max_queued_frames(queue),
                    TB,
                )
                .unwrap()
            },
            &frames,
        )
        .unwrap_or_else(|e| panic!("queue {queue}: {e:?}"));
    }
}

#[test]
fn the_default_speed_meets_the_contract_with_a_short_queue() {
    let frames: Vec<Frame> = (0..30)
        .map(|i| frame(i, 8, Subsampling::Yuv420, 32, 32, i as i64 * 40))
        .collect();
    check_video_encoder_contract(
        || {
            Av1VideoEncoder::new(
                Av1EncodeSettings::new(32, 32)
                    .with_threads(1)
                    .with_max_queued_frames(2),
                TB,
            )
            .unwrap()
        },
        &frames,
    )
    .unwrap();
}

#[test]
fn mismatched_input_is_refused() {
    let mut enc = Av1VideoEncoder::new(Av1EncodeSettings::new(32, 32), TB).unwrap();
    let wrong_size = frame(0, 8, Subsampling::Yuv420, 30, 32, 0);
    assert!(matches!(
        enc.send(&wrong_size, &Unstoppable),
        Err(SessionError::Codec(Av1SessionError::InputMismatch))
    ));
    let wrong_depth = frame(0, 10, Subsampling::Yuv420, 32, 32, 0);
    assert!(matches!(
        enc.send(&wrong_depth, &Unstoppable),
        Err(SessionError::Codec(Av1SessionError::InputMismatch))
    ));
    let wrong_sub = frame(0, 8, Subsampling::Yuv444, 32, 32, 0);
    assert!(matches!(
        enc.send(&wrong_sub, &Unstoppable),
        Err(SessionError::Codec(Av1SessionError::InputMismatch))
    ));
    let ok = frame(0, 8, Subsampling::Yuv420, 32, 32, 80);
    assert!(matches!(
        enc.send(&ok, &Unstoppable),
        Ok(SendStatus::Accepted)
    ));
    let earlier = frame(1, 8, Subsampling::Yuv420, 32, 32, 40);
    assert!(matches!(
        enc.send(&earlier, &Unstoppable),
        Err(SessionError::Codec(Av1SessionError::NonMonotonicTimestamp))
    ));
    let mut untimed = ok.timing();
    untimed.pts = None;
    let untimed = ok.represent(untimed, Presentation::default());
    assert!(matches!(
        enc.send(&untimed, &Unstoppable),
        Err(SessionError::Protocol(ProtocolViolation::MissingTimestamp))
    ));
    let same_time = frame(1, 8, Subsampling::Yuv420, 32, 32, 80);
    assert!(matches!(
        enc.send(&same_time, &Unstoppable),
        Err(SessionError::Codec(Av1SessionError::NonMonotonicTimestamp))
    ));
}

/// `ok` with its interpretation edited and new timing.
fn reinterpreted(ok: &Frame, pts: i64, edit: impl FnOnce(&mut FrameInterpretation)) -> Frame {
    let mut it = (**ok.interpretation()).clone();
    edit(&mut it);
    Frame::new(
        Arc::clone(ok.picture()),
        Arc::new(it),
        FrameTiming::at(Timestamp::new(pts, TB)),
        Presentation::default(),
    )
}

fn refused(enc: &mut Av1VideoEncoder, f: &Frame) -> bool {
    matches!(
        enc.send(f, &Unstoppable),
        Err(SessionError::Codec(Av1SessionError::InputMismatch))
    )
}

/// Colour claims, plane roles and the picture itself must agree with the
/// settings: none of these is converted, so each is refused.
#[test]
fn frames_that_would_need_conversion_are_refused() {
    let s = Av1EncodeSettings::new(32, 32);
    let mut enc = Av1VideoEncoder::new(s.clone(), TB).unwrap();
    let ok = frame_for(&s, 0, 0);
    // A full-range claim against limited-range settings.
    let full = reinterpreted(&ok, 40, |it| {
        it.color = ColorClaim::new(None, None, Some(Cicp::new(2, 2, 2, true)));
    });
    assert!(refused(&mut enc, &full), "range");
    // A BT.709 claim against unspecified settings.
    let bt709 = reinterpreted(&ok, 40, |it| {
        it.color = ColorClaim::new(None, None, Some(Cicp::new(1, 1, 1, false)));
    });
    assert!(refused(&mut enc, &bt709), "primaries and matrix");
    // Cb and Cr swapped; G, B, R under a YUV matrix; an alpha plane.
    for roles in [
        vec![PlaneRole::Luma, PlaneRole::Cr, PlaneRole::Cb],
        vec![PlaneRole::Green, PlaneRole::Blue, PlaneRole::Red],
        vec![
            PlaneRole::Luma,
            PlaneRole::Cb,
            PlaneRole::Cr,
            PlaneRole::Alpha,
        ],
    ] {
        let f = reinterpreted(&ok, 40, |it| it.roles = roles.clone());
        assert!(refused(&mut enc, &f), "{roles:?}");
    }
    // The settings' own claim is accepted, as is an unknown one.
    let same = reinterpreted(&ok, 40, |it| {
        it.color = ColorClaim::new(None, None, Some(Cicp::new(2, 2, 2, false)));
    });
    assert!(matches!(
        enc.send(&same, &Unstoppable),
        Ok(SendStatus::Accepted)
    ));
    assert!(matches!(
        enc.send(&frame_for(&s, 1, 80), &Unstoppable),
        Ok(SendStatus::Accepted)
    ));
    // Pictures that do not hold what the interpretation says: one plane,
    // short chroma, a different sample encoding.
    let interp = ok.interpretation();
    let luma = OwnedPlane {
        data: PlaneData::U8(vec![128; 32 * 32]),
        width: 32,
        height: 32,
        stride_bytes: 32,
    };
    let chroma = |w: usize, h: usize| OwnedPlane {
        data: PlaneData::U8(vec![128; w * h]),
        width: w,
        height: h,
        stride_bytes: w,
    };
    let pictures = [
        OwnedPlanes::new(vec![luma.clone()], SampleEncoding::U8).unwrap(),
        OwnedPlanes::new(
            vec![luma.clone(), chroma(16, 15), chroma(16, 15)],
            SampleEncoding::U8,
        )
        .unwrap(),
        OwnedPlanes::new(
            vec![luma.clone(), chroma(17, 16), chroma(17, 16)],
            SampleEncoding::U8,
        )
        .unwrap(),
    ];
    for (n, picture) in pictures.into_iter().enumerate() {
        let f = Frame::new(
            Arc::new(picture),
            Arc::clone(interp),
            FrameTiming::at(Timestamp::new(120 + n as i64, TB)),
            Presentation::default(),
        );
        assert!(refused(&mut enc, &f), "picture {n}");
    }
    // A 10-bit interpretation over MSB-aligned words.
    let s10 = Av1EncodeSettings::new(32, 32).with_bit_depth(10);
    let mut enc10 = Av1VideoEncoder::new(s10.clone(), TB).unwrap();
    let ok10 = frame_for(&s10, 0, 0);
    let msb = SampleEncoding::new(ChannelType::U16, 10, 6).unwrap();
    let word = |w: usize, h: usize| OwnedPlane {
        data: PlaneData::U16(vec![512 << 6; w * h]),
        width: w,
        height: h,
        stride_bytes: w * 2,
    };
    let shifted = Frame::new(
        Arc::new(OwnedPlanes::new(vec![word(32, 32), word(16, 16), word(16, 16)], msb).unwrap()),
        Arc::clone(ok10.interpretation()),
        FrameTiming::at(Timestamp::new(0, TB)),
        Presentation::default(),
    );
    assert!(refused(&mut enc10, &shifted), "bit shift");
    assert!(matches!(
        enc10.send(&ok10, &Unstoppable),
        Ok(SendStatus::Accepted)
    ));
}

/// The tick clock is not a frame rate: the same frames on a millisecond and
/// a 90 kHz clock give the same bitstream.
#[test]
fn the_clock_does_not_change_the_bitstream() {
    let s = Av1EncodeSettings::new(96, 64)
        .with_speed(10)
        .with_threads(1);
    let khz90 = TimeBase::new(1, 90_000).unwrap();
    let run = |clock: TimeBase, step: i64| -> Vec<Vec<u8>> {
        let mut enc = Av1VideoEncoder::new(s.clone(), clock).unwrap();
        let frames: Vec<Frame> = (0..6)
            .map(|i| {
                let f = frame_for(&s, i, 0);
                let mut t = f.timing();
                t.pts = Some(Timestamp::new(i as i64 * step, clock));
                f.represent(t, Presentation::default())
            })
            .collect();
        encode(&mut enc, &frames)
            .iter()
            .map(|p| p.bytes().to_vec())
            .collect()
    };
    assert_eq!(run(TB, 40), run(khz90, 3600));
}

#[test]
fn impossible_settings_are_refused() {
    let bad = |s| Av1VideoEncoder::new(s, TB).unwrap_err();
    assert!(matches!(
        bad(Av1EncodeSettings::new(16, 16).with_bit_depth(9)),
        Av1SessionError::UnsupportedBitDepth(9)
    ));
    assert!(matches!(
        bad(Av1EncodeSettings::new(0, 16)),
        Av1SessionError::Configuration(_)
    ));
    assert!(matches!(
        bad(Av1EncodeSettings::new(16, 16).with_color(Cicp::new(1, 13, 0, true))),
        Av1SessionError::Configuration(_)
    ));
    assert!(matches!(
        bad(Av1EncodeSettings::new(16, 16)
            .with_subsampling(Subsampling::Yuv444)
            .with_siting(ChromaSiting::Left)),
        Av1SessionError::Configuration(_)
    ));
}

#[test]
fn lossy_stream_decodes_in_presentation_order() {
    let pts: Vec<i64> = (0..30).map(|i| i * 40).collect();
    let frames: Vec<Frame> = pts
        .iter()
        .enumerate()
        .map(|(i, &t)| frame(i, 10, Subsampling::Yuv420, 64, 48, t))
        .collect();
    let mut enc = Av1VideoEncoder::new(
        Av1EncodeSettings::new(64, 48)
            .with_bit_depth(10)
            .with_key_frame_interval(12)
            .with_threads(2),
        TB,
    )
    .unwrap();
    let packets = encode(&mut enc, &frames);
    assert_eq!(packets.len(), 30);
    let keys = packets
        .iter()
        .filter(|p| p.random_access() == RandomAccess::Yes)
        .count();
    assert!(
        keys >= 3,
        "a key frame at least every 12 frames, got {keys}"
    );
    let decoded = decode(&enc, &packets);
    let shown: Vec<i64> = decoded
        .iter()
        .map(|f| f.timing().pts.unwrap().ticks())
        .collect();
    assert_eq!(shown, pts);
}

#[test]
fn the_decoder_meets_its_contract_on_encoded_streams() {
    for (bits, sub) in [
        (8, Subsampling::Yuv420),
        (10, Subsampling::Yuv444),
        (12, Subsampling::Mono),
    ] {
        let s = settings(bits, sub, 24, 16).with_quantizer(80);
        let frames: Vec<Frame> = (0..8).map(|i| frame_for(&s, i, i as i64 * 40)).collect();
        let mut enc = Av1VideoEncoder::new(s, TB).unwrap();
        let config: CodecConfig = enc.output_config().unwrap().clone();
        let packets = encode(&mut enc, &frames);
        check_video_decoder_contract(|| Av1VideoDecoder::new().unwrap(), &config, &packets)
            .unwrap_or_else(|e| panic!("{bits}-bit {sub:?}: {e:?}"));
    }
}

fn planes(f: &Frame) -> Vec<Vec<Option<u16>>> {
    let m = f.map().unwrap();
    (0..m.plane_count())
        .map(|c| {
            let p = m.plane(c).unwrap();
            (0..p.height())
                .flat_map(|y| (0..p.width()).map(move |x| (x, y)))
                .map(|(x, y)| p.code(x, y))
                .collect()
        })
        .collect()
}

#[test]
fn reset_decodes_the_same_frames_again_and_old_frames_stay_valid() {
    let frames: Vec<Frame> = (0..6)
        .map(|i| frame(i, 10, Subsampling::Yuv420, 17, 13, i as i64 * 40))
        .collect();
    let mut enc = Av1VideoEncoder::new(
        settings(10, Subsampling::Yuv420, 17, 13).with_quantizer(90),
        TB,
    )
    .unwrap();
    let config = enc.output_config().unwrap().clone();
    let packets = encode(&mut enc, &frames);
    let mut dec = Av1VideoDecoder::new().unwrap();
    let first = decode_with(&mut dec, &config, &packets);
    let before: Vec<_> = first.iter().map(planes).collect();
    dec.reset().unwrap();
    let again = decode_with(&mut dec, &config, &packets);
    assert_eq!(before, again.iter().map(planes).collect::<Vec<_>>());
    drop(dec);
    assert_eq!(before, first.iter().map(planes).collect::<Vec<_>>());
}

#[test]
fn packet_context_keeps_the_configuration_at_acceptance() {
    use zencodec_media::track::VideoTrackInfo;

    let s = settings(8, Subsampling::Yuv420, 17, 13);
    let input = frame_for(&s, 0, 40);
    let mut enc = Av1VideoEncoder::new(s.clone(), TB).unwrap();
    let old_color = s.color;
    let config = enc
        .output_config()
        .unwrap()
        .clone()
        .with_video(VideoTrackInfo::new(s.width, s.height).with_color(old_color));
    let packets = encode(&mut enc, std::slice::from_ref(&input));
    assert_eq!(packets.len(), 1);
    for threads in [1, 2] {
        let mut native = rav1d_safe::Settings::default();
        native.threads = threads;
        let mut dec = Av1VideoDecoder::with_settings(native).unwrap();
        dec.configure(&config, &Unstoppable).unwrap();
        let mut packet = packets[0].clone().with_ordinal(7).with_input_offset(1234);
        assert_eq!(
            dec.send(&mut packet, &Unstoppable).unwrap(),
            SendStatus::Accepted
        );
        assert!(packet.is_taken());

        // Change the clock and container evidence while the old picture is
        // still owned by the decoder. This must not reset reference pictures.
        let mut next = config.clone().with_epoch(1);
        next.time_base = TimeBase::hz(90_000).unwrap();
        next.video =
            Some(VideoTrackInfo::new(s.width, s.height).with_color(Cicp::new(9, 16, 9, true)));
        dec.configure(&next, &Unstoppable).unwrap();
        dec.end_input().unwrap();
        let Receive::Ready(output) = dec.receive(&Unstoppable).unwrap() else {
            panic!("queued frame was discarded during reconfiguration");
        };
        assert_eq!(output.timing().pts, Some(Timestamp::new(40, TB)));
        assert_eq!(output.interpretation().config_epoch, 0);
        assert_eq!(output.interpretation().color.container, Some(old_color));
        assert_eq!(output.timing().packet_ordinal, Some(7));
        assert_eq!(output.timing().input_offset, Some(1234));
        assert_eq!(planes(&output), planes(&input));
        assert!(matches!(
            dec.receive(&Unstoppable).unwrap(),
            Receive::EndOfStream
        ));
        dec.reset().unwrap();
        drop(dec);
        assert_eq!(planes(&output), planes(&input));
        assert_eq!(output.timing().pts, Some(Timestamp::new(40, TB)));
    }
}

#[test]
fn packet_context_keeps_offsets_for_every_presentation_in_one_packet() {
    let s = settings(8, Subsampling::Yuv420, 17, 13);
    let inputs: Vec<_> = (0..3).map(|i| frame_for(&s, i, i as i64 * 40)).collect();
    let mut enc = Av1VideoEncoder::new(s, TB).unwrap();
    let packets = encode(&mut enc, &inputs);
    let bytes = packets.iter().flat_map(|p| p.bytes()).copied().collect();
    let packet = Packet::from_vec(bytes)
        .unwrap()
        .with_ordinal(7)
        .with_input_offset(1234);
    let outputs = decode(&enc, &[packet]);
    assert_eq!(outputs.len(), inputs.len());
    for (output, input) in outputs.iter().zip(&inputs) {
        assert_eq!(planes(output), planes(input));
        assert_eq!(output.timing().packet_ordinal, Some(7));
        assert_eq!(output.timing().input_offset, Some(1234));
    }
}

#[test]
fn packet_context_distinguishes_queued_epochs_with_repeated_ordinals() {
    use zencodec_media::track::VideoTrackInfo;

    let s = settings(10, Subsampling::Yuv420, 17, 13).with_color(Cicp::new(2, 2, 2, true));
    let inputs: Vec<_> = (0..7).map(|i| frame_for(&s, i, i as i64 * 40)).collect();
    let mut enc = Av1VideoEncoder::new(s.clone(), TB).unwrap();
    let packets = encode(&mut enc, &inputs);
    assert_eq!(packets.len(), inputs.len());
    let clocks = [TB, TimeBase::hz(90_000).unwrap()];
    let colors = [Cicp::new(1, 13, 1, true), Cicp::new(9, 16, 9, true)];
    for threads in [1, 4] {
        let mut native = rav1d_safe::Settings::default();
        native.threads = threads;
        native.max_frame_delay = 4;
        let mut dec = Av1VideoDecoder::with_settings(native).unwrap();
        let mut outputs = Vec::new();
        for (i, packet) in packets.iter().enumerate() {
            let mut config = enc.output_config().unwrap().clone().with_epoch(i as u32);
            config.time_base = clocks[i % 2];
            config.video = Some(VideoTrackInfo::new(s.width, s.height).with_color(colors[i % 2]));
            dec.configure(&config, &Unstoppable).unwrap();
            let mut packet = packet
                .clone()
                .with_pts(Timestamp::new(i as i64 * 40, config.time_base))
                .with_config_epoch(config.epoch)
                .with_ordinal(7)
                .with_input_offset(1000 + i as u64);
            while dec.send(&mut packet, &Unstoppable).unwrap() != SendStatus::Accepted {
                let Receive::Ready(frame) = dec.receive(&Unstoppable).unwrap() else {
                    panic!("backpressure without output");
                };
                outputs.push(frame);
            }
        }
        let mut unused = enc.output_config().unwrap().clone().with_epoch(99);
        unused.time_base = TimeBase::SECONDS;
        dec.configure(&unused, &Unstoppable).unwrap();
        dec.end_input().unwrap();
        loop {
            match dec.receive(&Unstoppable).unwrap() {
                Receive::Ready(frame) => outputs.push(frame),
                Receive::EndOfStream => break,
                other => panic!("ended session failed to drain: {other:?}"),
            }
        }
        assert_eq!(outputs.len(), inputs.len());
        dec.reset().unwrap();
        drop(dec);
        for (i, (output, input)) in outputs.iter().zip(&inputs).enumerate() {
            assert_eq!(planes(output), planes(input));
            assert_eq!(output.interpretation().config_epoch, i as u32);
            assert_eq!(output.interpretation().color.bitstream, Some(s.color));
            assert_eq!(output.interpretation().color.container, Some(colors[i % 2]));
            assert_eq!(output.interpretation().color.resolved, Some(colors[i % 2]));
            assert_eq!(
                output.timing().pts,
                Some(Timestamp::new(i as i64 * 40, clocks[i % 2]))
            );
            assert_eq!(output.timing().packet_ordinal, Some(7));
            assert_eq!(output.timing().input_offset, Some(1000 + i as u64));
        }
    }
}

#[test]
fn native_decode_error_keeps_its_type_and_whereat_trace() {
    use std::error::Error;

    fn native<'a>(mut current: &'a (dyn Error + 'static)) -> &'a whereat::At<rav1d_safe::Error> {
        loop {
            if let Some(native) = current.downcast_ref::<whereat::At<rav1d_safe::Error>>() {
                assert_eq!(native.error(), &rav1d_safe::Error::InvalidData);
                assert!(native.frame_count() > 0);
                return native;
            }
            current = current
                .source()
                .expect("native cause was erased by the media adapter");
        }
    }

    let mut dec = Av1VideoDecoder::new().unwrap();
    dec.configure(&CodecConfig::new(CodecId::Av1, TB), &Unstoppable)
        .unwrap();
    let mut malformed = Packet::from_vec(vec![0xff]).unwrap();
    let error = dec.send(&mut malformed, &Unstoppable).unwrap_err();
    let locations = native(&error).frame_count();
    // Cloning the public error must not discard the trace either.
    let SessionError::Codec(codec) = error else {
        panic!("expected native codec failure")
    };
    let cloned = codec.clone();
    assert_eq!(native(&cloned).frame_count(), locations);

    let mut erased = zencodec_media::session::erase_video_decoder(Av1VideoDecoder::new().unwrap());
    erased
        .configure(&CodecConfig::new(CodecId::Av1, TB), &Unstoppable)
        .unwrap();
    let mut malformed = Packet::from_vec(vec![0xff]).unwrap();
    let erased_error = erased.send(&mut malformed, &Unstoppable).unwrap_err();
    native(&erased_error);
}

#[test]
fn native_picture_limit_refuses_before_a_media_frame_and_keeps_trace() {
    use std::error::Error;
    let settings = settings(10, Subsampling::Yuv420, 17, 13);
    let input = frame_for(&settings, 0, 0);
    let mut encoder = Av1VideoEncoder::new(settings, TB).unwrap();
    let packets = encode(&mut encoder, &[input]);
    for threads in [1, 4] {
        let mut native = rav1d_safe::Settings::default();
        native.threads = threads;
        native.max_picture_bytes = Some(0);
        let mut decoder = Av1VideoDecoder::with_settings(native).unwrap();
        decoder
            .configure(encoder.output_config().unwrap(), &Unstoppable)
            .unwrap();
        let mut failure = None;
        for mut packet in packets.iter().cloned() {
            if let Err(error) = decoder.send(&mut packet, &Unstoppable) {
                failure = Some(error);
                break;
            }
            match decoder.receive(&Unstoppable) {
                Err(error) => {
                    failure = Some(error);
                    break;
                }
                Ok(Receive::Ready(_)) => panic!("frame published before native admission"),
                Ok(_) => {}
            }
        }
        let error = failure.unwrap_or_else(|| {
            decoder.end_input().unwrap();
            match decoder.receive(&Unstoppable) {
                Err(error) => error,
                Ok(_) => panic!("native picture admission did not refuse"),
            }
        });
        let mut cause: &(dyn Error + 'static) = &error;
        loop {
            if let Some(native) = cause.downcast_ref::<whereat::At<rav1d_safe::Error>>() {
                assert!(
                    matches!(native.error(), rav1d_safe::Error::Other(message) if message == "native picture storage limit exceeded")
                );
                assert!(native.frame_count() > 0);
                break;
            }
            cause = cause.source().expect("native budget trace was erased");
        }
    }
}

fn decode_with(dec: &mut Av1VideoDecoder, config: &CodecConfig, packets: &[Packet]) -> Vec<Frame> {
    dec.configure(config, &Unstoppable).unwrap();
    let mut frames = Vec::new();
    for p in packets {
        let mut p = p.clone();
        while dec.send(&mut p, &Unstoppable).unwrap() != SendStatus::Accepted {
            match dec.receive(&Unstoppable).unwrap() {
                Receive::Ready(f) => frames.push(f),
                other => panic!("{other:?}"),
            }
        }
        while let Receive::Ready(f) = dec.receive(&Unstoppable).unwrap() {
            frames.push(f);
        }
    }
    dec.end_input().unwrap();
    while let Receive::Ready(f) = dec.receive(&Unstoppable).unwrap() {
        frames.push(f);
    }
    frames
}

/// Split an AV1 temporal unit into OBUs: (type, bytes of the whole OBU).
fn obus(mut b: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut out = Vec::new();
    while !b.is_empty() {
        let header = b[0];
        let kind = (header >> 3) & 0xF;
        let ext = header & 0x4 != 0;
        assert!(header & 0x2 != 0, "zenrav1e OBUs carry sizes");
        let mut i = 1 + usize::from(ext);
        let (mut size, mut shift) = (0usize, 0);
        loop {
            let byte = b[i];
            i += 1;
            size |= usize::from(byte & 0x7F) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
        }
        out.push((kind, b[..i + size].to_vec()));
        b = &b[i + size..];
    }
    out
}

/// An `av1C` whose `configOBUs` carry the sequence header lets the decoder
/// start on packets stripped of theirs, after `configure` and after `reset`.
#[test]
fn av1c_config_obus_restore_stripped_sequence_headers() {
    const SEQUENCE_HEADER: u8 = 1;
    let frames: Vec<Frame> = (0..6)
        .map(|i| frame(i, 8, Subsampling::Yuv420, 32, 24, i as i64 * 40))
        .collect();
    let mut enc = Av1VideoEncoder::new(settings(8, Subsampling::Yuv420, 32, 24), TB).unwrap();
    let record = enc.output_config().unwrap().clone();
    assert_eq!(
        record.private.as_deref().map(<[u8]>::len),
        Some(4),
        "no configOBUs"
    );
    let packets = encode(&mut enc, &frames);
    let mut seq = None;
    let stripped: Vec<Packet> = packets
        .iter()
        .map(|p| {
            let kept: Vec<u8> = obus(p.bytes())
                .into_iter()
                .filter(|(kind, bytes)| {
                    if *kind == SEQUENCE_HEADER {
                        seq.get_or_insert_with(|| bytes.clone());
                        false
                    } else {
                        true
                    }
                })
                .flat_map(|(_, bytes)| bytes)
                .collect();
            Packet::from_vec(kept).unwrap().with_pts(p.pts().unwrap())
        })
        .collect();
    let seq = seq.expect("zenrav1e writes sequence headers in key frames");
    let mut av1c = record.private.as_deref().unwrap().to_vec();
    av1c.extend_from_slice(&seq);
    let config = CodecConfig::new(CodecId::Av1, TB).with_private(av1c.into());
    let mut dec = Av1VideoDecoder::new().unwrap();
    let want: Vec<_> = decode_with(&mut dec, &record, &packets)
        .iter()
        .map(planes)
        .collect();
    dec.reset().unwrap();
    let got = decode_with(&mut dec, &config, &stripped);
    assert_eq!(want, got.iter().map(planes).collect::<Vec<_>>());
    dec.reset().unwrap();
    assert_eq!(
        decode_with(&mut dec, &config, &stripped).len(),
        frames.len(),
        "after reset"
    );
    // Without the record the stripped stream does not decode.
    let mut dec = Av1VideoDecoder::new().unwrap();
    dec.configure(&record, &Unstoppable).unwrap();
    let mut first = stripped[0].clone();
    let sent = dec.send(&mut first, &Unstoppable);
    let got = sent.is_ok() && matches!(dec.receive(&Unstoppable), Ok(Receive::Ready(_)));
    assert!(!got, "a stream without its sequence header must not decode");
}

/// MSB-first bit reader over an OBU payload.
struct Bits<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> u8 {
        let v = (self.b[self.pos / 8] >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        v
    }
    fn f(&mut self, n: usize) -> u32 {
        (0..n).fold(0, |acc, _| acc << 1 | u32::from(self.bit()))
    }
}

/// The `av1C`-relevant fields of an AV1 sequence header OBU (spec 5.5),
/// read independently of the encoder: (profile, level, tier, high_bitdepth,
/// twelve_bit, monochrome, subsampling_x, subsampling_y,
/// chroma_sample_position).
fn sequence_header_fields(obu: &[u8]) -> [u32; 9] {
    // OBU header and leb128 size.
    let mut i = 1 + usize::from(obu[0] & 0x4 != 0);
    while obu[i] & 0x80 != 0 {
        i += 1;
    }
    let mut r = Bits {
        b: &obu[i + 1..],
        pos: 0,
    };
    let profile = r.f(3);
    let _still = r.f(1);
    let reduced = r.f(1);
    let mut tier = 0;
    let level = if reduced == 1 {
        r.f(5)
    } else {
        assert_eq!(r.f(1), 0, "timing_info_present is off");
        let initial_display_delay = r.f(1);
        let ops = r.f(5) + 1;
        let mut first_level = 0;
        for op in 0..ops {
            let _idc = r.f(12);
            let lvl = r.f(5);
            let t = if lvl > 7 { r.f(1) } else { 0 };
            if initial_display_delay == 1 && r.f(1) == 1 {
                r.f(4);
            }
            if op == 0 {
                first_level = lvl;
                tier = t;
            }
        }
        first_level
    };
    let wb = r.f(4) as usize + 1;
    let hb = r.f(4) as usize + 1;
    r.f(wb);
    r.f(hb);
    if reduced == 0 && r.f(1) == 1 {
        r.f(7); // frame id lengths
    }
    r.f(3); // 128x128, filter intra, intra edge
    if reduced == 0 {
        r.f(4); // interintra, masked, warped, dual filter
        let order_hint = r.f(1);
        if order_hint == 1 {
            r.f(2); // jnt comp, ref frame mvs
        }
        let force_sct = if r.f(1) == 1 { 2 } else { r.f(1) };
        if force_sct > 0 && r.f(1) == 0 {
            r.f(1);
        }
        if order_hint == 1 {
            r.f(3);
        }
    }
    r.f(3); // superres, cdef, restoration
    let high = r.f(1);
    let twelve = if profile == 2 && high == 1 { r.f(1) } else { 0 };
    let mono = if profile == 1 { 0 } else { r.f(1) };
    let (mut cp, mut tc, mut mc) = (2, 2, 2);
    if r.f(1) == 1 {
        cp = r.f(8);
        tc = r.f(8);
        mc = r.f(8);
    }
    let (sx, sy, mut csp) = if mono == 1 {
        r.f(1);
        (1, 1, 0)
    } else if (cp, tc, mc) == (1, 13, 0) {
        (0, 0, 0)
    } else {
        r.f(1); // colour range
        match profile {
            0 => (1, 1, 0),
            1 => (0, 0, 0),
            _ if twelve == 1 => {
                let sx = r.f(1);
                let sy = if sx == 1 { r.f(1) } else { 0 };
                (sx, sy, 0)
            }
            _ => (1, 0, 0),
        }
    };
    if mono == 0 && sx == 1 && sy == 1 {
        csp = r.f(2);
    }
    [profile, level, tier, high, twelve, mono, sx, sy, csp]
}

/// The `av1C` record agrees with the in-band sequence header on every field
/// ISOBMFF requires (profile, level, tier, depth, monochrome, subsampling,
/// chroma position), the header signals what the settings ask for, and the
/// decoder reports the same siting.
#[test]
fn av1c_matches_the_sequence_header() {
    const SEQUENCE_HEADER: u8 = 1;
    for (bits, sub, siting, matrix) in [
        (8, Subsampling::Yuv420, ChromaSiting::Unknown, 1),
        (8, Subsampling::Yuv420, ChromaSiting::Left, 1),
        (10, Subsampling::Yuv420, ChromaSiting::TopLeft, 1),
        (12, Subsampling::Yuv422, ChromaSiting::Unknown, 1),
        (12, Subsampling::Yuv420, ChromaSiting::Left, 1),
        (8, Subsampling::Mono, ChromaSiting::Unknown, 1),
        (10, Subsampling::Yuv444, ChromaSiting::Unknown, 0),
        (8, Subsampling::Yuv444, ChromaSiting::Unknown, 1),
    ] {
        let name = format!("{bits}-bit {sub:?} {siting:?} mc={matrix}");
        let s = settings(bits, sub, 16, 16)
            .with_siting(siting)
            .with_color(Cicp::new(1, 13, matrix, true));
        let mut enc = Av1VideoEncoder::new(s.clone(), TB).unwrap();
        let record = enc.output_config().unwrap().clone();
        let b = record.private.as_deref().unwrap().to_vec();
        assert_eq!(b.len(), 4, "{name}: no configOBUs");
        assert_eq!(b[0], 0x81, "{name}: marker and version");
        let av1c = [
            u32::from(b[1] >> 5),
            u32::from(b[1] & 0x1F),
            u32::from(b[2] >> 7),
            u32::from(b[2] >> 6 & 1),
            u32::from(b[2] >> 5 & 1),
            u32::from(b[2] >> 4 & 1),
            u32::from(b[2] >> 3 & 1),
            u32::from(b[2] >> 2 & 1),
            u32::from(b[2] & 3),
        ];
        let packets = encode(&mut enc, &[frame_for(&s, 0, 0)]);
        let seq = obus(packets[0].bytes())
            .into_iter()
            .find(|(kind, _)| *kind == SEQUENCE_HEADER)
            .map(|(_, bytes)| bytes)
            .expect("a key frame carries the sequence header");
        let header = sequence_header_fields(&seq);
        assert_eq!(av1c, header, "{name}: av1C vs sequence header");
        let (sx, sy) = match sub {
            Subsampling::Yuv444 => (0, 0),
            Subsampling::Yuv422 => (1, 0),
            _ => (1, 1),
        };
        let csp = match (sub, siting) {
            (Subsampling::Yuv420, ChromaSiting::Left) => 1,
            (Subsampling::Yuv420, ChromaSiting::TopLeft) => 2,
            _ => 0,
        };
        let want_profile = match (sub, bits) {
            (_, 12) => 2,
            (Subsampling::Yuv422, _) => 2,
            (Subsampling::Yuv444, _) => 1,
            _ => 0,
        };
        assert_eq!(header[0], want_profile, "{name}: profile");
        assert_eq!(
            header[3..],
            [
                u32::from(bits > 8),
                u32::from(bits == 12),
                u32::from(matches!(sub, Subsampling::Mono)),
                sx,
                sy,
                csp
            ],
            "{name}: signalled format"
        );
        let mut dec = Av1VideoDecoder::new().unwrap();
        let out = decode_with(&mut dec, &record, &packets);
        assert_eq!(
            out[0].interpretation().siting,
            siting,
            "{name}: decoded siting"
        );
    }
}

/// zenav1-svt behind the same session.
#[cfg(feature = "zenav1-svt")]
mod svt {
    use super::*;
    use zenavif::media::Av1VideoBackend;

    /// A smooth moving gradient: content a lossy encode keeps close.
    fn smooth(s: &Av1EncodeSettings, i: usize, pts: i64) -> Frame {
        let max = (1u32 << s.bit_depth) - 1;
        let planes: Vec<OwnedPlane> = dims(s.subsampling, s.width, s.height)
            .into_iter()
            .enumerate()
            .map(|(c, (pw, ph))| {
                let v = move |x: usize, y: usize| {
                    let t = (x * 3 + y * 2 + i * 4 + c * 40) as u32 % 200 + 20;
                    (t * max / 255) as u16
                };
                let codes = (0..ph).flat_map(move |y| (0..pw).map(move |x| v(x, y)));
                if s.bit_depth == 8 {
                    OwnedPlane {
                        data: PlaneData::U8(codes.map(|v| v as u8).collect()),
                        width: pw,
                        height: ph,
                        stride_bytes: pw,
                    }
                } else {
                    OwnedPlane {
                        data: PlaneData::U16(codes.collect()),
                        width: pw,
                        height: ph,
                        stride_bytes: pw * 2,
                    }
                }
            })
            .collect();
        let template = frame_for(s, 0, pts);
        Frame::new(
            Arc::new(OwnedPlanes::new(planes, template.interpretation().encoding).unwrap()),
            Arc::clone(template.interpretation()),
            FrameTiming::at(Timestamp::new(pts, TB)).with_duration_ticks(40),
            Presentation::default(),
        )
    }

    fn psnr(a: &Frame, b: &Frame, bits: u8) -> f64 {
        let (ma, mb) = (a.map().unwrap(), b.map().unwrap());
        let mut worst = f64::INFINITY;
        for c in 0..ma.plane_count() {
            let (pa, pb) = (ma.plane(c).unwrap(), mb.plane(c).unwrap());
            let mut se = 0f64;
            for y in 0..pa.height() {
                for x in 0..pa.width() {
                    let d = f64::from(pa.code(x, y).unwrap()) - f64::from(pb.code(x, y).unwrap());
                    se += d * d;
                }
            }
            let mse = se / (pa.width() * pa.height()) as f64;
            let peak = f64::from((1u32 << bits) - 1);
            worst = worst.min(10.0 * (peak * peak / mse.max(1e-9)).log10());
        }
        worst
    }

    fn svt(bits: u8, sub: Subsampling, kfi: u64) -> Av1EncodeSettings {
        Av1EncodeSettings::new(48, 32)
            .with_backend(Av1VideoBackend::Zenav1Svt)
            .with_bit_depth(bits)
            .with_subsampling(sub)
            .with_color(Cicp::new(1, 1, 1, false))
            .with_quantizer(80)
            .with_speed(10)
            .with_key_frame_interval(kfi)
            .with_threads(1)
    }

    #[test]
    fn svt_streams_decode_with_their_timing_and_key_frames() {
        for (bits, sub, kfi) in [
            (8, Subsampling::Yuv420, 1),
            (8, Subsampling::Yuv444, 1),
            (8, Subsampling::Mono, 1),
            (10, Subsampling::Yuv420, 1),
            (10, Subsampling::Mono, 1),
        ] {
            let name = format!("{bits}-bit {sub:?} kfi {kfi}");
            let s = svt(bits, sub, kfi);
            let pts = [0i64, 40, 80, 125, 160, 200, 240];
            let frames: Vec<Frame> = pts
                .iter()
                .enumerate()
                .map(|(i, &t)| smooth(&s, i, t))
                .collect();
            let mut enc = Av1VideoEncoder::new(s, TB).unwrap();
            let packets = encode(&mut enc, &frames);
            assert_eq!(packets.len(), frames.len(), "{name}");
            let got: Vec<i64> = packets.iter().map(|p| p.pts().unwrap().ticks()).collect();
            assert_eq!(got, pts, "{name}");
            let keys: Vec<bool> = packets
                .iter()
                .map(|p| p.random_access() == RandomAccess::Yes)
                .collect();
            let want: Vec<bool> = (0..frames.len())
                .map(|i| (i as u64).is_multiple_of(kfi))
                .collect();
            assert_eq!(keys, want, "{name}: random-access flags");
            let decoded = decode(&enc, &packets);
            assert_eq!(decoded.len(), frames.len(), "{name}");
            for (i, f) in decoded.iter().enumerate() {
                assert_eq!(f.timing().pts.unwrap().ticks(), pts[i], "{name}");
                assert_eq!(
                    f.presentation().keyframe,
                    want[i],
                    "{name} frame {i}: key frame"
                );
                assert_eq!(f.interpretation().subsampling, sub, "{name}");
                assert_eq!(f.interpretation().encoding.code_bits(), bits, "{name}");
                assert_eq!(
                    f.interpretation().color.bitstream,
                    Some(Cicp::new(1, 1, 1, false)),
                    "{name}: colour"
                );
                let q = psnr(f, &frames[i], bits);
                assert!(q > 30.0, "{name} frame {i}: PSNR {q:.1} dB");
            }
        }
    }

    #[test]
    fn svt_sessions_meet_the_encoder_contract() {
        for queue in [1, 64] {
            let s = svt(8, Subsampling::Yuv420, 1).with_max_queued_frames(queue);
            let frames: Vec<Frame> = (0..9).map(|i| smooth(&s, i, i as i64 * 40)).collect();
            check_video_encoder_contract(|| Av1VideoEncoder::new(s.clone(), TB).unwrap(), &frames)
                .unwrap_or_else(|e| panic!("queue {queue}: {e:?}"));
        }
    }

    #[test]
    fn svt_av1c_matches_the_sequence_header() {
        const SEQUENCE_HEADER: u8 = 1;
        for (bits, sub, siting, kfi) in [
            (8, Subsampling::Yuv420, ChromaSiting::Left, 1),
            (8, Subsampling::Yuv420, ChromaSiting::Unknown, 1),
            (8, Subsampling::Yuv444, ChromaSiting::Unknown, 1),
            (8, Subsampling::Mono, ChromaSiting::Unknown, 1),
            (10, Subsampling::Yuv420, ChromaSiting::TopLeft, 1),
        ] {
            let name = format!("{bits}-bit {sub:?} {siting:?}");
            let s = svt(bits, sub, kfi).with_siting(siting);
            let mut enc = Av1VideoEncoder::new(s.clone(), TB).unwrap();
            let record = enc.output_config().unwrap().clone();
            let b = record.private.as_deref().unwrap().to_vec();
            let av1c = [
                u32::from(b[1] >> 5),
                u32::from(b[1] & 0x1F),
                u32::from(b[2] >> 7),
                u32::from(b[2] >> 6 & 1),
                u32::from(b[2] >> 5 & 1),
                u32::from(b[2] >> 4 & 1),
                u32::from(b[2] >> 3 & 1),
                u32::from(b[2] >> 2 & 1),
                u32::from(b[2] & 3),
            ];
            let packets = encode(&mut enc, &[smooth(&s, 0, 0)]);
            let seq = obus(packets[0].bytes())
                .into_iter()
                .find(|(kind, _)| *kind == SEQUENCE_HEADER)
                .map(|(_, bytes)| bytes)
                .expect("a key frame carries the sequence header");
            assert_eq!(av1c, sequence_header_fields(&seq), "{name}");
        }
    }

    #[test]
    fn svt_refuses_what_it_cannot_code() {
        let bad = |s: Av1EncodeSettings| Av1VideoEncoder::new(s, TB).unwrap_err();
        for s in [
            // Inter frames: imazen/zenav1-svt issue 26.
            svt(8, Subsampling::Yuv420, 3),
            svt(10, Subsampling::Yuv420, 4),
            svt(8, Subsampling::Yuv422, 1),
            svt(12, Subsampling::Yuv420, 1),
            svt(10, Subsampling::Yuv444, 1),
        ] {
            let what = format!("{s:?}");
            assert!(
                matches!(bad(s), Av1SessionError::Configuration(_)),
                "{what}"
            );
        }
    }
}

#[test]
fn owned_stops_are_polled_inside_the_encoder_backend() {
    use core::sync::atomic::{AtomicUsize, Ordering};
    use enough::{Stop, StopReason};
    #[derive(Default)]
    struct Probe(AtomicUsize);
    impl Stop for Probe {
        fn check(&self) -> Result<(), StopReason> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
    let owned = Arc::new(Probe::default());
    let s = settings(8, Subsampling::Yuv420, 32, 32);
    let f = frame_for(&s, 0, 0);
    let mut enc = Av1VideoEncoder::new(s, TB)
        .unwrap()
        .with_stop(owned.clone());
    assert!(matches!(
        enc.send(&f, &Unstoppable),
        Ok(SendStatus::Accepted)
    ));
    enc.end_input().unwrap();
    let before = owned.0.load(Ordering::Relaxed);
    assert!(matches!(enc.receive(&Unstoppable), Ok(Receive::Ready(_))));
    // The adapter checks before/after pull. More polls prove the owned
    // token reached zenrav1e rather than just the session boundary.
    assert!(owned.0.load(Ordering::Relaxed) - before > 2);

    #[cfg(feature = "zenav1-svt")]
    {
        let owned = Arc::new(Probe::default());
        let s = settings(8, Subsampling::Yuv420, 32, 32)
            .with_backend(zenavif::media::Av1VideoBackend::Zenav1Svt)
            .with_key_frame_interval(1);
        let f = frame_for(&s, 0, 0);
        let mut enc = Av1VideoEncoder::new(s, TB)
            .unwrap()
            .with_stop(owned.clone());
        assert!(matches!(
            enc.send(&f, &Unstoppable),
            Ok(SendStatus::Accepted)
        ));
        // Two checks surround encoding in the adapter; the pipeline must
        // also have polled the supplied owned token.
        assert!(owned.0.load(Ordering::Relaxed) > 2);
    }
}

#[test]
fn an_owned_decoder_stop_cancels_with_an_unstoppable_call_token() {
    use enough::{Stop, StopReason};
    struct Tripped;
    impl Stop for Tripped {
        fn check(&self) -> Result<(), StopReason> {
            Err(StopReason::Cancelled)
        }
    }
    let mut decoder = Av1VideoDecoder::new().unwrap().with_stop(Arc::new(Tripped));
    assert!(matches!(
        decoder.configure(&CodecConfig::new(CodecId::Av1, TB), &Unstoppable),
        Err(SessionError::Cancelled(StopReason::Cancelled))
    ));
}
