//! zenmp4's demuxer on hand-built files: progressive (moov first and last),
//! fragmented, the media testkit's contract and seek checks, and malformed
//! input. FFmpeg-written files are covered by `scripts/ffmpeg-oracle.sh`.

use enough::Unstoppable;
use zencodec::time::{TimeBase, Timestamp};
use zencodec_io::SliceCursor;
use zencodec_media::container::{DemuxEvent, DemuxStage, SeekMode, SeekScope, SeekTarget};
use zencodec_media::limits::MediaLimits;
use zencodec_media::packet::{Packet, RandomAccess};
use zencodec_media::track::{CodecId, TrackInfo, TrackKind};
use zencodec_media_testkit::{check_demuxer_contract, check_demuxer_seek};
use zenmp4::Mp4Demuxer;
use zenpixels::Cicp;

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version];
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_be_bytes()).collect()
}

const AV1C: [u8; 4] = [0x81, 0x00, 0x0C, 0x00];
const DOPS: [u8; 11] = [0, 2, 0x01, 0x38, 0, 0, 0xBB, 0x80, 0, 0, 0];

fn video_entry() -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend_from_slice(&1u16.to_be_bytes());
    b.extend_from_slice(&[0u8; 16]);
    b.extend_from_slice(&64u16.to_be_bytes());
    b.extend_from_slice(&48u16.to_be_bytes());
    b.extend_from_slice(&[0, 0x48, 0, 0, 0, 0x48, 0, 0, 0, 0, 0, 0, 0, 1]);
    b.extend_from_slice(&[0u8; 32]);
    b.extend_from_slice(&[0, 0x18, 0xFF, 0xFF]);
    b.extend_from_slice(&bx(b"av1C", &AV1C));
    let mut colr = b"nclx".to_vec();
    colr.extend_from_slice(&[0, 9, 0, 16, 0, 9, 0x80]);
    b.extend_from_slice(&bx(b"colr", &colr));
    b.extend_from_slice(&bx(b"pasp", &u32s(&[4, 3])));
    bx(b"av01", &b)
}

fn audio_entry() -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend_from_slice(&1u16.to_be_bytes());
    b.extend_from_slice(&[0u8; 8]);
    b.extend_from_slice(&2u16.to_be_bytes());
    b.extend_from_slice(&16u16.to_be_bytes());
    b.extend_from_slice(&[0u8; 4]);
    b.extend_from_slice(&(48_000u32 << 16).to_be_bytes());
    b.extend_from_slice(&bx(b"dOps", &DOPS));
    bx(b"Opus", &b)
}

fn stsd(entry: Vec<u8>) -> Vec<u8> {
    let mut b = 1u32.to_be_bytes().to_vec();
    b.extend_from_slice(&entry);
    full(b"stsd", 0, 0, &b)
}

fn trak(
    id: u32,
    handler: &[u8; 4],
    timescale: u32,
    entry: Vec<u8>,
    tables: &[Vec<u8>],
    edts: Option<Vec<u8>>,
) -> Vec<u8> {
    let mut tkhd = vec![0u8; 8];
    tkhd.extend_from_slice(&id.to_be_bytes());
    tkhd.extend_from_slice(&[0u8; 72]);
    let mut mdhd = vec![0u8; 8];
    mdhd.extend_from_slice(&timescale.to_be_bytes());
    mdhd.extend_from_slice(&0u32.to_be_bytes());
    mdhd.extend_from_slice(&[0u8; 4]);
    let mut hdlr = vec![0u8; 4];
    hdlr.extend_from_slice(handler);
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut stbl = stsd(entry);
    for t in tables {
        stbl.extend_from_slice(t);
    }
    let minf = bx(b"minf", &bx(b"stbl", &stbl));
    let mdia = bx(
        b"mdia",
        &cat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]),
    );
    let mut parts = vec![full(b"tkhd", 0, 3, &tkhd)];
    if let Some(e) = edts {
        parts.push(e);
    }
    parts.push(mdia);
    bx(b"trak", &cat(&parts))
}

fn mvhd(timescale: u32) -> Vec<u8> {
    let mut b = vec![0u8; 8];
    b.extend_from_slice(&timescale.to_be_bytes());
    b.extend_from_slice(&[0u8; 84]);
    full(b"mvhd", 0, 0, &b)
}

fn payload(track: u32, i: usize) -> Vec<u8> {
    let len = 5 + (i * 7 + track as usize * 3) % 23;
    (0..len)
        .map(|j| (track as usize * 101 + i * 13 + j) as u8)
        .collect()
}

/// What a test expects of one packet.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Want {
    track: u32,
    pts: i64,
    dts: i64,
    duration: u64,
    key: bool,
    ordinal: u64,
    bytes: Vec<u8>,
}

fn got(p: &Packet) -> Want {
    Want {
        track: p.track(),
        pts: p.pts().unwrap().ticks(),
        dts: p.dts().unwrap().ticks(),
        duration: p.duration_ticks().unwrap(),
        key: p.random_access() == RandomAccess::Yes,
        ordinal: p.ordinal().unwrap(),
        bytes: p.bytes().to_vec(),
    }
}

