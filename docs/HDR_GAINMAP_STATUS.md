# HDR & Gain Map Status

**Updated:** 2026-07-03. Everything below is measured/verified on that date
unless marked otherwise; test names are the proof anchors. Dev chain for
encode-side results: zenavif → zenravif 0.2.0 (path dep `../ravif/ravif`) →
zenrav1e master; decode is rav1d-safe (registry). Where a result depends on
an unpublished release it says so.

## HDR (10/12-bit, transfer functions, metadata)

### What works — verified

| Capability | Status | Proof |
|---|---|---|
| 10-bit encode (16-bit input → 10-bit AV1, identity/GBR, full range) | works | `tests/hdr_roundtrip.rs` |
| PQ (SMPTE 2084) + BT.2020 CICP echo through encode→parse→decode | works | `pq10_metadata_survives_encode_decode` |
| HLG CICP echo | works | `hlg10_cicp_survives_encode_decode` |
| clli (CLL/FALL) roundtrip | works | same tests + `tests/metadata_roundtrip.rs` |
| mdcv roundtrip (verbatim, **G,B,R wire order**) | works | `pq10_metadata_survives_encode_decode` |
| Re-encode chain (decode → config-from-info → encode) keeps CICP+clli+mdcv | works | `pq10_reencode_chain_preserves_hdr_metadata` |
| 10-bit pixel fidelity | quantizer-bounded | q95 max Δ=607/65535 (9 ten-bit steps), mean 50; sweep `benchmarks/hdr_pq10_fidelity_2026-07-03.tsv` |
| aomdec conformance, 10-bit PQ encodes | 20/20 clean | `benchmarks/hdr_conformance_2026-07-03.tsv` |
| Cross-decoder md5 (aomdec raw == rav1d-safe raw, 2-byte LE) | 20/20 agree | same; `examples/ivf_raw.rs` extended to 10/12-bit |
| HDR decode of libavif corpus vectors (PQ/HLG, 10-bit, gainmapped) | 57/57 with zenavif-parse ≥0.6.3 | see “Release gates” |
| 12-bit **decode** | works | corpus (`colors-animated-12bpc`, `weld_sato_12B`) decode as RGB16 |
| 12-bit **encode** via underlying zenravif (`BitDepth::Twelve`) | works | `examples/twelvebit_probe.rs`: profile-2 bitstream, container+seq say 12, rav1d-safe decodes, aomdec clean + md5-agrees |

### Honest gaps / scope notes

- **zenavif public encode API is 8/10-bit only.** `EncodeBitDepth` has no
  `Twelve`; adding a variant to the exhaustive pub enum is a breaking change
  (queued as an API-approval item). The capability exists end-to-end in the
  chain — see the probe above.
- **16-bit encode path always signals the identity matrix (MC=0, GBR).**
  Requesting `matrix_coefficients(9)` does not produce YCbCr 10-bit; the
  file honestly carries MC=0 (asserted by `pq10_metadata_survives_encode_decode`).
  `EncodePlan::matrix_coefficients_cicp` reports the truth.
- **mdcv wire order is GREEN, BLUE, RED** (ST 2086 / HEVC SEI slot order).
  `MasteringDisplayConfig.primaries` is written verbatim into those slots.
- No PQ↔linear math is applied by encode/decode — transfer functions are
  *signaled*, pixels pass through in the source transfer (by design).

## Gain maps (ISO 21496-1 `tmap`)

### Decode / extraction — verified

| Capability | Status | Proof |
|---|---|---|
| tmap parse: metadata + map AV1 payload + alt colr | works | `tests/gainmap_decode.rs` (19 tests) |
| Version-gate probes (`unsupported_gainmap_*`) refused as designed | works | `tests/integration_corpus.rs` expected-rejects |
| zencodec extras: `GainMapSource` + `DecodedGainMap` (Components) | works | `decode_gain_map_via_zencodec_extras` |
| `GainMapRender::ReconstructHdr` (ultrahdr-core math), SDR base | works | peak/CLL envelope tests + `examples/gainmap_render_probe.rs`: th=1.0 → peak exactly 1.0; th=2.0 clamps ≈1.99; full → 2.435 vs 2^1.3 = 2.46 envelope; verified on 4 vectors incl. ICC-base and gammazero |
| Streaming reconstruction == buffered | works | `streaming_reconstruct_matches_buffered` |
| **HDR-base (10-bit) reconstruction** | **honest refusal** | “ReconstructHdr requires an 8-bit base (10/12-bit not yet supported); use GainMapRender::Components” — extraction (Components) still works on those files |

