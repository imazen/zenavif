# ABLATION-zenavif-serialize — Conservative Public-API Review

**Date:** 2026-06-10
**Snapshot commit:** d628b108335b (main@origin)
**Snapshot file:** docs/public-api/zenavif-serialize.txt (393 items, no features)
**Grep template:** `grep -rn "<SYMBOL>" /home/lilith/work/ --include="*.rs" 2>/dev/null | grep -v "/zen/zenavif-serialize/" | grep -v "target/" | grep -v ".jj/"`

## Summary

**0 items flagged.** The surface is a clean AVIF container serializer API with three entry points (`Aviffy` for still images, `AnimatedImage` for animation, `GridImage` for grids), metadata box types, CICP constant enums, and `ChromaSubsampling`. No internals are leaked and all items are load-bearing API surface.

Known consumers as of this scan: ravif/zenrav1e (primary consumer of `Aviffy`, `AnimatedImage`, `Av1CBox`, constants).

## Items Investigated

### Box types with zero direct consumer hits (KEEP)

The following `#[non_exhaustive]` box types appear in the API as parameters to `AnimatedImage` / `GridImage` setters but have no direct `zenavif_serialize::XxxBox` references in consumer code:

`ClliBox`, `MdcvBox`, `ColrIccBox`, `ImirBox`, `IrotBox`, `ClapBox`, `ColrBox`, `PaspBox`, `ChromaSubsampling`

These are correctly public: they are the argument types required to call `set_clli()`, `set_mdcv()`, `set_colr()`, `set_clean_aperture()`, `set_chroma_subsampling()`, etc. The fact that current consumers don't call all setters doesn't make these types unused — they're part of the serializer's declared capability surface. `#[non_exhaustive]` protects all of them against field-level semver breakage. Conservative call: **KEEP**.

### `constants` module — `ColorPrimaries`, `MatrixCoefficients`, `TransferCharacteristics`

All three are actively used by ravif (`/home/lilith/work/zen/ravif/ravif/src/av1encoder.rs`: `constants::MatrixCoefficients::Rgb`, `Bt709`, etc.; `map_transfer_characteristics`; `map_color_primaries`). **KEEP**.

## Flagged Items

None.

## Digest

- Snapshot: 393 items (no optional features)
- Items investigated: ~10 box types + 3 constant enums
- Flagged A: 0
- Flagged B: 0
- 0% of surface flagged
- Surface is coherent; all box types are parameters to active setters on `AnimatedImage`/`GridImage`/`Aviffy`