/// A progressive file: video (timescale 1000, 6 samples, B-frame ctts,
/// key frames 1 and 4, an edit list with a leading empty edit) and Opus
/// (timescale 48000, 4 samples, co64), interleaved chunk by chunk.
/// Returns the file and the packets in file order.
fn progressive(moov_first: bool) -> (Vec<u8>, Vec<Want>) {
    // Chunks: V[0,1] A[0,1] V[2,3,4] A[2,3] V[5].
    let layout: [(u32, &[usize]); 5] = [
        (1, &[0, 1]),
        (2, &[0, 1]),
        (1, &[2, 3, 4]),
        (2, &[2, 3]),
        (1, &[5]),
    ];
    let v_dts = [0i64, 40, 80, 120, 160, 200];
    let v_cts = [80i64, 0, 0, 80, 0, 0];
    let v_dur = [40u64; 6];
    let v_key = [true, false, false, true, false, false];
    let a_dur = [960u64, 960, 960, 480];
    let mut mdat_body = Vec::new();
    let mut chunk_offsets: [Vec<u64>; 2] = [vec![], vec![]];
    let mut by_offset = Vec::new();
    // Offsets are relative to mdat body start; fixed up below.
    for (track, samples) in layout {
        chunk_offsets[track as usize - 1].push(mdat_body.len() as u64);
        for &i in samples {
            let p = payload(track, i);
            by_offset.push((mdat_body.len(), track, i));
            mdat_body.extend_from_slice(&p);
        }
    }
    let build_moov = |base: u64| -> Vec<u8> {
        let v_sizes: Vec<u32> = (0..6).map(|i| payload(1, i).len() as u32).collect();
        let a_sizes: Vec<u32> = (0..4).map(|i| payload(2, i).len() as u32).collect();
        let mut stsz_v = vec![0u8; 4];
        stsz_v.extend_from_slice(&6u32.to_be_bytes());
        stsz_v.extend_from_slice(&u32s(&v_sizes));
        let mut stsz_a = vec![0u8; 4];
        stsz_a.extend_from_slice(&4u32.to_be_bytes());
        stsz_a.extend_from_slice(&u32s(&a_sizes));
        let v_off: Vec<u32> = chunk_offsets[0].iter().map(|o| (o + base) as u32).collect();
        let mut stco = (v_off.len() as u32).to_be_bytes().to_vec();
        stco.extend_from_slice(&u32s(&v_off));
        let mut co64 = (chunk_offsets[1].len() as u32).to_be_bytes().to_vec();
        for o in &chunk_offsets[1] {
            co64.extend_from_slice(&(o + base).to_be_bytes());
        }
        let video = trak(
            1,
            b"vide",
            1000,
            video_entry(),
            &[
                full(b"stts", 0, 0, &u32s(&[1, 6, 40])),
                full(b"ctts", 0, 0, &u32s(&[4, 1, 80, 2, 0, 1, 80, 2, 0])),
                full(b"stsc", 0, 0, &u32s(&[3, 1, 2, 1, 2, 3, 1, 3, 1, 1])),
                full(b"stsz", 0, 0, &stsz_v),
                full(b"stco", 0, 0, &stco),
                full(b"stss", 0, 0, &u32s(&[2, 1, 4])),
            ],
            // 500 movie ticks (movie timescale 1000) empty, then media time 80.
            Some(bx(
                b"edts",
                &full(
                    b"elst",
                    0,
                    0,
                    &u32s(&[2, 500, u32::MAX, 0x0001_0000, 240, 80, 0x0001_0000]),
                ),
            )),
        );
        let audio = trak(
            2,
            b"soun",
            48_000,
            audio_entry(),
            &[
                full(b"stts", 0, 0, &u32s(&[2, 3, 960, 1, 480])),
                full(b"stsc", 0, 0, &u32s(&[1, 1, 2, 1])),
                full(b"stsz", 0, 0, &stsz_a),
                full(b"co64", 0, 0, &co64),
            ],
            None,
        );
        bx(b"moov", &cat(&[mvhd(1000), video, audio]))
    };
    let ftyp = bx(b"ftyp", b"isom\0\0\0\0isomav01");
    let mdat_header_len = 8u64;
    let file = if moov_first {
        let moov_len = build_moov(0).len() as u64;
        let base = ftyp.len() as u64 + moov_len + mdat_header_len;
        cat(&[ftyp.clone(), build_moov(base), bx(b"mdat", &mdat_body)])
    } else {
        let base = ftyp.len() as u64 + mdat_header_len;
        cat(&[ftyp.clone(), bx(b"mdat", &mdat_body), build_moov(base)])
    };
    let mut want = Vec::new();
    let mut a_dts = 0i64;
    let mut a_times = Vec::new();
    for d in a_dur {
        a_times.push(a_dts);
        a_dts += d as i64;
    }
    for (_, track, i) in by_offset {
        want.push(if track == 1 {
            Want {
                track: 0,
                pts: v_dts[i] + v_cts[i],
                dts: v_dts[i],
                duration: v_dur[i],
                key: v_key[i],
                ordinal: i as u64,
                bytes: payload(1, i),
            }
        } else {
            Want {
                track: 1,
                pts: a_times[i],
                dts: a_times[i],
                duration: a_dur[i],
                key: true,
                ordinal: i as u64,
                bytes: payload(2, i),
            }
        });
    }
    (file, want)
}

