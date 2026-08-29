//! What the production decoders tolerate in an AVIF container, and what they refuse.
//!
//! # Why this file exists
//!
//! `AvifDecoder::new` and `ManagedAvifDecoder::new` used to hand zenavif-parse
//! `DecodeConfig::default().lenient(true)`, overriding a parser default that is
//! deliberately strict. The comment that justified it — *"Use lenient parsing to
//! handle files with non-critical validation issues"* — was replaced with an
//! unrelated note about zero-copy parsing in commit `0a6606a`, while the
//! `.lenient(true)` call itself survived the refactor. The behaviour outlived
//! its reason, and nobody afterwards had any way to recover why it was there.
//!
//! What that blanket flag actually bought was four downgraded container
//! conformance checks: non-zero reserved flags in boxes required to have none,
//! and three `essential`-flag rules — the worst being an **unknown property
//! marked essential**, where zenavif-parse's own warning says the item "will be
//! unusable", and the file decoded anyway with nothing but a log line.
//!
//! Measured across the 227 AVIF files in this repo's corpus, exactly two needed
//! anything from leniency, and for different reasons than folklore assumed:
//!
//! * `extended_pixi.avif` — its `pixi` box carries `flags = 0x000001` (the
//!   extended-`pixi` marker) plus 6 bytes of extension payload. The first thing
//!   it tripped was the *flags* check, not the trailing-bytes check.
//! * `clap_irot_imir_non_essential.avif` — marks `clap`/`irot`/`imir`
//!   non-essential, which MIAF forbids.
//!
//! Both are now handled precisely inside zenavif-parse, so the decoders run
//! strict. These tests pin both halves: the two files still decode with
//! byte-identical pixels, and the checks that leniency used to silence are
//! enforced again.

use enough::Unstoppable;
use zenavif::{DecoderConfig, decode_with};

const EXTENDED_PIXI: &str = "tests/vectors/libavif/extended_pixi.avif";
const CLAP_NON_ESSENTIAL: &str = "tests/vectors/libavif/clap_irot_imir_non_essential.avif";

/// FNV-1a 64 over the tightly-packed decoded pixels.
///
/// Re-derive after an *intended* pixel change with
/// `cargo test --test parser_leniency_scope -- --nocapture` and read the value
/// out of the assertion message.
fn pixel_fingerprint(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Decode and return `(width, height, fingerprint)`.
fn decode_fingerprint(path: &str) -> (u32, u32, u64) {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let config = DecoderConfig::new().threads(1);
    let image = decode_with(&data, &config, &Unstoppable)
        .unwrap_or_else(|e| panic!("{path} must still decode under strict parsing: {e}"));
    let p = image.as_slice();
    let (w, h, stride) = (p.width() as usize, p.rows() as usize, p.stride());
    let bpp = p.descriptor().bytes_per_pixel();
    let b = p.as_strided_bytes();
    let packed: Vec<u8> = (0..h)
        .flat_map(|y| b[y * stride..][..w * bpp].to_vec())
        .collect();
    (w as u32, h as u32, pixel_fingerprint(&packed))
}

/// Byte offset of the single occurrence of `needle`.
///
/// Panics unless it appears exactly once, so a changed fixture fails loudly
/// instead of silently patching the wrong bytes (or nothing at all).
fn offset_of_only(data: &[u8], needle: &[u8]) -> usize {
    let hits: Vec<usize> = data
        .windows(needle.len())
        .enumerate()
        .filter(|(_, w)| *w == needle)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "fixture changed: expected exactly one {:?}, found {}",
        String::from_utf8_lossy(needle),
        hits.len()
    );
    hits[0]
}

