//! Print an MP4's tracks and packets as zenmp4 reads them, for comparison
//! with ffprobe (`scripts/mp4-ffmpeg-oracle.sh`).
//!
//! ```text
//! mp4_dump <file.mp4>
//! ```
//!
//! One line per track (`track index codec timebase extras`), then one per
//! packet: `track,pts,dts,size,key` with `key` 1 for a random-access point.

use zencodec::NoPulse;
use zencodec_io::SliceCursor;
use zencodec_media::container::{DemuxEvent, DemuxStage};
use zencodec_media::limits::MediaLimits;
use zenmp4::Mp4Demuxer;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: mp4_dump <file.mp4>")?;
    let bytes = std::fs::read(path)?;
    let mut dem = Mp4Demuxer::new(MediaLimits::default());
    let mut cursor = SliceCursor::new(1 << 16);
    let mut packets = Vec::new();
    loop {
        let y = cursor
            .next(&bytes, &mut DemuxStage(&mut dem), Some(&NoPulse))
            .map_err(|e| format!("{e:?}"))?;
        let done = y.is_finished();
        match y.into_inner() {
            DemuxEvent::Headers => {
                for t in zencodec_media::container::Demuxer::tracks(&dem) {
                    // The existing FFmpeg packet/seek oracle consumes this scalar.
                    // It is only a compatibility projection, not an edit executor.
                    #[allow(deprecated)]
                    let edit = t.edit_delay_ticks();
                    println!(
                        "track {} {:?} {}/{} private={} delay={:?} edit={:?} packets={:?} timeline={:?}",
                        t.index(),
                        t.codec(),
                        t.time_base().numerator(),
                        t.time_base().denominator(),
                        t.codec_private().map_or(0, |p| p.len()),
                        t.codec_delay(),
                        edit,
                        t.declared_packets(),
                        t.timeline()
                    );
                }
            }
            DemuxEvent::Packet(p) => packets.push(p),
            _ => {}
        }
        if done {
            break;
        }
    }
    for p in packets {
        println!(
            "{},{},{},{},{}",
            p.track(),
            p.pts().map_or(i64::MIN, |t| t.ticks()),
            p.dts().map_or(i64::MIN, |t| t.ticks()),
            p.len(),
            u8::from(p.random_access() == zencodec_media::packet::RandomAccess::Yes)
        );
    }
    Ok(())
}