/// A fragmented file: video only (timescale 90000), trex defaults
/// (duration 3000, non-sync), two fragments with tfdt, the first trun with
/// first-sample flags (sync) and per-sample sizes and composition offsets,
/// the second with a tfhd default size.
fn fragmented() -> (Vec<u8>, Vec<Want>) {
    let ftyp = bx(b"ftyp", b"iso5\0\0\0\0iso5");
    let mut trex = 1u32.to_be_bytes().to_vec();
    trex.extend_from_slice(&u32s(&[1, 3000, 0, 0x0001_0000]));
    let moov = bx(
        b"moov",
        &cat(&[
            mvhd(1000),
            trak(1, b"vide", 90_000, video_entry(), &[], None),
            bx(b"mvex", &full(b"trex", 0, 0, &trex)),
        ]),
    );
    let mut file = cat(&[ftyp, moov]);
    let mut want = Vec::new();
    // Fragment 1: 3 samples, sizes per sample, cts per sample.
    let sizes1 = [11u32, 7, 9];
    let cts1 = [6000u32, 0, 3000];
    let build =
        |seq: u32, base_dts: u64, trun: Vec<u8>, tfhd_extra: (u32, Vec<u8>), body_len: usize| {
            let mut tfhd = 1u32.to_be_bytes().to_vec();
            tfhd.extend_from_slice(&tfhd_extra.1);
            let traf = bx(
                b"traf",
                &cat(&[
                    full(b"tfhd", 0, 0x02_0000 | tfhd_extra.0, &tfhd),
                    full(b"tfdt", 1, 0, &base_dts.to_be_bytes()),
                    trun,
                ]),
            );
            let _ = body_len;
            bx(
                b"moof",
                &cat(&[full(b"mfhd", 0, 0, &seq.to_be_bytes()), traf]),
            )
        };
    // trun 1: data offset, first-sample flags, per-sample size and cts.
    let trun1_len_probe = |data_offset: i32| {
        let mut b = 3u32.to_be_bytes().to_vec();
        b.extend_from_slice(&data_offset.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes()); // first sample: sync
        for k in 0..3 {
            b.extend_from_slice(&sizes1[k].to_be_bytes());
            b.extend_from_slice(&cts1[k].to_be_bytes());
        }
        full(b"trun", 0, 0x1 | 0x4 | 0x200 | 0x800, &b)
    };
    let moof_len = build(1, 1_000_000, trun1_len_probe(0), (0, vec![]), 0).len();
    let moof1 = build(
        1,
        1_000_000,
        trun1_len_probe(moof_len as i32 + 8),
        (0, vec![]),
        0,
    );
    let mut mdat1 = Vec::new();
    for (k, &s) in sizes1.iter().enumerate() {
        let p: Vec<u8> = (0..s).map(|j| (k as u32 * 17 + j) as u8).collect();
        want.push(Want {
            track: 0,
            pts: 1_000_000 + k as i64 * 3000 + i64::from(cts1[k]),
            dts: 1_000_000 + k as i64 * 3000,
            duration: 3000,
            key: k == 0,
            ordinal: k as u64,
            bytes: p.clone(),
        });
        mdat1.extend_from_slice(&p);
    }
    file.extend_from_slice(&moof1);
    file.extend_from_slice(&bx(b"mdat", &mdat1));
    // Fragment 2: tfhd default size 5, sample flags per sample.
    let trun2 = |data_offset: i32| {
        let mut b = 2u32.to_be_bytes().to_vec();
        b.extend_from_slice(&data_offset.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes()); // sync
        b.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // non-sync
        full(b"trun", 0, 0x1 | 0x400, &b)
    };
    let moof2_len = build(
        2,
        1_009_000,
        trun2(0),
        (0x10, 5u32.to_be_bytes().to_vec()),
        0,
    )
    .len();
    let moof2 = build(
        2,
        1_009_000,
        trun2(moof2_len as i32 + 8),
        (0x10, 5u32.to_be_bytes().to_vec()),
        0,
    );
    let mut mdat2 = Vec::new();
    for k in 0..2 {
        let p: Vec<u8> = (0..5).map(|j| (200 + k * 5 + j) as u8).collect();
        want.push(Want {
            track: 0,
            pts: 1_009_000 + k as i64 * 3000,
            dts: 1_009_000 + k as i64 * 3000,
            duration: 3000,
            key: k == 0,
            ordinal: 3 + k as u64,
            bytes: p.clone(),
        });
        mdat2.extend_from_slice(&p);
    }
    file.extend_from_slice(&moof2);
    file.extend_from_slice(&bx(b"mdat", &mdat2));
    (file, want)
}

