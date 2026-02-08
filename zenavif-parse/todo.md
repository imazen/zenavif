# AVIF 1.2 Spec Compliance — zenavif-parse

Compared against https://github.com/AOMediaCodec/av1-avif/blob/main/index.bs (v1.2.0)

## Currently Supported

- ftyp parsing, validates avif/avis major brand
- meta box hierarchy: pitm, iinf/infe, iloc (v0/1/2), iref, iprp/ipco/ipma, idat
- Primary item: av01 and grid item types
- Grid images: dimg references, tile ordering by dimgIdx, GridConfig from explicit ImageGrid and ispe fallback
- Alpha: auxl references with urn:mpeg:mpegB:cicp:systems:auxiliary:alpha
- Premultiplied alpha: prem reference type
- Animation: moov/trak/mdia/minf/stbl, sample table, frame duration/location
- Properties: pixi, auxC, ispe, grid
- AV1 OBU metadata: sequence header parsing (bit depth, chroma, monochrome, dimensions)

## Priority 1 — Parse and Expose (needed by decoders)

- [x] av1C — AV1CodecConfigurationBox from ipco. Mandatory per spec for av01 items. zenavif currently gets this info from OBU parsing instead, but av1C is the canonical source and is needed for validation.
- [x] colr — ColourInformationBox (nclx for CICP values, rICC/prof for ICC profiles). Authoritative source for color info. zenavif currently gets CICP from rav1d's decoded sequence header; container colr should be the override.

## Priority 2 — Parse and Expose (needed for correct display)

These are transform/display properties. The parser should expose them; the decoder (zenavif) applies them.

- [x] irot — Rotation (0/90/180/270 degrees). Single byte: angle field.
- [x] imir — Mirror/flip. Single byte: axis field.
- [x] clap — Clean aperture (crop). 8 fields (4 rationals). Spec 1.2 adds: origin SHALL be anchored to 0,0 unless un-cropped image is a secondary item.
- [x] pasp — Pixel aspect ratio. If present, spec says SHALL be 1:1 for AVIF.

## Priority 3 — Parse and Expose (HDR metadata)

- [x] clli — Content Light Level Info (max_content_light_level, max_pic_average_light_level)
- [x] mdcv — Mastering Display Colour Volume (primaries, white point, luminance range)
- [ ] cclv — Content Colour Volume
- [ ] amve — Ambient Viewing Environment
- [ ] reve — Reference Viewing Environment (v0)
- [ ] ndwt — Nominal Diffuse White Luminance (v0)

## Priority 4 — Container-level validation

- [ ] hdlr — Parse and validate handler_type is 'pict'
- [ ] Brand validation — Check miaf in compatible_brands per spec requirement
- [ ] Expose compatible_brands and profile brands (MA1B, MA1A, avio)
- [ ] Validate no transformative properties on grid tile derivation chains (spec 1.2 constraint)

## Priority 5 — Advanced features (rare in practice)

- [ ] a1op — OperatingPointSelectorProperty (multi-operating-point images)
- [ ] lsel — Layer selector (progressive/layered decoding)
- [ ] a1lx — Layered image indexing (byte ranges for layers)
- [ ] sato — Sample Transform Derived Image Item (new in 1.2, enables >12bpc)
- [ ] tmap — Tone Map Derived Image Item (gain maps for HDR)
- [ ] grpl/altr — Entity groups (alternatives, for sato/tmap fallback)
- [ ] ster — Stereo pair groups
- [ ] thmb — Thumbnail references
- [ ] cdsc — Content description / metadata links

## Test Corpus Coverage

Which boxes have test files in av1-avif/ and link-u-samples/:

| FourCC | Found | Files | Notes |
|--------|-------|-------|-------|
| cclv | No | 0 | |
| amve | No | 0 | |
| reve | No | 0 | |
| ndwt | No | 0 | |
| a1op | Yes | 3 | Apple multilayer, Xiph quebec_3layer_op2 |
| lsel | Yes | 12 | Apple multilayer (7), Xiph (5) |
| a1lx | Yes | 6 | Apple multilayer (2), Xiph (4) |
| grpl | No | 0 | |
| altr | No | 0 | |
| thmb | Yes | 1 | Microsoft/Tomsk_with_thumbnails.avif |
| cdsc | Yes | 16 | All Microsoft test files (in iref) |
| sato | No | 0 | |
| tmap | No | 0 | |
| ster | No | 0 | |
| hdlr | Yes | all | handler_type = pict in all tested files |

## ftyp Brand Analysis

| Source | major_brand | compatible_brands |
|--------|-------------|-------------------|
| Microsoft stills | avif | mif1, avif, miaf, MA1B |
| Apple/Xiph multilayer | avif | mif1, avif, miaf |
| Link-U stills | avif | avif, mif1, miaf, MA1B |
| Netflix AVIS sequences | avis | avis, msf1, miaf, MA1B, iso8 |
| link-u-samples .avifs | avis | avis, msf1, miaf, MA1B |

- All files have minor_version=0
- mif1/msf1 = HEIF base brands; miaf = MIAF brand; MA1B = AV1 MIAF profile
- Multilayer files omit MA1B

## Notes

### What decoders handle vs what the parser exposes

The parser (zenavif-parse) should parse and expose all container-level properties.
The decoder (zenavif) is responsible for:
- Using colr nclx as authoritative color info (may override AV1 bitstream values)
- Applying irot/imir/clap transforms to the decoded pixels
- Validating pasp (should be 1:1)
- Passing HDR metadata through to the caller for tone mapping

### zenavif current state

zenavif gets CICP color info from rav1d after AV1 decode, not from the container.
The managed decoder extracts it from the decoded frame; the asm decoder hardcodes defaults (bug).
Neither reads colr, irot, imir, clap, pasp, or HDR metadata from the container.

### ravif

ravif is an encoder (powers cavif). Uses avif-serialize + rav1e. Not relevant for reader compliance.
