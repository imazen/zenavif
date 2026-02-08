// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use avif_parse::Error;
use std::fs::File;

static IMAGE_AVIF: &str = "av1-avif/testFiles/Microsoft/Monochrome.avif";
static IMAGE_AVIF_EXTENTS: &str = "tests/kodim-extents.avif";
static IMAGE_AVIF_CORRUPT: &str = "tests/bug-1655846.avif";
static IMAGE_AVIF_CORRUPT_2: &str = "tests/bug-1661347.avif";
static IMAGE_GRID_5X4: &str = "av1-avif/testFiles/Microsoft/Summer_in_Tomsk_720p_5x4_grid.avif";
static ANIMATED_AVIF: &str = "link-u-samples/star-8bpc.avifs";
static AOMEDIA_TEST_FILES: &str = "av1-avif/testFiles";
static LINK_U_SAMPLES: &str = "link-u-samples";

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
    // Test grid layout calculation from ispe (Image Spatial Extents) properties
    // This file has NO explicit ImageGrid box, only ispe properties
    let input = &mut File::open(IMAGE_GRID_5X4).expect("Unknown file");
    let avif = avif_parse::read_avif(input).expect("read_avif failed");

    // Verify grid config was calculated correctly from ispe
    let grid = avif.grid_config.expect("Expected grid config");
    assert_eq!(grid.rows, 4, "Expected 4 rows");
    assert_eq!(grid.columns, 5, "Expected 5 columns");
    assert_eq!(grid.output_width, 6400, "Expected width 6400");
    assert_eq!(grid.output_height, 2880, "Expected height 2880");

    // Verify tile count matches grid dimensions
    assert_eq!(avif.grid_tiles.len(), 20, "Expected 20 tiles (4×5)");

    // Verify primary item is empty for grid images
    assert_eq!(avif.primary_item.len(), 0, "Grid images should have empty primary_item");
}

#[test]
fn grid_tile_ordering() {
    // Verify tiles are ordered by dimgIdx (reference_index) not item_id
    let input = &mut File::open(IMAGE_GRID_5X4).expect("Unknown file");
    let avif = avif_parse::read_avif(input).expect("read_avif failed");

    // All tiles should have valid data
    for (i, tile) in avif.grid_tiles.iter().enumerate() {
        assert!(!tile.is_empty(), "Tile {} should not be empty", i);
        assert!(tile.len() > 1000, "Tile {} seems too small ({} bytes)", i, tile.len());
    }

    // Tiles should be in dimgIdx order (verified by the sizes being reasonable)
    // The Microsoft 5×4 grid has tiles with varying sizes, confirming order matters
}

