# Fuzz regression seeds

This directory holds previously-found crash inputs that have been fixed.
The `cargo test -p zenavif-parse --test fuzz_regression` harness walks
this directory (recursively, ignoring dotfiles and README.md) and runs
each file through every entry point the fuzz targets cover.

**This directory is deliberately empty right now, and the harness pins that.**
It declares `.min_seeds(0)`: zero seeds is accepted, but the directory itself
must exist and be readable, so the corpus cannot silently go from "no seeds
yet" to "the seeds were deleted" or "the directory was renamed". The seed count
is also asserted exactly, so **adding a seed without bumping `TRACKED_SEEDS`
fails the test** — that is intentional, and step 4 below is how you satisfy it.

To add a seed:
1. Minimize the crash with `cargo +nightly fuzz tmin <target> <input>`.
2. Verify it's small (target ≤ 1 KB, hard ceiling 8 KB per CLAUDE.md).
3. Drop it into this directory (optionally under a `fuzz_<target>/` subdir
   for organization) with a descriptive name.
4. Bump `TRACKED_SEEDS` in `tests/fuzz_regression.rs` to the new count, in the
   same commit. Once there is at least one seed the pin becomes meaningful:
   `min_seeds(0)` cannot tell "no corpus yet" from "the corpus vanished", and a
   non-zero pin can.
5. Re-run the regression harness to confirm it passes on the fix.

Per CLAUDE.md "Fuzz Corpus & Crash Storage": the working fuzz corpus and
unminimized crashes live in `/mnt/v/fuzzes/zenavif-parse/`, NOT in git.