fn decode_err(data: &[u8]) -> String {
    let config = DecoderConfig::new().threads(1);
    match decode_with(data, &config, &Unstoppable) {
        Ok(_) => panic!("expected this container to be REFUSED, but it decoded"),
        Err(e) => e.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The two files leniency was actually load-bearing for must keep decoding,
// byte for byte.
// ---------------------------------------------------------------------------

/// The extended-`pixi` form (`flags = 1` + a 6-byte extension payload) is
/// handled by `read_pixi` itself, so it no longer needs a lenient parser.
#[test]
fn extended_pixi_decodes_byte_identically_under_strict_parsing() {
    let got = decode_fingerprint(EXTENDED_PIXI);
    assert_eq!(
        got,
        (4, 4, 0x5e56_f34d_ab7b_ca2c),
        "extended_pixi.avif pixels changed (w, h, fnv1a64) — strict parsing must \
         not alter decoded output"
    );
}

/// `clap`/`irot`/`imir` present but mislabelled non-essential: warned, applied,
/// and the pixels are unchanged from when the decoders parsed leniently.
#[test]
fn mislabelled_non_essential_transform_decodes_byte_identically() {
    let got = decode_fingerprint(CLAP_NON_ESSENTIAL);
    assert_eq!(
        got,
        (12, 34, 0x6899_8027_5c22_4c26),
        "clap_irot_imir_non_essential.avif pixels changed (w, h, fnv1a64) — this \
         file's transforms must still be applied exactly as before"
    );
}

// ---------------------------------------------------------------------------
// The checks the blanket `lenient(true)` used to silence are enforced again.
// ---------------------------------------------------------------------------

/// An item carrying a property this parser does not model, marked `essential`,
/// must be REFUSED. The spec's whole point in the essential flag is "you cannot
/// render this item unless you honour this property"; under the old blanket
/// leniency such a file decoded with only a `warn!` saying the item "will be
/// unusable".
///
/// The fixture is built here rather than committed: `extended_pixi.avif` with
/// its `colr` property renamed to an unknown four-CC and that association's
/// essential bit set.
#[test]
fn unknown_property_marked_essential_is_refused() {
    let mut data = std::fs::read(EXTENDED_PIXI).expect("read fixture");

    // Make property #4 (`colr`) one this parser cannot model.
    let colr = offset_of_only(&data, b"colr");
    data[colr..colr + 4].copy_from_slice(b"zzzz");

    // `ipma` v0/flags0 payload: entry_count u32, then per entry item_ID u16,
    // association_count u8, then one byte per association (essential = bit 7).
    let ipma = offset_of_only(&data, b"ipma");
    let assoc = ipma + 4 + 4 + 4 + 2 + 1;
    assert_eq!(
        &data[assoc..assoc + 4],
        &[0x01, 0x02, 0x83, 0x04],
        "fixture changed: unexpected ipma associations"
    );
    data[assoc + 3] |= 0x80; // now-unknown property #4, marked essential

    let err = decode_err(&data);
    assert!(
        err.contains("unsupported property marked as essential"),
        "an unknown ESSENTIAL property must be refused, got: {err}"
    );
}

/// Non-zero flags in a box required to have none are refused again. `pitm` is
/// the probe: it is not `pixi`, so no tolerance applies to it.
#[test]
fn non_zero_reserved_flags_outside_pixi_are_refused() {
    let mut data = std::fs::read(EXTENDED_PIXI).expect("read fixture");
    let pitm = offset_of_only(&data, b"pitm");
    // full box: fourcc, then version u8, then flags u24.
    assert_eq!(
        &data[pitm + 4..pitm + 8],
        &[0, 0, 0, 0],
        "fixture changed: pitm flags"
    );
    data[pitm + 7] = 1;

    let err = decode_err(&data);
    assert!(
        err.contains("expected flags to be 0"),
        "non-zero flags outside pixi must be refused, got: {err}"
    );
}

/// The `pixi` tolerance is narrow: only the exact extension marker is accepted.
/// Any other flags value — including the marker plus an unknown bit — is still
/// refused, so this is not "pixi may carry whatever flags it likes".
#[test]
fn pixi_flags_other_than_the_extension_marker_are_refused() {
    let mut data = std::fs::read(EXTENDED_PIXI).expect("read fixture");
    let pixi = offset_of_only(&data, b"pixi");
    assert_eq!(
        &data[pixi + 4..pixi + 8],
        &[0, 0, 0, 1],
        "fixture changed: extended_pixi.avif must carry the pixi extension marker"
    );
    data[pixi + 7] = 3; // marker bit plus an unknown bit

    let err = decode_err(&data);
    assert!(
        err.contains("expected flags to be 0"),
        "pixi flags beyond the extension marker must be refused, got: {err}"
    );
}
