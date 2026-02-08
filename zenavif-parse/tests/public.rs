// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use avif_parse::Error;
use std::borrow::Cow;
use std::fs::File;

static IMAGE_AVIF: &str = "av1-avif/testFiles/Microsoft/Monochrome.avif";
static IMAGE_AVIF_EXTENTS: &str = "tests/kodim-extents.avif";
static IMAGE_AVIF_CORRUPT: &str = "tests/bug-1655846.avif";
static IMAGE_AVIF_CORRUPT_2: &str = "tests/bug-1661347.avif";
static IMAGE_GRID_5X4: &str = "av1-avif/testFiles/Microsoft/Summer_in_Tomsk_720p_5x4_grid.avif";
static ANIMATED_AVIF: &str = "link-u-samples/star-8bpc.avifs";
static AOMEDIA_TEST_FILES: &str = "av1-avif/testFiles";
static LINK_U_SAMPLES: &str = "link-u-samples";

// ============================================================================
// Eager path (read_avif) tests
// ============================================================================

#[test]
fn public_avif_primary_item() {
    let input = &mut File::open(IMAGE_AVIF).expect("Unknown file");
    let context = avif_parse::read_avif(input).expect("read_avif failed");
    assert_eq!(context.primary_item.len(), 6979);
    assert_eq!(context.primary_item[0..4], [0x12, 0x00, 0x0a, 0x0a]);
}

#[test]
fn public_avif_primary_item_split_extents() {
    let input = &mut File::open(IMAGE_AVIF_EXTENTS).expect("Unknown file");
    let context = avif_parse::read_avif(input).expect("read_avif failed");
    assert_eq!(context.primary_item.len(), 4387);
}

#[test]
fn public_avif_bug_1655846() {
    let input = &mut File::open(IMAGE_AVIF_CORRUPT).expect("Unknown file");
    assert!(avif_parse::read_avif(input).is_err());
}

#[test]
fn public_avif_bug_1661347() {
    let input = &mut File::open(IMAGE_AVIF_CORRUPT_2).expect("Unknown file");
    assert!(avif_parse::read_avif(input).is_err());
}

#[test]
fn aomedia_sample_images() {
    test_dir(AOMEDIA_TEST_FILES);
}

#[test]
fn linku_sample_images() {
    test_dir(LINK_U_SAMPLES);
}

#[test]
fn grid_5x4_ispe_calculation() {
    let input = &mut File::open(IMAGE_GRID_5X4).expect("Unknown file");
    let avif = avif_parse::read_avif(input).expect("read_avif failed");

    let grid = avif.grid_config.expect("Expected grid config");
    assert_eq!(grid.rows, 4, "Expected 4 rows");
    assert_eq!(grid.columns, 5, "Expected 5 columns");
    assert_eq!(grid.output_width, 6400, "Expected width 6400");
    assert_eq!(grid.output_height, 2880, "Expected height 2880");
    assert_eq!(avif.grid_tiles.len(), 20, "Expected 20 tiles (4×5)");
    assert_eq!(avif.primary_item.len(), 0, "Grid images should have empty primary_item");
}

#[test]
fn grid_tile_ordering() {
    let input = &mut File::open(IMAGE_GRID_5X4).expect("Unknown file");
    let avif = avif_parse::read_avif(input).expect("read_avif failed");

    for (i, tile) in avif.grid_tiles.iter().enumerate() {
        assert!(!tile.is_empty(), "Tile {} should not be empty", i);
        assert!(tile.len() > 1000, "Tile {} seems too small ({} bytes)", i, tile.len());
    }
}

