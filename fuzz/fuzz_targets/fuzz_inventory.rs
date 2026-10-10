#![no_main]

// Structural inventory fuzzer: `DecodeJob::inventory` must never panic, and
// every inventory it returns must cover the input exactly and pass
// `Inventory::validate`. The first byte toggles HDR reconstruction, which
// changes the gain map's disposition.

use libfuzzer_sys::fuzz_target;
use zencodec::decode::{DecodeJob, DecoderConfig};

fuzz_target!(|data: &[u8]| {
    let mut job = zenavif::AvifDecoderConfig::new().job();
    if data.first().is_some_and(|b| b & 1 != 0) {
        job = job.with_gain_map_render(zencodec::GainMapRender::ReconstructHdr {
            target_headroom: None,
        });
    }
    if let Ok(Some(inv)) = job.inventory(data) {
        assert_eq!(inv.input_len(), data.len() as u64);
        if let Err(e) = inv.validate() {
            panic!("invalid inventory: {e}\n{inv}");
        }
    }
});
