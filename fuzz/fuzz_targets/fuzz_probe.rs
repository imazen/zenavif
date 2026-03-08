#![no_main]

use libfuzzer_sys::fuzz_target;

/// Probe fuzzer: test the lightweight ManagedAvifDecoder creation + probe_info path.
/// This exercises AVIF container parsing without full AV1 decode.
fuzz_target!(|data: &[u8]| {
    let config = zenavif::DecoderConfig::new();
    if let Ok(decoder) = zenavif::ManagedAvifDecoder::new(data, &config) {
        let _ = decoder.probe_info();
    }
});
