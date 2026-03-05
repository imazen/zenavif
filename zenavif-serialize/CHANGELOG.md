# Changelog

## Unreleased (since v0.1.0)

### Changed
- Comprehensive CI: 7-platform matrix, i686, WASM, Codecov

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
