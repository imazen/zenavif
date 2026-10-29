//! Stream-copy an MP4 through zenmp4: demux every packet and mux it again.
//!
//! ```text
//! mp4_remux <in.mp4> <out.mp4> [fragmented]
//! ```

use zencodec::NoPulse;
use zencodec_io::SliceCursor;
use zencodec_media::container::{DemuxEvent, DemuxStage, Demuxer, MuxHint, Muxer};
use zencodec_media::limits::MediaLimits;
use zenmp4::{Mp4Demuxer, Mp4Muxer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, rest @ ..] = args.as_slice() else {
        return Err("usage: mp4_remux <in.mp4> <out.mp4> [fragmented]".into());
    };
    let bytes = std::fs::read(input)?;
    let mut dem = Mp4Demuxer::new(MediaLimits::default());
    let mut cursor = SliceCursor::new(1 << 16);
    let mut mux = if rest.first().is_some_and(|a| a == "fragmented") {
        Mp4Muxer::fragmented()
    } else {
        Mp4Muxer::new()
    };
    let mut out = Vec::new();
    loop {
        let y = cursor
            .next(&bytes, &mut DemuxStage(&mut dem), Some(&NoPulse))
            .map_err(|e| format!("demux: {e:?}"))?;
        let done = y.is_finished();
        match y.into_inner() {
            DemuxEvent::Headers => {
                mux.begin(dem.tracks(), MuxHint::default())
                    .map_err(|e| format!("begin: {e:?}"))?;
            }
            DemuxEvent::Packet(p) => {
                mux.write_packet(p, Some(&NoPulse))
                    .map_err(|e| format!("write: {e:?}"))?;
                mux.take_output(&mut out);
            }
            _ => {}
        }
        if done {
            break;
        }
    }
    let finish = mux
        .finish(Some(&NoPulse))
        .map_err(|e| format!("finish: {e:?}"))?;
    finish
        .apply_to(&mut out)
        .map_err(|e| format!("patch: {e:?}"))?;
    std::fs::write(output, out)?;
    Ok(())
}
