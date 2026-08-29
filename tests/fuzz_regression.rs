//! Replay seed inputs from `fuzz/regression/` through every fuzz target
//! entry point.
//!
//! ## Why this file carries its own seed-expectation machinery
//!
//! A regression suite that replays *zero* seeds passes — loudly, quickly, and
//! green — while testing nothing. Every way a corpus can go missing (a renamed
//! directory, seeds swallowed by `.gitignore`, a path the target platform
//! refuses to open) lands on that same outcome, and nothing in the test output
//! distinguishes it from a corpus that ran clean.
//!
//! Until 2026-08-29 this harness had **no guard of any kind**: it called
//! `zenutils_fuzz::RegressionSuite` 0.1.0, whose `run()` treats a missing or
//! empty seed directory as a silent no-op. With a single committed seed
//! (`fuzz_decode_animation/crash-cdef-tile-overlap.avif`, the rav1d-safe CDEF
//! tile race), deleting that one file would have left the test green while
//! replaying nothing.
//!
//! The local `regress` module below mirrors the `min_seeds` /
//! `RegressionReport` API that fixes this at the library layer in
//! `zenutils-fuzz`, which is **not published yet** — crates.io still has
//! 0.1.0. Migration is then mechanical: delete the module, restore
//! `use zenutils_fuzz::{RegressionSuite};`, and leave the
//! `RegressionSuite::new(..).min_seeds(..).target(..).run()` chain untouched.

use regress::RegressionSuite;

/// Number of seeds tracked under `fuzz/regression/`. `README`-style meta files
/// never count.
///
/// Pinned, not a floor-of-convenience: deleting the seed fails this test and
/// says how many went missing. Bump it in the same commit that adds seeds.
///
/// (`fuzz/artifacts/` also holds six crash/OOM files that predate the
/// `fuzz/artifacts/` `.gitignore` entry and stayed tracked; they are raw,
/// unminimized fuzzer output, not the minimized fixed-bug seeds this gate
/// replays. Promoting any of them is a separate decision — minimize with
/// `cargo +nightly fuzz tmin` first, then move it here and bump this constant.)
const TRACKED_SEEDS: usize = 1;

#[test]
fn fuzz_regression() {
    let report = RegressionSuite::new("fuzz/regression")
        .min_seeds(TRACKED_SEEDS)
        .target("decode", |input| {
            // Mirrors fuzz_decode.rs: 4 MP frame_size_limit.
            let config = zenavif::DecoderConfig::new().frame_size_limit(4 * 1024 * 1024);
            let _ = zenavif::decode_with(input, &config, &enough::Unstoppable);
        })
        .target("decode_limited", |input| {
            // Mirrors fuzz_decode_limited.rs: tight 1 MP cap.
            let config = zenavif::DecoderConfig::new().frame_size_limit(1024 * 1024);
            let _ = zenavif::decode_with(input, &config, &enough::Unstoppable);
        })
        .target("decode_animation", |input| {
            // Mirrors fuzz_decode_animation.rs.
            let config = zenavif::DecoderConfig::new().frame_size_limit(4 * 1024 * 1024);
            let _ = zenavif::decode_animation_with(input, &config, &enough::Unstoppable);
            if let Ok(mut anim) = zenavif::AnimationDecoder::new(input, &config) {
                while let Ok(Some(_frame)) = anim.next_frame(&enough::Unstoppable) {}
            }
        })
        .target("probe", |input| {
            // Mirrors fuzz_probe.rs: lightweight container parse + probe_info.
            let config = zenavif::DecoderConfig::new();
            if let Ok(decoder) = zenavif::ManagedAvifDecoder::new(input, &config) {
                let _ = decoder.probe_info();
            }
        })
        .target("decode_negotiate", |input| {
            // Mirrors fuzz_decode_negotiate.rs: the zencodec adapter with a
            // caller `preferred` list, on all three entry points. The fuzz
            // target steers the preference and the path from two in-band
            // bytes; a replay seed is a whole file, so sweep the steering
            // instead of consuming it.
            use std::borrow::Cow;
            use zencodec::decode::{
                Decode as _, DecodeJob as _, DecoderConfig as _, StreamingDecode as _,
            };
            use zenpixels::PixelDescriptor;

            let limits = zencodec::ResourceLimits::none().with_max_pixels(1 << 20);
            for pref in [
                &[][..],
                &[PixelDescriptor::RGB8_SRGB][..],
                &[PixelDescriptor::RGBA8_SRGB][..],
                &[PixelDescriptor::GRAY8_SRGB][..],
            ] {
                let cfg = zenavif::AvifDecoderConfig::new();
                if let Ok(dec) = cfg
                    .clone()
                    .job()
                    .with_limits(limits)
                    .decoder(Cow::Borrowed(input), pref)
                {
                    let _ = dec.decode();
                }
                if let Ok(mut dec) = cfg
                    .clone()
                    .job()
                    .with_limits(limits)
                    .streaming_decoder(Cow::Borrowed(input), pref)
                {
                    for _ in 0..4096 {
                        match dec.next_batch() {
                            Ok(Some(_)) => {}
                            _ => break,
                        }
                    }
                }
                let mut sink = ScratchSink { buf: Vec::new() };
                let _ = cfg.job().with_limits(limits).push_decoder(
                    Cow::Borrowed(input),
                    &mut sink,
                    pref,
                );
            }
        })
        .run();

    println!("{report}");
    assert_eq!(
        report.seeds_replayed(),
        TRACKED_SEEDS,
        "seed count drifted from the pinned value; update TRACKED_SEEDS in the \
         same commit that adds or removes a seed"
    );
}

