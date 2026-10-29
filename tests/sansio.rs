//! Incremental (sans-I/O) decode through `dyn_incremental_decoder`: the
//! testkit's incremental contract on committed AVIF files.

use zenavif::AvifDecoderConfig;
use zencodec::decode::DecoderConfig as _;

#[test]
fn incremental_contract_holds() {
    assert!(AvifDecoderConfig::capabilities().incremental());
    for name in [
        "tests/vectors/libavif/kodim03_yuv420_8bpc.avif",
        "tests/vectors/libavif/clap_irot_imir_non_essential.avif",
        "tests/vectors/zenavif/mono_gradient_10b_full.avif",
        "tests/vectors/zenavif/mono_gradient_8b_p3icc.avif",
        "zenavif-parse/tests/gainmap/seine_sdr_gainmap_srgb.avif",
        "zenavif-parse/tests/colors-animated-8bpc.avif",
    ] {
        let path = format!("{}/{name}", env!("CARGO_MANIFEST_DIR"));
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        zencodec_testkit::check_incremental_capability_honesty(AvifDecoderConfig::new(), &data)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
}

#[test]
fn incremental_input_limits_bound_growing_prefix_requests() {
    use zencodec::NoPulse;
    use zencodec::ResourceLimits;
    use zencodec::decode::{DecodeJob, Input, InputRequest, StepError};
    use zencodec_io::CollectSink;

    let data = include_bytes!("vectors/libavif/kodim03_yuv420_8bpc.avif");
    for known_length in [false, true] {
        let limit = 32;
        let mut decoder = AvifDecoderConfig::new()
            .job()
            .with_limits(ResourceLimits::none().with_max_input_bytes(limit))
            .dyn_incremental_decoder(&[])
            .unwrap();
        let mut sink = CollectSink::new();
        for request in decoder.requested() {
            assert!(
                matches!(request, InputRequest::AtLeast { min_len, .. } if *min_len <= limit as usize + 1)
            );
        }
        let input = if known_length {
            Input::empty().with_total_len(data.len() as u64)
        } else {
            Input::new(0, &data[..limit as usize + 1])
        };
        assert!(matches!(
            decoder.step(input, &mut sink, Some(&NoPulse)),
            Err(StepError::Codec(_))
        ));
    }
}

#[test]
fn absent_call_control_keeps_the_retained_incremental_job_stop() {
    use enough::{Stop, StopReason};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use zencodec::decode::DecodeJob;
    use zencodec::{CodecErrorExt, ErrorCategory, SharedPulse};
    use zencodec_io::{CollectSink, drive_from_slice};
    struct Switch(Arc<AtomicBool>);
    impl Stop for Switch {
        fn check(&self) -> Result<(), StopReason> {
            if self.0.load(Ordering::SeqCst) {
                Err(StopReason::TimedOut)
            } else {
                Ok(())
            }
        }
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let pulse = SharedPulse::new(zencodec::how_far::StopOnly::new(Switch(stopped.clone())));
    let mut decoder = AvifDecoderConfig::new()
        .job()
        .with_pulse(pulse)
        .dyn_incremental_decoder(&[])
        .unwrap();
    stopped.store(true, Ordering::SeqCst);
    let data = include_bytes!("vectors/libavif/kodim03_yuv420_8bpc.avif");
    let mut sink = CollectSink::new();
    let error = drive_from_slice(&mut decoder, data, &mut sink, None, data.len()).unwrap_err();
    assert_eq!(
        error.error_category(),
        Some(ErrorCategory::Stopped(StopReason::TimedOut))
    );
    assert!(matches!(
        zencodec::find_cause::<zenavif::Error>(&error),
        Some(zenavif::Error::Cancelled(StopReason::TimedOut))
    ));
    assert_eq!(sink.rows_written(), 0);
    assert!(!sink.finished());
}
