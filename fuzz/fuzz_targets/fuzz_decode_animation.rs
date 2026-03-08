#![no_main]

use libfuzzer_sys::fuzz_target;

/// Animation decode fuzzer: exercise the multi-frame AVIF path.
/// Tests frame iteration, timing, and compositing.
/// Uses frame_size_limit to prevent OOM on crafted large-dimension inputs.
fuzz_target!(|data: &[u8]| {
    let config = zenavif::DecoderConfig::new()
        .frame_size_limit(4 * 1024 * 1024); // 4 megapixels

    // Try full animation decode with limits
    let _ = zenavif::decode_animation_with(data, &config, &enough::Unstoppable);

    // Also try frame-by-frame iteration
    if let Ok(mut anim) = zenavif::AnimationDecoder::new(data, &config) {
        while let Ok(Some(_frame)) = anim.next_frame(&enough::Unstoppable) {
            // drain frames
        }
    }
});
