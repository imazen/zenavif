# Changelog

All notable changes to zenavif are documented here. zenavif is an AVIF encoder
and decoder built on the excellent work of the [rav1d-safe](https://github.com/imazen/rav1d-safe)
decoder (our fork of [dav1d](https://code.videolan.org/videolan/dav1d) via
[rav1d](https://github.com/memorysafety/rav1d)),
the [zenrav1e](https://github.com/imazen/zenrav1e) encoder (our fork of
[rav1e](https://github.com/xiph/rav1e)), and the
[zenavif-parse](https://github.com/imazen/zenavif-parse) container parser.

## [Unreleased]

## [0.1.5] - 2026-04-17

### Added
- Encoder `Auto` bit depth now matches input type: 8-bit input produces 8-bit
  AV1 and 16-bit input produces 10-bit AV1. Previously `Auto` always selected
  10-bit, which surprised callers decoding 8-bit sources back out as `Rgb16`
  (9bf934c).
- `DecoderConfig::prefer_8bit(bool)` (default `false`) for callers who want to
  downscale 10/12-bit AV1 to 8-bit RGB when decoding files produced by other
  encoders that default to higher bit depths (9bf934c).
- `RGBX8_SRGB` and `BGRX8_SRGB` pixel descriptors are now accepted by the
  encode dispatch. The padding byte is stripped before encoding and BGRX
  additionally swaps B/R channels; output is byte-identical to encoding the
  equivalent packed RGB8 (d9863f1).
- Encoder configuration guide in `README.md` covering speed/quality tradeoffs,
  quality parameter mapping, QM behaviour, and bit depth selection, with all
  numbers sourced from the committed sweep data on CID22-512 (a483b4a).
- Committed fine-grained encode sweep (q5-q100 step 5, speeds 1/2/4/6, QM
  on/off) under `benchmarks/` and a broader combinatorial sweep covering
  100 configurations, both measured with zensim-regress (53fff28, ef5cb08).
- `.workongoing` added to `.gitignore` for the main-with-lockfile agent
  workflow (d9863f1).

### Changed
- Default encoder profile for `[profile.test]` is now `opt-level = 2` so
  tests exercise optimised codec paths without requiring `--release`
  (9bf934c).
- Bumped `fast-ssim2` to `0.8.0`; `0.7.2` and `0.7.3` were yanked upstream
  after an accidental semver break from `yuvxyb` re-exports, which `0.8.0`
  removes (ef0500d).
- Minor documentation alignment between the zennode feature table in
  `README.md` and `Cargo.toml` (4b3a1df).
- Bump zencodec to 0.1.19

### Fixed
- `EncoderConfig::with_lossless()` and `with_lossless_mode()` (from the
  zencodec trait) now propagate `lossless = true` into the inner encoder
  config, so rav1e's lossless mode is actually engaged when requested via
  the trait API (ef5cb08).
- Quantization matrices (QM) are now auto-disabled when lossless encoding
  is selected. QM is still enabled by default for lossy quality levels
  (q5-q95), where it saves 9-13% on file size at speeds 4 and above with
  negligible quality impact, but combining QM with a quantizer of zero
  produced corrupt output (ef5cb08).
- `ColorAuthority::Cicp` is now set when the decoded image carries no ICC
  profile, reflecting the AVIF/MIAF precedence of `ICC > nclx > AV1 SPS
  CICP`. When no `colr` box is present, CICP (populated from nclx or SPS
  fallback) is the authoritative colour description (7d6b4e6, #3).
- `examples/encode_sweep.rs` — committed harness that regenerates the
  per-image TSVs under `benchmarks/`. CLI accepts `--image`, `--speeds`,
  `--qualities` (list or `START..=END:STEP`), `--qm {off,on,both}`, and
  `--force-bottomup {auto,off,on,both}` — the last is what reproduces
  zenrav1e#6's scenario now that ravif/40ddb66 disables bottom-up by
  default. `just sweep -- <flags>` is the shortcut. Gated on
  `encode-imazen,encode-threading`.

## [0.1.4] - 2026-04-05

### Added
- Generic SIMD YUV420-to-RGB8 path with autoversion dispatch covering NEON
  and WebAssembly in addition to x86, and 4:2:2 / 4:4:4 variants, built on
  the magetypes generic SIMD abstraction (0089d18, 0b7b333).
- Fuzz dictionary for AVIF and a nightly fuzz workflow that runs a short
  smoke fuzz on every push and a longer nightly run (b367344, ee107a7).
- Regression seed capturing the CDEF tile race fixed in rav1d-safe, so the
  race can never silently regress through the AVIF decode path (6d18489).

### Changed
- Replaced the platform-specific YUV-to-RGB SIMD implementations with a
  single magetypes-based generic dispatch. The x86-specific paths were
  collapsed into the generic implementation without measurable regression
  (0b7b333).
- Bumped `zencodec` to `0.1.13` (cfc1f7b).
- Committed `fuzz/Cargo.lock` for reproducible fuzz builds; `profraw` files
  and other tooling noise are now gitignored and excluded from published
  packages (193cbd0, ec244d8, 91912b7).

### Fixed
- Reverted the `max_frame_delay = 1` workaround added for a rav1d-safe CDEF
  threading race once the underlying race was fixed in rav1d-safe itself.
  The workaround served its purpose while the upstream fix was being
  developed (e089793, 1d1f838).

## [0.1.3] - Earlier

### Fixed
- Gated the `StopExt` import behind the `encode` feature so builds with
  `default-features = false` remain clean (812b817).

### Changed
- Bumped `zenavif-parse` to `0.6.0` and switched to the published `From`
  impl for gain map conversion (933db7a).
- Set correct minimum versions for `zenflate` and `linear-srgb`, and moved
  `linear-srgb` to a semver spec (d48d69b, a1c1131, 210c255).

## [0.1.1] - Earlier

### Changed
- Switched `rav1d-safe` from a git revision to the published `0.5.3`
  release on crates.io (55d009a).
- Removed local path overrides that broke CI (20500cd).

### Fixed
- Temporarily pinned `rav1d-safe` to a git revision containing the
  aarch64 panic fixes while the fix was making its way through a release
  (01c02d0).
