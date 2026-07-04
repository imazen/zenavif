# Changelog

## [Unreleased]

**Next release is 0.2.0** — the breaking changes below are already on main
(the `At<SerializeError>` write-path migration landed), so the manifest
version is pre-bumped to prevent an accidental semver-violating 0.1.5.

### QUEUED BREAKING CHANGES
<!-- Batch these into the next minor (0.x) release. -->
- The `io::Write` muxing entry points (`serialize`, `Aviffy::write`,
  `Aviffy::write_slice`) now return `Result<(), whereat::At<SerializeError>>`
  instead of `io::Result<()>`, so failures carry a `file:line` source location
  for server logs. New public `SerializeError` enum (`InvalidInput` / `Io` /
  `Oom`) and `Result<T>` alias. Get the cause with `e.error()` / `e.decompose().0`.
  The infallible `Vec` path (`serialize_to_vec`, `Aviffy::to_vec`) is unchanged,
  and the per-byte `WriterBackend` write path stays bare (no per-write `At<>`
  allocation — the crate deliberately avoids that overhead).

### Added
- **`Aviffy::set_gain_map_alt_icc`** — ICC profile for the gain map's
  alternate rendition, written as a `colr` box of type `prof` on the `tmap`
  item (may be combined with the nclx `set_gain_map_alt_colr`; ISOBMFF allows
  one `colr` of each type). Matches libavif interop vectors whose tmap colr
  is an ICC profile (`seine_sdr_gainmap_srgb_icc.avif`).
- **Fallible `Vec` serialization** (zenavif-serialize#6): `Aviffy::try_to_vec`
  and free `try_serialize_to_vec` return `Result<Vec<u8>>` (located
  `SerializeError`) instead of panicking on invalid input — for request paths
  that cannot afford a panic. `to_vec` / `serialize_to_vec` keep their
  documented panic-on-misuse contract and now route through the same checked
  path.
- **Opt-in output-size cap** (zenavif-serialize#3):
  `Aviffy::set_max_output_bytes` / `with_max_output_bytes` rejects a
  projected file size above the cap before any output is produced
  (`SerializeError::InvalidInput`).
- Versioned public-API surface snapshot at `docs/public-api/zenavif-serialize.txt`,
  regenerated on every `cargo test` via `tests/public_api_doc.rs`
  (`ZEN_API_DOC=check` verifies in CI's clippy job, `=off` skips elsewhere).
  Justfile recipes `api-doc` / `api-doc-check`.

### Changed
- README overhaul: canonical badge row (added MSRV, dropped `branch=`, fixed the
  lib.rs badge), a `## Quick start`, and a refreshed crosslink footer. Corrected
  stale claims against source — the `io::Write` entry points return
  `Result<(), whereat::At<SerializeError>>` (not `io::Result<()>`), the setter is
  `set_full_color_range`, the example `use` paths use the `constants` / `animated`
  / `grid` modules, and the crate depends on `arrayvec` + `whereat`. crates.io now
  renders a badge-free `README.crates.md` (`readme = "README.crates.md"`).

### Fixed
- **Outputs over `u32::MAX` bytes are rejected instead of silently corrupted**
  (zenavif-serialize#3): iloc extent offsets/lengths are serialized as `u32`
  (no largesize), so a >4 GiB file truncated `data.len() as u32` and wrapped
  `next_start` — a structurally corrupt file with no error. `write_header` now
  guards the projected file size up front (every entry point funnels through
  it), and the iloc arithmetic is checked as a backstop. `write_to_vec` also
  no longer swallows mdat write errors (OOM previously returned `Ok` with a
  truncated buffer).
- **README `no_std` claim removed** (zenavif-serialize#3): the crate uses
  `std::io` throughout; README.md and README.crates.md said `no_std`-compatible
  in three places. Replaced with the accurate `#![forbid(unsafe_code)]` + `std::io`
  statement.
- **README now states the AV1 input packaging contract**: `color_av1_data` must be
  the raw AV1 OBU bitstream for a single keyframe with the sequence header in-band
  (no length-prefix / Annex-B framing), `av1C` is built from builder settings rather
  than parsed from the bitstream, dimensions/`depth_bits` are passed separately, and
  `set_content_light_level` takes MaxCLL/MaxFALL in cd/m². Adds a signature reference
  and documents that `colr` is omitted when no color field is set. README only.
- **Raw TIFF Exif is now framed with the mandatory `exif_tiff_header_offset`**
  (0c43e53, port of upstream avif-serialize 37e6152). The HEIF/MIAF `Exif` item
  payload must begin with a 4-byte offset to the TIFF block; previously
  `set_exif` bytes were written verbatim, so a raw TIFF block (`II*\0`/`MM\0*` —
  the usual form from JPEG APP1 / `kamadak-exif`) produced a malformed item whose
  first 4 bytes a strict reader misreads as a bogus offset. Already-framed input
  is detected and emitted unchanged; raw TIFF gets a zero-offset header prepended
  as a separate iloc extent (payload bytes are not copied). `IlocItem.extents`
  widened to hold up to two extents.
- Monochrome primary images now serialize spec-correct properties: `pixi`
  declares 1 channel (was hardcoded 3), `av1C` forces
  `chroma_subsampling_x/y = 1` and seq_profile 0 (8/10-bit) or 2 (12-bit)
  — profile 1 is 4:4:4-only and Chrome validates `av1C` against the
  sequence header (supports imazen/zenavif#5/#6)
- Grid serializer no longer scans the output buffer for `0xBAADF00D` sentinels when patching `iloc` extent offsets; tile payloads that legitimately contain those bytes are no longer corrupted (05e2353)
- Animated serializer no longer scans the output buffer for `0xDEADBEEF` / `0xDEADBEE0` sentinels; AV1 frame payloads containing those bytes are preserved exactly (5e4af02)
- `tkhd` width and height encoding saturates at the 16.16 fixed-point maximum instead of debug-panicking / silently wrapping for dimensions >= 65536 (d4d1648)

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
