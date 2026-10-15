//! zenrav1e and rav1d-safe through the `zencodec-media` sessions: lossless frames decode
//! sample-exact through `Av1VideoDecoder`, timing survives, and the session
//! meets the encoder contract.

use std::sync::Arc;

use enough::Unstoppable;
use zenavif::media::{Av1EncodeSettings, Av1SessionError, Av1VideoDecoder, Av1VideoEncoder};
use zencodec::time::{TimeBase, Timestamp};
use zencodec_media::frame::{
    ChromaSiting, Frame, FrameGeometry, FrameInterpretation, FrameTiming, OwnedPlane, OwnedPlanes,
    PlaneData, PlaneRole, Presentation, SampleEncoding, Subsampling,
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
    let roles = if matches!(sub, Subsampling::Mono) {
        vec![PlaneRole::Luma]
    } else {
        vec![PlaneRole::Luma, PlaneRole::Cb, PlaneRole::Cr]
    };
    let interp = FrameInterpretation::new(FrameGeometry::full(w, h), sub, siting, enc, roles);
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
        let frames: Vec<Frame> = pts
            .iter()
            .enumerate()
            .map(|(i, &t)| frame(i, bits, sub, w, h, t))
            .collect();
        let mut enc = Av1VideoEncoder::new(settings(bits, sub, w, h), TB).unwrap();
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
            assert_eq!(
                it.color.bitstream,
                Some(settings(bits, sub, w, h).color),
                "{name}"
            );
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
        let frames: Vec<Frame> = (0..8)
            .map(|i| frame(i, bits, sub, 24, 16, i as i64 * 40))
            .collect();
        let mut enc =
            Av1VideoEncoder::new(settings(bits, sub, 24, 16).with_quantizer(80), TB).unwrap();
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

/// The `av1C` record agrees with the sequence header on depth, subsampling
/// and chroma position (ISOBMFF requires it); the decoder reports the same
/// siting.
#[test]
fn av1c_matches_the_signalled_format() {
    for (bits, sub, siting, csp) in [
        (8, Subsampling::Yuv420, ChromaSiting::Unknown, 0u8),
        (8, Subsampling::Yuv420, ChromaSiting::Left, 1),
        (10, Subsampling::Yuv420, ChromaSiting::TopLeft, 2),
        (12, Subsampling::Yuv422, ChromaSiting::Unknown, 0),
        (8, Subsampling::Mono, ChromaSiting::Unknown, 0),
        (10, Subsampling::Yuv444, ChromaSiting::Unknown, 0),
    ] {
        let name = format!("{bits}-bit {sub:?} {siting:?}");
        let mut s = settings(bits, sub, 16, 16).with_siting(siting);
        if !matches!(sub, Subsampling::Yuv444) {
            s = s.with_color(Cicp::new(1, 13, 1, true));
        }
        let mut enc = Av1VideoEncoder::new(s, TB).unwrap();
        let record = enc.output_config().unwrap().clone();
        let b = record.private.as_deref().unwrap().to_vec();
        assert_eq!(b[0], 0x81, "{name}");
        let flags = b[2];
        assert_eq!(flags >> 6 & 1 == 1, bits > 8, "{name}: high_bitdepth");
        assert_eq!(flags >> 5 & 1 == 1, bits == 12, "{name}: twelve_bit");
        assert_eq!(
            flags >> 4 & 1 == 1,
            matches!(sub, Subsampling::Mono),
            "{name}: monochrome"
        );
        let (sx, sy) = match sub {
            Subsampling::Yuv444 => (false, false),
            Subsampling::Yuv422 => (true, false),
            _ => (true, true),
        };
        assert_eq!(
            (flags >> 3 & 1 == 1, flags >> 2 & 1 == 1),
            (sx, sy),
            "{name}: subsampling"
        );
        assert_eq!(flags & 0b11, csp, "{name}: chroma_sample_position");
        let input = frame_sited(0, bits, sub, siting, 16, 16, 0);
        let packets = encode(&mut enc, &[input]);
        let mut dec = Av1VideoDecoder::new().unwrap();
        let out = decode_with(&mut dec, &record, &packets);
        assert_eq!(
            out[0].interpretation().siting,
            siting,
            "{name}: decoded siting"
        );
    }
}
