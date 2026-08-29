//! Replay seed inputs from `fuzz/regression/` through every fuzz target
//! entry point.
//!
//! ## This corpus is deliberately empty — and that is now a checked fact
//!
//! `fuzz/regression/` currently holds **no seeds**, only its `README.md`. That
//! is not an accident and not a lost corpus:
//!
//! * The harness and the README were added together as scaffolding — a
//!   template ported from zenwebp — *before* any crash existed to put in it
//!   (see `CHANGELOG.md`, "regression-harness template ported from zenwebp").
//! * `zenavif-parse/fuzz/artifacts/` does not exist. The nightly `Fuzz`
//!   workflow has been running `fuzz_parse` and `fuzz_parse_limited` against
//!   `isobmff.dict` and has not produced a crash that was fixed and worth a
//!   minimized seed.
//! * No fuzz-found fix appears in this crate's changelog.
//!
//! So `TRACKED_SEEDS` is `0` and the suite declares `.min_seeds(0)`, which is
//! **not** the same as saying nothing: the seed directory must still exist and
//! be readable, so the corpus cannot quietly go from "no seeds yet" to "the
//! seeds were deleted" or "the directory was renamed" without this test going
//! red. Replaying zero seeds is a deliberate, pinned state rather than an
//! invisible one.
//!
//! **The moment a seed lands here, bump `TRACKED_SEEDS` to the real count in
//! the same commit** — `min_seeds(0)` cannot tell "no corpus yet" from "the
//! corpus vanished", and a non-zero pin can.
//!
//! ## Why this file carries its own seed-expectation machinery
//!
//! Until 2026-08-29 this harness had **no guard of any kind**: it called
//! `zenutils_fuzz::RegressionSuite` 0.1.0, whose `run()` treats a missing or
//! empty seed directory as a silent no-op. So it replayed zero seeds, passed
//! green, and reported nothing that distinguished that from a corpus that ran
//! clean — and it would have kept doing so if the directory were deleted
//! outright.
//!
//! The local `regress` module below mirrors the `min_seeds` /
//! `RegressionReport` API that fixes this at the library layer in
//! `zenutils-fuzz`, which is **not published yet** — crates.io still has
//! 0.1.0. Migration is then mechanical: delete the module, restore
//! `use zenutils_fuzz::RegressionSuite;`, and leave the
//! `RegressionSuite::new(..).min_seeds(..).target(..).run()` chain untouched.

use regress::RegressionSuite;

/// Number of seeds tracked under `fuzz/regression/`. `README.md` is not a seed
/// and never counts.
///
/// Zero is the deliberate, documented current state (see the module docs).
/// Bump it in the same commit that adds the first seed.
const TRACKED_SEEDS: usize = 0;

#[test]
fn fuzz_regression() {
    let report = RegressionSuite::new("fuzz/regression")
        .min_seeds(TRACKED_SEEDS)
        .target("parse", |input| {
            if let Ok(parser) = zenavif_parse::AvifParser::from_bytes(input) {
                let _ = parser.primary_data();
                let _ = parser.alpha_data();
                let _ = parser.animation_info();
                let _ = parser.grid_config();
                let _ = parser.av1_config();
                let _ = parser.color_info();
            }
        })
        .target("parse_limited", |input| {
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
                    "RegressionSuite: seed directory {:?} does not exist. Even a \
                     deliberately empty corpus (`min_seeds(0)`) must keep its directory: \
                     it was renamed, never checked out, or the path does not resolve on \
                     this target. A missing corpus is a FAILURE, never a skip — skipping \
                     would report green while replaying nothing.",
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

    /// Recursive walk — the README documents dropping seeds either at the top
    /// level or under a `fuzz_<target>/` subdirectory. Skips dotfiles
    /// (`.gitkeep`, `.DS_Store`) and the `*.md` / `*.txt` meta files a corpus
    /// directory carries alongside its seeds, so this corpus's `README.md` is
    /// never miscounted as a seed. Every I/O error propagates: a directory that
    /// cannot be read is a broken gate, not an empty one.
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
