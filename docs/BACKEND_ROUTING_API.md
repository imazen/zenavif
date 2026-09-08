# Still routing, replay and support

The execution router selects and runs a supported backend. It currently has
**no calibrated RD/time ranking**: automatic selection keeps an eligible
preferred backend, otherwise uses a deterministic capability fallback. Explicit
selection and `SvtParity` never fall back after a refusal or encoder error.

## Inputs and settings

`backend_router::StillInput` distinguishes RGB8, RGBA8, RGB16, RGBA16 and Gray8.
Gray8 requires `encode-mono`; there is no Gray16 pixel entry point. Legacy
`PlanInput` converts to its corresponding RGB/RGBA kind without a breaking
field addition. `query_still_backends`, `validate_still_input` and
`resolve_route` accept either type. `ResolvedRoute::input_kind` retains the
exact kind; its encode methods reject a changed kind or dimensions.

```rust
use zenavif::{Av1Backend, EncodeChromaSubsampling, EncoderConfig};
use zenavif::backend_router::{Effort, RoutingRequest, StillInput};

let route = EncoderConfig::new()
    .backend(Av1Backend::Zenav1Svt)
    .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
    .quality(75.0)
    .threads(Some(1))
    .resolve_route(
        RoutingRequest::default().with_effort(Effort::new(0.7)?),
        StillInput::Rgb8 { width: 640, height: 480 },
    )?;
// route.encode_rgb8(image, stop) executes this configuration.
# Ok::<(), zenavif::backend_router::RouteError>(())
```

Effort is checked and currently maps to native buckets, including SVT -1. It is
not fractional adaptive search or a portable time budget. Explicit SVT tools
and grain cannot move to another backend. Primary grayscale honors its selected
native preset; auxiliary alpha retains the documented wrapper speed mapping.

`EncoderConfig::with_svt_film_grain(SvtFilmGrainConfig)` exposes the translated
C model/denoiser or supplied `SvtFilmGrainTable`. Grain applies to the primary
4:2:0 color stream, including 10-bit color, while alpha remains ungrained.
Validation checks parameter bounds before allocating or encoding. Monochrome
refuses grain. Mainline parity permits C grain on its supported color path. The legacy
`SvtParams::tune` slot 5 means FilmGrain in this adapter, while C slot 5 means
VMAF and rejects all-intra. `SvtParity` refuses that collision; it does not
silently reinterpret the tune number.

## Portable replay and cache keys

Enable `routing-replay`. A route provides:

- `to_replay_json(implementation_id)` — complete version-1 record of config,
  metadata/gain-map bytes, input kind/dimensions, policy, selection, effort,
  reference, native preset, enhancements and grain parameters.
- `ResolvedRoute::from_replay_json(json, implementation_id)` — restores the
  stored backend/configuration and revalidates support without rerouting.
- `cache_key(implementation_id, pixel_digest)` — SHA-256 of the canonical
  record and caller-supplied SHA-256 of logical pixel samples. Exclude row
  padding from the pixel digest; include all channels and precision bits.

Use an immutable deployment-artifact/build-manifest digest as
`implementation_id`. It must identify zenavif, all backend/converter/muxer
revisions, actual dependency resolution (including patches), compiler options,
target and features. Package versions and branch names are insufficient.
The caller supplies it because a library cannot discover a consumer's real
build provenance. Matching arbitrary labels do not establish compatibility.

Pin a positive thread count before resolving. Export refuses host-dependent
thread defaults rather than inventing portability. Import refuses changed
identities/features, unknown schemas/fields, invalid native presets, non-finite
controls and contradictory policy/effort/enhancement records. Input checks
still apply at execution. The record is configuration, not pixels or a saved
bitstream; replay does not promise equivalence across different builds.

The wire-config macro destructures `EncoderConfig` exhaustively. Adding a field
without covering it in replay fails compilation. Grain-table conversions are
also exhaustive. JSON uses float-roundtrip parsing. Cache keys intentionally
retain configuration distinctions even when two requests happen to emit the
same bytes; the existing sweep fingerprint remains a separate alias reducer.

## Audited still adapter support

This table describes wrapper support, not everything the raw codecs can do.

