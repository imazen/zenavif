# Changelog

## Unreleased

### Changed (BREAKING)
- **Error results now carry a `whereat` source location.** The public
  `Result<T, E = Error>` alias is now `Result<T, E = whereat::At<Error>>`, so
  every error returned from the parser records the file:line where it
  originated (server-side production debuggability). All ~147 error origins in
  the box/ILOC/OBU/sample-table/grid state machines are tagged via `at!()`, and
  foreign errors (`bitreader`, `io`, `TryReserve`, `TryFromInt`) capture
  location at the propagation site. Callers matching on the bare `Error` must
  now unwrap the `At` first — `err.error()` borrows `&Error`, `err.decompose().0`
  owns the `Error`. The underlying `Error` enum, its variants, and parsing
  behavior are unchanged.

### Added
- **Reader entry points accept unsized readers (`&mut dyn Read`)** (c1c95e5,
  parity with upstream avif-parse 8fc5fe0). `AvifParser::from_reader[_with_config]`
  and the deprecated `read_avif[_with_options/_with_config]` now bound on
  `Read + ?Sized`, so a `&mut dyn Read` trait object works without a generic
  monomorphization per concrete reader. Pure bound relaxation — non-breaking.

### Changed
- **Dev build: `env_logger` no longer pulls its default features** (3976225,
  parity with upstream avif-parse 24ea3a2). Test logging only uses the builder
  API, so dropping `auto-color`/`humantime`/`regex` trims the test-only
  dependency graph with no behavior change. Library deps are unaffected.
- **`iloc` item loading moves a whole-mdat extent instead of realloc+copy**
  (c2a92f0, parity with upstream avif-parse 440760b). When a single-extent item
  covers an `mdat` exactly and the destination buffer is still empty, the mdat's
  buffer is moved (`mem::take`) rather than allocating a new buffer and copying.
  Multi-extent items still append. Output is byte-identical. (The zero-copy
  `AvifParser` path already returns `Cow::Borrowed` for single extents; this
  helps the owned-copy eager path.)

### Fixed
- **docs(readme): document color/CICP extraction + `primary_data()` OBU contract + error/metadata type names** — found by an insulated external-developer (README-only) usability test. The README now shows how to read CICP/`nclx` (`color_primaries`/`transfer_characteristics`/`matrix_coefficients`/`full_range`) and embedded ICC via `color_info() -> Option<&ColorInformation>` (plus the `mdcv`/`clli`/`cclv`/`amve` HDR accessors), states the precise `primary_data()` byte contract (raw `mdat`/`idat` extent = the full AV1 OBU temporal unit with the sequence header inline, decoder-ready; `av1C` `configOBUs` not prepended; empty for a pure image sequence; grid-header bytes for a grid primary), and names the `Result`/`Error` (`whereat::At<Error>`, both `std::error::Error`) and `AV1Metadata` types. No code change.
- **`AuxiliaryTypeProperty::type_subtype` is now panic-proof by construction**
  (8a5b1be, parity with upstream avif-parse 3801195). The split-on-NUL used
  `split_at(pos)` + `&rest[1..]`; the indices were always in range given a
  `position()`-derived offset, so no panic was reachable, but the slicing now
  uses `split_at_checked` + `get(1..).unwrap_or(rest)` so a future refactor
  can't reintroduce one. Output is byte-identical.
- **i686/wasm32: `calculate_frame_duration` no longer overflows on a crafted
  `stts`.** The per-entry `current_sample += entry.sample_count as usize`
  accumulator (and its comparison) could overflow 32-bit `usize` from an
  attacker-controlled time-to-sample table, panicking in debug or wrapping in
  release on 32-bit targets. Now uses `saturating_add` — semantically identical
  on 64-bit (sums never approach `usize::MAX`), overflow-safe on 32-bit.
