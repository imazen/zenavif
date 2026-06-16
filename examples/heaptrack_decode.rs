//! Heaptrack harness for AVIF decode-from-bytes allocation profiling.
//!
//! Profiles the production-critical path: `zenavif::decode(&bytes)` — decoding an
//! AVIF file (untrusted input) all the way to a `PixelBuffer`, via the default
//! pure-Rust rav1d-safe AV1 backend. The goal is to surface allocation
//! *pathologies* that don't show up in a wall-clock benchmark: a high allocation
//! *count* relative to image size, per-pixel or per-block/per-tile mallocs, large
//! transient peaks, or unbounded growth across repeated decodes (a leak). High
//! allocation churn hurts most under contended allocators (Windows, multi-threaded
//! servers) where a single decode of an untrusted upload turns into thousands of
//! lock round-trips.
//!
//! NOTE: the AV1 decode work is done by `rav1d-safe` (a safe Rust port of dav1d),
//! so the bulk of the allocation call-sites originate in that dependency; zenavif
//! owns the container parse (zenavif-parse) and the YUV->RGB conversion buffers.
//!
//! Usage:
//!   cargo build -p zenavif --release --example heaptrack_decode
//!   heaptrack ./target/release/examples/heaptrack_decode                 # default fixture
//!   heaptrack ./target/release/examples/heaptrack_decode <file.avif> [iters]
//!
//! Then inspect:
//!   heaptrack_print heaptrack.heaptrack_decode.*.zst | less
//!
//! Defaults to the committed `tests/vectors/libavif/kodim03_yuv420_8bpc.avif`
//! (768x512 Kodak photo, 4:2:0 8-bit — a meaningful AV1 superblock/tile count for
//! judging the allocation count) decoded 8 times. A large fixture should be
//! decoded fewer times (pass a smaller `iters`).

use std::hint::black_box;
use std::path::{Path, PathBuf};

/// Resolve the default bundled fixture relative to the crate manifest so the
/// example runs from any working directory.
fn default_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/libavif/kodim03_yuv420_8bpc.avif")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let path: PathBuf = match args.get(1) {
        Some(p) => PathBuf::from(p),
        None => default_fixture(),
    };
    // Default 8 iterations; a leak shows up as monotonic growth across them, and a
    // healthy decoder's steady-state per-decode allocation count is iterations-stable.
    let iters: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);

    let data = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("failed to read {}: {e}", path.display());
        std::process::exit(1);
    });

    // Decode once up front to report the dimensions the alloc count is relative to.
    {
        let probe = zenavif::decode(&data).unwrap_or_else(|e| {
            eprintln!("probe decode failed for {}: {e}", path.display());
            std::process::exit(1);
        });
        let (w, h) = (probe.width(), probe.height());
        eprintln!("fixture: {} ({} bytes on disk)", path.display(), data.len());
        eprintln!(
            "  decoded image: {}x{} ({:.2} MP)",
            w,
            h,
            (f64::from(w) * f64::from(h)) / 1.0e6
        );
    }

    eprintln!("decoding {iters}x via zenavif::decode(..) ...");

    let mut total_pixels: u64 = 0;
    for i in 0..iters {
        let buf = zenavif::decode(&data).unwrap_or_else(|e| {
            eprintln!("decode iteration {i} failed: {e}");
            std::process::exit(1);
        });
        total_pixels += u64::from(buf.width()) * u64::from(buf.height());
        // Consume the decoded buffer so the optimizer can't elide the decode or the
        // allocation of the output PixelBuffer.
        black_box(buf.width());
        black_box(buf.height());
        black_box(&buf);
    }

    eprintln!("done: decoded {total_pixels} total pixels across {iters} iterations");
}