| Requirement | zenravif | zenav1-svt | zenav1-aom |
|---|---|---|---|
| RGB8 / RGB16 | Yes; deep input uses identity RGB/4:4:4 | Yes, 4:2:0 | Yes, 4:2:0 |
| RGBA8 / RGBA16 | Yes | Yes | Refused: no auxiliary alpha item |
| Gray8 | Yes | Yes, including selected native -1 | Yes; full-range input now converted to the coded studio range |
| Coded depth | 8/10 through this adapter | 8/10 | 8/10/12 color; 8 mono |
| Color subsampling | 4:4:4 / 4:2:0 where input/model permit | 4:2:0 | 4:2:0; raw codec also has 4:2:2/4:4:4 |
| Pixel range | Existing zenravif model/range validation | Full | Limited |
| Explicit conversion matrix | Existing zenravif path | BT.601 color; unspecified mono | BT.601 color; unspecified mono |
| ICC + explicit nclx | Owner muxer behavior | Both retained independently | Both retained independently |
| Transfer codes represented by shared muxer | Owner mapping | Full represented set 1/2/4–18 | Full represented set 1/2/4–18 |
| Exif/XMP, rotation/mirror, CLLI/mastering display | Existing paths | Existing paths, replayed | Existing paths, replayed |
| Source-lossless / gain map | Existing supported formats | Refused by RGB adapter (raw coded-lossless planes exist) | Refused by wrapper |
| C film-grain model or table | No exposed route requirement | Wired for primary color | No exposed wrapper control |
| SvtParity / named C reference | Refused | Supported policy, subject to tracked parity divergences | Refused |
| Explicit Zen SVT enhancements | Refused | Opt-in, research tip only | Refused |
| Palette / fast-tier recommendation overrides | Not forwarded by pinned owner; explicitly refused | Refused | Refused |
| Portable replay/cache identity | Yes with pinned build/threads | Yes, including native/reference/grain | Yes with pinned build/threads |
| Measured optimality report | Not exposed | Uncalibrated | Not exposed |

Unsupported matrices/primaries and foreign backend controls now fail explicitly,
rather than being accepted and ignored. The existing automatic picker now
attaches only its live quality/speed controls. Palette/fast-tier recommendation
helpers, models and research remain available; enabling their output requires
owner passthrough and measured wiring, not merely storing the enum values.

## Corrections and verification

The AOM Gray8 regression was reproduced before correction: input 16 displayed
as 0. The adapter now maps full-range Gray8 to 16..235 luma, matching the stream's
limited-range flag. An exact displayed-sample gate covers 0/16/64/128/235/255.
The existing lossy coded-luma test now compares against studio-range source
samples, keeping its original error threshold.

Additional regressions cover strided grayscale across backends/depths,
unsupported-all/feature-off behavior, unchanged replayed bytes, metadata and
source/build-sensitive cache keys, malformed records, native -1, both references,
explicit enhancements and supplied grain with visible decoded output. ICC and
non-default nclx are tested independently on the shared serializer path. A
90-cell matrix covers three backends, three depths, two chroma requests and
five pixel entry points: query and encode agree, and every success decodes.
These are correctness checks, not a corpus RD or speed calibration.

Local gates: 416 passing library/integration tests with the full audited
control feature set (one pre-existing ignored probe test), five passing
replay tests without optional encoders/monochrome, and 89 serializer tests.
Library Clippy with the same control features passes with warnings denied.
The SVT companion passes 2631 workspace tests. These counts describe the
listed gates, not a claim that every repository feature was exercised.

Reproduction (serialize heavy jobs through `run-heavy`):

```sh
cargo test --features routing-replay,encode-imazen,encode-mono,zenav1-svt,zenav1-aom-encode,__expert,two-pass-butteraugli,two-pass-zensim,auto-tune --test resolved_routing --test svt_rs_backend --test aom_encode_backend --test validation --test bit_depth_request --lib
cargo test --features routing-replay --test resolved_routing
cargo test -p zenavif-serialize --lib
```

Remaining backend functionality is explicit: Gray16, wider AOM wrapper formats,
AOM alpha/gain-map/lossless controls, the full SVT HDR-fork/superres/video wrapper
surface, palette/fast-tier owner passthrough, and corpus-calibrated adaptive
search/routing. The SVT source audit and native10 deferred-cell manifest live in
`zenav1-svt/rust/docs/API-SUPPORT-AUDIT-2026-09-08.md` and
`deferred-native10-parity.json`. AOM implementation work remains coordinated in
[zenav1-aom #16](https://github.com/imazen/zenav1-aom/issues/16).
