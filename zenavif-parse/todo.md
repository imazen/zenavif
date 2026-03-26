# AVIF 1.2 Spec Compliance — zenavif-parse

Compared against https://github.com/AOMediaCodec/av1-avif/blob/main/index.bs (v1.2.0)
and libavif (AOMediaCodec/libavif) as of 2026-02-13.

## Currently Supported

- ftyp parsing, validates avif/avis major brand, exposes major_brand and compatible_brands
- meta box hierarchy: pitm, iinf/infe, iloc (v0/1/2), iref, iprp/ipco/ipma, idat, grpl
- hdlr validation: handler_type must be 'pict'
- Primary item: av01 and grid item types
- Grid images: dimg references, tile ordering by dimgIdx, GridConfig from explicit ImageGrid and ispe fallback
- Grid validation: warns on transformative properties on tile items (spec 1.2)
- Alpha: auxl references with urn:mpeg:mpegB:cicp:systems:auxiliary:alpha
- Premultiplied alpha: prem reference type
- Animation: moov/trak/mdia/minf/stbl, sample table, frame duration/location, alpha tracks, stsd codec config
- Essential property validation: must-be-essential (a1op, lsel, clap, irot, imir), must-not-be-essential (a1lx), unsupported-essential rejection
- Properties: pixi, auxC, ispe, grid, av1C, colr, irot, imir, clap, pasp, clli, mdcv, cclv, amve, a1op, lsel, a1lx
- AV1 OBU metadata: sequence header parsing (bit depth, chroma, monochrome, dimensions)
- EXIF and XMP metadata via cdsc references
- HDR gain maps: tmap derived image items, ISO 21496-1 metadata, gain map AV1 data, alternate color info
- Entity groups: grpl box parsing (altr and other grouping types)
- Cooperative cancellation via `enough::Stop`
- Resource limits via `DecodeConfig`

## Completed

### Priority 1 — Parse and Expose (needed by decoders)

- [x] av1C — AV1CodecConfigurationBox from ipco and stsd
- [x] colr — ColourInformationBox (nclx for CICP values, rICC/prof for ICC profiles) from ipco and stsd

### Priority 2 — Parse and Expose (needed for correct display)

- [x] irot — Rotation (0/90/180/270 degrees)
- [x] imir — Mirror/flip
- [x] clap — Clean aperture (crop)
- [x] pasp — Pixel aspect ratio

### Priority 3 — HDR metadata

- [x] clli — Content Light Level Info
- [x] mdcv — Mastering Display Colour Volume (zenavif-parse parses into typed struct; libavif stores as opaque blob)
- [x] cclv — Content Colour Volume (zenavif-parse parses into typed struct; libavif stores as opaque blob)
- [x] amve — Ambient Viewing Environment (zenavif-parse parses into typed struct; libavif stores as opaque blob)

### Priority 4 — Container-level validation

- [x] hdlr — Parse and validate handler_type is 'pict'
- [x] Expose compatible_brands and profile brands
- [x] Validate no transformative properties on grid tile derivation chains (spec 1.2)
- [x] Essential property validation — must-be-essential (a1op, lsel, clap, irot, imir), must-not-be-essential (a1lx), unsupported-essential rejection. Strict by default, lenient mode warns.
- [x] stsd — Parse SampleDescriptionBox in animation tracks to extract av1C and colr from VisualSampleEntry. Track codec config used as fallback for pure sequences.

### Priority 5 — Advanced features

- [x] a1op — OperatingPointSelectorProperty (multi-operating-point images)
- [x] lsel — Layer selector (progressive/layered decoding)
- [x] a1lx — Layered image indexing (byte ranges for layers)
- [x] cdsc — Content description / metadata links (EXIF, XMP)
- [x] tmap — Tone Map Derived Image Item (gain maps for HDR). Tested with libavif test files.
- [x] grpl/altr — Entity groups parsed (GroupsListBox with EntityToGroupBox children)

## Remaining Gaps

