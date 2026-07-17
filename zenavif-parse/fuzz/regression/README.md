# Fuzz regression seeds

This directory holds previously-found crash inputs that have been fixed.
The `cargo test -p zenavif-parse --test fuzz_regression` harness walks
this directory (recursively, ignoring dotfiles and README.md) and runs
each file through every entry point the fuzz targets cover.

To add a seed:
1. Minimize the crash with `cargo +nightly fuzz tmin <target> <input>`.
2. Verify it's small (target ≤ 1 KB, hard ceiling 8 KB per CLAUDE.md).
3. Drop it into this directory (optionally under a `fuzz_<target>/` subdir
   for organization) with a descriptive name.
4. Re-run the regression harness to confirm it passes on the fix.

Per CLAUDE.md "Fuzz Corpus & Crash Storage": the working fuzz corpus and
unminimized crashes live in `/mnt/v/fuzzes/zenavif-parse/`, NOT in git.