#[test]
fn animated_avif_frame_extraction() {
    let input = &mut File::open(ANIMATED_AVIF).expect("Unknown file");
    let avif = avif_parse::read_avif(input).expect("read_avif failed");

    let animation = avif.animation.expect("Expected animation data");
    assert_eq!(animation.frames.len(), 5, "Expected 5 frames");

    for (i, frame) in animation.frames.iter().enumerate() {
        assert!(!frame.data.is_empty(), "Frame {} should not be empty", i);
        assert!(frame.duration_ms > 0, "Frame {} should have positive duration", i);
    }

    for frame in &animation.frames {
        assert_eq!(frame.duration_ms, 100, "Expected 100ms frame duration");
    }

    assert_eq!(avif.primary_item.len(), animation.frames[0].data.len(),
        "Primary item should match first frame");
}

fn test_dir(dir: &str) {
    let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::max()).try_init();
    let mut errors = 0;

    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry.expect("AVIF entry");
        let path = entry.path();
        let ext = path.extension().unwrap_or_default();
        if !path.is_file() || (ext != "avif" && ext != "avifs") {
            continue;
        }
        log::debug!("parsing {:?}", path.display());
        let input = &mut File::open(path).expect("bad file");
        match avif_parse::read_avif(input) {
            Ok(avif) => {
                if avif.grid_config.is_none() {
                    avif.primary_item_metadata().unwrap();
                    avif.alpha_item_metadata().unwrap();
                } else {
                    assert!(!avif.grid_tiles.is_empty(), "Grid image has no tiles");
                }
            },
            Err(Error::Unsupported(why)) => log::warn!("{why}"),
            Err(err) => {
                log::error!("{:?}: {err}", path.display());
                errors += 1;
            },
        }
    }
    assert_eq!(0, errors);
}

// ============================================================================
// AvifParser (zero-copy) tests
// ============================================================================

#[test]
fn parser_from_bytes_primary() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let parser = avif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let primary = parser.primary_data().expect("primary_data failed");
    assert_eq!(primary.len(), 6979);
    assert_eq!(&primary[0..4], &[0x12, 0x00, 0x0a, 0x0a]);

    // Single-extent → must be Cow::Borrowed (true zero-copy)
    assert!(matches!(primary, Cow::Borrowed(_)), "Expected Cow::Borrowed for single-extent");
    assert!(parser.animation_info().is_none());
}

#[test]
fn parser_from_bytes_multi_extent() {
    let bytes = std::fs::read(IMAGE_AVIF_EXTENTS).expect("read file");
    let parser = avif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let primary = parser.primary_data().expect("primary_data failed");
    assert_eq!(primary.len(), 4387);

    // Multi-extent → Cow::Owned
    assert!(matches!(primary, Cow::Owned(_)), "Expected Cow::Owned for multi-extent");
}

#[test]
fn parser_from_owned_primary() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let parser = avif_parse::AvifParser::from_owned(bytes).expect("from_owned failed");

    let primary = parser.primary_data().expect("primary_data failed");
    assert_eq!(primary.len(), 6979);
    assert_eq!(&primary[0..4], &[0x12, 0x00, 0x0a, 0x0a]);
}

#[test]
fn parser_from_reader_primary() {
    let parser = avif_parse::AvifParser::from_reader(
        &mut File::open(IMAGE_AVIF).expect("Unknown file"),
    ).expect("from_reader failed");

    let primary = parser.primary_data().expect("primary_data failed");
    assert_eq!(primary.len(), 6979);
    assert_eq!(&primary[0..4], &[0x12, 0x00, 0x0a, 0x0a]);
}

#[test]
fn parser_grid() {
    let bytes = std::fs::read(IMAGE_GRID_5X4).expect("read file");
    let parser = avif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let grid = parser.grid_config().expect("Expected grid config");
    assert_eq!(grid.rows, 4);
    assert_eq!(grid.columns, 5);
    assert_eq!(parser.grid_tile_count(), 20);

    let tile = parser.tile_data(0).expect("tile_data failed");
    assert!(!tile.is_empty());

    for i in 0..20 {
        let t = parser.tile_data(i).expect("tile_data failed");
        assert!(!t.is_empty(), "Tile {} empty", i);
    }

    assert!(parser.tile_data(20).is_err());
}