- **`size=0` (extends-to-EOF) boxes parse again** (imazen/zenavif#16,
  f3c9f043): the OOM clamp added in 4fdc077 bounds the box reader to the
  bytes actually available, but `skip_box_content` still compared the
  `u64::MAX` extends-to-EOF sentinel against the clamped reader and failed
  every such file with "box content size mismatch" — including all of
  libavif's Apple-style HDR gain-map vectors, which carry a size=0 `mdat`
  (10/57 of zenavif's corpus, validated 45/57 → 55/57). The sentinel now
  skips exactly the remaining bytes.
- The OOM clamp under-accounted the just-consumed header bytes, so a
  clamped box could report more `bytes_left()` than the reader can deliver.
  The content budget now subtracts the header first. Both behaviors pinned
  by `skip_box_content_accepts_size_zero_extends_to_eof` (f3c9f043).


### Added
- Versioned public-API surface snapshot at `docs/public-api/zenavif-parse.txt`,
  regenerated on every `cargo test` via `tests/public_api_doc.rs`
  (`ZEN_API_DOC=check` verifies in CI's clippy job, `=off` skips elsewhere).
  Justfile recipes `api-doc` / `api-doc-check`.

### Changed
- Added `CHANGELOG.md` to published package `include` list so release history ships with the crate.

### Changed
- `tests/fuzz_regression.rs` now uses the shared `zen-fuzz-regress`
  test-helper crate (DEDUP-J2). Behaviour is unchanged — same
  `fuzz/regression/` seeds, same two targets (`parse`, `parse_limited`),
  same panic-propagation failure semantics. The in-file `collect_seeds`
  scaffolding is now provided by `RegressionSuite`. Net shrinkage of
  the harness from ~440 LOC (legacy multi-test scaffolding) to ~40 LOC.

### QUEUED BREAKING CHANGES
- Restore `GainMapMetadata::writer_version: u16` field for ISO 21496-1
  round-trip fidelity. Removed in 0.6.2 to avoid a semver break; should
  come back with `#[non_exhaustive]` on `GainMapMetadata` in the next
  minor bump.

### Added

- `tests/fuzz_regression.rs` regression-harness template ported from
  zenwebp (DEDUP-J). Walks `fuzz/regression/` (incl. per-target subdirs)
  and runs every seed through `AvifParser::from_bytes` and
  `AvifParser::from_bytes_with_config` plus the parser's accessor methods
  on the stable toolchain — no nightly required. Created
  `fuzz/regression/README.md` documenting how to add minimized crash
  seeds.

## [0.6.2] - 2026-04-17

### Changed
- `zencodec` is now a required dependency; the `From`/`Into` impls between
  `GainMapMetadata` and `zencodec::GainMapParams` are always compiled (243ed6f).
  The `zencodec` feature is kept as a no-op for backward compatibility.
- `GainMapMetadata::writer_version` field removed — `to_bytes()` always emits
  writer_version=0. The field was added in unreleased code and never shipped;
  the parser still validates writer_version >= minimum_version on decode.
- Test suite no longer wraps parser calls in `catch_unwind`. A parser processing
  untrusted input must never panic, so any reintroduced `debug_assert` or unwrap
  surfaces as a loud test failure rather than a silently-swallowed panic
  (065bab2).

### Fixed
- Parser no longer panics on several malformed AVIF inputs (e4e8eca, #2).
- Accept `pict` in addition to `auxv` as the handler_type for animated alpha
  auxiliary tracks; some AVIF sequences use `pict` for the alpha track that
  references the color track via `tref`/`auxl` (7531df5, #1).
- `parse_tone_map_image` now honours ISO 21496-1 bit 3 (`FLAG_COMMON_DENOMINATOR`),
  reading the compact encoding and expanding each fraction to `(num, common_d)`
  form. Four previously-failing ISO 21496-1 fixtures (05, 06, 21, 22) now pass;
  added a 22-case fixture suite covering direction, channels, common-denom,
  negative values, boundary fractions, varied denominators, gamma, writer_version,
  and all-flags-combined cases (9ddab78, #4).
- Corrected the 0.6.0 entry's description of `zencodec` (the dep is unconditional,
  not an optional feature) and dropped an intra-doc link to `read_avif` that was
  unresolved in default-feature builds; replaced with plain backticks. Refreshed
  a stale inline comment near the ISO 21496-1 flag constants (f2224a2).

## 0.6.0 — 2026-04-01

### Added
- `GainMapMetadata::backward_direction` field (ISO 21496-1 flags byte bit 2). When true,
  the base image is HDR and the alternate rendition is SDR (reversed from the default).
  Previously this flag was silently ignored during parsing.
- `zencodec` From/Into conversions added: `From<&GainMapMetadata>` → `zencodec::GainMapParams`
  and the reverse `From<&zencodec::GainMapParams>` → `GainMapMetadata`. Rational fractions
  are encoded using the continued-fraction algorithm (matching libultrahdr's canonical form).
  Also provides `From<&GainMapChannel>` ↔ `From<&zencodec::GainMapChannel>`.

### Changed
- **Breaking:** `GainMapMetadata` has a new `backward_direction: bool` field. Code
  constructing this struct with struct literal syntax must add `backward_direction: false`
  (or the appropriate value). Parser output is not affected — the field is now populated
  from the ISO 21496-1 flags byte.

## 0.5.0

### Added
- `AvifGainMap` type bundling gain map metadata, AV1 data, and alt color info
- `AvifDepthMap` type for depth auxiliary image extraction (URN-based `auxl`/`auxC`)
- `AvifData::gain_map()` and `AvifData::depth_map()` convenience methods (eager feature)
- `AvifParser::gain_map()` and `AvifParser::depth_map()` methods
- AV1 frame header parsing for lossless/QP detection
- `AV1Metadata::base_q_idx` and `AV1Metadata::lossless` fields
- `SequenceHeaderObu::frame_width_bits` and `frame_height_bits` fields
- Fuzz targets (`fuzz_parse`, `fuzz_parse_limited`)

### Changed
- **Breaking:** `AV1Metadata` has two new fields (`base_q_idx`, `lossless`)
- **Breaking:** `AvifData` has new depth-related fields
- `c_api` feature now implies `eager`
- `enough` bumped to 0.4.2, `env_logger` to 0.11.10
- Test files excluded from published crate

### Fixed
- Removed `debug_assert!` panics on malformed AVIF input
- Capped pre-allocations to prevent OOM on malformed containers
- Replaced unchecked arithmetic with proper error handling
- Clippy warnings in frame header parser and range-contains lint

## 0.4.0

### Added
- `ChromaSubsampling` named struct with constants (`NONE`, `YUV420`, `YUV422`)
- `From<(bool, bool)>` and `From<ChromaSubsampling> for (bool, bool)` for compat

### Changed
- **Breaking:** `AV1Metadata::chroma_subsampling` field type changed from `(bool, bool)` to `ChromaSubsampling`
- `enough` dependency bumped to 0.4
- Edition 2024
- Comprehensive CI: 6-platform matrix, i686, WASM, Codecov

## 0.3.0

### Added
- **Gain map (tmap) and entity groups (grpl) support**
- **Essential property validation** and stsd track codec config
- **clli and mdcv HDR metadata parsing**
- Absorb upstream avif-parse v2.0.0

### Changed
- Precompute sample byte offsets for O(1) frame lookup
- Avoid double allocation in multi-extent and EXIF paths

### Fixed
- Eliminate all panic paths from core library
- `corpus_local_parser` robustness on constrained platforms

## 0.2.1

Publication prep release.

## 0.2.0

### Added
- EXIF/XMP item parsing via cdsc references
- Pure AVIF sequence support (without meta box)
- Multi-track animation support (color + alpha)
- clli, amve, a1op, lsel, a1lx box parsing
- ftyp brand exposure
- hdlr validation
- Warn on transformative properties on grid tiles
- Zero-copy streaming API (`AvifParser`)
- Resource limits (max items, max extents, max box depth)

### Changed
- Fork of avif-parse with zero-copy API as primary interface
- Edition 2024
- Semver-breaking API restructure

## 0.1.0

Initial fork release from avif-parse. Zero-copy AVIF container parser with
grid image, animation, and alpha plane support.
