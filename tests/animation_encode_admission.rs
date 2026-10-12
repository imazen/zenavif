#![cfg(feature = "encode")]

use std::{
    borrow::Cow,
    sync::atomic::{AtomicUsize, Ordering},
};
use zencodec::{
    ResourceLimits,
    animation::FrameDuration,
    decode::{AnimationFrameDecoder, DecodeJob, DecoderConfig},
    encode::{AnimationFrameEncoder, EncodeJob, EncoderConfig},
};
use zenpixels::{PixelBuffer, PixelDescriptor};

fn pixels(w: u32, h: u32) -> PixelBuffer {
    PixelBuffer::from_vec(
        vec![80; w as usize * h as usize * 3],
        w,
        h,
        PixelDescriptor::RGB8_SRGB,
    )
    .unwrap()
}

fn config() -> zenavif::AvifEncoderConfig {
    zenavif::AvifEncoderConfig::new().with_effort_u32(10)
}

#[test]
fn rejected_frame_does_not_commit_canvas_format_or_clock() {
    let config = config();
    let mut encoder = config
        .job()
        .with_limits(ResourceLimits::none().with_max_width(17))
        .with_loop_count(Some(2))
        .animation_frame_encoder()
        .unwrap();
    let wide = pixels(18, 13);
    assert!(
        encoder
            .push_frame_timed(
                wide.as_slice(),
                FrameDuration::new(1, u32::MAX).unwrap(),
                None
            )
            .is_err()
    );
    let good = pixels(17, 13);
    let first = FrameDuration::new(1001, 30000).unwrap();
    encoder
        .push_frame_timed(good.as_slice(), first, None)
        .unwrap();
    let wrong_height = pixels(17, 12);
    assert!(
        encoder
            .push_frame_timed(
                wrong_height.as_slice(),
                FrameDuration::new(1, 30001).unwrap(),
                None
            )
            .is_err()
    );
    encoder
        .push_frame_timed(good.as_slice(), first, None)
        .unwrap();
    let bytes = encoder.finish(None).unwrap();
    let decoder_config = zenavif::AvifDecoderConfig::new();
    let mut decoder = decoder_config
        .job()
        .animation_frame_decoder(Cow::Borrowed(bytes.data()), &[])
        .unwrap();
    assert_eq!(decoder.loop_count(), Some(2));
    for index in 0..2 {
        let frame = decoder.render_next_frame(None).unwrap().unwrap();
        assert_eq!(frame.frame_index(), index);
        assert_eq!(frame.duration(), first);
        assert_eq!((frame.pixels().width(), frame.pixels().rows()), (17, 13));
    }
    assert!(decoder.render_next_frame(None).unwrap().is_none());
}

#[test]
fn buffered_memory_and_duration_limits_count_all_accepted_frames() {
    let input = pixels(17, 13);
    let bytes = 17 * 13 * 3;
    let config = config();
    let mut memory = config
        .clone()
        .job()
        .with_limits(ResourceLimits::none().with_max_memory(bytes * 2))
        .animation_frame_encoder()
        .unwrap();
    for _ in 0..2 {
        memory.push_frame(input.as_slice(), 10, None).unwrap();
    }
    assert!(memory.push_frame(input.as_slice(), 10, None).is_err());
    let mut timing = config
        .job()
        .with_limits(ResourceLimits::none().with_max_animation_ms(1))
        .animation_frame_encoder()
        .unwrap();
    let part = FrameDuration::new(1, 3000).unwrap();
    for _ in 0..3 {
        timing
            .push_frame_timed(input.as_slice(), part, None)
            .unwrap();
    }
    assert!(
        timing
            .push_frame_timed(input.as_slice(), part, None)
            .is_err()
    );
}

struct PollLimit(AtomicUsize);
impl enough::Stop for PollLimit {
    fn check(&self) -> Result<(), enough::StopReason> {
        if self.0.fetch_add(1, Ordering::Relaxed) >= 8 {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[test]
fn borrowed_finish_stop_is_checked_during_native_preparation() {
    let config = config();
    let mut encoder = config.job().animation_frame_encoder().unwrap();
    let input = pixels(128, 256);
    encoder.push_frame(input.as_slice(), 10, None).unwrap();
    let stop = PollLimit(AtomicUsize::new(0));
    let result = encoder.finish(Some(&stop));
    assert!(
        result.is_err(),
        "finish ignored cancellation after admission"
    );
    assert!(stop.0.load(Ordering::Relaxed) > 8);
}

#[test]
fn animation_decode_keeps_job_stop_and_counts_skipped_pixels() {
    use std::sync::{Arc, atomic::AtomicBool};
    struct Toggle(Arc<AtomicBool>);
    impl enough::Stop for Toggle {
        fn check(&self) -> Result<(), enough::StopReason> {
            if self.0.load(Ordering::Relaxed) {
                Err(enough::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let input = pixels(17, 13);
    let mut encoder = config().job().animation_frame_encoder().unwrap();
    for _ in 0..3 {
        encoder.push_frame(input.as_slice(), 10, None).unwrap();
    }
    let bytes = encoder.finish(None).unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut decoder = zenavif::AvifDecoderConfig::new()
        .job()
        .with_stop(zencodec::StopToken::new(Toggle(cancelled.clone())))
        .animation_frame_decoder(Cow::Borrowed(bytes.data()), &[])
        .unwrap();
    cancelled.store(true, Ordering::Relaxed);
    assert!(
        decoder.render_next_frame(None).is_err(),
        "animation discarded job cancellation"
    );
    cancelled.store(false, Ordering::Relaxed);
    assert!(
        decoder.render_next_frame(None).is_err(),
        "failed decoder resumed after cancellation"
    );
    let mut limited = zenavif::AvifDecoderConfig::new()
        .job()
        .with_limits(ResourceLimits::none().with_max_total_pixels(17 * 13 * 2))
        .with_start_frame_index(2)
        .animation_frame_decoder(Cow::Borrowed(bytes.data()), &[])
        .unwrap();
    assert!(
        limited.render_next_frame(None).is_err(),
        "skips bypassed cumulative pixel budget"
    );
    let mut skipped = zenavif::AvifDecoderConfig::new()
        .job()
        .with_start_frame_index(2)
        .animation_frame_decoder(Cow::Borrowed(bytes.data()), &[])
        .unwrap();
    assert_eq!(
        skipped
            .render_next_frame(None)
            .unwrap()
            .unwrap()
            .frame_index(),
        2
    );
    assert!(skipped.render_next_frame(None).unwrap().is_none());
}
