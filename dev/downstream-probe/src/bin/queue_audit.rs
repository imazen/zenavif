//! Measure accepted frames before the AV1 session first promises output.
use std::sync::Arc;

use enough::Unstoppable;
use zenavif::media::{Av1EncodeSettings, Av1VideoEncoder};
use zencodec::time::{TimeBase, Timestamp};
use zencodec_media::frame::{
    ChromaSiting, Frame, FrameGeometry, FrameInterpretation, FrameTiming, OwnedPlane, OwnedPlanes,
    PlaneData, PlaneRole, Presentation, SampleEncoding, Subsampling,
};
use zencodec_media::session::{Receive, SendStatus, VideoEncoder};

fn frame(index: i64) -> Frame {
    let planes = [(32, 32), (16, 16), (16, 16)]
        .into_iter()
        .map(|(width, height)| OwnedPlane {
            data: PlaneData::U8(vec![128; width * height]),
            width,
            height,
            stride_bytes: width,
        })
        .collect();
    Frame::new(
        Arc::new(OwnedPlanes::new(planes, SampleEncoding::U8).unwrap()),
        Arc::new(FrameInterpretation::new(
            FrameGeometry::full(32, 32),
            Subsampling::Yuv420,
            ChromaSiting::Unknown,
            SampleEncoding::U8,
            vec![PlaneRole::Luma, PlaneRole::Cb, PlaneRole::Cr],
        )),
        FrameTiming::at(Timestamp::new(index * 40, TimeBase::MILLIS)).with_duration_ticks(40),
        Presentation::default(),
    )
}

fn main() {
    println!(
        "speed\tconfigured_max_queued_frames\trdo_lookahead_frames\taccepted_before_first_backpressure\texcess"
    );
    for (speed, cap) in [(10, 1), (10, 3), (6, 2), (6, 64)] {
        let settings = Av1EncodeSettings::new(32, 32)
            .with_speed(speed)
            .with_threads(1)
            .with_max_queued_frames(cap);
        let mut encoder = Av1VideoEncoder::new(settings, TimeBase::MILLIS).unwrap();
        let mut accepted = 0usize;
        let mut pending = false;
        for index in 0..128 {
            match encoder.send(&frame(index), &Unstoppable).unwrap() {
                SendStatus::Accepted => accepted += 1,
                SendStatus::ReceivePending => {
                    let Receive::Ready(packet) = encoder.receive(&Unstoppable).unwrap() else {
                        panic!("backpressure did not provide output");
                    };
                    assert_eq!(packet.pts(), Some(Timestamp::new(0, TimeBase::MILLIS)));
                    pending = true;
                    break;
                }
                other => panic!("unknown send status: {other:?}"),
            }
        }
        assert!(pending, "no backpressure after 128 accepted frames");
        let lookahead = zenrav1e::prelude::EncoderConfig::with_speed_preset(speed)
            .speed_settings
            .rdo_lookahead_frames;
        println!(
            "{speed}\t{cap}\t{lookahead}\t{accepted}\t{}",
            accepted.saturating_sub(cap)
        );
    }
}
