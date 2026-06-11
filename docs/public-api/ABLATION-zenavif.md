# zenavif Public API Ablation Report

**Date:** 2026-06-11
**Snapshot commit:** 595d1bc4 (main)
**Snapshot counts:** 532 default / 1214 all-except-_*
**Mode:** CONSERVATIVE — default KEEP; flag only clear mistakes

**Grep template (external consumers, excluding this repo):**
```
ugrep -rn "<symbol>" /home/lilith/work/ --include="*.rs" \
  --exclude-dir=target --exclude-dir=.jj --exclude-dir=zenavif
```

---

## Summary

| Scope | Total items | Flagged A | Flagged B | Total flagged | % of total |
|-------|------------|-----------|-----------|---------------|------------|
| Default (532) | 532 | 2 | 1 | 3 | 0.6% |
| All-features delta (682 additional) | 682 | 0 | 0 | 0 | 0% |
| **Grand total** | **1214** | **2** | **1** | **3** | **0.2%** |

All-features delta is entirely feature-gated (`encode`, `zencodec`, `auto-tune`, `__expert`) — those are deliberate opt-in surfaces. No flags there.

---

## Scan Evidence

### Consumer map (as of 2026-06-11)

| Consumer | What it uses |
|----------|-------------|
| `zenpipe/zencodecs` | `AvifDecoderConfig`, `AvifEncoderConfig`, `EncoderConfig`, `encode_rgb8`, `decode_av1_obu`, `ManagedAvifDecoder`, `DecoderConfig`, `GainMapMetadata`, `GainMapChannel`, `EncodeAlphaMode`, `EncodeBitDepth`, `EncodeColorModel` (via re-export in `zencodecs/config.rs`) |
| `imageflow/imageflow_core` | `AvifDecoderConfig`, `AvifEncoderConfig` |
| `squintly` | `zenavif::decode` (free function) |
| `zenmetrics` | `EncoderConfig`, `encode_rgb8`, `encode_rgba8`, `expert::InternalParams` |
| `coefficient` | `EncodeBitDepth::Ten`, `encode_animation_rgba8`, `EncoderConfig` |
| `codec-eval` | old `PixelData` API (not in current snapshot — stale consumer, not relevant) |

**Not consumed externally (zero hits after scan):**
- `zenavif::detect` module (no live hits)
- `zenavif::QualityTarget` / `AutoTuneOptions` / `AutoTuneError` (auto-tune feature, no external hits)
- `zenavif::ColorPrimaries` / `MatrixCoefficients` / `TransferCharacteristics` as `zenavif::` prefixed types
- `zenavif::AvifDepthMap` / `AvifGainMap` (depth/gain map info structs returned from decode)
- `zenavif::DecodedAnimation` / `AnimationDecoder` as `zenavif::` prefixed types

---

## Module Tables

### Default features — flagged items

#### `pub mod zenavif::detect`

The `detect` module (`detect.rs`) provides AVIF quality estimation/re-encoding probe — `AvifProbe`, `QualityEstimate`, `ChromaSampling` (detect-local copy), `Confidence`, `ProbeError`, `Recommendation`.

**External consumer scan:** Zero hits for `zenavif::detect::` in live consumers (`zenpipe`, `imageflow`, `zenmetrics`, `squintly`, `coefficient`, `codec-eval`).

| Item | Flag | Rationale |
|------|------|-----------|
| `pub mod zenavif::detect` (entire module: ~80 items) | **A** | No external consumers found. Useful capability but unused — mark `#[doc(hidden)]` until a consumer wires it in. zenjpeg's `detect` module is used internally (its `ReencodeRecommendation` drives recompress decisions); zenavif's equivalent has no wiring in any consumer. Single-point flag for the module covers all its types. |

#### `pub struct zenavif::ColorPrimaries(pub u8)` — accidental pub field

`ColorPrimaries`, `TransferCharacteristics`, `MatrixCoefficients` are newtype wrappers over raw CICP numeric codes returned inside `ImageInfo`. The inner `u8` field is `pub` (tuple struct public field). No consumer uses these fields directly — callers receive `ImageInfo` and inspect via convenience methods or match on values from zenavif-parse re-exports.

