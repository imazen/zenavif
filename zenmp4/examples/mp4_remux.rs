//! Stream-copy an MP4 through zenmp4: demux every packet and mux it again.
//!
//! ```text
//! mp4_remux <in.mp4> <out.mp4> [progressive|fragmented|ready] [stop-after-packets]
//! ```
//!
//! `ready` preserves exact supplied timing and publishes after each packet.
//! A packet limit in that mode writes a prefix without calling `finish`, for
//! independent inspection of publication before the rest of the input.

use zencodec::NoPulse;
use zencodec_io::SliceCursor;
use zencodec_media::container::{DemuxEvent, DemuxStage, Demuxer, MuxHint, Muxer};
use zencodec_media::limits::MediaLimits;
use zenmp4::{Mp4Demuxer, Mp4Muxer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, rest @ ..] = args.as_slice() else {
        return Err("usage: mp4_remux <in.mp4> <out.mp4> [progressive|fragmented|ready] [stop-after-packets]".into());
    };
    let mode = rest.first().map(String::as_str).unwrap_or("progressive");
    if !matches!(mode, "progressive" | "fragmented" | "ready") || rest.len() > 2 {
        return Err("unknown layout or extra arguments".into());
    }
    let limit = rest.get(1).map(|s| s.parse::<usize>()).transpose()?;
    if limit.is_some_and(|n| n == 0 || mode != "ready") {
        return Err("a positive packet limit requires ready mode".into());
    }
    let mut written = 0;
    let bytes = std::fs::read(input)?;
    let mut dem = Mp4Demuxer::new(MediaLimits::default());
    let mut cursor = SliceCursor::new(1 << 16);
    let mut mux = if mode != "progressive" {
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
                let prepared = if mode == "ready" {
                    mux.begin_exact_timing(dem.tracks(), MuxHint::forward_only())
                } else {
                    mux.begin(dem.tracks(), MuxHint::default())
                };
                prepared.map_err(|e| format!("begin: {e:?}"))?;
            }
            DemuxEvent::Packet(p) => {
                mux.write_packet(p, Some(&NoPulse))
                    .map_err(|e| format!("write: {e:?}"))?;
                if mode == "ready" {
                    mux.flush_ready_fragment(Some(&NoPulse))
                        .map_err(|e| format!("flush: {e:?}"))?;
                }
                mux.take_output(&mut out);
                written += 1;
                if limit == Some(written) {
                    std::fs::write(output, out)?;
                    return Ok(());
                }
            }
            _ => {}
        }
        if done {
            break;
        }
    }
    if limit.is_some() {
        return Err("input ended before the requested packet limit".into());
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
