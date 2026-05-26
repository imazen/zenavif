//! Fuzz crash regression suite (DEDUP-J template, ported from zenwebp).
//!
//! Runs every file in `fuzz/regression/` through every decoder entry point that
//! has a fuzz target. Each seed file is a previously-found crash that has been
//! fixed; this test ensures none of them re-introduce a panic.
//!
//! Reproduces what the `fuzz_decode`, `fuzz_decode_animation`,
//! `fuzz_decode_limited`, and `fuzz_probe` fuzz targets do, but as a regular
//! `cargo test` — no nightly toolchain needed. Failures here mean a regression
//! of a previously-fixed bug.
//!
//! To add a new seed: drop the (preferably minimized) crash file into
//! `fuzz/regression/` with a `crash-<sha>` name, no other action required.

use std::fs;
use std::path::PathBuf;

fn regression_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fuzz/regression")
}

/// Recursively collect every regular file under `dir`. Skips dotfiles
/// (e.g. `.gitkeep`) and silently tolerates a missing directory.
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
        if name.starts_with('.') {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_file() => out.push(path),
            Ok(t) if t.is_dir() => collect_seeds(&path, out),
            _ => {}
        }
    }
}

fn run_decode(input: &[u8]) {
    // Mirrors fuzz_decode.rs: 4 MP frame_size_limit.
    let config = zenavif::DecoderConfig::new().frame_size_limit(4 * 1024 * 1024);
    let _ = zenavif::decode_with(input, &config, &enough::Unstoppable);
}

fn run_decode_limited(input: &[u8]) {
    // Mirrors fuzz_decode_limited.rs: tight 1 MP cap.
    let config = zenavif::DecoderConfig::new().frame_size_limit(1024 * 1024);
    let _ = zenavif::decode_with(input, &config, &enough::Unstoppable);
}

fn run_decode_animation(input: &[u8]) {
    // Mirrors fuzz_decode_animation.rs.
    let config = zenavif::DecoderConfig::new().frame_size_limit(4 * 1024 * 1024);
    let _ = zenavif::decode_animation_with(input, &config, &enough::Unstoppable);
    if let Ok(mut anim) = zenavif::AnimationDecoder::new(input, &config) {
        while let Ok(Some(_frame)) = anim.next_frame(&enough::Unstoppable) {}
    }
}

fn run_probe(input: &[u8]) {
    // Mirrors fuzz_probe.rs: lightweight container parse + probe_info.
    let config = zenavif::DecoderConfig::new();
    if let Ok(decoder) = zenavif::ManagedAvifDecoder::new(input, &config) {
        let _ = decoder.probe_info();
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
        run_decode(&input);
        run_decode_limited(&input);
        run_decode_animation(&input);
        run_probe(&input);

        eprintln!("ok: {name} ({} bytes)", input.len());
    }
}
