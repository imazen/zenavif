//! Replay seed inputs from `fuzz/regression/` through every fuzz target
//! entry point. Shared scaffolding lives in `zen-fuzz-regress`.

use zen_fuzz_regress::RegressionSuite;

#[test]
fn fuzz_regression() {
    RegressionSuite::new("fuzz/regression")
        .target("parse", |input| {
            if let Ok(parser) = zenavif_parse::AvifParser::from_bytes(input) {
                let _ = parser.primary_data();
                let _ = parser.alpha_data();
                let _ = parser.animation_info();
                let _ = parser.grid_config();
                let _ = parser.av1_config();
                let _ = parser.color_info();
            }
        })
        .target("parse_limited", |input| {
            let config = zenavif_parse::DecodeConfig::default()
                .with_peak_memory_limit(64 * 1024 * 1024)
                .with_total_megapixels_limit(16)
                .with_max_animation_frames(100)
                .with_max_grid_tiles(64);
            if let Ok(parser) = zenavif_parse::AvifParser::from_bytes_with_config(
                input,
                &config,
                &enough::Unstoppable,
            ) {
                let _ = parser.primary_data();
                let _ = parser.alpha_data();
                let _ = parser.animation_info();
                let _ = parser.grid_config();
            }
        })
        .run();
}
