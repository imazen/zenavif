#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let config = zenavif_parse::DecodeConfig::default()
        .with_peak_memory_limit(64 * 1024 * 1024)
        .with_total_megapixels_limit(16)
        .with_max_animation_frames(100)
        .with_max_grid_tiles(64);

    if let Ok(parser) = zenavif_parse::AvifParser::from_bytes_with_config(
        data, &config, &enough::Unstoppable,
    ) {
        let _ = parser.primary_data();
        let _ = parser.alpha_data();
        let _ = parser.animation_info();
        let _ = parser.grid_config();
    }
});