#[test]
fn parser_animation_frames() {
    let bytes = std::fs::read(ANIMATED_AVIF).expect("read file");
    let parser = avif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let info = parser.animation_info().expect("Expected animation");
    assert_eq!(info.frame_count, 5);

    for i in 0..5 {
        let frame = parser.frame(i).expect("frame failed");
        assert!(!frame.data.is_empty(), "Frame {} empty", i);
        assert_eq!(frame.duration_ms, 100);

        // from_bytes → single-extent frames should be Cow::Borrowed
        assert!(matches!(frame.data, Cow::Borrowed(_)), "Frame {} should be Cow::Borrowed", i);
    }

    assert!(parser.frame(5).is_err());
}

#[test]
fn parser_frames_iterator() {
    let bytes = std::fs::read(ANIMATED_AVIF).expect("read file");
    let parser = avif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let frames: Vec<_> = parser.frames().collect();
    assert_eq!(frames.len(), 5);

    for (i, result) in frames.iter().enumerate() {
        let frame = result.as_ref().expect("frame failed");
        assert!(!frame.data.is_empty(), "Frame {} empty", i);
        assert_eq!(frame.duration_ms, 100);
    }
}

#[test]
fn parser_metadata() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let parser = avif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let meta = parser.primary_metadata().expect("primary_metadata failed");
    assert!(meta.monochrome); // Monochrome.avif
    assert!(meta.still_picture);

    assert!(parser.alpha_metadata().is_none());
}

#[test]
fn parser_to_avif_data_matches_eager() {
    let bytes = std::fs::read(ANIMATED_AVIF).expect("read file");
    let parser = avif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");
    let converted = parser.to_avif_data().expect("to_avif_data failed");

    let direct = avif_parse::read_avif(&mut File::open(ANIMATED_AVIF).expect("file"))
        .expect("read_avif failed");

    assert_eq!(converted.primary_item.len(), direct.primary_item.len());
    assert_eq!(converted.primary_item.as_slice(), direct.primary_item.as_slice());

    let conv_anim = converted.animation.as_ref().expect("animation");
    let dir_anim = direct.animation.as_ref().expect("animation");
    assert_eq!(conv_anim.frames.len(), dir_anim.frames.len());

    for (i, (c, d)) in conv_anim.frames.iter().zip(dir_anim.frames.iter()).enumerate() {
        assert_eq!(c.data.as_slice(), d.data.as_slice(), "Frame {} data mismatch", i);
        assert_eq!(c.duration_ms, d.duration_ms, "Frame {} duration mismatch", i);
    }
}

#[test]
fn parser_to_avif_data_grid() {
    let bytes = std::fs::read(IMAGE_GRID_5X4).expect("read file");
    let parser = avif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");
    let converted = parser.to_avif_data().expect("to_avif_data failed");

    let direct = avif_parse::read_avif(&mut File::open(IMAGE_GRID_5X4).expect("file"))
        .expect("read_avif failed");

    assert_eq!(converted.grid_tiles.len(), direct.grid_tiles.len());
    for (i, (c, d)) in converted.grid_tiles.iter().zip(direct.grid_tiles.iter()).enumerate() {
        assert_eq!(c.as_slice(), d.as_slice(), "Tile {} data mismatch", i);
    }
}

// ============================================================================
// Corpus-wide tests: all parsing paths
// ============================================================================

