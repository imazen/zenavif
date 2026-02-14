// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
#![allow(deprecated)]
use std::borrow::Cow;
use std::fs::File;

static IMAGE_AVIF: &str = "av1-avif/testFiles/Microsoft/Monochrome.avif";
static IMAGE_AVIF_EXTENTS: &str = "tests/kodim-extents.avif";
#[cfg(feature = "eager")]
static IMAGE_AVIF_CORRUPT: &str = "tests/bug-1655846.avif";
#[cfg(feature = "eager")]
static IMAGE_AVIF_CORRUPT_2: &str = "tests/bug-1661347.avif";
static IMAGE_GRID_5X4: &str = "av1-avif/testFiles/Microsoft/Summer_in_Tomsk_720p_5x4_grid.avif";
static ANIMATED_AVIF: &str = "link-u-samples/star-8bpc.avifs";
static IMAGE_AVIF_ALPHA: &str = "av1-avif/testFiles/Microsoft/bbb_alpha_inverted.avif";
#[cfg(feature = "eager")]
static AOMEDIA_TEST_FILES: &str = "av1-avif/testFiles";
#[cfg(feature = "eager")]
static LINK_U_SAMPLES: &str = "link-u-samples";

// ============================================================================
// Eager path (read_avif) tests
// ============================================================================

#[cfg(feature = "eager")]
#[test]
fn public_avif_primary_item() {
    let input = &mut File::open(IMAGE_AVIF).expect("Unknown file");
    let context = zenavif_parse::read_avif(input).expect("read_avif failed");
    assert_eq!(context.primary_item.len(), 6979);
    assert_eq!(context.primary_item[0..4], [0x12, 0x00, 0x0a, 0x0a]);
}

#[cfg(feature = "eager")]
#[test]
fn public_avif_primary_item_split_extents() {
    let input = &mut File::open(IMAGE_AVIF_EXTENTS).expect("Unknown file");
    let context = zenavif_parse::read_avif(input).expect("read_avif failed");
    assert_eq!(context.primary_item.len(), 4387);
}

#[cfg(feature = "eager")]
#[test]
fn public_avif_bug_1655846() {
    let input = &mut File::open(IMAGE_AVIF_CORRUPT).expect("Unknown file");
    assert!(zenavif_parse::read_avif(input).is_err());
}

#[cfg(feature = "eager")]
#[test]
fn public_avif_bug_1661347() {
    let input = &mut File::open(IMAGE_AVIF_CORRUPT_2).expect("Unknown file");
    assert!(zenavif_parse::read_avif(input).is_err());
}

#[cfg(feature = "eager")]
#[test]
fn aomedia_sample_images() {
    test_dir(AOMEDIA_TEST_FILES);
}

#[cfg(feature = "eager")]
#[test]
fn linku_sample_images() {
    test_dir(LINK_U_SAMPLES);
}

#[cfg(feature = "eager")]
#[test]
fn grid_5x4_ispe_calculation() {
    let input = &mut File::open(IMAGE_GRID_5X4).expect("Unknown file");
    let avif = zenavif_parse::read_avif(input).expect("read_avif failed");

    let grid = avif.grid_config.expect("Expected grid config");
    assert_eq!(grid.rows, 4, "Expected 4 rows");
    assert_eq!(grid.columns, 5, "Expected 5 columns");
    assert_eq!(grid.output_width, 6400, "Expected width 6400");
    assert_eq!(grid.output_height, 2880, "Expected height 2880");
    assert_eq!(avif.grid_tiles.len(), 20, "Expected 20 tiles (4x5)");
    assert_eq!(avif.primary_item.len(), 0, "Grid images should have empty primary_item");
}

#[cfg(feature = "eager")]
#[test]
fn grid_tile_ordering() {
    let input = &mut File::open(IMAGE_GRID_5X4).expect("Unknown file");
    let avif = zenavif_parse::read_avif(input).expect("read_avif failed");

    for (i, tile) in avif.grid_tiles.iter().enumerate() {
        assert!(!tile.is_empty(), "Tile {} should not be empty", i);
        assert!(tile.len() > 1000, "Tile {} seems too small ({} bytes)", i, tile.len());
    }
}

#[cfg(feature = "eager")]
#[test]
fn animated_avif_frame_extraction() {
    let input = &mut File::open(ANIMATED_AVIF).expect("Unknown file");
    let avif = zenavif_parse::read_avif(input).expect("read_avif failed");

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

#[cfg(feature = "eager")]
fn test_dir(dir: &str) {
    use zenavif_parse::Error;
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
        match zenavif_parse::read_avif(input) {
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
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let primary = parser.primary_data().expect("primary_data failed");
    assert_eq!(primary.len(), 6979);
    assert_eq!(&primary[0..4], &[0x12, 0x00, 0x0a, 0x0a]);

    // Single-extent -> must be Cow::Borrowed (true zero-copy)
    assert!(matches!(primary, Cow::Borrowed(_)), "Expected Cow::Borrowed for single-extent");
    assert!(parser.animation_info().is_none());
}

#[test]
fn parser_from_bytes_multi_extent() {
    let bytes = std::fs::read(IMAGE_AVIF_EXTENTS).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let primary = parser.primary_data().expect("primary_data failed");
    assert_eq!(primary.len(), 4387);

    // Multi-extent -> Cow::Owned
    assert!(matches!(primary, Cow::Owned(_)), "Expected Cow::Owned for multi-extent");
}

#[test]
fn parser_from_owned_primary() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_owned(bytes).expect("from_owned failed");

    let primary = parser.primary_data().expect("primary_data failed");
    assert_eq!(primary.len(), 6979);
    assert_eq!(&primary[0..4], &[0x12, 0x00, 0x0a, 0x0a]);
}

#[test]
fn parser_from_reader_primary() {
    let parser = zenavif_parse::AvifParser::from_reader(
        &mut File::open(IMAGE_AVIF).expect("Unknown file"),
    ).expect("from_reader failed");

    let primary = parser.primary_data().expect("primary_data failed");
    assert_eq!(primary.len(), 6979);
    assert_eq!(&primary[0..4], &[0x12, 0x00, 0x0a, 0x0a]);
}

#[test]
fn parser_from_owned_with_config() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let config = zenavif_parse::DecodeConfig::default();
    let parser = zenavif_parse::AvifParser::from_owned_with_config(
        bytes, &config, &zenavif_parse::Unstoppable,
    ).expect("from_owned_with_config failed");

    let primary = parser.primary_data().expect("primary_data failed");
    assert_eq!(primary.len(), 6979);
}