fn demux_with(
    bytes: &[u8],
    limits: MediaLimits,
    window: usize,
) -> Result<(Vec<TrackInfo>, Vec<Packet>), String> {
    let mut dem = Mp4Demuxer::new(limits);
    let mut cursor = SliceCursor::new(window);
    let mut out = Vec::new();
    loop {
        let y = cursor
            .next(bytes, &mut DemuxStage(&mut dem), &Unstoppable)
            .map_err(|e| format!("{e:?}"))?;
        let done = y.is_finished();
        if let DemuxEvent::Packet(p) = y.into_inner() {
            out.push(p);
        }
        if done {
            use zencodec_media::container::Demuxer;
            return Ok((dem.tracks().to_vec(), out));
        }
    }
}

fn demux(bytes: &[u8]) -> Result<(Vec<TrackInfo>, Vec<Packet>), String> {
    demux_with(bytes, MediaLimits::default(), 64)
}

#[test]
fn progressive_files_read_the_same_with_moov_first_or_last() {
    for moov_first in [true, false] {
        let (file, want) = progressive(moov_first);
        let (tracks, packets) = demux(&file).unwrap();
        let got: Vec<Want> = packets.iter().map(got).collect();
        assert_eq!(got, want, "moov first: {moov_first}");
        assert_eq!(tracks.len(), 2);
        let v = &tracks[0];
        assert_eq!((v.kind(), v.codec()), (TrackKind::Video, &CodecId::Av1));
        assert_eq!(v.time_base(), TimeBase::new(1, 1000).unwrap());
        assert_eq!(v.codec_private().map(|p| p.to_vec()), Some(AV1C.to_vec()));
        let vi = v.video_info().unwrap();
        assert_eq!((vi.coded_width, vi.coded_height), (64, 48));
        assert_eq!(vi.display, Some((85, 48)), "pasp 4:3");
        assert_eq!(vi.color, Some(Cicp::new(9, 16, 9, true)));
        // Media time 80, less 500 movie ticks of empty edit (= 500 media ticks).
        assert_eq!(v.edit_delay_ticks(), Some(80 - 500));
        assert_eq!(v.declared_packets(), Some(6));
        let a = &tracks[1];
        assert_eq!((a.kind(), a.codec()), (TrackKind::Audio, &CodecId::Opus));
        let head: Vec<u8> = [
            &b"OpusHead"[..],
            &[1, 2],
            &312u16.to_le_bytes(),
            &48_000u32.to_le_bytes(),
            &[0, 0, 0],
        ]
        .concat();
        assert_eq!(
            a.codec_private().map(|p| p.to_vec()),
            Some(head),
            "dOps as OpusHead"
        );
        assert_eq!(a.audio_info().unwrap().channels.count, 2);
        assert_eq!(
            a.codec_delay().map(|d| (d.numerator(), d.denominator())),
            Some((13, 2000))
        );
    }
}

#[test]
fn edit_offsets_rescale_signed_shifts_with_nearest_ties_up() {
    for (movie_scale, empty_ticks, expected) in [(3000u32, 2u32, 79), (2000, 1, 80), (2000, 3, 79)]
    {
        let (mut file, _) = progressive(true);
        let mvhd = file.windows(4).position(|w| w == b"mvhd").unwrap();
        file[mvhd + 16..mvhd + 20].copy_from_slice(&movie_scale.to_be_bytes());
        let elst = file.windows(4).position(|w| w == b"elst").unwrap();
        file[elst + 12..elst + 16].copy_from_slice(&empty_ticks.to_be_bytes());
        assert_eq!(
            demux(&file).unwrap().0[0].edit_delay_ticks(),
            Some(expected)
        );
    }
}

#[test]
fn fragmented_files_follow_fragment_defaults_and_times() {
    let (file, want) = fragmented();
    let (tracks, packets) = demux(&file).unwrap();
    assert_eq!(tracks.len(), 1);
    let got: Vec<Want> = packets.iter().map(got).collect();
    assert_eq!(got, want);
}

#[test]
fn the_window_size_does_not_change_the_result() {
    for (file, _) in [progressive(true), progressive(false), fragmented()] {
        let reference = demux_with(&file, MediaLimits::default(), 1 << 20).unwrap();
        for window in [1, 7, 16, 33] {
            let (t, p) = demux_with(&file, MediaLimits::default(), window).unwrap();
            assert_eq!(
                format!("{t:?}"),
                format!("{:?}", reference.0),
                "window {window}"
            );
            assert_eq!(
                p.iter().map(got).collect::<Vec<_>>(),
                reference.1.iter().map(got).collect::<Vec<_>>(),
                "window {window}"
            );
        }
    }
}