/// Minimal sink for the `decode_negotiate` replay: hands back a correctly
/// sized scratch buffer and discards it. Mirrors the one in
/// `fuzz/fuzz_targets/fuzz_decode_negotiate.rs`.
struct ScratchSink {
    buf: Vec<u8>,
}

impl zencodec::decode::DecodeRowSink for ScratchSink {
    fn provide_next_buffer(
        &mut self,
        _y: u32,
        height: u32,
        width: u32,
        descriptor: zenpixels::PixelDescriptor,
    ) -> Result<zenpixels::PixelSliceMut<'_>, zencodec::decode::SinkError> {
        let stride = (width as usize).saturating_mul(descriptor.bytes_per_pixel());
        let needed = (height as usize).saturating_mul(stride);
        self.buf.clear();
        self.buf.resize(needed, 0);
        zenpixels::PixelSliceMut::new(&mut self.buf, width, height, stride, descriptor)
            .map_err(|_| zencodec::decode::SinkError::from("scratch buffer rejected"))
    }
}

/// Local stand-in for `zenutils_fuzz::RegressionSuite`.
///
/// Same builder shape and same semantics as the shared crate's unpublished
/// seed-expectation API, so swapping this module out for the real one is a
/// two-line change. The one rule that matters: **the counter lives inside the
/// filter**, so the number this reports can never drift from the number it
/// actually replayed. Hand-rolled guards that count directory entries
/// separately from the walk are how `README.md` ends up counted as a seed.
mod regress {
    use std::fmt;
    use std::fs;
    use std::io;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::{Path, PathBuf};

    type TargetFn = Box<dyn Fn(&[u8]) + Send + Sync>;

    /// Why scanning the seed directory did not produce a seed list.
    enum ScanError {
        /// The seed directory does not exist.
        Absent,
        /// The seed directory (or something inside it) could not be read, or
        /// the seed path is not a directory at all.
        Io { path: PathBuf, err: io::Error },
    }

    /// What a completed [`RegressionSuite::run`] actually did.
    pub struct RegressionReport {
        seed_dir: PathBuf,
        seed_paths: Vec<PathBuf>,
        target_count: usize,
    }

    impl RegressionReport {
        /// Number of seed files replayed through every target.
        pub fn seeds_replayed(&self) -> usize {
            self.seed_paths.len()
        }

        /// Number of registered targets.
        pub fn targets(&self) -> usize {
            self.target_count
        }
    }

