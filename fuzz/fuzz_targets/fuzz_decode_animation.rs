#![no_main]

use libfuzzer_sys::fuzz_target;

/// Animation decode fuzzer: exercise the multi-frame AVIF path.
/// Tests frame iteration, timing, and compositing.
fuzz_target!(|data: &[u8]| {
    // Try full animation decode
    let _ = zenavif::decode_animation(data);

    // Also try frame-by-frame iteration
    let config = zenavif::DecoderConfig::new();
    if let Ok(mut anim) = zenavif::AnimationDecoder::new(data, &config) {
        while let Ok(Some(_frame)) = anim.next_frame(&enough::Unstoppable) {
            // drain frames
        }
    }
});
