# Changelog

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
