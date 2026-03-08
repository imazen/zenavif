#![no_main]

use libfuzzer_sys::fuzz_target;

/// Resource-limited decode fuzzer: verify that frame_size_limit is enforced.
/// Uses tight limits to catch resource exhaustion bugs.
fuzz_target!(|data: &[u8]| {
    let config = zenavif::DecoderConfig::new()
        .frame_size_limit(1024 * 1024); // 1 megapixel limit

    let _ = zenavif::decode_with(data, &config, &enough::Unstoppable);
});
