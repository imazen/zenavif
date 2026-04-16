# Changelog

## Unreleased

### Added
- `GainMapMetadata::writer_version: u16` field — round-tripped through parse and
  `to_bytes()` instead of being dropped on parse and hardcoded to 0 on serialize
  (9ddab78).

### Changed
- `zencodec` is now a required dependency; the `From`/`Into` impls between
  `GainMapMetadata` and `zencodec::GainMapParams` are always compiled (243ed6f).
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