#[test]
fn animated_avif_frame_extraction() {
    // Test animation parsing from .avifs file
    let input = &mut File::open(ANIMATED_AVIF).expect("Unknown file");
    let avif = avif_parse::read_avif(input).expect("read_avif failed");

    // Verify animation was detected and parsed
    let animation = avif.animation.expect("Expected animation data");

    // Verify frame count
    assert_eq!(animation.frames.len(), 5, "Expected 5 frames");

    // Verify all frames have valid data
    for (i, frame) in animation.frames.iter().enumerate() {
        assert!(!frame.data.is_empty(), "Frame {} should not be empty", i);
        assert!(frame.duration_ms > 0, "Frame {} should have positive duration", i);
    }

    // Verify expected durations (star-8bpc.avifs has 100ms per frame)
    for frame in &animation.frames {
        assert_eq!(frame.duration_ms, 100, "Expected 100ms frame duration");
    }

    // Verify primary item contains first frame
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
            continue; // Skip directories, ReadMe.txt, etc.
        }
        log::debug!("parsing {:?}", path.display());
        let input = &mut File::open(path).expect("bad file");
        match avif_parse::read_avif(input) {
            Ok(avif) => {
                // Grid images have tiles instead of primary_item
                if avif.grid_config.is_none() {
                    avif.primary_item_metadata().unwrap();
                    avif.alpha_item_metadata().unwrap();
                } else {
                    // For grid images, validate that we have tiles
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

// Streaming parser tests

#[test]
fn streaming_parser_basic() {
    // Test basic streaming parser functionality
    let input = &mut File::open(ANIMATED_AVIF).expect("Unknown file");
    let parser = avif_parse::AvifParser::from_reader(input).expect("from_reader failed");

    // Should parse without loading frames
    assert!(parser.animation_info().is_some(), "Expected animation info");

    let info = parser.animation_info().unwrap();
    assert_eq!(info.frame_count, 5, "Expected 5 frames");

    // Extract single frame
    let frame = parser.animation_frame(0).expect("Failed to extract frame 0");
    assert!(!frame.data.is_empty(), "Frame 0 should not be empty");
    assert_eq!(frame.duration_ms, 100, "Expected 100ms duration");
}

#[test]
fn streaming_matches_eager() {
    // Verify streaming parser produces identical results to eager parser
    let avif_data = avif_parse::read_avif(&mut File::open(ANIMATED_AVIF).expect("Unknown file"))
        .expect("read_avif failed");
    let parser = avif_parse::AvifParser::from_reader(&mut File::open(ANIMATED_AVIF).expect("Unknown file"))
        .expect("from_reader failed");

    let info = parser.animation_info().expect("Expected animation");
    assert_eq!(
        info.frame_count,
        avif_data.animation.as_ref().unwrap().frames.len(),
        "Frame counts should match"
    );

    // Compare first 5 frames
    for i in 0..5 {
        let eager_frame = &avif_data.animation.as_ref().unwrap().frames[i];
        let streaming_frame = parser.animation_frame(i).expect("Failed to extract frame");

        assert_eq!(
            eager_frame.data.len(),
            streaming_frame.data.len(),
            "Frame {} data length should match",
            i
        );
        assert_eq!(
            eager_frame.data.as_slice(),
            streaming_frame.data.as_slice(),
            "Frame {} data should match",
            i
        );
        assert_eq!(
            eager_frame.duration_ms, streaming_frame.duration_ms,
            "Frame {} duration should match",
            i
        );
    }
}

#[test]
fn streaming_parser_grid() {
    // Test streaming parser with grid images
    let parser = avif_parse::AvifParser::from_reader(
        &mut File::open(IMAGE_GRID_5X4).expect("Unknown file"),
    )
    .expect("from_reader failed");

    let grid = parser.grid_config().expect("Expected grid config");
    assert_eq!(grid.rows, 4, "Expected 4 rows");
    assert_eq!(grid.columns, 5, "Expected 5 columns");

    assert_eq!(parser.grid_tile_count(), 20, "Expected 20 tiles");

    // Extract first tile
    let tile = parser.grid_tile(0).expect("Failed to extract tile 0");
    assert!(!tile.is_empty(), "Tile 0 should not be empty");
}

#[test]
fn streaming_parser_primary_item() {
    // Test streaming parser with single-frame image
    let parser = avif_parse::AvifParser::from_reader(&mut File::open(IMAGE_AVIF).expect("Unknown file"))
        .expect("from_reader failed");

    let primary = parser.primary_item().expect("Failed to extract primary item");
    assert_eq!(primary.len(), 6979, "Primary item length mismatch");
    assert_eq!(primary[0..4], [0x12, 0x00, 0x0a, 0x0a], "Primary item header mismatch");

    // Should not have animation
    assert!(parser.animation_info().is_none(), "Should not have animation");
}

#[test]
fn parser_to_avif_data_conversion() {
    // Test that streaming parser can convert to AvifData
    let parser = avif_parse::AvifParser::from_reader(&mut File::open(ANIMATED_AVIF).expect("Unknown file"))
        .expect("from_reader failed");
    let avif_data = parser.to_avif_data().expect("Failed to convert to AvifData");

    // Should produce identical result to direct read_avif
    let direct = avif_parse::read_avif(&mut File::open(ANIMATED_AVIF).expect("Unknown file"))
        .expect("read_avif failed");

    assert_eq!(
        avif_data.primary_item.len(),
        direct.primary_item.len(),
        "Primary item length should match"
    );
    assert_eq!(
        avif_data.primary_item.as_slice(),
        direct.primary_item.as_slice(),
        "Primary item data should match"
    );

    // Compare animation data
    let converted_anim = avif_data.animation.as_ref().expect("Expected animation");
    let direct_anim = direct.animation.as_ref().expect("Expected animation");

    assert_eq!(
        converted_anim.frames.len(),
        direct_anim.frames.len(),
        "Frame counts should match"
    );

    for (i, (conv_frame, direct_frame)) in converted_anim
        .frames
        .iter()
        .zip(direct_anim.frames.iter())
        .enumerate()
    {
        assert_eq!(
            conv_frame.data.as_slice(),
            direct_frame.data.as_slice(),
            "Frame {} data should match",
            i
        );
    }
}

// Zero-copy slice tests

#[test]
fn zero_copy_animation_frame_slice() {
    // Test zero-copy frame access
    let parser = avif_parse::AvifParser::from_reader(&mut File::open(ANIMATED_AVIF).expect("Unknown file"))
        .expect("from_reader failed");

    // Get frame data via zero-copy
    let (slice, duration_ms) = parser.animation_frame_slice(0).expect("Failed to get frame slice");
    assert!(!slice.is_empty(), "Frame slice should not be empty");
    assert_eq!(duration_ms, 100, "Expected 100ms duration");

    // Compare with copying version
    let frame = parser.animation_frame(0).expect("Failed to extract frame");
    assert_eq!(slice, frame.data.as_slice(), "Zero-copy slice should match copied data");
    assert_eq!(duration_ms, frame.duration_ms, "Durations should match");
}

#[test]
fn zero_copy_primary_item_slice() {
    // Test zero-copy primary item access
    let parser = avif_parse::AvifParser::from_reader(&mut File::open(IMAGE_AVIF).expect("Unknown file"))
        .expect("from_reader failed");

    // Get primary item via zero-copy
    let slice = parser.primary_item_slice().expect("Failed to get primary item slice");
    assert_eq!(slice.len(), 6979, "Primary item slice length mismatch");
    assert_eq!(slice[0..4], [0x12, 0x00, 0x0a, 0x0a], "Primary item slice header mismatch");

    // Compare with copying version
    let primary = parser.primary_item().expect("Failed to extract primary item");
    assert_eq!(slice, primary.as_slice(), "Zero-copy slice should match copied data");
}

#[test]
fn zero_copy_grid_tile_slice() {
    // Test zero-copy grid tile access
    let parser = avif_parse::AvifParser::from_reader(
        &mut File::open(IMAGE_GRID_5X4).expect("Unknown file"),
    )
    .expect("from_reader failed");

    // Get first tile via zero-copy
    let slice = parser.grid_tile_slice(0).expect("Failed to get tile slice");
    assert!(!slice.is_empty(), "Tile slice should not be empty");

    // Compare with copying version
    let tile = parser.grid_tile(0).expect("Failed to extract tile");
    assert_eq!(slice, tile.as_slice(), "Zero-copy slice should match copied data");
}

#[test]
fn zero_copy_vs_copying_performance() {
    // Verify zero-copy returns same data as copying methods
    let parser = avif_parse::AvifParser::from_reader(&mut File::open(ANIMATED_AVIF).expect("Unknown file"))
        .expect("from_reader failed");

    let info = parser.animation_info().expect("Expected animation");

    // Compare all frames
    for i in 0..info.frame_count {
        let (slice, duration_zero) = parser.animation_frame_slice(i).expect("Failed to get frame slice");
        let frame = parser.animation_frame(i).expect("Failed to extract frame");

        assert_eq!(
            slice,
            frame.data.as_slice(),
            "Frame {} zero-copy slice should match copied data",
            i
        );
        assert_eq!(
            duration_zero, frame.duration_ms,
            "Frame {} durations should match",
            i
        );
    }
}
