//! Seek an MP4 and print where it lands, for comparison with ffprobe
//! (`scripts/ffmpeg-oracle.sh`).
//!
//! ```text
//! mp4_seek <file.mp4> <track> <ticks> [any]
//! ```
//!
//! Seeks `track` to `ticks` of its own clock, at or before, to a
//! random-access point (or any packet with `any`), and prints
//! `pts ordinal key` of the position and of the next packet of that track.

use enough::Unstoppable;
use zencodec::time::Timestamp;
use zencodec_io::SliceCursor;
use zencodec_media::container::{
    DemuxEvent, DemuxStage, Demuxer, SeekMode, SeekScope, SeekStatus, SeekTarget,
};
use zencodec_media::limits::MediaLimits;
use zencodec_media::packet::RandomAccess;
use zenmp4::Mp4Demuxer;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [file, track, ticks, rest @ ..] = a.as_slice() else {
        return Err("usage: mp4_seek <file.mp4> <track> <ticks> [any]".into());
    };
    let bytes = std::fs::read(file)?;
    let mut dem = Mp4Demuxer::new(MediaLimits::default());
    let mut cursor = SliceCursor::new(1 << 12);
    while !matches!(
        cursor
            .next(&bytes, &mut DemuxStage(&mut dem), &Unstoppable)
            .map_err(|e| format!("{e:?}"))?
            .into_inner(),
        DemuxEvent::Headers
    ) {}
    let scope = if rest.first().is_some_and(|s| s == "any") {
        SeekScope::Any
    } else {
        SeekScope::RandomAccessPoints
    };
    let track: u32 = track.parse()?;
    let base = dem
        .tracks()
        .get(track as usize)
        .ok_or("no such track")?
        .time_base();
    let target = SeekTarget::new(
        track,
        Timestamp::new(ticks.parse()?, base),
        SeekMode::AtOrBefore,
        scope,
    );
    let r = match dem.seek(&target).map_err(|e| format!("{e:?}"))? {
        SeekStatus::Positioned(r) => r,
        _ => loop {
            if let DemuxEvent::Seeked(r) = cursor
                .next(&bytes, &mut DemuxStage(&mut dem), &Unstoppable)
                .map_err(|e| format!("{e:?}"))?
                .into_inner()
            {
                break r;
            }
        },
    };
    let next = loop {
        match cursor
            .next(&bytes, &mut DemuxStage(&mut dem), &Unstoppable)
            .map_err(|e| format!("{e:?}"))?
            .into_inner()
        {
            DemuxEvent::Packet(p) if p.track() == track => break p,
            DemuxEvent::Done => return Err("no packet after the seek".into()),
            _ => {}
        }
    };
    let key = |ra| u8::from(ra == RandomAccess::Yes);
    println!("{} {} {}", r.pts.ticks(), r.ordinal, key(r.random_access));
    println!(
        "{} {} {}",
        next.pts().map_or(i64::MIN, |t| t.ticks()),
        next.ordinal().unwrap_or(u64::MAX),
        key(next.random_access())
    );
    Ok(())
}
