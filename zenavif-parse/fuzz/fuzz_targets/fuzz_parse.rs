#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(parser) = zenavif_parse::AvifParser::from_bytes(data) {
        let _ = parser.primary_data();
        let _ = parser.alpha_data();
        let _ = parser.animation_info();
        let _ = parser.grid_config();
        let _ = parser.av1_config();
        let _ = parser.color_info();
    }
});
