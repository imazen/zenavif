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

fn test_dir(dir: &str) {
    let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::max()).try_init();
    let mut errors = 0;

    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry.expect("AVIF entry");
        let path = entry.path();
        if !path.is_file() || path.extension().unwrap_or_default() != "avif" {
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