| Item | Flag | Rationale |
|------|------|-----------|
| `pub struct ColorPrimaries(pub u8)` — the `.0` field | **B** | Change to `ColorPrimaries(u8)` (private field); add `pub fn value(&self) -> u8` accessor. Same for `TransferCharacteristics` and `MatrixCoefficients`. Currently zero external destructuring; would be a micro break if anyone used tuple field access, but grep confirms no external uses. Queue for next 0.x minor. |

> Note: `TransferCharacteristics(pub u8)` and `MatrixCoefficients(pub u8)` have the same issue — aggregate under this single B entry.

---

### All-features delta — no flags

The 682-item all-features delta is gated across five features:
- **`encode`** — `EncoderConfig`, `AnimationFrame*`, `encode_*`, `Av1Backend`, `EncodeBitDepth`, etc. Consumed by `zenmetrics`, `zenpipe/zencodecs`, `coefficient`. KEEP.
- **`zencodec`** — `AvifDecoderConfig`, `AvifEncoderConfig`, `AvifDecodeJob`, `AvifEncodeJob`, `AvifAnimationFrameDecoder/Encoder`. Consumed by `zenpipe/zencodecs` and `imageflow`. KEEP.
- **`auto-tune`** — `AutoTuneOptions`, `AutoTuneError`, `QualityTarget`. Zero external consumers found. However: the feature exists by design (picker system, docs in `docs/RAV1E_PICKER_PLAN.md`) and is feature-gated — no-cost to non-users. KEEP as-is; the `__expert` double-underscore prefix on the underlying `__expert` feature already signals instability.
- **`__expert`** — `expert::InternalParams`. Consumed by zenmetrics sweep encoder. KEEP.
- **`unsafe-asm`** — `AvifDecoder` (the C-FFI decoder). Legitimate platform-specific surface. KEEP.

---

## Top-3 Findings

1. **`pub mod zenavif::detect`** (A — `#[doc(hidden)]`): 80 items with zero live consumers. The module is complete and useful, but not yet wired into any consumer. Hiding it reserves the right to evolve the API before committing. Flip to unhidden when a consumer (e.g., zencodecs select.rs) wires it in.

2. **`ColorPrimaries(pub u8)` / `TransferCharacteristics(pub u8)` / `MatrixCoefficients(pub u8)`** (B — make field private, add accessor): Accidental tuple struct pub fields. No external destructuring; queuing as a batch B for the next minor bump.

3. **`pub struct zenavif::AvifGainMap`** — intentionally kept: this is referenced in a comment in zencodecs as the "older `zenavif::AvifGainMap`" type (deprecated path), confirmed NOT in the snapshot and NOT flagged. The comment is informational only.

---

## Items Explicitly KEPT (conservative)

- All encode/decode codec adapters (zencodec traits): KEEP — multiple consumers.
- `decode_av1_obu`: KEEP — used by `zenpipe/zencodecs` gain map and depth map decode paths.
- `ManagedAvifDecoder`: KEEP — used by `zenpipe/zencodecs/decode.rs`.
- `decode`/`decode_with`/`decode_animation`/`decode_animation_with` free functions: KEEP — used by `squintly`, `zencodecs`.
- `GainMapMetadata`, `GainMapChannel`, `ImageMirror`, `ImageRotation`, etc. (from zenavif-parse re-exports): KEEP — consumed by zencodecs gain map handling.
- `ImageInfo`, `DecodedAnimation`, `AnimationDecoder`, `DecodedFrame`: KEEP — returned from public decode paths; callers need the types even without direct `.` access today.
- `expert::InternalParams`: KEEP — live consumer in zenmetrics.
- `Av1Backend` enum: KEEP — part of `EncoderConfig` builder; `coefficient` uses `EncoderConfig` knobs.
- `EncodePlan`, `PlanInput`, `SpeedDerived`, `TilesResolution`: KEEP — `encode` feature, deliberate calibration surface.
- `auto-tune` feature surface: KEEP — feature-gated, no cost to non-users; removing would require a semver bump.