fn test_dir_all_paths(dir: &str) {
    let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::max()).try_init();
    let config = avif_parse::DecodeConfig::default();
    let mut errors = 0;

    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry.expect("AVIF entry");
        let path = entry.path();
        let ext = path.extension().unwrap_or_default();
        if !path.is_file() || (ext != "avif" && ext != "avifs") {
            continue;
        }

        // Path 1: eager
        let eager_result = avif_parse::read_avif_with_config(
            &mut File::open(path).expect("bad file"),
            &config,
            &avif_parse::Unstoppable,
        );

        // Path 2: zero-copy from_bytes
        let file_bytes = std::fs::read(path).expect("read file");
        let parser_result = avif_parse::AvifParser::from_bytes_with_config(
            &file_bytes,
            &config,
            &avif_parse::Unstoppable,
        );

        match (&eager_result, &parser_result) {
            (Ok(avif), Ok(parser)) => {
                if avif.grid_config.is_none() {
                    let parser_primary = parser.primary_data()
                        .expect("primary_data failed");
                    assert_eq!(
                        avif.primary_item.len(),
                        parser_primary.len(),
                        "{}: primary_item length mismatch",
                        path.display(),
                    );
                } else {
                    assert!(!avif.grid_tiles.is_empty(), "{}: grid has no tiles", path.display());
                    assert_eq!(
                        avif.grid_tiles.len(),
                        parser.grid_tile_count(),
                        "{}: tile count mismatch",
                        path.display(),
                    );
                }
            }
            (Err(Error::Unsupported(why)), _) | (_, Err(Error::Unsupported(why))) => {
                log::warn!("{}: {why}", path.display());
            }
            (Err(_), Err(_)) => {
                log::debug!("{}: both paths rejected", path.display());
            }
            (Err(e), Ok(parser)) => {
                log::debug!("{}: eager rejected ({e}), verifying parser fails on extraction", path.display());
                let extraction_ok = parser.primary_data().is_ok()
                    || parser.grid_tile_count() > 0;
                if extraction_ok {
                    log::error!("{}: eager failed ({e}) but parser extraction succeeded", path.display());
                    errors += 1;
                }
            }
            (Ok(_), Err(e)) => {
                log::error!("{}: parser failed but eager succeeded: {e}", path.display());
                errors += 1;
            }
        }
    }
    assert_eq!(0, errors);
}

#[test]
fn corpus_aomedia_all_paths() {
    test_dir_all_paths(AOMEDIA_TEST_FILES);
}

#[test]
fn corpus_linku_all_paths() {
    test_dir_all_paths(LINK_U_SAMPLES);
}

#[test]
fn corpus_local_all_paths() {
    test_dir_all_paths("tests");
}

// ============================================================================
// Resource Limit Tests
// ============================================================================