### Encode / mux — verified + in flight

| Capability | Status | Proof |
|---|---|---|
| `EncoderConfig::with_gain_map` (byte-carry AV1 + ISO metadata blob) | works | `tests/gainmap_encode.rs` |
| Gain map + EXIF + ICC coexistence | works | `encode_gain_map_with_exif_and_icc` |
| Gain map on a 10-bit PQ base (+clli/mdcv) | works | `pq10_base_with_gain_map_roundtrips` |
| `GainMapMetadata::to_bytes()` normal-form metadata re-serialization | works | zenavif-parse (since 0.6.0) |
| **tmap alternate-rendition colr on encode** | **in flight** | `EncoderConfig::with_gain_map_alt_color` + zenravif `GainMapData.alt_colr_cicp` — see “Known roundtrip gaps” |
| **av1C honesty for byte-carried maps** | **in flight** | payload-derived subsampling/monochrome + dims/depth validation — same change |
| Real-vector decode→re-encode roundtrip contracts | **in flight** | `tests/gainmap_roundtrip.rs` (4 classes), lands with the fix |

### Known roundtrip gaps (being fixed in the same change series)

Found 2026-07-03 by probing the seine vector family (all gain maps there are
**4:4:4 full-color 8-bit**, and every file carries a tmap `colr`):

1. **tmap alternate colr dropped on encode.** Decode extracts it
   (`AvifGainMap.alt_color_info`); the encode chain (zenavif `GainMapConfig` →
   zenravif `GainMapData` → zenavif-serialize) had no carrier, so re-encodes
   lost the alternate rendition's CICP (e.g. the PQ signal on an SDR base).
   zenavif-serialize already has `set_gain_map_alt_colr` — the fix threads it
   through zenravif and exposes `EncoderConfig::with_gain_map_alt_color`.
   nclx-only: **every libavif vector's alt colr is nclx** (the `_icc` vector's
   ICC is on the *base*); an ICC alternate is documented-unsupported.
2. **Gain-map item `av1C` wrote defaults (4:2:0, color) instead of the
   payload's real properties.** A byte-carried 4:4:4 map (the seine family)
   was muxed with an av1C claiming 4:2:0. Fix: parse the payload's sequence
   header (`zenavif_parse::AV1Metadata`) at encode time, derive
   subsampling/monochrome, and validate caller-declared width/height/depth
   against the payload (mismatch = honest encode error). Consequence: the
   gain-map payload must now be real AV1 — hand-rolled test blobs no longer
   encode.
3. **Gain-map item's own colr** (libavif writes an `unspecified` nclx on the
   map item itself) is not written and not exposed by zenavif-parse. Readers
   use the tmap colr + ISO metadata for rendering; parity for this box is
   out of scope for now.

### zencodec trait layer (cross-codec gain-map encode)

`EncodeJob::with_gain_map_{pixels,encoded}` **landed on zencodec main**
(unpublished; registry 0.1.25 lacks it). zenavif's implementation exists as
parked workspace change `wrnqptsz` (“feat(encode): with_gain_map_… [PARKED]”,
rebased onto current main). Land order: zenavif's CodecError-envelope
migration (caterr workspace) → zencodec release past 0.1.25 → rework the
parked impl (incl. replacing its silent non-AVIF `GainMapSource` drop with an
honest error) → land. Encoding-defaults research for the pixels path:
`~/work/zen/gainmap-fidelity-study` (camera reality: ¼-per-axis grayscale
maps at ~q94; render RD knee q90).

## Release gates

- **zenavif#16 (12/57 corpus vectors fail at parse):** root-caused +
  fixed in zenavif-parse `f3c9f043` (size=0 extends-to-EOF mdat vs the OOM
  clamp). **57/57 verified 2026-07-03** against zenavif-parse rev `c36b822`
  (the 0.6.3 release-prep commit; CI green there). Ships when zenavif-parse
  0.6.3 is published from that rev — zenavif's `zenavif-parse = "0.6.0"`
  picks it up without a code change. NOTE: zenavif-parse *main* has since
  taken breaking error-API changes (`feat(error)!` At<Error> + CategorizedError)
  while still versioned 0.6.3 — 0.6.3 must be published from `c36b822`, and
  main needs a version bump before any publish from head.
- **Gain-map roundtrip fixes:** zenravif `GainMapData` field additions ride
  the unpublished 0.1.3→0.2.0 bump; zenavif `with_gain_map_alt_color` is
  additive on unpublished 0.1.7.
