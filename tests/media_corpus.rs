//! The lossless AV1 corpus (48 files: 8/10/12-bit, mono/4:2:0/4:2:2/4:4:4,
//! full/narrow, odd and even sizes, 4 frames each) through the session:
//! every sample equals the generator's formula.

use zenavif::media::Av1VideoDecoder;
use zencodec::NoPulse;
use zencodec::time::{TimeBase, Timestamp};
use zencodec_media::frame::{Frame, PlaneRole, Subsampling};
use zencodec_media::packet::Packet;
use zencodec_media::session::{CodecConfig, Receive, SendStatus, VideoDecoder};
use zencodec_media::track::CodecId;
use zencodec_media_testkit::check_video_decoder_contract;
use zenpixels::{ChannelType, Cicp};

/// IVF: 32-byte header (time base at 16..24), then 12-byte packet headers.
fn ivf(bytes: &[u8]) -> (TimeBase, Vec<Packet>) {
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let header_len = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    let tb = TimeBase::new(u32_at(20), u32_at(16)).unwrap();
    let mut pos = header_len;
    let mut out = Vec::new();
    while pos < bytes.len() {
        let len = u32_at(pos) as usize;
        let pts = i64::from_le_bytes(bytes[pos + 4..pos + 12].try_into().unwrap());
        let data = bytes[pos + 12..pos + 12 + len].to_vec();
        let ordinal = out.len() as u64;
        out.push(
            Packet::from_vec(data)
                .unwrap()
                .with_pts(Timestamp::new(pts, tb))
                .with_ordinal(ordinal)
                .with_input_offset((pos + 12) as u64),
        );
        pos += 12 + len;
    }
    (tb, out)
}

/// The corpus generator's source signal (PR 129 `synthetic::code`).
fn code(frame: usize, component: usize, x: usize, y: usize, bits: u8, full: bool) -> u16 {
    let scale = 1u32 << (bits - 8);
    let (low, high) = if full {
        (0, (1u32 << bits) - 1)
    } else if component == 0 {
        (16 * scale, 235 * scale)
    } else {
        (16 * scale, 240 * scale)
    };
    let level = if x == 0 && y == 0 {
        if frame % 2 == 0 { 0 } else { 256 }
    } else {
        (17 * x + 29 * y + 43 * frame + 71 * component) % 257
    };
    (low + ((high - low) * level as u32 + 128) / 256) as u16
}