#[test]
fn parser_from_reader_with_config() {
    let config = zenavif_parse::DecodeConfig::default();
    let parser = zenavif_parse::AvifParser::from_reader_with_config(
        &mut File::open(IMAGE_AVIF).expect("Unknown file"),
        &config,
        &zenavif_parse::Unstoppable,
    ).expect("from_reader_with_config failed");

    let primary = parser.primary_data().expect("primary_data failed");
    assert_eq!(primary.len(), 6979);
}

#[test]
fn parser_from_bytes_with_config_happy_path() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let config = zenavif_parse::DecodeConfig::default();
    let parser = zenavif_parse::AvifParser::from_bytes_with_config(
        &bytes, &config, &zenavif_parse::Unstoppable,
    ).expect("from_bytes_with_config failed");

    let primary = parser.primary_data().expect("primary_data failed");
    assert_eq!(primary.len(), 6979);
}

#[test]
fn parser_grid() {
    let bytes = std::fs::read(IMAGE_GRID_5X4).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let grid = parser.grid_config().expect("Expected grid config");
    assert_eq!(grid.rows, 4);
    assert_eq!(grid.columns, 5);
    assert_eq!(grid.output_width, 6400);
    assert_eq!(grid.output_height, 2880);
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
fn parser_grid_via_reader() {
    let config = zenavif_parse::DecodeConfig::default();
    let parser = zenavif_parse::AvifParser::from_reader_with_config(
        &mut File::open(IMAGE_GRID_5X4).expect("Unknown file"),
        &config,
        &zenavif_parse::Unstoppable,
    ).expect("from_reader_with_config failed");

    let grid = parser.grid_config().expect("Expected grid config");
    assert_eq!(grid.rows, 4);
    assert_eq!(grid.columns, 5);
    assert_eq!(parser.grid_tile_count(), 20);

    for i in 0..20 {
        let t = parser.tile_data(i).expect("tile_data failed");
        assert!(!t.is_empty(), "Tile {} empty", i);
    }
}

#[test]
fn parser_animation_frames() {
    let bytes = std::fs::read(ANIMATED_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let info = parser.animation_info().expect("Expected animation");
    assert_eq!(info.frame_count, 5);
    assert_eq!(info.loop_count, 1); // elst flags=0 → play once

    for i in 0..5 {
        let frame = parser.frame(i).expect("frame failed");
        assert!(!frame.data.is_empty(), "Frame {} empty", i);
        assert_eq!(frame.duration_ms, 100);

        // from_bytes -> single-extent frames should be Cow::Borrowed
        assert!(matches!(frame.data, Cow::Borrowed(_)), "Frame {} should be Cow::Borrowed", i);
    }

    assert!(parser.frame(5).is_err());
}

#[test]
fn parser_animation_via_reader() {
    let parser = zenavif_parse::AvifParser::from_reader(
        &mut File::open(ANIMATED_AVIF).expect("Unknown file"),
    ).expect("from_reader failed");

    let info = parser.animation_info().expect("Expected animation");
    assert_eq!(info.frame_count, 5);

    for i in 0..5 {
        let frame = parser.frame(i).expect("frame failed");
        assert!(!frame.data.is_empty(), "Frame {} empty", i);
        assert_eq!(frame.duration_ms, 100);
    }
}

#[test]
fn parser_frames_iterator() {
    let bytes = std::fs::read(ANIMATED_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let frames: Vec<_> = parser.frames().collect();
    assert_eq!(frames.len(), 5);

    for (i, result) in frames.iter().enumerate() {
        let frame = result.as_ref().expect("frame failed");
        assert!(!frame.data.is_empty(), "Frame {} empty", i);
        assert_eq!(frame.duration_ms, 100);
    }
}

#[test]
fn parser_frames_iterator_on_still_image() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let frames: Vec<_> = parser.frames().collect();
    assert_eq!(frames.len(), 0, "Still image should yield no frames");
}

#[test]
fn parser_frame_out_of_range_on_still_image() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    assert!(parser.frame(0).is_err(), "Still image has no frames");
}

#[test]
fn parser_metadata() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let meta = parser.primary_metadata().expect("primary_metadata failed");
    assert!(meta.monochrome); // Monochrome.avif
    assert!(meta.still_picture);
    assert_eq!(meta.bit_depth, 8);
    assert_eq!(meta.seq_profile, 0);

    assert!(parser.alpha_metadata().is_none());
}

#[test]
fn parser_av1_config() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let av1c = parser.av1_config().expect("av1C should be present");
    assert_eq!(av1c.profile, 0); // Main profile
    assert_eq!(av1c.bit_depth, 8);
    assert!(av1c.monochrome); // Monochrome.avif
}

#[test]
fn parser_av1_config_alpha_file() {
    let bytes = std::fs::read(IMAGE_AVIF_ALPHA).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let av1c = parser.av1_config().expect("av1C should be present");
    assert_eq!(av1c.bit_depth, 8);
    assert!(!av1c.monochrome);
}

#[test]
fn parser_color_info() {
    // Test colr parsing on a file that has one. The Microsoft test files
    // may or may not have colr boxes, so we test parsing on what's available.
    let bytes = std::fs::read(IMAGE_AVIF_ALPHA).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    // color_info may be None for files without colr — that's fine.
    // Just verify the accessor doesn't panic.
    let _color = parser.color_info();
}

#[cfg(feature = "eager")]
#[test]
fn eager_av1_config() {
    let input = &mut std::fs::File::open(IMAGE_AVIF).expect("Unknown file");
    let context = zenavif_parse::read_avif(input).expect("read_avif failed");

    let av1c = context.av1_config.expect("av1C should be present");
    assert_eq!(av1c.profile, 0);
    assert_eq!(av1c.bit_depth, 8);
    assert!(av1c.monochrome);
}

// ============================================================================
// Transform / display property tests
// ============================================================================

