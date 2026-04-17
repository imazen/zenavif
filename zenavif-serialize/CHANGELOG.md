# Changelog

## [Unreleased]

## [0.1.4] - 2026-04-17

### Changed
- Expanded `.gitignore` to cover tooling artifacts (`.superwork/`, `.claude/`, `.zenbench/`, `copter-report/`, profraw/profdata, fuzz logs, Cargo.toml backups) (4a5c9ca)

## 0.1.3 (2026-04-01)

### Added
- Gain map byte-exact and backward_direction parity tests

### Changed
- Bumped dependency versions (archmage, magetypes, enough, whereat, linear-srgb)
- Removed local patch override for zenavif-parse

## 0.1.2

### Changed
- Dependency updates and patch bumps

## 0.1.1

### Added
- `ChromaSubsampling` named struct with constants (`NONE`, `YUV420`, `YUV422`)
- `set_chroma_subsampling()` now accepts `impl Into<ChromaSubsampling>` (tuples still work)

### Changed
- Comprehensive CI: 7-platform matrix, i686, WASM, Codecov
- zenavif-parse dependency bumped to 0.4

## 0.1.0

Initial release. AVIF container serializer (MPEG/HEIF/MIAF/ISO-BMFF) with
animation and grid support. Fork of Cloudflare's cavif serializer with
expanded format support.

### Features
- Still image AVIF serialization
- Animated AVIF with frame timing
- Grid image (multiple tiles) support
- ICC color profile embedding
- EXIF/XMP metadata embedding
- Tested against zenavif-parse, libavif, and gpac parsers