    impl fmt::Display for RegressionReport {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "fuzz regression: replayed {} seed(s) from {:?} through {} target(s) = {} invocation(s)",
                self.seeds_replayed(),
                self.seed_dir,
                self.targets(),
                self.seeds_replayed() * self.targets()
            )
        }
    }

    /// Builder + runner for a fuzz-regression seed corpus.
    pub struct RegressionSuite {
        seed_dir: PathBuf,
        targets: Vec<(String, TargetFn)>,
        min_seeds: Option<usize>,
    }

    impl RegressionSuite {
        pub fn new<P: Into<PathBuf>>(seed_dir: P) -> Self {
            Self {
                seed_dir: seed_dir.into(),
                targets: Vec::new(),
                min_seeds: None,
            }
        }

        /// Require the corpus to replay at least `n` seeds.
        ///
        /// The seed directory must exist and be readable; a missing,
        /// unreadable, empty or short corpus fails [`Self::run`] with a
        /// message saying which of those it was. `n` counts *replayed* seeds
        /// — dotfiles, `*.md` and `*.txt` never count, so a `README.md` in the
        /// corpus directory does not inflate the number passed here.
        ///
        /// `min_seeds(0)` still requires the directory to exist and be
        /// readable, but accepts a corpus with no seeds in it — the setting
        /// for a corpus that is *deliberately* empty and whose directory must
        /// not silently disappear.
        pub fn min_seeds(mut self, n: usize) -> Self {
            self.min_seeds = Some(n);
            self
        }

        pub fn target<F>(mut self, name: &str, f: F) -> Self
        where
            F: Fn(&[u8]) + Send + Sync + 'static,
        {
            self.targets.push((name.to_string(), Box::new(f)));
            self
        }

        /// Replay every seed through every target.
        ///
        /// Panics — which is what a `#[test]` wants — if no seed expectation
        /// was declared, if no targets were registered, if the corpus does not
        /// meet the expectation, or if a target panics on a seed.
        pub fn run(self) -> RegressionReport {
            let Some(min_seeds) = self.min_seeds else {
                panic!(
                    "RegressionSuite at {:?}: no seed expectation declared, so this \
                     suite would pass without proving it replayed anything. Call \
                     `.min_seeds(n)`.",
                    self.seed_dir
                );
            };
            assert!(
                !self.targets.is_empty(),
                "RegressionSuite at {:?}: no targets registered. Call \
                 `.target(name, fn)` at least once before `.run()`.",
                self.seed_dir
            );

            let seeds = match collect_seeds(&self.seed_dir) {
                Ok(seeds) => seeds,
                Err(ScanError::Absent) => panic!(
                    "RegressionSuite: seed directory {:?} does not exist, but at least \
                     {min_seeds} seed(s) were required. The corpus was renamed, never \
                     checked out, or the path does not resolve on this target. A missing \
                     corpus is a FAILURE, never a skip: skipping would report green while \
                     replaying nothing.",
                    self.seed_dir
                ),
                Err(ScanError::Io { path, err }) => panic!(
                    "RegressionSuite: seed directory {:?} exists but could not be scanned \
                     ({path:?}: {err}). This is a broken harness, not an empty corpus: the \
                     suite would otherwise have replayed nothing and passed.",
                    self.seed_dir
                ),
            };

            assert!(
                seeds.len() >= min_seeds,
                "RegressionSuite: seed directory {:?} yielded {} seed(s) but at least \
                 {min_seeds} were required — {} seed(s) went missing. (Dotfiles, `*.md` \
                 and `*.txt` are never counted as seeds, so a directory holding only a \
                 README counts as empty.) Replayed: {:?}",
                self.seed_dir,
                seeds.len(),
                min_seeds - seeds.len(),
                seeds,
            );

            for seed_path in &seeds {
                let bytes = match fs::read(seed_path) {
                    Ok(b) => b,
                    Err(e) => {
                        panic!("RegressionSuite: failed to read seed {seed_path:?}: {e}")
                    }
                };

                for (target_name, target_fn) in &self.targets {
                    let res = catch_unwind(AssertUnwindSafe(|| target_fn(&bytes)));
                    if let Err(payload) = res {
                        panic!(
                            "RegressionSuite: target {target_name:?} panicked on seed \
                             {seed_path:?} ({} bytes, first 32: {:?}): {}",
                            bytes.len(),
                            &bytes[..bytes.len().min(32)],
                            panic_payload_str(&*payload),
                        );
                    }
                }
            }

            RegressionReport {
                seed_dir: self.seed_dir,
                seed_paths: seeds,
                target_count: self.targets.len(),
            }
        }
    }

    fn collect_seeds(dir: &Path) -> Result<Vec<PathBuf>, ScanError> {
        match fs::metadata(dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(ScanError::Io {
                    path: dir.to_path_buf(),
                    err: io::Error::other("seed path exists but is not a directory"),
                });
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Err(ScanError::Absent),
            Err(err) => {
                return Err(ScanError::Io {
                    path: dir.to_path_buf(),
                    err,
                });
            }
        }
        let mut seeds = Vec::new();
        walk(dir, &mut seeds)?;
        seeds.sort();
        Ok(seeds)
    }

    /// Recursive walk — this corpus stores seeds one level down, in a
    /// per-discovering-target subdirectory. Skips dotfiles (`.gitkeep`,
    /// `.DS_Store`) and the `*.md` / `*.txt` meta files a corpus directory
    /// carries alongside its seeds. Every I/O error propagates: a directory
    /// that cannot be read is a broken gate, not an empty one.
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), ScanError> {
        let entries = fs::read_dir(dir).map_err(|err| ScanError::Io {
            path: dir.to_path_buf(),
            err,
        })?;
        for entry in entries {
            let entry = entry.map_err(|err| ScanError::Io {
                path: dir.to_path_buf(),
                err,
            })?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let ft = entry.file_type().map_err(|err| ScanError::Io {
                path: path.clone(),
                err,
            })?;
            if ft.is_dir() {
                walk(&path, out)?;
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(".md") || lower.ends_with(".txt") {
                continue;
            }
            out.push(path);
        }
        Ok(())
    }

    fn panic_payload_str(payload: &(dyn std::any::Any + Send)) -> String {
        if let Some(s) = payload.downcast_ref::<&'static str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        }
    }
}
