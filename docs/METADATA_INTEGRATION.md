# Metadata integration — 2026-10-12

Inventory is an optional, default-off `inventory` feature. The shared
`DecodeJob::inventory` method remains callable without it and returns typed
`UnsupportedOperation::Inventory`. Baseline EXIF, XMP, color, orientation and
ISO gain-map transport do not require this feature, zenmeta or zenpipe.

The container walker and regression cases reuse [PR 61](https://github.com/imazen/zenavif/pull/61)
at `d2f17783e39b9e13521cc7491f7d3c5657f29fc4`. Its five binary regression
fixtures are retained unchanged. The parser's self-referential auxiliary-track
fix is retained. The adapter now uses zencodec `c1c3292f`, returns typed input
and part limits, preserves inventory invariant failures, and checks cancellation
even for empty or malformed input.

## Validation

- `just metadata-check`: inventory cases, existing fuzz regressions, feature
  boundary, strict clippy, and no-default-feature compilation passed.
- `just metadata-backends`: EXIF, XMP and orientation round-trip through
  zenrav1e/zenravif, SVT and AOM encoders, each read by rav1d-safe and AOM.
  Existing product AOM and metadata tests passed too. This does not establish
  ICC or gain-map round-trip coverage across every backend pair.
- `just inventory-oracle`: 214 committed vectors and 137 conformance files;
  metadata payloads cross-checked against probe in 58 EXIF, 42 XMP and 14 ICC
  files. ExifTool reported zero mismatches for the inspected box ranges.
- i686 inventory, feature-boundary and stable fuzz replay passed. The standalone
  `inventory` fuzz target compiled. Public API snapshots for all three workspace
  crates were regenerated.

## Published API comparison

`just metadata-api-check` compares explicit rustdoc JSON against the downloaded
crates.io **zenavif 0.1.6**, with `--release-type patch` so the existing 0.2.0
version bump cannot hide breaks. cargo-semver-checks 0.50.0 reports 219 passing,
four failing and 31 skipped checks. All reported differences were already in
main `85dd0d2b` before this integration:

- Added `DecodedAnimationInfo::{exif,xmp,hdr,spatial}` and `DecodedFrame::timing`
  break exhaustive struct literals.
- `EncodeBitDepth`, `EncodeAlphaMode`, `EncodeColorModel` and `Av1Backend`
  became non-exhaustive.
- `EncodeBitDepth::Auto` changed its numeric discriminant from 2 to 3.
- `Av1Backend::Svtav1` became deprecated (an additive warning-level change).

Manual inspection also finds nine codec-trait associated `Error` types changed
from `At<zenavif::Error>` to `At<zencodec::CodecError>` before this integration.
The automated report does not catch those associated-type changes. This is
an audited baseline, not a blanket compatibility pass or a publish approval.

Inventory itself was absent from 0.1.6. Its feature and the variant
`Error::Inventory` on the existing non-exhaustive enum are additive. Changing
the unpublished inventory method to return `Result<Inventory, At<CodecError>>`
does not remove a crates.io API. Internal walkers expose no zenmeta-owned types.

## Remaining limits

The inventory describes container bytes and decoder disposition; it does not
parse AV1 OBU internals. Traversal and extent limits are deliberate bounds, not
claims of exhaustive interpretation. It retains PR 61's decoder/configuration
semantics and is not a container rewrite implementation. Broad cross-backend
ICC/gain-map tests and remote CI completion remain separate validation work.

## Subsequent backend gain-map integration

The expanded backend test exposed explicit SVT/AOM gain-map refusals. Both now
use the existing serializer's still-image gain-map support and share zenravif's
payload dimension/depth validation. No additional public API is introduced.
SVT animation remains explicitly unsupported for gain maps.

`just metadata-backend-gainmap-check` exercises base ICC, alternate ICC, ISO
parameters, byte-carried map payload and geometry mismatch refusal for all
three encoder choices, with metadata checked before and after base decoding by
both decoders. Existing metadata, gain-map, validation and animation tests and
clippy are included. The test uses the existing libavif ICC gain-map vector;
the tiny monochrome ICC test vector has an intentionally empty profile table
and is unsuitable for this positive ICC validation.

`just metadata-backend-gainmap-oracle` additionally opens and decodes each
backend output with libheif and inspects it with ExifTool. This checks base
image interoperability; it does not independently reconstruct HDR from the
alternate ICC profile. Arbitrary HDR/bit-depth/map-encoding combinations still
need their existing specialized tests, and remote CI remains a release gate.

The independent-reader run opens and decodes all three outputs, but ExifTool
reports two warnings on each: `ViewingConditions pointer references previous
ViewingConditions directory` and `Measurement pointer references previous
Measurement directory`. The pre-existing zenravif path produces the same
warnings as SVT/AOM. All base/alternate profile bytes match the source vector
in the tests; the source itself reports `Validate: OK` and exposes one profile
to ExifTool, while these outputs expose two. This is recorded as an unresolved
oracle difference, not a clean ExifTool validation result.

### ICC oracle difference resolved

The serializer emitted duplicate identical ICC properties for base and alternate
items. Reusing one property with both item associations preserves both profiles
and removes the reader ambiguity. After this change, the existing serializer
suite, backend metadata/gain-map/animation suites and clippy pass, libheif opens
and decodes each backend output, and ExifTool reports `Validate: OK` on all
three outputs. Reproduce with `just metadata-shared-icc-check`.

## Existing animation branch reconciliation

The integration includes the animation and native 12-bit work from
`60794df9` and `b777fa12`, already consumed by zencodecs. It retains the
`dfd01e32` decoder revision and the pixel dependency used by that consumer.
`just metadata-animation-integration-check` passes the decoded-color, frame
admission, exact-timing, dual-color and bit-depth suites, followed by the
shared serializer and all-backend gain-map tests, strict clippy and independent
libheif/ExifTool checks. Public API snapshots were regenerated with `just api-doc`.
The merged routing regression retains Zenravif and AOM for 12-bit input. SVT is enabled in the backend metadata and gain-map checks; its adapter explicitly rejects 12-bit input. `just metadata-backend-routing-check` also runs the SVT adapter tests.
