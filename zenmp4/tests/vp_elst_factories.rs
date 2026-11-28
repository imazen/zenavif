//! Integration tests for VP8/VP9 canonical `vpcC`, multi-segment `elst`
//! roundtrips, `SeekableDemuxer`/`BoundedMuxer`/`DemuxFactory`/`MuxFactory`,
//! and independent `ffmpeg`/`ffprobe` interoperability.

use std::num::NonZeroU32;
use std::process::Command;

use zencodec::source::{AccessPattern, SourceAccess};
use zencodec::storage::StorageBudget;
use zencodec::time::{FrameDuration, TimeBase, Timestamp};
use zencodec::{JobResources, NoPulse, Pulse as _, ResourceLimits};
use zencodec_media::container::{
    DemuxEvent, Demuxer, Input, MuxHint, Muxer, PacketWrite, SeekMode, SeekScope, SeekStatus,
    SeekTarget, SeekableDemuxer,
};
use zencodec_media::limits::MediaLimits;
use zencodec_media::packet::{Packet, RandomAccess};
use zencodec_media::registry::{
    ContainerId, ContainerProbe, DemuxFactory, DemuxNeeds, MuxFactory, MuxNeeds, ProbeLimits,
    TimingPolicy,
};
use zencodec_media::session::SessionError;
use zencodec_media::timeline::{EditRate, EditSegment, TrackTiming};
use zencodec_media::track::{CodecId, TrackInfo, VideoTrackInfo};
use zenmp4::{BoundedMp4Muxer, Mp4DemuxFactory, Mp4Demuxer, Mp4MuxFactory, Mp4Muxer};
use zenpixels::Cicp;

fn default_run() -> JobResources {
    JobResources::new(
        ResourceLimits::none(),
        StorageBudget::new(64 << 20, 1024),
        NoPulse.share().unwrap(),
    )
}

fn demux_all(bytes: &[u8]) -> (Vec<TrackInfo>, Vec<Packet>) {
    let mut d = Mp4Demuxer::new(MediaLimits::DEFAULT);
    let mut packets = Vec::new();
    let mut tracks = Vec::new();
    let mut offset = 0usize;
    loop {
        if let Some(req) = d.requested().first() {
            offset = match *req {
                zencodec_media::container::InputRequest::AtLeast { offset, .. } => offset as usize,
                zencodec_media::container::InputRequest::Range(r) => r.offset as usize,
                _ => 0,
            };
        }
        let slice = &bytes[offset.min(bytes.len())..];
        let mut inp = Input::new(offset as u64, slice);
        inp.total_len = Some(bytes.len() as u64);
        let prog = d.step(inp, None).unwrap();
        match prog.event {
            DemuxEvent::Headers => tracks = d.tracks().to_vec(),
            DemuxEvent::Packet(p) => packets.push(p),
            DemuxEvent::Done => break,
            _ => {}
        }
    }
    (tracks, packets)
}

fn mux_file(mut m: Mp4Muxer, tracks: &[TrackInfo], hint: MuxHint, packets: Vec<Packet>) -> Vec<u8> {
    m.begin(tracks, hint).unwrap();
    let mut out = Vec::new();
    for p in packets {
        m.write_packet(p, None).unwrap();
        m.take_output(&mut out);
    }
    let finish = m.finish(None).unwrap();
    finish.apply_to(&mut out).unwrap();
    out
}

