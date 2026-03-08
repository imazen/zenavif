#![no_main]

use libfuzzer_sys::fuzz_target;

/// Primary AVIF decode fuzzer: arbitrary bytes through the full decode pipeline.
/// Any panic or abort is a bug. Uses frame_size_limit to prevent OOM.
fuzz_target!(|data: &[u8]| {
    let config = zenavif::DecoderConfig::new()
        .frame_size_limit(4 * 1024 * 1024); // 4 megapixels
    let _ = zenavif::decode_with(data, &config, &enough::Unstoppable);
});
