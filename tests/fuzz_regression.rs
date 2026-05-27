//! Replay seed inputs from `fuzz/regression/` through every fuzz target
//! entry point. Shared scaffolding lives in `zen-fuzz-regress`.

use zenutils_fuzz::RegressionSuite;

#[test]
fn fuzz_regression() {
    RegressionSuite::new("fuzz/regression")
        .target("decode", |input| {
            // Mirrors fuzz_decode.rs: 4 MP frame_size_limit.
            let config = zenavif::DecoderConfig::new().frame_size_limit(4 * 1024 * 1024);
            let _ = zenavif::decode_with(input, &config, &enough::Unstoppable);
        })
        .target("decode_limited", |input| {
            // Mirrors fuzz_decode_limited.rs: tight 1 MP cap.
            let config = zenavif::DecoderConfig::new().frame_size_limit(1024 * 1024);
            let _ = zenavif::decode_with(input, &config, &enough::Unstoppable);
        })
        .target("decode_animation", |input| {
            // Mirrors fuzz_decode_animation.rs.
            let config = zenavif::DecoderConfig::new().frame_size_limit(4 * 1024 * 1024);
            let _ = zenavif::decode_animation_with(input, &config, &enough::Unstoppable);
            if let Ok(mut anim) = zenavif::AnimationDecoder::new(input, &config) {
                while let Ok(Some(_frame)) = anim.next_frame(&enough::Unstoppable) {}
            }
        })
        .target("probe", |input| {
            // Mirrors fuzz_probe.rs: lightweight container parse + probe_info.
            let config = zenavif::DecoderConfig::new();
            if let Ok(decoder) = zenavif::ManagedAvifDecoder::new(input, &config) {
                let _ = decoder.probe_info();
            }
        })
        .run();
}