#[test]
fn the_demuxer_meets_the_contract() {
    for (name, (file, _)) in [
        ("moov first", progressive(true)),
        ("moov last", progressive(false)),
        ("fragmented", fragmented()),
    ] {
        check_demuxer_contract(|| Mp4Demuxer::new(MediaLimits::default()), &file)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
}

#[test]
fn seeking_lands_on_real_samples() {
    let (file, _) = progressive(true);
    let ms = TimeBase::new(1, 1000).unwrap();
    let targets = [
        SeekTarget::new(
            0,
            Timestamp::new(130, ms),
            SeekMode::AtOrBefore,
            SeekScope::RandomAccessPoints,
        ),
        SeekTarget::new(
            0,
            Timestamp::new(130, ms),
            SeekMode::Nearest,
            SeekScope::Any,
        ),
        SeekTarget::new(
            0,
            Timestamp::new(0, ms),
            SeekMode::AtOrAfter,
            SeekScope::Any,
        ),
        SeekTarget::new(
            1,
            Timestamp::new(1, TimeBase::new(1, 30).unwrap()),
            SeekMode::Nearest,
            SeekScope::Any,
        ),
    ];
    check_demuxer_seek(|| Mp4Demuxer::new(MediaLimits::default()), &file, &targets).unwrap();
}

#[test]
fn seeking_a_fragmented_file_indexes_its_fragments() {
    let (file, want) = fragmented();
    let tb = TimeBase::new(1, 90_000).unwrap();
    let at = |t: i64, mode, scope| SeekTarget::new(0, Timestamp::new(t, tb), mode, scope);
    let targets = [
        // Into the second fragment, then back into the first, then forward.
        at(
            1_011_000,
            SeekMode::AtOrBefore,
            SeekScope::RandomAccessPoints,
        ),
        at(0, SeekMode::AtOrAfter, SeekScope::Any),
        at(
            1_007_000,
            SeekMode::AtOrBefore,
            SeekScope::RandomAccessPoints,
        ),
        at(1_012_500, SeekMode::Nearest, SeekScope::Any),
    ];
    check_demuxer_seek(|| Mp4Demuxer::new(MediaLimits::default()), &file, &targets).unwrap();

    // After a seek the rest of the file comes out exactly as a straight
    // read gives it, with the fragment index's decode times and ordinals,
    // and the demuxer finishes instead of scanning past the end again.
    for (target, from) in [(targets[0], 3), (targets[1], 1), (targets[2], 0)] {
        let mut dem = Mp4Demuxer::new(MediaLimits::default());
        let mut cursor = SliceCursor::new(16);
        let mut next = || {
            let y = cursor
                .next(&file, &mut DemuxStage(&mut dem), &Unstoppable)
                .unwrap();
            (y.is_finished(), y.into_inner())
        };
        // Read one packet so the seek interrupts a fragment in progress.
        while !matches!(next().1, DemuxEvent::Packet(_)) {}
        drop(next);
        use zencodec_media::container::{Demuxer, SeekStatus};
        let status = dem.seek(&target).unwrap();
        assert!(matches!(status, SeekStatus::Scanning), "{status:?}");
        let mut out = Vec::new();
        let mut seeked = None;
        loop {
            let y = cursor
                .next(&file, &mut DemuxStage(&mut dem), &Unstoppable)
                .unwrap();
            let done = y.is_finished();
            match y.into_inner() {
                DemuxEvent::Seeked(r) => seeked = Some(r),
                DemuxEvent::Packet(p) => out.push(got(&p)),
                _ => {}
            }
            if done {
                break;
            }
        }
        let r = seeked.expect("the scan reports its position");
        assert_eq!(r.ordinal, want[from].ordinal);
        assert_eq!(out, want[from..], "seek to {target:?}");
        // A second seek reuses the index: no scan.
        let again = dem.seek(&target).unwrap();
        assert!(matches!(again, SeekStatus::Positioned(_)), "{again:?}");
    }
}

fn refused(file: &[u8]) -> String {
    demux(file).err().unwrap_or_else(|| panic!("accepted"))
}

#[test]
fn malformed_files_are_refused() {
    // Minimized from the first fuzz_mp4 CI crash: an open moov with a
    // UUID child lacking its mandatory 16-byte user type.
    assert!(refused(b"\0\0\0\0moov\0\0\0\0uuid").contains("Truncated"));
    let (good, _) = progressive(true);
    let moov_at = good.windows(4).position(|w| w == b"moov").unwrap() - 4;
    // Truncated inside moov.
    assert!(refused(&good[..moov_at + 40]).contains("Truncated"));
    // No moov at all.
    let ftyp = bx(b"ftyp", b"isom\0\0\0\0isom");
    assert!(refused(&cat(&[ftyp.clone(), bx(b"mdat", b"abc")])).contains("NoMovie"));
    // A box size below its header.
    let mut bad = ftyp.clone();
    bad.extend_from_slice(&[0, 0, 0, 4, b'f', b'r', b'e', b'e']);
    assert!(refused(&bad).contains("BadBoxSize"));
    // An mdat running to the end before any moov.
    let mut open = ftyp.clone();
    open.extend_from_slice(&[0, 0, 0, 0, b'm', b'd', b'a', b't', 1, 2, 3]);
    assert!(refused(&open).contains("MovieAfterOpenMdat"));
    // A fragment before the movie.
    assert!(refused(&cat(&[ftyp.clone(), bx(b"moof", &[])])).contains("NoMovie"));
    // Two sample descriptions.
    let two = {
        let mut b = 2u32.to_be_bytes().to_vec();
        b.extend_from_slice(&video_entry());
        b.extend_from_slice(&video_entry());
        full(b"stsd", 0, 0, &b)
    };
    let mut f = good.clone();
    let one = stsd(video_entry());
    let at = f
        .windows(one.len())
        .position(|w| w == one.as_slice())
        .unwrap();
    f.splice(at..at + one.len(), two.iter().copied());
    // The enclosing sizes are now wrong; the reader must still refuse cleanly.
    assert!(demux(&f).is_err());
}

#[test]
fn limits_are_enforced_before_allocation() {
    let (file, _) = progressive(true);
    let limit = |l: MediaLimits| demux_with(&file, l, 64).err().unwrap_or_default();
    assert!(limit(MediaLimits::default().with_max_tracks(1)).contains("Limit"));
    assert!(limit(MediaLimits::default().with_max_table_entries(3)).contains("Limit"));
    assert!(limit(MediaLimits::default().with_max_header_bytes(64)).contains("Limit"));
    assert!(limit(MediaLimits::default().with_max_packet_bytes(8)).contains("Limit"));
}

#[test]
fn combined_seek_index_and_crafted_counts_obey_limits() {
    use zencodec_media::container::{Demuxer, Input};
    use zencodec_media::session::SessionError;

    let (file, _) = progressive(true);
    // The individual video/audio tables have six/four samples, but the
    // combined seek index must fit the same six-entry limit.
    let result = demux_with(&file, MediaLimits::default().with_max_table_entries(6), 64);
    assert!(
        matches!(result, Err(ref e) if e.contains("Limit")),
        "{result:?}"
    );

    for (tag, delta) in [
        (b"stsz", 12),
        (b"stco", 8),
        (b"stss", 8),
        (b"stts", 8),
        (b"stsc", 8),
    ] {
        let (mut file, _) = progressive(true);
        let at = file.windows(4).position(|w| w == tag).unwrap() + delta;
        file[at..at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        let moov = file.windows(4).position(|w| w == b"moov").unwrap() - 4;
        let mut demuxer = Mp4Demuxer::new(MediaLimits::default());
        demuxer
            .step(
                Input::new(0, &file).with_total_len(file.len() as u64),
                &Unstoppable,
            )
            .unwrap();
        let error = demuxer
            .step(
                Input::new(moov as u64, &file[moov..]).with_total_len(file.len() as u64),
                &Unstoppable,
            )
            .unwrap_err();
        assert!(
            matches!(error, SessionError::Limit(_)),
            "{tag:?}: {error:?}"
        );
    }
}

mod muxer {
    use super::*;
    use std::sync::Arc;
    use zencodec_media::container::{MuxHint, Muxer, apply_finish};
    use zencodec_media::session::SessionError;
    use zencodec_media::track::{AudioTrackInfo, ChannelLayout, VideoTrackInfo};
    use zencodec_media_testkit::check_muxer_output_honesty;
    use zenmp4::{Mp4Error, Mp4Muxer};

    fn opus_head() -> Vec<u8> {
        [
            &b"OpusHead"[..],
            &[1, 2],
            &312u16.to_le_bytes(),
            &48_000u32.to_le_bytes(),
            &[0, 0, 0],
        ]
        .concat()
    }

    fn tracks(av1c: Option<&[u8]>) -> Vec<TrackInfo> {
        let mut v = TrackInfo::video(
            0,
            CodecId::Av1,
            TimeBase::new(1, 1000).unwrap(),
            VideoTrackInfo::new(64, 48).with_color(Cicp::new(1, 1, 1, false)),
        )
        .with_edit_delay_ticks(80);
        if let Some(c) = av1c {
            v = v.with_codec_private(Arc::from(c));
        }
        vec![
            v.build().unwrap(),
            TrackInfo::audio(
                1,
                CodecId::Opus,
                TimeBase::new(1, 48_000).unwrap(),
                AudioTrackInfo::new(48_000, ChannelLayout::new(2)),
            )
            .with_codec_private(Arc::from(opus_head()))
            .build()
            .unwrap(),
        ]
    }

    /// A sequence header OBU from FFmpeg/libaom (8-bit 4:2:0), as a key
    /// frame's prefix.
    const SEQ: [u8; 12] = [
        0x0a, 0x0a, 0x00, 0x00, 0x00, 0x03, 0x2d, 0xff, 0xcd, 0xaf, 0x90, 0x04,
    ];

    /// Video every 40 ms with B-frame composition offsets and a key frame
    /// every 4; Opus every 20 ms. Interleaved by decode time.
    fn packets(n_video: usize) -> Vec<Packet> {
        let ms = TimeBase::new(1, 1000).unwrap();
        let khz48 = TimeBase::new(1, 48_000).unwrap();
        let cts = [80i64, 0, 0, 40];
        let mut out = Vec::new();
        for i in 0..n_video {
            let dts = 40 * i as i64;
            let mut body = vec![0x12, 0x00];
            if i % 4 == 0 {
                body.extend_from_slice(&SEQ);
            }
            body.extend((0..10 + i % 7).map(|j| (i * 3 + j) as u8));
            out.push(
                Packet::from_vec(body)
                    .unwrap()
                    .with_track(0)
                    .with_dts(Timestamp::new(dts, ms))
                    .with_pts(Timestamp::new(dts + cts[i % 4], ms))
                    .with_random_access(if i % 4 == 0 {
                        RandomAccess::Yes
                    } else {
                        RandomAccess::No
                    }),
            );
            for k in 0..2 {
                let n = 2 * i + k;
                out.push(
                    Packet::from_vec((0..3 + n % 5).map(|j| (100 + n + j) as u8).collect())
                        .unwrap()
                        .with_track(1)
                        .with_pts(Timestamp::new(960 * n as i64, khz48))
                        .with_random_access(RandomAccess::Yes),
                );
            }
        }
        out
    }

    fn mux(
        m: Mp4Muxer,
        tracks: &[TrackInfo],
        packets: &[Packet],
        hint: MuxHint,
    ) -> Result<Vec<u8>, String> {
        let mut m = m;
        m.begin(tracks, hint).map_err(|e| format!("{e:?}"))?;
        let mut out = Vec::new();
        for p in packets {
            m.write_packet(p.clone(), &Unstoppable)
                .map_err(|e| format!("{e:?}"))?;
            m.take_output(&mut out);
        }
        apply_finish(
            &mut out,
            m.finish(&Unstoppable).map_err(|e| format!("{e:?}"))?,
        )
        .map_err(|e| format!("{e:?}"))?;
        Ok(out)
    }

    fn key(p: &Packet) -> (u32, i64, i64, Vec<u8>, bool) {
        (
            p.track(),
            p.pts().unwrap().ticks(),
            p.dts().map_or(p.pts().unwrap().ticks(), |d| d.ticks()),
            p.bytes().to_vec(),
            p.random_access() == RandomAccess::Yes,
        )
    }

    #[test]
    fn written_files_read_back_packet_for_packet() {
        let av1c = [&[0x81u8, 0x00, 0x0c, 0x00][..], &SEQ].concat();
        let input = packets(9);
        let mut want: Vec<_> = input.iter().map(key).collect();
        want.sort_by_key(|k| (k.0, k.2));
        for (name, m, hint) in [
            ("progressive", Mp4Muxer::new(), MuxHint::default()),
            ("forward-only", Mp4Muxer::new(), MuxHint::forward_only()),
            ("fragmented", Mp4Muxer::fragmented(), MuxHint::default()),
        ] {
            let file = mux(m, &tracks(Some(&av1c)), &input, hint).unwrap();
            let (t, p) = demux(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
            let mut got: Vec<_> = p.iter().map(key).collect();
            got.sort_by_key(|k| (k.0, k.2));
            assert_eq!(got, want, "{name}");
            assert_eq!(
                t[0].codec_private().map(|c| c.to_vec()),
                Some(av1c.clone()),
                "{name}"
            );
            assert_eq!(
                t[1].codec_private().map(|c| c.to_vec()),
                Some(opus_head()),
                "{name}"
            );
            assert_eq!(t[0].edit_delay_ticks(), Some(80), "{name}");
            assert_eq!(
                t[0].video_info().unwrap().color,
                Some(Cicp::new(1, 1, 1, false)),
                "{name}"
            );
            // Durations are the steps between decode times; the last repeats.
            let video: Vec<_> = p.iter().filter(|p| p.track() == 0).collect();
            let durations: Vec<_> = video.iter().map(|p| p.duration_ticks()).collect();
            assert!(
                durations.iter().all(|d| *d == Some(40)),
                "{name}: {durations:?}"
            );
        }
    }

    #[test]
    fn an_av1_track_without_a_record_gets_one_from_its_sequence_header() {
        let av1c = [&[0x81u8, 0x00, 0x0c, 0x00][..], &SEQ].concat();
        for m in [Mp4Muxer::new(), Mp4Muxer::fragmented()] {
            let file = mux(m, &tracks(None), &packets(5), MuxHint::default()).unwrap();
            let (t, _) = demux(&file).unwrap();
            assert_eq!(t[0].codec_private().map(|c| c.to_vec()), Some(av1c.clone()));
        }
        // No sequence header anywhere: refused, not guessed.
        let mut bare = packets(1);
        bare[0] = Packet::from_vec(vec![0x12, 0x00, 0x32, 0x01, 0x00])
            .unwrap()
            .with_track(0)
            .with_pts(Timestamp::new(0, TimeBase::new(1, 1000).unwrap()));
        let e = mux(Mp4Muxer::new(), &tracks(None), &bare, MuxHint::default()).unwrap_err();
        assert!(e.contains("Missing(\"av1C\")"), "{e}");
    }

    #[test]
    fn the_muxer_output_is_honest() {
        let av1c = [&[0x81u8, 0x00, 0x0c, 0x00][..], &SEQ].concat();
        for make in [Mp4Muxer::new as fn() -> Mp4Muxer, Mp4Muxer::fragmented] {
            check_muxer_output_honesty(make, &tracks(Some(&av1c)), &packets(9)).unwrap();
        }
    }

    #[test]
    fn what_mp4_cannot_carry_is_refused() {
        let av1c = [&[0x81u8, 0x00, 0x0c, 0x00][..], &SEQ].concat();
        let ok = tracks(Some(&av1c));
        // A clock that is not 1/timescale.
        let ntsc = vec![
            TrackInfo::video(
                0,
                CodecId::Av1,
                TimeBase::new(1001, 30_000).unwrap(),
                VideoTrackInfo::new(64, 48),
            )
            .with_codec_private(Arc::from(av1c.as_slice()))
            .build()
            .unwrap(),
        ];
        let mut m = Mp4Muxer::new();
        assert!(matches!(
            m.begin(&ntsc, MuxHint::default()),
            Err(SessionError::Codec(Mp4Error::UnsupportedTimeBase))
        ));
        // Decode times must increase.
        let mut p = packets(2);
        p.swap(0, 3);
        let e = mux(Mp4Muxer::new(), &ok, &p, MuxHint::default()).unwrap_err();
        assert!(e.contains("DecodeOrder"), "{e}");
        // Opus needs its OpusHead; a codec with no sample entry is refused.
        let mut bad = ok.clone();
        bad[1] = TrackInfo::audio(
            1,
            CodecId::Opus,
            TimeBase::new(1, 48_000).unwrap(),
            AudioTrackInfo::new(48_000, ChannelLayout::new(2)),
        )
        .with_codec_private(Arc::from(&b"not opus"[..]))
        .build()
        .unwrap();
        assert!(matches!(
            Mp4Muxer::new().begin(&bad, MuxHint::default()),
            Err(SessionError::Codec(Mp4Error::BadCodecPrivate))
        ));
        bad[1] = TrackInfo::audio(
            1,
            CodecId::Pcm,
            TimeBase::new(1, 48_000).unwrap(),
            AudioTrackInfo::new(48_000, ChannelLayout::new(2)),
        )
        .build()
        .unwrap();
        assert!(matches!(
            Mp4Muxer::new().begin(&bad, MuxHint::default()),
            Err(SessionError::Codec(Mp4Error::UnsupportedCodec))
        ));
    }

    #[test]
    fn unsupported_trims_and_mismatched_dts_clocks_are_refused() {
        use zencodec::time::FrameDuration;
        use zencodec_media::packet::Discard;
        use zencodec_media::session::{ProtocolViolation, UnsupportedMedia};

        let ms = TimeBase::MILLIS;
        let negative = TrackInfo::video(0, CodecId::Av1, ms, VideoTrackInfo::new(64, 48))
            .with_codec_private(Arc::from(AV1C.as_slice()))
            .with_edit_delay_ticks(-420)
            .build()
            .unwrap();
        let valid = TrackInfo::video(0, CodecId::Av1, ms, VideoTrackInfo::new(64, 48))
            .with_codec_private(Arc::from(AV1C.as_slice()))
            .build()
            .unwrap();
        let padding = FrameDuration::new(1, 1000).unwrap();
        for make in [Mp4Muxer::new as fn() -> Mp4Muxer, Mp4Muxer::fragmented] {
            assert!(matches!(
                make().begin(core::slice::from_ref(&negative), MuxHint::default()),
                Err(SessionError::Unsupported(UnsupportedMedia::Container))
            ));
            for discard in [
                Discard::start(padding),
                Discard::end(padding),
                Discard::start(padding).with_end(padding),
            ] {
                let mut muxer = make();
                muxer
                    .begin(core::slice::from_ref(&valid), MuxHint::default())
                    .unwrap();
                let packet = Packet::from_vec(vec![0x32, 1, 0])
                    .unwrap()
                    .with_pts(Timestamp::new(1000, ms))
                    .with_duration_ticks(40)
                    .with_discard(discard);
                assert!(matches!(
                    muxer.write_packet(packet, &Unstoppable),
                    Err(SessionError::Unsupported(UnsupportedMedia::Container))
                ));
            }
            let mut muxer = make();
            muxer
                .begin(core::slice::from_ref(&valid), MuxHint::default())
                .unwrap();
            let packet = Packet::from_vec(vec![0x32, 1, 0])
                .unwrap()
                .with_pts(Timestamp::new(1000, ms))
                .with_dts(Timestamp::new(1, TimeBase::new(1, 1).unwrap()))
                .with_duration_ticks(40);
            assert!(matches!(
                muxer.write_packet(packet, &Unstoppable),
                Err(SessionError::Protocol(ProtocolViolation::ClockMismatch))
            ));
        }
    }
}