#[test]
fn multi_segment_elst_roundtrips_exact_and_refuses_unrepresentable() {
    let tb = TimeBase::new(1, 30_000).unwrap();
    let edits = vec![
        EditSegment::Empty {
            duration: FrameDuration::new(250, 1000).unwrap(),
        },
        EditSegment::Media {
            start: Timestamp::new(1001, tb),
            duration: FrameDuration::new(2002, 30_000).unwrap(),
            rate: EditRate::UNIT,
        },
        EditSegment::Media {
            start: Timestamp::new(6006, tb),
            duration: FrameDuration::new(3003, 30_000).unwrap(),
            rate: EditRate::new(3, NonZeroU32::new(2).unwrap()),
        },
    ];
    let timing = TrackTiming::empty().with_edits(edits.clone());
    let track = TrackInfo::video(0, CodecId::Vp8, tb, VideoTrackInfo::new(32, 32))
        .with_timing(timing)
        .build()
        .unwrap();

    for fragmented in [false, true] {
        let m = if fragmented {
            Mp4Muxer::fragmented()
        } else {
            Mp4Muxer::new()
        };
        let pkt = Packet::from_vec(vec![0x10, 0x00, 0x00, 0x9D, 0x01, 0x2A, 32, 0, 32, 0])
            .unwrap()
            .with_track(0)
            .with_pts(Timestamp::new(0, tb))
            .with_duration_ticks(1001)
            .with_random_access(RandomAccess::Yes);
        let bytes = mux_file(
            m,
            core::slice::from_ref(&track),
            MuxHint::default(),
            vec![pkt],
        );
        let (tracks, packets) = demux_all(&bytes);
        assert_eq!(tracks.len(), 1);
        assert_eq!(packets.len(), 1);
        let got_edits = &tracks[0].timing().edits;
        assert_eq!(got_edits.len(), 3, "fragmented={fragmented}");
        assert_eq!(got_edits[0], edits[0]);
        assert_eq!(got_edits[1], edits[1]);
        // 3/2 in 16.16 fixed point is 98304/65536 = 3/2.
        match got_edits[2] {
            EditSegment::Media {
                start,
                duration,
                rate,
            } => {
                assert_eq!(start, Timestamp::new(6006, tb));
                assert_eq!(duration, FrameDuration::new(3003, 30_000).unwrap());
                assert_eq!(rate.numerator as i64 * 2, rate.denominator.get() as i64 * 3);
            }
            other => panic!("unexpected segment: {other:?}"),
        }
    }

    // Refuse non-16.16 rate (1/3)
    let bad_rate_track = TrackInfo::video(0, CodecId::Vp8, tb, VideoTrackInfo::new(32, 32))
        .with_timing(TrackTiming::empty().with_edits(vec![EditSegment::Media {
            start: Timestamp::new(0, tb),
            duration: FrameDuration::new(1, 1).unwrap(),
            rate: EditRate::new(1, NonZeroU32::new(3).unwrap()),
        }]))
        .build()
        .unwrap();
    assert!(matches!(
        Mp4Muxer::new().begin(&[bad_rate_track], MuxHint::default()),
        Err(SessionError::Unsupported(_))
    ));

    // Refuse negative media start
    let neg_track = TrackInfo::video(0, CodecId::Vp8, tb, VideoTrackInfo::new(32, 32))
        .with_timing(TrackTiming::empty().with_edits(vec![EditSegment::Media {
            start: Timestamp::new(-10, tb),
            duration: FrameDuration::new(1, 1).unwrap(),
            rate: EditRate::UNIT,
        }]))
        .build()
        .unwrap();
    assert!(matches!(
        Mp4Muxer::new().begin(&[neg_track], MuxHint::default()),
        Err(SessionError::Unsupported(_))
    ));

    // Exact edit clock LCM for denominators 65_536 and 65_535 (product 4_294_901_760 fits in u32)
    let tricky_track = TrackInfo::video(0, CodecId::Vp8, tb, VideoTrackInfo::new(32, 32))
        .with_timing(TrackTiming::empty().with_edits(vec![
            EditSegment::Empty {
                duration: FrameDuration::new(1, 65_536).unwrap(),
            },
            EditSegment::Media {
                start: Timestamp::new(0, tb),
                duration: FrameDuration::new(1, 65_535).unwrap(),
                rate: EditRate::UNIT,
            },
        ]))
        .build()
        .unwrap();
    assert!(
        Mp4Muxer::new()
            .begin(&[tricky_track], MuxHint::default())
            .is_ok()
    );

    // Progressive mvhd & tkhd durations MUST sum finite edits (1s empty + 1s media = 2s, not 1s!)
    let edit_dur_track = TrackInfo::video(0, CodecId::Vp8, tb, VideoTrackInfo::new(32, 32))
        .with_timing(TrackTiming::empty().with_edits(vec![
            EditSegment::Empty {
                duration: FrameDuration::new(1, 1).unwrap(),
            },
            EditSegment::Media {
                start: Timestamp::new(0, tb),
                duration: FrameDuration::new(1, 1).unwrap(),
                rate: EditRate::UNIT,
            },
        ]))
        .build()
        .unwrap();
    let mut dmux = Mp4Muxer::new();
    dmux.begin(&[edit_dur_track], MuxHint::default()).unwrap();
    let mut dbytes = Vec::new();
    dmux.take_output(&mut dbytes);
    dmux.write_packet(
        Packet::from_vec(vec![0x10; 16])
            .unwrap()
            .with_track(0)
            .with_pts(Timestamp::new(0, tb))
            .with_duration_ticks(1000)
            .with_random_access(RandomAccess::Yes),
        None,
    )
    .unwrap();
    dmux.take_output(&mut dbytes);
    dmux.finish(None).unwrap().apply_to(&mut dbytes).unwrap();
    let mvhd_pos = dbytes.windows(4).position(|w| w == b"mvhd").unwrap();
    // mvhd v1: fourcc(4) + ver/flags(4) + create(8) + mod(8) + timescale(4) + duration(8)
    let mvhd_ts = u32::from_be_bytes(dbytes[mvhd_pos + 24..mvhd_pos + 28].try_into().unwrap());
    let mvhd_dur = u64::from_be_bytes(dbytes[mvhd_pos + 28..mvhd_pos + 36].try_into().unwrap());
    assert_eq!(
        mvhd_dur,
        u64::from(mvhd_ts) * 2,
        "mvhd must report 2s edited duration"
    );
    let tkhd_pos = dbytes.windows(4).position(|w| w == b"tkhd").unwrap();
    // tkhd v1: fourcc(4) + ver/flags(4) + create(8) + mod(8) + track_id(4) + reserved(4) + duration(8)
    let tkhd_dur = u64::from_be_bytes(dbytes[tkhd_pos + 32..tkhd_pos + 40].try_into().unwrap());
    assert_eq!(
        tkhd_dur,
        u64::from(mvhd_ts) * 2,
        "tkhd must report 2s edited duration"
    );

    // Reject vpcC with nonzero codecIntializationDataSize, invalid profile/chroma, or RGB+subsampled.
    let bad_vpcc_init: Vec<u8> = vec![1, 0, 0, 0, 0, 0, 0x80, 1, 1, 1, 0, 2, 0xAA, 0xBB];
    let bad_vp8 = TrackInfo::video(0, CodecId::Vp8, tb, VideoTrackInfo::new(32, 32))
        .with_codec_private(bad_vpcc_init.into())
        .build()
        .unwrap();
    assert!(
        Mp4Muxer::new()
            .begin(&[bad_vp8], MuxHint::default())
            .is_err()
    );
    // VP9 profile 1 with 4:2:0 must be rejected (profile 1 requires 4:2:2 or 4:4:4).
    let bad_vp9_p1_420: Vec<u8> = vec![1, 0, 0, 0, 1, 0, 0x80, 1, 1, 1, 0, 0];
    let bad_vp9_track = TrackInfo::video(0, CodecId::Vp9, tb, VideoTrackInfo::new(32, 32))
        .with_codec_private(bad_vp9_p1_420.into())
        .build()
        .unwrap();
    assert!(
        Mp4Muxer::new()
            .begin(&[bad_vp9_track], MuxHint::default())
            .is_err()
    );
    // VP8 synthesized path with RGB identity matrix (0) must be rejected.
    let bad_vp8_rgb = TrackInfo::video(
        0,
        CodecId::Vp8,
        tb,
        VideoTrackInfo::new(32, 32).with_color(Cicp::new(1, 13, 0, true)),
    )
    .build()
    .unwrap();
    assert!(
        Mp4Muxer::new()
            .begin(&[bad_vp8_rgb], MuxHint::default())
            .is_err()
    );

    // Fragmented 256 KiB cap with two 128 KiB best-effort (duration=None) packets,
    // second non-sync: must roll fragment 1 before accepting packet 2 so neither pending
    // nor finish ever exceeds 256 KiB.
    let frag_track = TrackInfo::video(0, CodecId::Vp8, tb, VideoTrackInfo::new(64, 48))
        .build()
        .unwrap();
    let cap_256k = 256usize << 10;
    let frag_offer = zencodec_media::container::MuxOffer::new(
        zencodec_media::container::MuxTiming::streaming(),
        zencodec::source::NativeBound::UpperBound(cap_256k as u64),
        zencodec::source::NativeBound::UpperBound(cap_256k as u64),
        zencodec::source::NativeBound::UpperBound(cap_256k as u64),
    );
    let mut bfrag = BoundedMp4Muxer::new(Mp4Muxer::fragmented(), frag_offer, cap_256k);
    zencodec_media::container::DynMuxer::begin(
        &mut bfrag,
        core::slice::from_ref(&frag_track),
        MuxHint::forward_only(),
    )
    .unwrap();
    let mut frag_bytes = Vec::new();
    zencodec_media::container::DynMuxer::take_output(&mut bfrag, &mut frag_bytes);
    assert!(frag_bytes.len() <= cap_256k);
    let mut p1 = Some(
        Packet::from_vec(vec![0x11u8; 128 << 10])
            .unwrap()
            .with_track(0)
            .with_pts(Timestamp::new(0, tb))
            .with_random_access(RandomAccess::Yes),
    );
    assert_eq!(
        zencodec_media::container::BoundedMuxer::try_write_packet(&mut bfrag, &mut p1, None)
            .unwrap(),
        PacketWrite::Accepted
    );
    assert!(zencodec_media::container::DynMuxer::pending_output(&bfrag).len() <= cap_256k);
    let mut p2 = Some(
        Packet::from_vec(vec![0x22u8; 128 << 10])
            .unwrap()
            .with_track(0)
            .with_pts(Timestamp::new(33, tb))
            .with_random_access(RandomAccess::No),
    );
    // Packet 2 rolls fragment 1 and asks caller to drain BEFORE accepting packet 2,
    // leaving p2 untouched so an immediate finish() after any Accepted never exceeds cap_256k.
    assert_eq!(
        zencodec_media::container::BoundedMuxer::try_write_packet(&mut bfrag, &mut p2, None)
            .unwrap(),
        PacketWrite::DrainOutput
    );
    assert!(p2.is_some());
    let pending_after_roll = zencodec_media::container::DynMuxer::pending_output(&bfrag).len();
    assert!(
        pending_after_roll > (128 << 10) && pending_after_roll <= cap_256k,
        "fragment 1 must have rolled within 256 KiB cap, got {pending_after_roll}"
    );
    zencodec_media::container::DynMuxer::take_output(&mut bfrag, &mut frag_bytes);
    assert_eq!(
        zencodec_media::container::BoundedMuxer::try_write_packet(&mut bfrag, &mut p2, None)
            .unwrap(),
        PacketWrite::Accepted
    );
    assert!(p2.is_none());
    // Call finish() directly WITHOUT draining after Accepted — must still stay <= 256 KiB!
    let fdone = zencodec_media::container::DynMuxer::finish(Box::new(bfrag), None).unwrap();
    assert!(
        fdone.tail.len() <= cap_256k,
        "finish tail must stay <= 256 KiB cap even without pre-finish drain, got {}",
        fdone.tail.len()
    );

    // Partial VP9 profile-2 TLV (`[1, 1, 2]`) must defer to keyframe (NOT fabricate profile-2 8-bit)
    let partial_vp9 = TrackInfo::video(0, CodecId::Vp9, tb, VideoTrackInfo::new(32, 32))
        .with_codec_private(vec![1u8, 1, 2].into())
        .build()
        .unwrap();
    assert!(
        Mp4Muxer::new()
            .begin(&[partial_vp9], MuxHint::default())
            .is_ok()
    );

    // PR 3B delta #4 regressions:
    // 1. 128-track fragmented mux under 256 KiB cap must never exceed 262,144 B on finish().
    let tracks_128: Vec<TrackInfo> = (0u32..128)
        .map(|i| {
            TrackInfo::video(i, CodecId::Vp8, tb, VideoTrackInfo::new(32, 32))
                .build()
                .unwrap()
        })
        .collect();
    let mut b128 = BoundedMp4Muxer::new(Mp4Muxer::fragmented(), frag_offer, cap_256k);
    zencodec_media::container::DynMuxer::begin(&mut b128, &tracks_128, MuxHint::forward_only())
        .unwrap();
    let mut init_128 = Vec::new();
    zencodec_media::container::DynMuxer::take_output(&mut b128, &mut init_128);
    let mut accepted_128 = 0usize;
    let mut refused_128 = false;
    for i in 0u32..128 {
        let mut p = Some(
            Packet::from_vec(vec![0x77u8; 1984])
                .unwrap()
                .with_track(i)
                .with_pts(Timestamp::new(0, tb))
                .with_random_access(RandomAccess::No),
        );
        match zencodec_media::container::BoundedMuxer::try_write_packet(&mut b128, &mut p, None) {
            Ok(PacketWrite::Accepted) => accepted_128 += 1,
            Ok(PacketWrite::DrainOutput) => {
                zencodec_media::container::DynMuxer::take_output(&mut b128, &mut init_128);
                if matches!(
                    zencodec_media::container::BoundedMuxer::try_write_packet(
                        &mut b128, &mut p, None
                    ),
                    Ok(PacketWrite::Accepted)
                ) {
                    accepted_128 += 1;
                } else {
                    refused_128 = true;
                    break;
                }
            }
            Err(_) => {
                refused_128 = true;
                break;
            }
            Ok(_) => unreachable!(),
        }
    }
    assert!(
        refused_128 && accepted_128 < 128,
        "128 x 1984 B tracks with traf overhead exceed 256 KiB and must stop before exceeding cap (accepted={accepted_128})"
    );
    let fin_128 = zencodec_media::container::DynMuxer::finish(Box::new(b128), None).unwrap();
    assert!(
        fin_128.tail.len() <= cap_256k,
        "128-track fragmented finish tail exceeded 256 KiB cap: {}",
        fin_128.tail.len()
    );

    // 2. Invalid packet (DTS=33, missing PTS) must be rejected BEFORE closing packet 1's duration
    //    or returning DrainOutput.
    let mut bval = BoundedMp4Muxer::new(Mp4Muxer::fragmented(), frag_offer, cap_256k);
    zencodec_media::container::DynMuxer::begin(
        &mut bval,
        core::slice::from_ref(&frag_track),
        MuxHint::forward_only(),
    )
    .unwrap();
    let mut bval_bytes = Vec::new();
    zencodec_media::container::DynMuxer::take_output(&mut bval, &mut bval_bytes);
    let mut vp1 = Some(
        Packet::from_vec(vec![0x11u8; 128 << 10])
            .unwrap()
            .with_track(0)
            .with_pts(Timestamp::new(0, tb))
            .with_random_access(RandomAccess::Yes),
    );
    assert_eq!(
        zencodec_media::container::BoundedMuxer::try_write_packet(&mut bval, &mut vp1, None)
            .unwrap(),
        PacketWrite::Accepted
    );
    let mut bad_p2 = Some(
        Packet::from_vec(vec![0x22u8; 128 << 10])
            .unwrap()
            .with_track(0)
            .with_dts(Timestamp::new(33, tb))
            .with_random_access(RandomAccess::No),
    );
    assert!(
        matches!(
            zencodec_media::container::BoundedMuxer::try_write_packet(&mut bval, &mut bad_p2, None),
            Err(SessionError::Protocol(
                zencodec_media::session::ProtocolViolation::MissingTimestamp
            ))
        ),
        "packet missing PTS must fail with MissingTimestamp before rolling fragment"
    );
    assert!(bad_p2.is_some());
    assert!(
        zencodec_media::container::DynMuxer::pending_output(&bval).is_empty(),
        "failed packet must not have flushed fragment 1"
    );
    // Supplying a subsequent valid packet at PTS=50 must still give packet 1 duration 50 (not 33).
    let mut good_p2 = Some(
        Packet::from_vec(vec![0x33u8; 128 << 10])
            .unwrap()
            .with_track(0)
            .with_pts(Timestamp::new(50, tb))
            .with_random_access(RandomAccess::No),
    );
    assert_eq!(
        zencodec_media::container::BoundedMuxer::try_write_packet(&mut bval, &mut good_p2, None)
            .unwrap(),
        PacketWrite::DrainOutput
    );
    zencodec_media::container::DynMuxer::take_output(&mut bval, &mut bval_bytes);
    assert_eq!(
        zencodec_media::container::BoundedMuxer::try_write_packet(&mut bval, &mut good_p2, None)
            .unwrap(),
        PacketWrite::Accepted
    );
    let fval = zencodec_media::container::DynMuxer::finish(Box::new(bval), None).unwrap();
    fval.apply_to(&mut bval_bytes).unwrap();
    let (_, demuxed_pkts) = demux_all(&bval_bytes);
    let got_pts: Vec<_> = demuxed_pkts
        .iter()
        .map(|p| (p.pts().unwrap().ticks(), p.duration_ticks()))
        .collect();
    assert_eq!(got_pts, vec![(0, Some(50)), (50, Some(50))]);

    // PR 3B delta #5 regressions:
    // 1. Deferred AV1 track with oversized derived sequence-header OBU (~144 KiB) must charge
    //    the derived `av1C` in `init_overhead` so immediate finish() cannot exceed 256 KiB.
    let deferred_av1 = TrackInfo::video(0, CodecId::Av1, tb, VideoTrackInfo::new(32, 32))
        .build()
        .unwrap();
    let mut bav1 = BoundedMp4Muxer::new(Mp4Muxer::fragmented(), frag_offer, cap_256k);
    zencodec_media::container::DynMuxer::begin(
        &mut bav1,
        core::slice::from_ref(&deferred_av1),
        MuxHint::forward_only(),
    )
    .unwrap();
    let mut av1_init = Vec::new();
    zencodec_media::container::DynMuxer::take_output(&mut bav1, &mut av1_init);
    // Construct a valid Sequence Header OBU (type 1, has_size_field=1) padded to 140 KiB
    // so derived `av1C` (~140 KiB) + payload (~140 KiB) > 256 KiB.
    let mut big_seq_obu = Vec::with_capacity(140 << 10);
    big_seq_obu.push(0x0A); // obu_type=1 (SEQUENCE_HEADER), has_size_field=1
    let seq_body_len: usize = 140 << 10;
    let mut rem = seq_body_len;
    loop {
        let mut byte = (rem & 0x7F) as u8;
        rem >>= 7;
        if rem != 0 {
            byte |= 0x80;
        }
        big_seq_obu.push(byte);
        if rem == 0 {
            break;
        }
    }
    // Minimal sequence header prefix bytes + zero padding to seq_body_len.
    big_seq_obu.extend_from_slice(&[0x00, 0x00, 0x00, 0x04, 0x10, 0x00, 0x00, 0x00]);
    big_seq_obu.resize(big_seq_obu.len() + seq_body_len - 8, 0);
    let mut big_av1_pkt = Some(
        Packet::from_vec(big_seq_obu)
            .unwrap()
            .with_track(0)
            .with_pts(Timestamp::new(0, tb))
            .with_random_access(RandomAccess::Yes),
    );
    assert!(
        zencodec_media::container::BoundedMuxer::try_write_packet(
            &mut bav1,
            &mut big_av1_pkt,
            None
        )
        .is_err(),
        "deferred AV1 keyframe whose derived av1C + payload exceeds 256 KiB must be refused"
    );

    // 2. A packet whose payload was already taken (`Packet::take()`) must fail with EmptyPacket
    //    BEFORE closing sample 1's duration or returning DrainOutput.
    let mut bempty = BoundedMp4Muxer::new(Mp4Muxer::fragmented(), frag_offer, cap_256k);
    zencodec_media::container::DynMuxer::begin(
        &mut bempty,
        core::slice::from_ref(&frag_track),
        MuxHint::forward_only(),
    )
    .unwrap();
    let mut bempty_out = Vec::new();
    zencodec_media::container::DynMuxer::take_output(&mut bempty, &mut bempty_out);
    let mut ep1 = Some(
        Packet::from_vec(vec![0x44u8; 262_000])
            .unwrap()
            .with_track(0)
            .with_pts(Timestamp::new(0, tb))
            .with_random_access(RandomAccess::Yes),
    );
    assert_eq!(
        zencodec_media::container::BoundedMuxer::try_write_packet(&mut bempty, &mut ep1, None)
            .unwrap(),
        PacketWrite::Accepted
    );
    let mut taken_pkt = Packet::from_vec(vec![0x55u8; 16])
        .unwrap()
        .with_track(0)
        .with_pts(Timestamp::new(33, tb));
    let _ = taken_pkt.take();
    let mut ep2 = Some(taken_pkt);
    assert!(
        matches!(
            zencodec_media::container::BoundedMuxer::try_write_packet(&mut bempty, &mut ep2, None),
            Err(SessionError::Protocol(
                zencodec_media::session::ProtocolViolation::EmptyPacket
            ))
        ),
        "already-taken packet must fail with EmptyPacket before rolling fragment 1"
    );
    assert!(
        zencodec_media::container::DynMuxer::pending_output(&bempty).is_empty(),
        "already-taken packet must not flush or mutate sample 1"
    );
}