#[test]
fn resource_limit_peak_memory() {
    let input = &mut File::open(IMAGE_AVIF).expect("Unknown file");
    let config = avif_parse::DecodeConfig::default().with_peak_memory_limit(1_000);
    let result = avif_parse::read_avif_with_config(input, &config, &avif_parse::Unstoppable);

    match result {
        Err(avif_parse::Error::ResourceLimitExceeded(msg)) => {
            assert_eq!(msg, "peak memory limit exceeded");
        }
        Ok(_) => panic!("Expected peak memory limit error"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[test]
fn resource_limit_total_megapixels() {
    let input = &mut File::open(IMAGE_GRID_5X4).expect("Unknown file");
    let config = avif_parse::DecodeConfig::default().with_total_megapixels_limit(10);
    let result = avif_parse::read_avif_with_config(input, &config, &avif_parse::Unstoppable);

    match result {
        Err(avif_parse::Error::ResourceLimitExceeded(msg)) => {
            assert_eq!(msg, "total megapixels limit exceeded");
        }
        Ok(_) => panic!("Expected total megapixels limit error"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[test]
fn resource_limit_grid_tiles() {
    let input = &mut File::open(IMAGE_GRID_5X4).expect("Unknown file");
    let config = avif_parse::DecodeConfig::default().with_max_grid_tiles(10);
    let result = avif_parse::read_avif_with_config(input, &config, &avif_parse::Unstoppable);

    match result {
        Err(avif_parse::Error::ResourceLimitExceeded(msg)) => {
            assert_eq!(msg, "grid tile count limit exceeded");
        }
        Ok(_) => panic!("Expected grid tile count limit error"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[test]
fn resource_limit_animation_frames() {
    let input = &mut File::open(ANIMATED_AVIF).expect("Unknown file");
    let config = avif_parse::DecodeConfig::default().with_max_animation_frames(3);
    let result = avif_parse::read_avif_with_config(input, &config, &avif_parse::Unstoppable);

    match result {
        Err(avif_parse::Error::ResourceLimitExceeded(msg)) => {
            assert_eq!(msg, "animation frame count limit exceeded");
        }
        Ok(_) => panic!("Expected animation frame count limit error"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[test]
fn cancellation_during_parse() {
    struct ImmediatelyCancelled;
    impl avif_parse::Stop for ImmediatelyCancelled {
        fn check(&self) -> std::result::Result<(), avif_parse::StopReason> {
            Err(avif_parse::StopReason::Cancelled)
        }
    }

    let input = &mut File::open(IMAGE_AVIF).expect("Unknown file");
    let config = avif_parse::DecodeConfig::default();
    let result = avif_parse::read_avif_with_config(input, &config, &ImmediatelyCancelled);

    match result {
        Err(avif_parse::Error::Stopped(reason)) => {
            assert_eq!(reason, avif_parse::StopReason::Cancelled);
        }
        Ok(_) => panic!("Expected cancellation"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[test]
fn unstoppable_completes() {
    let input = &mut File::open(IMAGE_AVIF).expect("Unknown file");
    let config = avif_parse::DecodeConfig::default();
    let result = avif_parse::read_avif_with_config(input, &config, &avif_parse::Unstoppable);
    assert!(result.is_ok(), "Unstoppable should never cancel");
}

#[test]
fn decode_config_unlimited_backwards_compat() {
    let result_old = avif_parse::read_avif(&mut File::open(IMAGE_AVIF).expect("Unknown file"))
        .expect("read_avif failed");
    let config = avif_parse::DecodeConfig::unlimited();
    let result_new = avif_parse::read_avif_with_config(
        &mut File::open(IMAGE_AVIF).expect("Unknown file"),
        &config,
        &avif_parse::Unstoppable,
    )
    .expect("read_avif_with_config failed");

    assert_eq!(result_old.primary_item.len(), result_new.primary_item.len());
    assert_eq!(result_old.primary_item.as_slice(), result_new.primary_item.as_slice());
}

#[test]
fn decode_config_default_has_sane_limits() {
    let config = avif_parse::DecodeConfig::default();
    assert_eq!(config.peak_memory_limit, Some(1_000_000_000));
    assert_eq!(config.total_megapixels_limit, Some(512));
    assert_eq!(config.max_animation_frames, Some(10_000));
    assert_eq!(config.max_grid_tiles, Some(1_000));
    assert!(!config.lenient);
}

// Parser-specific resource limit tests

#[test]
fn parser_resource_limit_grid_tiles() {
    let bytes = std::fs::read(IMAGE_GRID_5X4).expect("read file");
    let config = avif_parse::DecodeConfig::default().with_max_grid_tiles(10);

    let result = avif_parse::AvifParser::from_bytes_with_config(
        &bytes,
        &config,
        &avif_parse::Unstoppable,
    );

    match result {
        Err(avif_parse::Error::ResourceLimitExceeded(msg)) => {
            assert_eq!(msg, "grid tile count limit exceeded");
        }
        Ok(_) => panic!("Expected grid tile count limit error"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[test]
fn parser_cancellation_during_parse() {
    struct ImmediatelyCancelled;
    impl avif_parse::Stop for ImmediatelyCancelled {
        fn check(&self) -> std::result::Result<(), avif_parse::StopReason> {
            Err(avif_parse::StopReason::Cancelled)
        }
    }

    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let config = avif_parse::DecodeConfig::default();
    let result = avif_parse::AvifParser::from_bytes_with_config(
        &bytes,
        &config,
        &ImmediatelyCancelled,
    );

    match result {
        Err(avif_parse::Error::Stopped(reason)) => {
            assert_eq!(reason, avif_parse::StopReason::Cancelled);
        }
        Ok(_) => panic!("Expected cancellation"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}
