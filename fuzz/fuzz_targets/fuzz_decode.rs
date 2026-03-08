#![no_main]

use libfuzzer_sys::fuzz_target;

/// Primary AVIF decode fuzzer: arbitrary bytes through the full decode pipeline.
/// Any panic or abort is a bug.
fuzz_target!(|data: &[u8]| {
    let _ = zenavif::decode(data);
});