#[test]
fn vpcc_color_fallback_and_factories_and_bounded_backpressure() {
    let tb = TimeBase::new(1, 1000).unwrap();
    let run = default_run();

    // VP8 track with BT.709 color (no codec_private -> synthesizes canonical 12-byte v1 vpcC)
    let track = TrackInfo::video(
        0,
        CodecId::Vp8,
        tb,
        VideoTrackInfo::new(64, 48).with_color(Cicp::new(1, 1, 1, false)),
    )
    .build()
    .unwrap();

    // Factory quote + open + BoundedMuxer backpressure test
    let mux_factory = Mp4MuxFactory::new();
    let needs = MuxNeeds::new(vec![track.clone()], MuxHint::default())
        .with_timing(TimingPolicy::Exact)
        .with_maximum_packets(4);
    let plan = mux_factory.quote(&needs, &run).unwrap();
    assert!(plan.supports_bounded_writes);
    assert_eq!(plan.container, ContainerId::Mp4);

    // Direct BoundedMp4Muxer with 128-byte cap: ftyp+mdat header (~48B) + 96B packet > 128 ->
    // DrainOutput (packet untouched); after full drain 96 <= 128 -> Accepted; oversized 200B
    // packet after full drain MUST be refused rather than silently exceeding max_pending.
    let mut bounded = BoundedMp4Muxer::new(Mp4Muxer::new(), plan.offer, 128);
    zencodec_media::container::DynMuxer::begin(
        &mut bounded,
        core::slice::from_ref(&track),
        MuxHint::default(),
    )
    .unwrap();
    let mut pkt = Some(
        Packet::from_vec(vec![0xAA; 96])
            .unwrap()
            .with_track(0)
            .with_pts(Timestamp::new(0, tb))
            .with_duration_ticks(33)
            .with_random_access(RandomAccess::Yes),
    );
    assert_eq!(
        zencodec_media::container::BoundedMuxer::try_write_packet(&mut bounded, &mut pkt, None)
            .unwrap(),
        PacketWrite::DrainOutput
    );
    assert!(pkt.is_some(), "DrainOutput must leave packet untouched");

    let mut mp4_bytes = Vec::new();
    zencodec_media::container::DynMuxer::take_output(&mut bounded, &mut mp4_bytes);
    // Packet larger than entire max_pending credit must refuse honestly.
    let mut oversized = Some(
        Packet::from_vec(vec![0xBB; 200])
            .unwrap()
            .with_track(0)
            .with_pts(Timestamp::new(0, tb))
            .with_duration_ticks(33)
            .with_random_access(RandomAccess::Yes),
    );
    assert!(
        zencodec_media::container::BoundedMuxer::try_write_packet(
            &mut bounded,
            &mut oversized,
            None
        )
        .is_err()
    );
    assert!(oversized.is_some());

    assert_eq!(
        zencodec_media::container::BoundedMuxer::try_write_packet(&mut bounded, &mut pkt, None)
            .unwrap(),
        PacketWrite::Accepted
    );
    assert!(pkt.is_none());
    assert!(zencodec_media::container::DynMuxer::pending_output(&bounded).len() <= 128);
    let finish = zencodec_media::container::DynMuxer::finish(Box::new(bounded), None).unwrap();
    finish.apply_to(&mut mp4_bytes).unwrap();

    // Probe positive on MP4, negative on non-MP4 (JPEG, WebM, IVF, still AVIF)
    let demux_factory = Mp4DemuxFactory::new();
    let caps = ProbeLimits::default();
    assert!(matches!(
        demux_factory
            .probe(Input::new(0, &mp4_bytes), &caps)
            .unwrap(),
        ContainerProbe::Confirmed {
            container: ContainerId::Mp4,
            ..
        }
    ));
    assert_eq!(
        demux_factory
            .probe(
                Input::new(0, &[0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F']),
                &caps
            )
            .unwrap(),
        ContainerProbe::NoMatch
    );
    assert_eq!(
        demux_factory
            .probe(Input::new(0, b"DKIF\0\0\x20\0VP90\x20\0\x20\0"), &caps)
            .unwrap(),
        ContainerProbe::NoMatch
    );
    // Still AVIF (`ftyp` major=`avif`, compat=`mif1`) — both direct and preceded by `free` —
    // must NOT match MP4 video demuxer.
    let still_avif = [
        0, 0, 0, 20, b'f', b't', b'y', b'p', b'a', b'v', b'i', b'f', 0, 0, 0, 0, b'm', b'i', b'f',
        b'1',
    ];
    assert_eq!(
        demux_factory
            .probe(Input::new(0, &still_avif), &caps)
            .unwrap(),
        ContainerProbe::NoMatch
    );
    let mut padded_still_avif = vec![0, 0, 0, 8, b'f', b'r', b'e', b'e'];
    padded_still_avif.extend_from_slice(&still_avif);
    assert_eq!(
        demux_factory
            .probe(Input::new(0, &padded_still_avif), &caps)
            .unwrap(),
        ContainerProbe::NoMatch
    );
    // Caller probe caps (max_probe_bytes < 8 or max_boxes == 1 after padding) must be honored.
    let mut tight_caps = ProbeLimits::default();
    tight_caps.max_probe_bytes = 4;
    assert_eq!(
        demux_factory
            .probe(Input::new(0, &mp4_bytes), &tight_caps)
            .unwrap(),
        ContainerProbe::NoMatch
    );
    let mut one_box_caps = ProbeLimits::default();
    one_box_caps.max_boxes = 1;
    let mut padded_mp4 = vec![0, 0, 0, 8, b'f', b'r', b'e', b'e'];
    padded_mp4.extend_from_slice(&mp4_bytes);
    assert_eq!(
        demux_factory
            .probe(Input::new(0, &padded_mp4), &one_box_caps)
            .unwrap(),
        ContainerProbe::NoMatch
    );

    // DemuxFactory quote + open + SeekableDemuxer
    let offer = demux_factory
        .quote(
            &DemuxNeeds::default(),
            &SourceAccess::borrowed(mp4_bytes.len() as u64),
            &run,
        )
        .unwrap();
    assert_eq!(offer.required_access, AccessPattern::RandomAccess);

    let mut d = Mp4Demuxer::new(MediaLimits::DEFAULT);
    let mut got_headers = false;
    while !got_headers {
        let off = d
            .requested()
            .first()
            .map(|r| match *r {
                zencodec_media::container::InputRequest::AtLeast { offset, .. } => offset as usize,
                zencodec_media::container::InputRequest::Range(r) => r.offset as usize,
                _ => 0,
            })
            .unwrap_or(0);
        let mut step_in = Input::new(off as u64, &mp4_bytes[off..]);
        step_in.total_len = Some(mp4_bytes.len() as u64);
        if matches!(d.step(step_in, None).unwrap().event, DemuxEvent::Headers) {
            got_headers = true;
        }
    }
    assert_eq!(*d.tracks()[0].codec(), CodecId::Vp8);
    assert_eq!(
        d.tracks()[0].video_info().unwrap().color,
        Some(Cicp::new(1, 1, 1, false))
    );
    let target = SeekTarget::new(
        0,
        Timestamp::new(0, tb),
        SeekMode::AtOrBefore,
        SeekScope::RandomAccessPoints,
    );
    let seek_offer = d.seek_offer(&target).unwrap();
    assert!(!seek_offer.requires_index);
    assert!(seek_offer.landing_estimate.is_some());
    assert!(matches!(
        d.begin_seek(&target, None).unwrap(),
        SeekStatus::Positioned(_)
    ));
}

struct ParsedIvf {
    width: u32,
    height: u32,
    rate: u32,
    scale: u32,
    frames: Vec<(u64, Vec<u8>)>,
}

fn parse_ivf_packets(ivf: &[u8]) -> ParsedIvf {
    assert_eq!(&ivf[0..4], b"DKIF");
    let width = u32::from(u16::from_le_bytes([ivf[12], ivf[13]]));
    let height = u32::from(u16::from_le_bytes([ivf[14], ivf[15]]));
    let rate = u32::from_le_bytes([ivf[16], ivf[17], ivf[18], ivf[19]]);
    let scale = u32::from_le_bytes([ivf[20], ivf[21], ivf[22], ivf[23]]);
    let mut pos = 32usize;
    let mut frames = Vec::new();
    while pos + 12 <= ivf.len() {
        let len = u32::from_le_bytes(ivf[pos..pos + 4].try_into().unwrap()) as usize;
        let pts = u64::from_le_bytes(ivf[pos + 4..pos + 12].try_into().unwrap());
        pos += 12;
        frames.push((pts, ivf[pos..pos + len].to_vec()));
        pos += len;
    }
    ParsedIvf {
        width,
        height,
        rate,
        scale,
        frames,
    }
}

#[test]
fn ffmpeg_interop_vp8_vp9_and_av1_mp4_progressive_and_fragmented() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        panic!("ffmpeg oracle required for interop test");
    }
    let tmp_dir =
        std::path::PathBuf::from(std::env::var("HOME").unwrap()).join("tmp/zenmp4-interop");
    std::fs::create_dir_all(&tmp_dir).unwrap();

    for (name, encoder, codec_id) in [
        ("vp8", "libvpx", CodecId::Vp8),
        ("vp9", "libvpx-vp9", CodecId::Vp9),
        ("av1", "libaom-av1", CodecId::Av1),
    ] {
        let ivf_path = tmp_dir.join(format!("{name}.ivf"));
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=0.3:size=64x48:rate=10",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            encoder,
            "-g",
            "2",
        ]);
        if encoder == "libaom-av1" {
            cmd.args(["-cpu-used", "8"]);
        }
        cmd.args(["-f", "ivf"]).arg(&ivf_path);
        let status = cmd.status().unwrap();
        assert!(status.success(), "ffmpeg failed generating {name} IVF");

        let ivf_bytes = std::fs::read(&ivf_path).unwrap();
        let ParsedIvf {
            width: w,
            height: h,
            rate,
            scale,
            frames,
        } = parse_ivf_packets(&ivf_bytes);
        assert_eq!((w, h), (64, 48));
        assert!(frames.len() >= 3);
        let tb = TimeBase::new(1, rate / scale.max(1)).unwrap();

        // Mux into BOTH progressive and fragmented MP4 WITHOUT codec_private
        // (proving av1C & vpcC derivation from raw elementary packets!)
        for fragmented in [false, true] {
            let track = TrackInfo::video(
                0,
                codec_id.clone(),
                tb,
                VideoTrackInfo::new(w, h).with_color(Cicp::new(1, 1, 1, false)),
            )
            .build()
            .unwrap();
            let pkts: Vec<Packet> = frames
                .iter()
                .enumerate()
                .map(|(i, (pts, data))| {
                    Packet::from_vec(data.clone())
                        .unwrap()
                        .with_track(0)
                        .with_pts(Timestamp::new(*pts as i64, tb))
                        .with_duration_ticks(1)
                        .with_random_access(if i % 2 == 0 {
                            RandomAccess::Yes
                        } else {
                            RandomAccess::No
                        })
                })
                .collect();
            let m = if fragmented {
                Mp4Muxer::fragmented()
            } else {
                Mp4Muxer::new()
            };
            let mp4 = mux_file(m, &[track], MuxHint::default(), pkts);

            let mp4_path = tmp_dir.join(format!(
                "{name}_{}.mp4",
                if fragmented { "frag" } else { "prog" }
            ));
            std::fs::write(&mp4_path, &mp4).unwrap();

            // Verify ffprobe identifies codec + dimensions
            let probe = Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=codec_name,width,height",
                    "-of",
                    "csv=p=0",
                ])
                .arg(&mp4_path)
                .output()
                .unwrap();
            assert!(
                probe.status.success(),
                "ffprobe failed on {name} fragmented={fragmented}: {}",
                String::from_utf8_lossy(&probe.stderr)
            );
            let probe_line = String::from_utf8_lossy(&probe.stdout).trim().to_string();
            assert_eq!(
                probe_line,
                format!("{name},64,48"),
                "ffprobe mismatch for {name} fragmented={fragmented}"
            );

            // Verify ffmpeg decodes every frame cleanly with zero errors
            let decode = Command::new("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&mp4_path)
                .args(["-f", "null", "-"])
                .output()
                .unwrap();
            assert!(
                decode.status.success() && decode.stderr.is_empty(),
                "ffmpeg decode failed on {name} fragmented={fragmented}: {}",
                String::from_utf8_lossy(&decode.stderr)
            );

            // Roundtrip back through zenmp4 demuxer and verify exact packets & codec
            let (d_tracks, d_pkts) = demux_all(&mp4);
            assert_eq!(d_tracks.len(), 1);
            assert_eq!(*d_tracks[0].codec(), codec_id);
            assert_eq!(d_pkts.len(), frames.len());
            assert!(d_tracks[0].codec_private().is_some());

            let _ = std::fs::remove_file(&mp4_path);
        }
        let _ = std::fs::remove_file(&ivf_path);
    }
    let _ = std::fs::remove_dir(&tmp_dir);
}
