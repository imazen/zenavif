//! Fuzz crash regression suite (DEDUP-J template, ported from zenwebp).
//!
//! Runs every file in `fuzz/regression/` through every parser entry point that
//! has a fuzz target. Each seed file is a previously-found crash that has been
//! fixed; this test ensures none of them re-introduce a panic.
//!
//! Reproduces what the `fuzz_parse` and `fuzz_parse_limited` fuzz targets do,
//! but as a regular `cargo test` — no nightly toolchain needed. Failures here
//! mean a regression of a previously-fixed bug.
//!
//! To add a new seed: drop the (preferably minimized) crash file into
//! `fuzz/regression/` (or a per-target subdir under it), no other action
//! required.

use std::fs;
use std::path::PathBuf;

fn regression_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fuzz/regression")
}

/// Recursively collect every regular file under `dir`. Skips dotfiles and
/// README-style meta files, and silently tolerates a missing directory.
fn collect_seeds(dir: &PathBuf, out: &mut Vec<PathBuf>) {
    let read = match fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return,
    };
    for entry in read.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') || name.eq_ignore_ascii_case("README.md") {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_file() => out.push(path),
            Ok(t) if t.is_dir() => collect_seeds(&path, out),
            _ => {}
        }
    }
}

fn run_parse(input: &[u8]) {
    // Mirrors fuzz_targets/fuzz_parse.rs.
    if let Ok(parser) = zenavif_parse::AvifParser::from_bytes(input) {
        let _ = parser.primary_data();
        let _ = parser.alpha_data();
        let _ = parser.animation_info();
        let _ = parser.grid_config();
        let _ = parser.av1_config();
        let _ = parser.color_info();
    }
}

fn run_parse_limited(input: &[u8]) {
    // Mirrors fuzz_targets/fuzz_parse_limited.rs.
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
}

#[test]
fn fuzz_regression_seeds_do_not_panic() {
    let dir = regression_dir();
    let mut seeds = Vec::new();
    collect_seeds(&dir, &mut seeds);

    if seeds.is_empty() {
        eprintln!(
            "note: no regression seeds found under {} — nothing to check",
            dir.display()
        );
        return;
    }

    for path in seeds {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unnamed>")
            .to_owned();
        let input = fs::read(&path).unwrap_or_else(|e| panic!("read {name}: {e}"));

        // Each entry point may return Err but must not panic. If any panics,
        // the test fails with the seed name in the unwind message.
        run_parse(&input);
        run_parse_limited(&input);

        eprintln!("ok: {name} ({} bytes)", input.len());
    }
}