struct Case {
    name: String,
    bits: u8,
    sampling: String,
    full: bool,
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

fn cases() -> Vec<Case> {
    // Not committed (binary corpus); `just media-corpus` sets the path.
    let dir = std::path::PathBuf::from(
        std::env::var_os("ZENAVIF_AV1_MEDIA_CORPUS")
            .expect("ZENAVIF_AV1_MEDIA_CORPUS must name the corpus directory"),
    );
    let mut out: Vec<Case> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ivf"))
        .map(|p| {
            // av1-{bits}-{sampling}-{range}-{w}x{h}.ivf
            let name = p.file_stem().unwrap().to_str().unwrap().to_string();
            let f: Vec<&str> = name.split('-').collect();
            let (w, h) = f[4].split_once('x').unwrap();
            Case {
                bits: f[1].parse().unwrap(),
                sampling: f[2].to_string(),
                full: f[3] == "full",
                width: w.parse().unwrap(),
                height: h.parse().unwrap(),
                bytes: std::fs::read(&p).unwrap(),
                name,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(out.len(), 48);
    out
}

fn decode(dec: &mut Av1VideoDecoder, config: &CodecConfig, packets: &[Packet]) -> Vec<Frame> {
    dec.configure(config, &NoPulse).unwrap();
    let mut frames = Vec::new();
    let mut pending: Vec<Packet> = packets.to_vec();
    pending.reverse();
    while let Some(mut p) = pending.pop() {
        loop {
            match dec.send(&mut p, &NoPulse).unwrap() {
                SendStatus::Accepted => break,
                _ => match dec.receive(&NoPulse).unwrap() {
                    Receive::Ready(f) => frames.push(f),
                    other => panic!("ReceivePending without output: {other:?}"),
                },
            }
        }
        while let Receive::Ready(f) = dec.receive(&NoPulse).unwrap() {
            frames.push(f);
        }
    }
    dec.end_input().unwrap();
    loop {
        match dec.receive(&NoPulse).unwrap() {
            Receive::Ready(f) => frames.push(f),
            Receive::EndOfStream => break,
            Receive::NeedInput => panic!("NeedInput after end_input"),
            _ => unreachable!(),
        }
    }
    frames
}

#[test]
fn every_corpus_sample_matches_the_source_signal() {
    for case in cases() {
        let (tb, packets) = ivf(&case.bytes);
        assert_eq!(tb, TimeBase::new(1001, 30000).unwrap());
        let config = CodecConfig::new(CodecId::Av1, tb);
        let mut dec = Av1VideoDecoder::new().unwrap();
        let frames = decode(&mut dec, &config, &packets);
        // Frames outlive their decoder.
        drop(dec);
        assert_eq!(frames.len(), 4, "{}", case.name);
        let (subsampling, planes, (sx, sy)) = match case.sampling.as_str() {
            "mono" => (Subsampling::Mono, 1, (1, 1)),
            "420" => (Subsampling::Yuv420, 3, (2, 2)),
            "422" => (Subsampling::Yuv422, 3, (2, 1)),
            _ => (Subsampling::Yuv444, 3, (1, 1)),
        };
        for (i, frame) in frames.iter().enumerate() {
            let it = frame.interpretation();
            assert_eq!(it.subsampling, subsampling, "{}", case.name);
            assert_eq!(
                (it.geometry.coded_width, it.geometry.coded_height),
                (case.width, case.height)
            );
            assert_eq!(it.encoding.code_bits(), case.bits);
            assert_eq!(it.encoding.bit_shift(), 0);
            assert_eq!(
                it.encoding.storage(),
                if case.bits == 8 {
                    ChannelType::U8
                } else {
                    ChannelType::U16
                }
            );
            let roles = if planes == 1 {
                vec![PlaneRole::Luma]
            } else {
                vec![PlaneRole::Luma, PlaneRole::Cb, PlaneRole::Cr]
            };
            assert_eq!(it.roles, roles);
            assert_eq!(
                it.color.bitstream,
                Some(Cicp::new(1, 1, 1, case.full)),
                "{}",
                case.name
            );
            assert_eq!(it.color.resolved, it.color.bitstream);
            let timing = frame.timing();
            assert_eq!(
                timing.pts,
                Some(Timestamp::new(
                    i as i64,
                    TimeBase::new(1001, 30000).unwrap()
                ))
            );
            assert_eq!(timing.packet_ordinal, Some(i as u64));
            assert!(timing.input_offset.is_some());
            let mapped = frame.map().unwrap();
            assert_eq!(mapped.plane_count(), planes);
            for c in 0..planes {
                let p = mapped.plane(c).unwrap();
                let (pw, ph) = if c == 0 {
                    (case.width as usize, case.height as usize)
                } else {
                    (
                        (case.width as usize).div_ceil(sx),
                        (case.height as usize).div_ceil(sy),
                    )
                };
                assert_eq!((p.width(), p.height()), (pw, ph), "{} plane {c}", case.name);
                for y in 0..ph {
                    for x in 0..pw {
                        assert_eq!(
                            p.code(x, y),
                            Some(code(i, c, x, y, case.bits, case.full)),
                            "{} frame {i} plane {c} ({x},{y})",
                            case.name
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn the_session_meets_the_decoder_contract() {
    for case in cases().into_iter().step_by(7) {
        let (tb, packets) = ivf(&case.bytes);
        let config = CodecConfig::new(CodecId::Av1, tb);
        check_video_decoder_contract(|| Av1VideoDecoder::new().unwrap(), &config, &packets)
            .unwrap_or_else(|e| panic!("{}: {e:?}", case.name));
    }
}
