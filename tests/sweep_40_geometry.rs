//! Regressions for the 2026-08-26 ultracode sweep findings tracked in
//! zenavif#40 — all three were silent-corruption paths that returned `Ok`
//! with wrong pixels:
//!
//! 1. grid AVIFs carrying alpha auxiliary items decoded the color grid only
//!    and shipped opaque pixels (both per-tile `auxl` alpha and alpha-grid
//!    shapes);
//! 2. an alpha item coded at different dimensions than the primary was
//!    accepted, leaving the un-zipped bottom rows at the converter's opaque
//!    default;
//! 3. grid stitch placed every tile by its OWN decoded dims (unit-tested in
//!    `decoder_managed::grid::stitch_tests`).
//!
//! These run on default features — no encoder needed: the crafted container
//! is muxed from AV1 payloads lifted out of the libavif vectors.

use almost_enough::Unstoppable;
use zenavif::{DecoderConfig, ManagedAvifDecoder};

fn vector(name: &str) -> Vec<u8> {
    vector_at(&format!("libavif/{name}"))
}

fn vector_at(rel: &str) -> Vec<u8> {
    let path = format!("tests/vectors/{rel}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e} (run: just download-vectors)"))
}

/// Grid + alpha is refused on every default-feature entry point, with an
/// error that names alpha — never an opaque `Ok`.
#[test]
fn grid_with_alpha_aux_items_is_refused_not_decoded_opaque() {
    for name in [
        // per-tile `auxl` alpha items (primary-only alpha detection is blind)
        "color_grid_alpha_nogrid.avif",
        // alpha GRID item auxl-referencing the primary grid item
        "color_grid_alpha_grid_gainmap_nogrid.avif",
        "color_grid_alpha_grid_tile_shared_in_dimg.avif",
    ] {
        let data = vector(name);
        let parser = zenavif_parse::AvifParser::from_bytes(&data).expect("container parses");
        assert!(
            parser.grid_config().is_some(),
            "{name}: fixture is not a grid"
        );
        assert!(
            parser.has_alpha_aux_items(),
            "{name}: fixture carries no alpha aux items — test premise broken"
        );

        let err = zenavif::decode(&data)
            .expect_err(&format!("{name}: buffered decode must refuse grid+alpha"));
        assert!(err.to_string().contains("alpha"), "{name}: {err}");

        let err = ManagedAvifDecoder::new(&data, &DecoderConfig::new())
            .expect("construct")
            .decode_full(&Unstoppable)
            .expect_err(&format!("{name}: decode_full must refuse grid+alpha"));
        assert!(err.to_string().contains("alpha"), "{name}: {err}");
    }

    // And a plain grid still decodes: the guard must not over-fire.
    let data = vector("sofa_grid1x5_420.avif");
    let parser = zenavif_parse::AvifParser::from_bytes(&data).expect("parses");
    assert!(!parser.has_alpha_aux_items());
    zenavif::decode(&data).expect("alpha-free grid still decodes");
}

/// A container whose alpha item codes the same width but FEWER rows than
/// the primary must error rather than silently ship output whose bottom
/// rows keep the converter's opaque default. (A narrower alpha was already
/// caught by the per-row width check; height-short was the open hole — the
/// `zip` in `add_alpha8` just stopped at the shorter plane.)
#[test]
fn alpha_item_with_fewer_rows_than_primary_is_rejected() {
    // 1204x800 8-bit mono primary + a 1204x799 8-bit mono payload as alpha.
    let color = zenavif_parse::AvifParser::from_bytes(&vector_at(
        "link-u/fox.profile0.8bpc.yuv420.monochrome.avif",
    ))
    .expect("parse color")
    .primary_data()
    .expect("color payload")
    .into_owned();
    let alpha_short = zenavif_parse::AvifParser::from_bytes(&vector_at(
        "link-u/fox.profile0.8bpc.yuv420.monochrome.odd-height.avif",
    ))
    .expect("parse alpha source")
    .primary_data()
    .expect("alpha payload")
    .into_owned();

    let mut crafted = Vec::new();
    zenavif_serialize::serialize(&mut crafted, &color, Some(&alpha_short), 1204, 800, 8)
        .expect("mux crafted container");
    let parser = zenavif_parse::AvifParser::from_bytes(&crafted).expect("crafted parses");
    assert!(
        parser.alpha_data().is_some(),
        "crafted container lost its alpha item"
    );

    let err = zenavif::decode(&crafted)
        .expect_err("alpha coded 1 row short of the primary must be rejected");
    assert!(
        err.to_string().contains("alpha"),
        "error should name the alpha mismatch, got: {err}"
    );

    // Control: the same primary doubling as its own (matched-dims) alpha
    // decodes with a live alpha channel — the guard must not reject legal input.
    let mut control = Vec::new();
    zenavif_serialize::serialize(&mut control, &color, Some(&color), 1204, 800, 8)
        .expect("mux control container");
    let out = zenavif::decode(&control).expect("matched-dims alpha decodes");
    assert_eq!((out.width(), out.height()), (1204, 800));
    assert!(out.descriptor().alpha.is_some(), "control must carry alpha");
}