#[test]
fn parser_rotation_90() {
    let bytes = std::fs::read("av1-avif/testFiles/Link-U/kimono.rotate90.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let irot = parser.rotation().expect("irot should be present");
    // File has angle_code=3, which is 270° CCW
    assert_eq!(irot.angle, 270);
}

#[test]
fn parser_rotation_270() {
    let bytes = std::fs::read("av1-avif/testFiles/Link-U/kimono.rotate270.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let irot = parser.rotation().expect("irot should be present");
    // File has angle_code=1, which is 90° CCW
    assert_eq!(irot.angle, 90);
}

#[test]
fn parser_mirror_horizontal() {
    let bytes = std::fs::read("av1-avif/testFiles/Link-U/kimono.mirror-horizontal.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let imir = parser.mirror().expect("imir should be present");
    assert_eq!(imir.axis, 1);
    assert!(parser.rotation().is_none());
}

#[test]
fn parser_mirror_vertical() {
    let bytes = std::fs::read("av1-avif/testFiles/Link-U/kimono.mirror-vertical.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let imir = parser.mirror().expect("imir should be present");
    assert_eq!(imir.axis, 0);
    assert!(parser.rotation().is_none());
}

#[test]
fn parser_clean_aperture() {
    let bytes = std::fs::read("av1-avif/testFiles/Link-U/kimono.crop.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let clap = parser.clean_aperture().expect("clap should be present");
    assert_eq!(clap.width_n, 385);
    assert_eq!(clap.width_d, 1);
    assert_eq!(clap.height_n, 330);
    assert_eq!(clap.height_d, 1);
    assert_eq!(clap.horiz_off_n, 207);
    assert_eq!(clap.horiz_off_d, 2);
    assert_eq!(clap.vert_off_n, -616);
    assert_eq!(clap.vert_off_d, 2);
}

#[test]
fn parser_pixel_aspect_ratio() {
    let bytes = std::fs::read("av1-avif/testFiles/Link-U/kimono.crop.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let pasp = parser.pixel_aspect_ratio().expect("pasp should be present");
    assert_eq!(pasp.h_spacing, 1);
    assert_eq!(pasp.v_spacing, 1);
}

#[test]
fn parser_combined_transforms() {
    // This file has irot + imir + clap + pasp all together
    let bytes = std::fs::read("av1-avif/testFiles/Link-U/kimono.mirror-vertical.rotate270.crop.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let irot = parser.rotation().expect("irot should be present");
    assert_eq!(irot.angle, 90); // angle_code=1

    let imir = parser.mirror().expect("imir should be present");
    assert_eq!(imir.axis, 0);

    let clap = parser.clean_aperture().expect("clap should be present");
    assert_eq!(clap.width_n, 330);
    assert_eq!(clap.width_d, 1);
    assert_eq!(clap.height_n, 385);
    assert_eq!(clap.height_d, 1);

    assert!(parser.pixel_aspect_ratio().is_some());
}

#[test]
fn parser_hdr_metadata() {
    let bytes = std::fs::read("av1-avif/testFiles/Microsoft/Chimera_10bit_cropped_to_1920x1008_with_HDR_metadata.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let clli = parser.content_light_level().expect("clli should be present");
    assert_eq!(clli.max_content_light_level, 2000);
    assert_eq!(clli.max_pic_average_light_level, 1500);

    let mdcv = parser.mastering_display().expect("mdcv should be present");
    // Green, Blue, Red primaries (SMPTE ST 2086 order)
    assert_eq!(mdcv.primaries[0], (15000, 20000));
    assert_eq!(mdcv.primaries[1], (25000, 30000));
    assert_eq!(mdcv.primaries[2], (5000, 10000));
    assert_eq!(mdcv.white_point, (35000, 40000));
    assert_eq!(mdcv.max_luminance, 100_000_000);
    assert_eq!(mdcv.min_luminance, 200_000);

    // This file also has clap
    let clap = parser.clean_aperture().expect("clap should be present");
    assert_eq!(clap.width_n, 1920);
    assert_eq!(clap.height_n, 1008);
}

#[test]
fn parser_no_transforms_on_simple_image() {
    // Monochrome.avif has no transform properties
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    assert!(parser.rotation().is_none());
    assert!(parser.mirror().is_none());
    assert!(parser.clean_aperture().is_none());
    assert!(parser.pixel_aspect_ratio().is_none());
    assert!(parser.content_light_level().is_none());
    assert!(parser.mastering_display().is_none());
}

#[cfg(feature = "eager")]
#[test]
fn eager_transforms() {
    let input = &mut std::fs::File::open("av1-avif/testFiles/Link-U/kimono.mirror-vertical.rotate270.crop.avif").expect("Unknown file");
    let context = zenavif_parse::read_avif(input).expect("read_avif failed");

    let irot = context.rotation.expect("irot should be present");
    assert_eq!(irot.angle, 90);

    let imir = context.mirror.expect("imir should be present");
    assert_eq!(imir.axis, 0);

    assert!(context.clean_aperture.is_some());
    assert!(context.pixel_aspect_ratio.is_some());
}

#[cfg(feature = "eager")]
#[test]
fn eager_hdr_metadata() {
    let input = &mut std::fs::File::open("av1-avif/testFiles/Microsoft/Chimera_10bit_cropped_to_1920x1008_with_HDR_metadata.avif").expect("Unknown file");
    let context = zenavif_parse::read_avif(input).expect("read_avif failed");

    let clli = context.content_light_level.expect("clli should be present");
    assert_eq!(clli.max_content_light_level, 2000);
    assert_eq!(clli.max_pic_average_light_level, 1500);

    let mdcv = context.mastering_display.expect("mdcv should be present");
    assert_eq!(mdcv.max_luminance, 100_000_000);
    assert_eq!(mdcv.min_luminance, 200_000);
}

// ============================================================================
// Layered image property tests
// ============================================================================

#[test]
fn parser_operating_point_selector() {
    let bytes = std::fs::read("av1-avif/testFiles/Xiph/quebec_3layer_op2.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let a1op = parser.operating_point().expect("a1op should be present");
    assert_eq!(a1op.op_index, 2);
}

#[test]
fn parser_layer_selector() {
    // quebec_3layer_op2 has lsel on the primary item
    let bytes = std::fs::read("av1-avif/testFiles/Xiph/quebec_3layer_op2.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let lsel = parser.layer_selector().expect("lsel should be present");
    assert_eq!(lsel.layer_id, 0xFFFF); // progressive (all layers)
}

#[test]
fn parser_a1op_and_lsel_on_primary() {
    // Apple a1op_lsel file has both on the primary item
    let bytes = std::fs::read("av1-avif/testFiles/Apple/multilayer_examples/animals_00_multilayer_a1op_lsel.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    let a1op = parser.operating_point().expect("a1op should be present");
    assert_eq!(a1op.op_index, 0);

    let lsel = parser.layer_selector().expect("lsel should be present");
    assert_eq!(lsel.layer_id, 1);
}

#[test]
fn parser_a1lx_on_grid_tiles() {
    // In grid_a1lx file, a1lx and lsel are on tile items (not primary).
    // Primary item (grid) should NOT have a1lx, but the file should parse without errors.
    let bytes = std::fs::read("av1-avif/testFiles/Apple/multilayer_examples/animals_00_multilayer_grid_a1lx.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    // a1lx is on tiles, not primary
    assert!(parser.layered_image_indexing().is_none());
    // lsel is also on tiles in this file
    assert!(parser.layer_selector().is_none());
}

// ============================================================================
// Brand / ftyp tests
// ============================================================================

#[test]
fn parser_brands_avif() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    assert_eq!(parser.major_brand(), b"avif");
    let compat = parser.compatible_brands();
    assert!(compat.iter().any(|b| b == b"miaf"), "should have miaf brand");
    assert!(compat.iter().any(|b| b == b"avif"), "should have avif brand");
}

#[test]
fn parser_brands_avis() {
    let bytes = std::fs::read(ANIMATED_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    assert_eq!(parser.major_brand(), b"avis");
    let compat = parser.compatible_brands();
    assert!(compat.iter().any(|b| b == b"avis"), "should have avis brand");
    assert!(compat.iter().any(|b| b == b"miaf"), "should have miaf brand");
    assert!(compat.iter().any(|b| b == b"MA1B"), "should have MA1B brand");
}

#[cfg(feature = "eager")]
#[test]
fn eager_brands() {
    let input = &mut std::fs::File::open(IMAGE_AVIF).expect("Unknown file");
    let context = zenavif_parse::read_avif(input).expect("read_avif failed");

    assert_eq!(context.major_brand, *b"avif");
    assert!(context.compatible_brands.iter().any(|b| b == b"miaf"));
}

#[cfg(feature = "eager")]
#[test]
fn eager_layered_properties() {
    let input = &mut std::fs::File::open("av1-avif/testFiles/Xiph/quebec_3layer_op2.avif").expect("Unknown file");
    let context = zenavif_parse::read_avif(input).expect("read_avif failed");

    let a1op = context.operating_point.expect("a1op should be present");
    assert_eq!(a1op.op_index, 2);
}

#[test]
fn parser_alpha_data() {
    let bytes = std::fs::read(IMAGE_AVIF_ALPHA).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    // Primary channel
    let primary = parser.primary_data().expect("primary_data failed");
    assert!(!primary.is_empty());

    // Alpha channel
    let alpha = parser.alpha_data().expect("image should have alpha");
    let alpha = alpha.expect("alpha_data failed");
    assert!(!alpha.is_empty());

    // Alpha metadata
    let alpha_meta = parser.alpha_metadata().expect("Expected alpha metadata");
    let alpha_meta = alpha_meta.expect("alpha_metadata failed");
    assert!(alpha_meta.monochrome, "Alpha should be monochrome");
    assert_eq!(alpha_meta.bit_depth, 8);

    // Primary metadata
    let primary_meta = parser.primary_metadata().expect("primary_metadata failed");
    assert_eq!(primary_meta.max_frame_width, alpha_meta.max_frame_width,
        "Primary and alpha should have same width");
    assert_eq!(primary_meta.max_frame_height, alpha_meta.max_frame_height,
        "Primary and alpha should have same height");
}

#[test]
fn parser_premultiplied_alpha_false() {
    // bbb_alpha_inverted.avif has alpha but is not premultiplied
    let bytes = std::fs::read(IMAGE_AVIF_ALPHA).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    assert!(!parser.premultiplied_alpha());
}

#[test]
fn parser_no_alpha_on_monochrome() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    assert!(parser.alpha_data().is_none(), "Monochrome.avif has no alpha");
    assert!(parser.alpha_metadata().is_none());
    assert!(!parser.premultiplied_alpha());
}

#[test]
fn parser_no_grid_on_single_image() {
    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");

    assert!(parser.grid_config().is_none());
    assert_eq!(parser.grid_tile_count(), 0);
    assert!(parser.tile_data(0).is_err());
}

#[test]
fn parser_corrupt_files_rejected() {
    for path in &["tests/bug-1655846.avif", "tests/bug-1661347.avif"] {
        let bytes = std::fs::read(path).expect("read file");
        match zenavif_parse::AvifParser::from_bytes(&bytes) {
            Err(_) => {} // rejected at parse time
            Ok(parser) => {
                // AvifParser defers data extraction, so corrupt extents
                // may only fail when accessing the data
                assert!(
                    parser.primary_data().is_err(),
                    "{} should fail to extract data", path,
                );
            }
        }
    }
}

// ============================================================================
// Corpus-wide parser-only tests
// ============================================================================

fn test_dir_parser(dir: &str) {
    let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::max()).try_init();
    let config = zenavif_parse::DecodeConfig::default();

    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry.expect("AVIF entry");
        let path = entry.path();
        let ext = path.extension().unwrap_or_default();
        if !path.is_file() || (ext != "avif" && ext != "avifs") {
            continue;
        }

        let file_bytes = std::fs::read(path).expect("read file");
        let parser_result = zenavif_parse::AvifParser::from_bytes_with_config(
            &file_bytes, &config, &zenavif_parse::Unstoppable,
        );

        match parser_result {
            Ok(parser) => {
                if parser.grid_config().is_some() {
                    assert!(parser.grid_tile_count() > 0, "{}: grid has no tiles", path.display());
                    for i in 0..parser.grid_tile_count() {
                        let tile = parser.tile_data(i).expect("tile_data failed");
                        assert!(!tile.is_empty(), "{}: tile {} empty", path.display(), i);
                    }
                } else if parser.animation_info().is_some() {
                    let info = parser.animation_info().unwrap();
                    for i in 0..info.frame_count {
                        let frame = parser.frame(i).expect("frame failed");
                        assert!(!frame.data.is_empty(), "{}: frame {} empty", path.display(), i);
                    }
                } else {
                    match parser.primary_data() {
                        Ok(primary) => {
                            assert!(!primary.is_empty(), "{}: primary_data empty", path.display());
                            if let Err(e) = parser.primary_metadata() {
                                // Stub AV1 data (e.g. upstream test fixtures) may not
                                // contain a valid sequence header
                                log::warn!("{}: primary_metadata failed: {e}", path.display());
                            }
                        }
                        Err(e) => {
                            // AvifParser defers extent validation to data access;
                            // corrupt files may parse metadata but fail on extraction
                            log::warn!("{}: primary_data failed: {e}", path.display());
                        }
                    }
                }
            }
            Err(err) => {
                // Parse errors are expected for malformed/stub files —
                // this test just verifies we don't panic
                log::warn!("{}: {err}", path.display());
            }
        }
    }
}

#[test]
fn corpus_aomedia_parser() {
    test_dir_parser("av1-avif/testFiles");
}

#[test]
fn corpus_linku_parser() {
    test_dir_parser("link-u-samples");
}

#[test]
fn corpus_local_parser() {
    test_dir_parser("tests");
}

// ============================================================================
// Cross-path tests (eager <-> parser)
// ============================================================================

#[cfg(feature = "eager")]
#[test]
fn parser_to_avif_data_matches_eager() {
    let bytes = std::fs::read(ANIMATED_AVIF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");
    let converted = parser.to_avif_data().expect("to_avif_data failed");

    let direct = zenavif_parse::read_avif(&mut File::open(ANIMATED_AVIF).expect("file"))
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

#[cfg(feature = "eager")]
#[test]
fn parser_to_avif_data_grid() {
    let bytes = std::fs::read(IMAGE_GRID_5X4).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("from_bytes failed");
    let converted = parser.to_avif_data().expect("to_avif_data failed");

    let direct = zenavif_parse::read_avif(&mut File::open(IMAGE_GRID_5X4).expect("file"))
        .expect("read_avif failed");

    assert_eq!(converted.grid_tiles.len(), direct.grid_tiles.len());
    for (i, (c, d)) in converted.grid_tiles.iter().zip(direct.grid_tiles.iter()).enumerate() {
        assert_eq!(c.as_slice(), d.as_slice(), "Tile {} data mismatch", i);
    }
}

// ============================================================================
// Corpus-wide tests: all parsing paths (eager + parser)
// ============================================================================

#[cfg(feature = "eager")]
fn test_dir_all_paths(dir: &str) {
    use zenavif_parse::Error;
    let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::max()).try_init();
    let config = zenavif_parse::DecodeConfig::default();
    let mut errors = 0;

    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry.expect("AVIF entry");
        let path = entry.path();
        let ext = path.extension().unwrap_or_default();
        if !path.is_file() || (ext != "avif" && ext != "avifs") {
            continue;
        }

        // Path 1: eager
        let eager_result = zenavif_parse::read_avif_with_config(
            &mut File::open(path).expect("bad file"),
            &config,
            &zenavif_parse::Unstoppable,
        );

        // Path 2: zero-copy from_bytes
        let file_bytes = std::fs::read(path).expect("read file");
        let parser_result = zenavif_parse::AvifParser::from_bytes_with_config(
            &file_bytes,
            &config,
            &zenavif_parse::Unstoppable,
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

#[cfg(feature = "eager")]
#[test]
fn corpus_aomedia_all_paths() {
    test_dir_all_paths("av1-avif/testFiles");
}

#[cfg(feature = "eager")]
#[test]
fn corpus_linku_all_paths() {
    test_dir_all_paths("link-u-samples");
}

#[cfg(feature = "eager")]
#[test]
fn corpus_local_all_paths() {
    test_dir_all_paths("tests");
}

// ============================================================================
// Resource Limit Tests (eager)
// ============================================================================

#[cfg(feature = "eager")]
#[test]
fn resource_limit_peak_memory() {
    let input = &mut File::open(IMAGE_AVIF).expect("Unknown file");
    let config = zenavif_parse::DecodeConfig::default().with_peak_memory_limit(1_000);
    let result = zenavif_parse::read_avif_with_config(input, &config, &zenavif_parse::Unstoppable);

    match result {
        Err(zenavif_parse::Error::ResourceLimitExceeded(msg)) => {
            assert_eq!(msg, "peak memory limit exceeded");
        }
        Ok(_) => panic!("Expected peak memory limit error"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[cfg(feature = "eager")]
#[test]
fn resource_limit_total_megapixels() {
    let input = &mut File::open(IMAGE_GRID_5X4).expect("Unknown file");
    let config = zenavif_parse::DecodeConfig::default().with_total_megapixels_limit(10);
    let result = zenavif_parse::read_avif_with_config(input, &config, &zenavif_parse::Unstoppable);

    match result {
        Err(zenavif_parse::Error::ResourceLimitExceeded(msg)) => {
            assert_eq!(msg, "total megapixels limit exceeded");
        }
        Ok(_) => panic!("Expected total megapixels limit error"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[cfg(feature = "eager")]
#[test]
fn resource_limit_grid_tiles() {
    let input = &mut File::open(IMAGE_GRID_5X4).expect("Unknown file");
    let config = zenavif_parse::DecodeConfig::default().with_max_grid_tiles(10);
    let result = zenavif_parse::read_avif_with_config(input, &config, &zenavif_parse::Unstoppable);

    match result {
        Err(zenavif_parse::Error::ResourceLimitExceeded(msg)) => {
            assert_eq!(msg, "grid tile count limit exceeded");
        }
        Ok(_) => panic!("Expected grid tile count limit error"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[cfg(feature = "eager")]
#[test]
fn resource_limit_animation_frames() {
    let input = &mut File::open(ANIMATED_AVIF).expect("Unknown file");
    let config = zenavif_parse::DecodeConfig::default().with_max_animation_frames(3);
    let result = zenavif_parse::read_avif_with_config(input, &config, &zenavif_parse::Unstoppable);

    match result {
        Err(zenavif_parse::Error::ResourceLimitExceeded(msg)) => {
            assert_eq!(msg, "animation frame count limit exceeded");
        }
        Ok(_) => panic!("Expected animation frame count limit error"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[cfg(feature = "eager")]
#[test]
fn cancellation_during_parse() {
    struct ImmediatelyCancelled;
    impl zenavif_parse::Stop for ImmediatelyCancelled {
        fn check(&self) -> std::result::Result<(), zenavif_parse::StopReason> {
            Err(zenavif_parse::StopReason::Cancelled)
        }
    }

    let input = &mut File::open(IMAGE_AVIF).expect("Unknown file");
    let config = zenavif_parse::DecodeConfig::default();
    let result = zenavif_parse::read_avif_with_config(input, &config, &ImmediatelyCancelled);

    match result {
        Err(zenavif_parse::Error::Stopped(reason)) => {
            assert_eq!(reason, zenavif_parse::StopReason::Cancelled);
        }
        Ok(_) => panic!("Expected cancellation"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[cfg(feature = "eager")]
#[test]
fn unstoppable_completes() {
    let input = &mut File::open(IMAGE_AVIF).expect("Unknown file");
    let config = zenavif_parse::DecodeConfig::default();
    let result = zenavif_parse::read_avif_with_config(input, &config, &zenavif_parse::Unstoppable);
    assert!(result.is_ok(), "Unstoppable should never cancel");
}

#[cfg(feature = "eager")]
#[test]
fn decode_config_unlimited_backwards_compat() {
    let result_old = zenavif_parse::read_avif(&mut File::open(IMAGE_AVIF).expect("Unknown file"))
        .expect("read_avif failed");
    let config = zenavif_parse::DecodeConfig::unlimited();
    let result_new = zenavif_parse::read_avif_with_config(
        &mut File::open(IMAGE_AVIF).expect("Unknown file"),
        &config,
        &zenavif_parse::Unstoppable,
    )
    .expect("read_avif_with_config failed");

    assert_eq!(result_old.primary_item.len(), result_new.primary_item.len());
    assert_eq!(result_old.primary_item.as_slice(), result_new.primary_item.as_slice());
}

// ============================================================================
// DecodeConfig / resource limit tests (no eager needed)
// ============================================================================

#[test]
fn decode_config_default_has_sane_limits() {
    let config = zenavif_parse::DecodeConfig::default();
    assert_eq!(config.peak_memory_limit, Some(1_000_000_000));
    assert_eq!(config.total_megapixels_limit, Some(512));
    assert_eq!(config.max_animation_frames, Some(10_000));
    assert_eq!(config.max_grid_tiles, Some(1_000));
    assert!(!config.lenient);
}

#[test]
fn decode_config_unlimited() {
    let config = zenavif_parse::DecodeConfig::unlimited();
    assert_eq!(config.peak_memory_limit, None);
    assert_eq!(config.total_megapixels_limit, None);
    assert_eq!(config.max_animation_frames, None);
    assert_eq!(config.max_grid_tiles, None);
    assert!(!config.lenient);
}

#[test]
fn decode_config_builder_methods() {
    let config = zenavif_parse::DecodeConfig::default()
        .with_peak_memory_limit(42)
        .with_total_megapixels_limit(7)
        .with_max_animation_frames(3)
        .with_max_grid_tiles(5)
        .lenient(true);

    assert_eq!(config.peak_memory_limit, Some(42));
    assert_eq!(config.total_megapixels_limit, Some(7));
    assert_eq!(config.max_animation_frames, Some(3));
    assert_eq!(config.max_grid_tiles, Some(5));
    assert!(config.lenient);
}

// Parser-specific resource limit tests

#[test]
fn parser_resource_limit_grid_tiles() {
    let bytes = std::fs::read(IMAGE_GRID_5X4).expect("read file");
    let config = zenavif_parse::DecodeConfig::default().with_max_grid_tiles(10);

    let result = zenavif_parse::AvifParser::from_bytes_with_config(
        &bytes,
        &config,
        &zenavif_parse::Unstoppable,
    );

    match result {
        Err(zenavif_parse::Error::ResourceLimitExceeded(msg)) => {
            assert_eq!(msg, "grid tile count limit exceeded");
        }
        Ok(_) => panic!("Expected grid tile count limit error"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[test]
fn parser_resource_limit_animation_frames() {
    let bytes = std::fs::read(ANIMATED_AVIF).expect("read file");
    let config = zenavif_parse::DecodeConfig::default().with_max_animation_frames(3);

    let result = zenavif_parse::AvifParser::from_bytes_with_config(
        &bytes,
        &config,
        &zenavif_parse::Unstoppable,
    );

    match result {
        Err(zenavif_parse::Error::ResourceLimitExceeded(msg)) => {
            assert_eq!(msg, "animation frame count limit exceeded");
        }
        Ok(_) => panic!("Expected animation frame count limit error"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[test]
fn parser_cancellation_during_parse() {
    struct ImmediatelyCancelled;
    impl zenavif_parse::Stop for ImmediatelyCancelled {
        fn check(&self) -> std::result::Result<(), zenavif_parse::StopReason> {
            Err(zenavif_parse::StopReason::Cancelled)
        }
    }

    let bytes = std::fs::read(IMAGE_AVIF).expect("read file");
    let config = zenavif_parse::DecodeConfig::default();
    let result = zenavif_parse::AvifParser::from_bytes_with_config(
        &bytes,
        &config,
        &ImmediatelyCancelled,
    );

    match result {
        Err(zenavif_parse::Error::Stopped(reason)) => {
            assert_eq!(reason, zenavif_parse::StopReason::Cancelled);
        }
        Ok(_) => panic!("Expected cancellation"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[test]
fn parser_cancellation_via_reader() {
    struct ImmediatelyCancelled;
    impl zenavif_parse::Stop for ImmediatelyCancelled {
        fn check(&self) -> std::result::Result<(), zenavif_parse::StopReason> {
            Err(zenavif_parse::StopReason::Cancelled)
        }
    }

    let config = zenavif_parse::DecodeConfig::default();
    let result = zenavif_parse::AvifParser::from_reader_with_config(
        &mut File::open(IMAGE_AVIF).expect("Unknown file"),
        &config,
        &ImmediatelyCancelled,
    );

    match result {
        Err(zenavif_parse::Error::Stopped(reason)) => {
            assert_eq!(reason, zenavif_parse::StopReason::Cancelled);
        }
        Ok(_) => panic!("Expected cancellation"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

// ============================================================================
// Multi-track animation tests
// ============================================================================

static ANIM_8BPC: &str = "tests/colors-animated-8bpc.avif";
static ANIM_8BPC_ALPHA: &str = "tests/colors-animated-8bpc-alpha-exif-xmp.avif";
static ANIM_12BPC_KF: &str = "tests/colors-animated-12bpc-keyframes-0-2-3.avif";
static ANIM_8BPC_AUDIO: &str = "tests/colors-animated-8bpc-audio.avif";
static ANIM_8BPC_DEPTH: &str = "tests/colors-animated-8bpc-depth-exif-xmp.avif";

// -- Track association and metadata --

#[test]
fn anim_single_track_no_alpha() {
    let bytes = std::fs::read(ANIM_8BPC).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    let info = parser.animation_info().expect("Expected animation");
    assert_eq!(info.frame_count, 5);
    assert!(!info.has_alpha, "Single-track animation should not have alpha");
    assert!(info.timescale > 0, "Timescale should be positive");

    for i in 0..info.frame_count {
        let frame = parser.frame(i).expect("frame failed");
        assert!(!frame.data.is_empty(), "Frame {} empty", i);
        assert!(frame.alpha_data.is_none(), "Frame {} should not have alpha", i);
        assert!(frame.duration_ms > 0, "Frame {} should have positive duration", i);
    }
}

#[test]
fn anim_two_tracks_with_alpha() {
    let bytes = std::fs::read(ANIM_8BPC_ALPHA).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    let info = parser.animation_info().expect("Expected animation");
    assert_eq!(info.frame_count, 5);
    assert!(info.has_alpha, "Two-track animation should have alpha");
    // flags bit 0 set = infinite looping -> loop_count=0
    assert_eq!(info.loop_count, 0, "Expected infinite loop (loop_count=0)");

    for i in 0..info.frame_count {
        let frame = parser.frame(i).expect("frame failed");
        assert!(!frame.data.is_empty(), "Frame {} color data empty", i);
        let alpha = frame
            .alpha_data
            .as_ref()
            .unwrap_or_else(|| panic!("Frame {} should have alpha", i));
        assert!(!alpha.is_empty(), "Frame {} alpha data empty", i);
    }
}

#[test]
fn anim_12bpc_with_alpha() {
    let bytes = std::fs::read(ANIM_12BPC_KF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    let info = parser.animation_info().expect("Expected animation");
    assert_eq!(info.frame_count, 5);
    assert!(info.has_alpha, "12bpc animation should have alpha track");
    assert_eq!(info.loop_count, 0, "Expected infinite loop");

    for i in 0..info.frame_count {
        let frame = parser.frame(i).expect("frame failed");
        assert!(!frame.data.is_empty());
        assert!(frame.alpha_data.is_some(), "Frame {} should have alpha", i);
    }
}

#[test]
fn anim_audio_track_skipped() {
    let bytes = std::fs::read(ANIM_8BPC_AUDIO).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    let info = parser.animation_info().expect("Expected animation");
    assert_eq!(info.frame_count, 5);
    assert!(!info.has_alpha, "Audio track should not produce alpha");

    // Audio track should be silently skipped - only color frames present
    for i in 0..info.frame_count {
        let frame = parser.frame(i).expect("frame failed");
        assert!(!frame.data.is_empty());
        assert!(frame.alpha_data.is_none());
    }
}

#[test]
fn anim_depth_track_with_alpha() {
    let bytes = std::fs::read(ANIM_8BPC_DEPTH).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    let info = parser.animation_info().expect("Expected animation");
    assert_eq!(info.frame_count, 5);
    // This file has a depth/alpha track (auxv handler + tref/auxl)
    assert!(info.has_alpha, "Depth file should have alpha track");

    for i in 0..info.frame_count {
        let frame = parser.frame(i).expect("frame failed");
        assert!(!frame.data.is_empty());
        assert!(frame.alpha_data.is_some(), "Frame {} should have alpha", i);
    }
}

// -- Loop count (elst flags parsing) --

#[test]
fn anim_loop_count_play_once() {
    // colors-animated-8bpc.avif has elst flags=0x000000 -> loop_count=1 (play once)
    let bytes = std::fs::read(ANIM_8BPC).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    let info = parser.animation_info().expect("Expected animation");
    assert_eq!(info.loop_count, 1, "Expected loop_count=1 (play once)");
}

#[test]
fn anim_loop_count_infinite() {
    // colors-animated-8bpc-alpha-exif-xmp.avif has elst flags=0x000001 -> infinite
    let bytes = std::fs::read(ANIM_8BPC_ALPHA).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    let info = parser.animation_info().expect("Expected animation");
    assert_eq!(info.loop_count, 0, "Expected loop_count=0 (infinite)");
}

#[test]
fn anim_loop_count_audio_file() {
    // Audio file uses v0 elst with flags=0 -> play once
    let bytes = std::fs::read(ANIM_8BPC_AUDIO).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    let info = parser.animation_info().expect("Expected animation");
    assert_eq!(info.loop_count, 1, "Audio file color track should play once");
}

// -- Zero-copy verification --

#[test]
fn anim_zerocopy_color_frames_borrowed() {
    // from_bytes -> single-extent color frames should be Cow::Borrowed
    let bytes = std::fs::read(ANIM_8BPC).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    for i in 0..5 {
        let frame = parser.frame(i).expect("frame failed");
        assert!(
            matches!(frame.data, Cow::Borrowed(_)),
            "Color frame {} should be Cow::Borrowed (zero-copy)",
            i
        );
    }
}

#[test]
fn anim_zerocopy_alpha_frames_borrowed() {
    // from_bytes -> single-extent alpha frames should also be Cow::Borrowed
    let bytes = std::fs::read(ANIM_8BPC_ALPHA).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    for i in 0..5 {
        let frame = parser.frame(i).expect("frame failed");
        assert!(
            matches!(frame.data, Cow::Borrowed(_)),
            "Color frame {} should be Cow::Borrowed",
            i
        );
        let alpha = frame.alpha_data.as_ref().expect("alpha should be present");
        assert!(
            matches!(alpha, Cow::Borrowed(_)),
            "Alpha frame {} should be Cow::Borrowed (zero-copy)",
            i
        );
    }
}

#[test]
fn anim_zerocopy_12bpc_alpha_borrowed() {
    // 12bpc file also uses single extents -> Cow::Borrowed
    let bytes = std::fs::read(ANIM_12BPC_KF).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    for i in 0..5 {
        let frame = parser.frame(i).expect("frame failed");
        assert!(matches!(frame.data, Cow::Borrowed(_)));
        let alpha = frame.alpha_data.as_ref().expect("alpha");
        assert!(
            matches!(alpha, Cow::Borrowed(_)),
            "12bpc alpha frame {} should be Cow::Borrowed",
            i
        );
    }
}

#[test]
fn anim_zerocopy_color_points_into_raw_buffer() {
    // Verify borrowed slices actually point into the original byte buffer
    let bytes = std::fs::read(ANIM_8BPC_ALPHA).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    let frame0 = parser.frame(0).expect("frame 0");
    let frame1 = parser.frame(1).expect("frame 1");

    if let (Cow::Borrowed(s0), Cow::Borrowed(s1)) = (&frame0.data, &frame1.data) {
        // Both slices should have distinct, non-overlapping offsets into the file
        let ptr0 = s0.as_ptr() as usize;
        let ptr1 = s1.as_ptr() as usize;
        assert_ne!(ptr0, ptr1, "Frames 0 and 1 should point to different offsets");

        // Both should be within the original buffer
        let buf_start = bytes.as_ptr() as usize;
        let buf_end = buf_start + bytes.len();
        assert!(ptr0 >= buf_start && ptr0 < buf_end, "Frame 0 should point into original buffer");
        assert!(ptr1 >= buf_start && ptr1 < buf_end, "Frame 1 should point into original buffer");
    } else {
        panic!("Expected Cow::Borrowed for from_bytes");
    }

    // Same for alpha
    if let (Some(Cow::Borrowed(a0)), Some(Cow::Borrowed(a1))) =
        (&frame0.alpha_data, &frame1.alpha_data)
    {
        let buf_start = bytes.as_ptr() as usize;
        let buf_end = buf_start + bytes.len();
        let aptr0 = a0.as_ptr() as usize;
        let aptr1 = a1.as_ptr() as usize;
        assert_ne!(aptr0, aptr1, "Alpha frames 0 and 1 should differ");
        assert!(aptr0 >= buf_start && aptr0 < buf_end, "Alpha 0 in buffer");
        assert!(aptr1 >= buf_start && aptr1 < buf_end, "Alpha 1 in buffer");
    } else {
        panic!("Expected Cow::Borrowed alpha data");
    }
}

// -- from_reader / from_owned path --

#[test]
fn anim_from_reader_alpha() {
    let parser = zenavif_parse::AvifParser::from_reader(
        &mut File::open(ANIM_8BPC_ALPHA).expect("open"),
    )
    .expect("parse failed");

    let info = parser.animation_info().expect("animation");
    assert_eq!(info.frame_count, 5);
    assert!(info.has_alpha);
    assert_eq!(info.loop_count, 0);

    for i in 0..5 {
        let frame = parser.frame(i).expect("frame");
        assert!(!frame.data.is_empty());
        assert!(frame.alpha_data.is_some());
    }
}

#[test]
fn anim_from_owned_alpha() {
    let bytes = std::fs::read(ANIM_8BPC_ALPHA).expect("read");
    let parser = zenavif_parse::AvifParser::from_owned(bytes).expect("parse failed");

    let info = parser.animation_info().expect("animation");
    assert_eq!(info.frame_count, 5);
    assert!(info.has_alpha);

    for i in 0..5 {
        let frame = parser.frame(i).expect("frame");
        assert!(!frame.data.is_empty());
        let alpha = frame.alpha_data.as_ref().expect("alpha");
        assert!(!alpha.is_empty());
        // from_owned still produces Cow::Borrowed (borrows from internal owned buffer)
        assert!(matches!(frame.data, Cow::Borrowed(_)));
        assert!(matches!(alpha, Cow::Borrowed(_)));
    }
}

#[test]
fn anim_from_reader_audio_skipped() {
    let parser = zenavif_parse::AvifParser::from_reader(
        &mut File::open(ANIM_8BPC_AUDIO).expect("open"),
    )
    .expect("parse failed");

    let info = parser.animation_info().expect("animation");
    assert_eq!(info.frame_count, 5);
    assert!(!info.has_alpha, "Audio track should be skipped");
}

// -- Edge cases and bounds --

#[test]
fn anim_frame_out_of_bounds_with_alpha() {
    let bytes = std::fs::read(ANIM_8BPC_ALPHA).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    assert!(parser.frame(5).is_err(), "Frame 5 should be out of bounds (only 0-4 exist)");
    assert!(parser.frame(100).is_err(), "Frame 100 should be out of bounds");
}

#[test]
fn anim_still_image_no_alpha_data() {
    let bytes = std::fs::read("tests/kodim-extents.avif").expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    assert!(parser.animation_info().is_none(), "Still image has no animation");
    assert!(parser.frame(0).is_err(), "Still image has no frames");
}

#[test]
fn anim_iterator_on_no_alpha() {
    let bytes = std::fs::read(ANIM_8BPC).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    let frames: Vec<_> = parser.frames().collect();
    assert_eq!(frames.len(), 5);

    for (i, result) in frames.iter().enumerate() {
        let frame = result.as_ref().expect("frame failed");
        assert!(!frame.data.is_empty());
        assert!(
            frame.alpha_data.is_none(),
            "Frame {} should not have alpha via iterator",
            i
        );
    }
}

#[test]
fn anim_iterator_with_alpha() {
    let bytes = std::fs::read(ANIM_8BPC_ALPHA).expect("read file");
    let parser = zenavif_parse::AvifParser::from_bytes(&bytes).expect("parse failed");

    let frames: Vec<_> = parser.frames().collect();
    assert_eq!(frames.len(), 5);

    for (i, result) in frames.iter().enumerate() {
        let frame = result.as_ref().expect("frame failed");
        assert!(!frame.data.is_empty());
        assert!(
            frame.alpha_data.is_some(),
            "Frame {} should have alpha via iterator",
            i
        );
    }
}

// -- Timescale --

#[test]
fn anim_timescale_exposed() {
    // Verify timescale is propagated correctly for all test files
    for (path, desc) in [
        (ANIM_8BPC, "8bpc"),
        (ANIM_8BPC_ALPHA, "8bpc+alpha"),
        (ANIM_12BPC_KF, "12bpc"),
        (ANIM_8BPC_AUDIO, "audio"),
        (ANIM_8BPC_DEPTH, "depth"),
    ] {
        let bytes = std::fs::read(path).unwrap_or_else(|_| panic!("read {}", desc));
        let parser =
            zenavif_parse::AvifParser::from_bytes(&bytes).unwrap_or_else(|_| panic!("parse {}", desc));
        let info = parser
            .animation_info()
            .unwrap_or_else(|| panic!("{} should be animated", desc));
        assert!(info.timescale > 0, "{}: timescale should be positive", desc);
    }
}

// -- Eager API (feature-gated) --

#[cfg(feature = "eager")]
#[test]
fn anim_eager_loop_count_parsed() {
    // Verify the eager API now has correct loop_count from elst
    let input = &mut File::open(ANIM_8BPC_ALPHA).expect("open");
    let avif = zenavif_parse::read_avif(input).expect("read_avif failed");

    let animation = avif.animation.expect("Expected animation");
    assert_eq!(animation.loop_count, 0, "Eager API should parse infinite loop from elst");
    assert_eq!(animation.frames.len(), 5);
}

#[cfg(feature = "eager")]
#[test]
fn anim_eager_play_once() {
    let input = &mut File::open(ANIM_8BPC).expect("open");
    let avif = zenavif_parse::read_avif(input).expect("read_avif failed");

    let animation = avif.animation.expect("Expected animation");
    assert_eq!(animation.loop_count, 1, "Eager API should parse play-once from elst");
}

#[cfg(feature = "eager")]
#[test]
fn anim_eager_audio_skipped() {
    let input = &mut File::open(ANIM_8BPC_AUDIO).expect("open");
    let avif = zenavif_parse::read_avif(input).expect("read_avif failed");

    let animation = avif.animation.expect("Expected animation");
    assert_eq!(animation.frames.len(), 5, "Should only have color frames, audio skipped");
}