### P1 — Would use if test files existed

- [ ] **pixi public accessor** — Parsed into internal `ItemProperty::Channels` but no public API. Add `pixel_information() -> Option<&[u8]>` to expose plane depths. Trivial effort; blocked only on "is it useful to callers." libavif uses pixi for plane depth validation.

### P2 — Nice-to-have conformance

- [ ] **Brand validation** — Check `miaf` in compatible_brands per spec requirement. Trivial; warn or error if missing.
- [ ] **Opaque property forwarding** — libavif forwards unrecognized properties as opaque blobs via `avifImage::properties`. zenavif-parse drops them as `Unsupported`. Low impact but improves extensibility for callers who want to inspect unknown boxes.

### P3 — Blocked on spec or test files

- [ ] **reve** — Reference Viewing Environment (v0). No spec available (ISO 23008-12:2025 Amd 1), no implementations exist. libavif stores as opaque blob.
- [ ] **ndwt** — Nominal Diffuse White Luminance (v0). No spec available, no implementations exist. libavif stores as opaque blob.
- [ ] **sato** — Sample Transform Derived Image Item (new in 1.2, enables >12bpc via expression-based pixel reconstruction). libavif has full implementation but disabled it by default. No test files in the wild. Large effort.
- [ ] **ster** — Stereo pair groups. Neither zenavif-parse nor libavif actually processes this. No test files.

### P4 — Edge cases

- [ ] **thmb** — Thumbnail references. iref type is parsed; could add a named accessor. Only 1 test file (Microsoft/Tomsk_with_thumbnails.avif).
- [ ] **Grid gain maps** — When the gain map image is itself a grid, expose tile data. Needs test files to validate.
- [ ] **elst repetition count** — Parse full edit list for finite repetition counts (currently only infinite vs play-once). libavif computes `trackDuration / segmentDuration` for finite loops.

## Test Corpus Coverage

| FourCC | Found | Files | Notes |
|--------|-------|-------|-------|
| cclv | No | 0 | Parsed but no corpus coverage |
| amve | No | 0 | Parsed but no corpus coverage |
| reve | No | 0 | Spec unavailable |
| ndwt | No | 0 | Spec unavailable |
| a1op | Yes | 3 | Apple multilayer, Xiph quebec_3layer_op2 |
| lsel | Yes | 12 | Apple multilayer (7), Xiph (5) |
| a1lx | Yes | 6 | Apple multilayer (2), Xiph (4) |
| grpl | Yes | 3 | libavif gainmap test files |
| altr | Yes | 3 | libavif gainmap test files |
| thmb | Yes | 1 | Microsoft/Tomsk_with_thumbnails.avif |
| cdsc | Yes | 16 | All Microsoft test files (in iref) |
| sato | No | 0 | Spec still stabilizing |
| tmap | Yes | 5 | libavif gainmap test files (tests/gainmap/) |
| ster | No | 0 | No implementations exist |
| stsd | Yes | all avis | Parsed in animation tracks |
| hdlr | Yes | all | handler_type = pict in all tested files |

## Notes

### What decoders handle vs what the parser exposes

The parser (zenavif-parse) should parse and expose all container-level properties.
The decoder (zenavif) is responsible for:
- Using colr nclx as authoritative color info (may override AV1 bitstream values)
- Applying irot/imir/clap transforms to the decoded pixels
- Validating pasp (should be 1:1)
- Passing HDR metadata through to the caller for tone mapping
- Reconstructing HDR from gain map metadata + gain map image + base SDR

### Where zenavif-parse exceeds libavif

- mdcv, cclv, amve parsed into typed structs (libavif stores as opaque blobs in standard ipco path)
- Zero-copy API with `Cow<[u8]>` (libavif always copies)
- Cooperative cancellation via `enough::Stop` (libavif has no equivalent)
- `no_std` compatible with `alloc` (libavif requires full libc)
- Fallible allocations throughout (libavif uses standard malloc)
