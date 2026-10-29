#![no_main]
#![forbid(unsafe_code)]

use libfuzzer_sys::fuzz_target;
use zencodec::NoPulse;
use zencodec_io::SliceCursor;
use zencodec_media::container::DemuxStage;
use zencodec_media::limits::MediaLimits;
use zenmp4::Mp4Demuxer;

fuzz_target!(|data: &[u8]| {
    let limits = MediaLimits::default()
        .with_max_header_bytes(64 << 10)
        .with_max_table_entries(1024)
        .with_max_tracks(16)
        .with_max_packet_bytes(64 << 10);
    let mut demuxer = Mp4Demuxer::new(limits);
    let mut cursor = SliceCursor::new(17);
    while let Ok(event) = cursor.next(data, &mut DemuxStage(&mut demuxer), Some(&NoPulse)) {
        if event.is_finished() {
            break;
        }
    }
});
