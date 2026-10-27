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
            decoder.step(input, &mut sink, &NoPulse),
            Err(StepError::Codec(_))
        ));
    }
}
