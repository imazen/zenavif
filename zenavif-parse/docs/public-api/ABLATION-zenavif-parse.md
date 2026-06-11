# ABLATION-zenavif-parse — Conservative Public-API Review

**Date:** 2026-06-10
**Snapshot commit:** 4c23c48883e3 (main@origin)
**Snapshot file:** docs/public-api/zenavif-parse.txt (598 default items / 692 all-features items)
**Grep template:** `grep -rn "<SYMBOL>" /home/lilith/work/ --include="*.rs" 2>/dev/null | grep -v "/zen/zenavif-parse/" | grep -v "target/" | grep -v ".jj/"`

## Summary

**1 item flagged (A).** The surface is a well-structured AVIF parse API. The primary concern is `ParseOptions`, which lacks a `#[deprecated]` annotation that its sole public consumer (`read_avif_with_options`) already carries. Everything else is coherent.

Known consumers as of this scan: zenavif (main consumer, grids/animation/gainmap), pre-filter/zenavif, .jplag/zenavif, corpus-test.

## Deprecated Eager-Feature Items (already deprecated in source, note only)

The following items in the `eager`-feature surface carry `#[deprecated(since = "1.5.0")]` in the source. `cargo public-api --simplified` does not surface attribute annotations, so they appear without warning in the snapshot. They are correct as-is — the deprecation is there, just invisible in the snapshot output.

| Item | Deprecated since | Note |
|------|-----------------|------|
| `AvifData` (struct) | 1.5.0 | Use `AvifParser` instead |
| `AnimationConfig` (struct) | 1.5.0 | Use `AvifParser::animation_info()` + `frames()` |
| `AnimationFrame` (struct) | 1.5.0 | Use `AvifParser::frame()` returning `FrameRef` |
| `read_avif` (fn) | 1.5.0 | Use `AvifParser::from_reader()` |
| `read_avif_with_options` (fn) | 1.5.0 | Use `AvifParser::from_reader_with_config()` |
| `read_avif_with_config` (fn) | 1.5.0 | Use `AvifParser::from_reader_with_config()` |
| `AvifData::from_reader` (method) | 1.5.0 | Use `AvifParser::from_reader()` |
| `AvifParser::to_avif_data` (method) | 1.5.0 | Use `AvifParser` methods directly |

Consumer grep: 0 hits for `AvifData`, `AnimationConfig`, `AnimationFrame`, `read_avif`, `read_avif_with_options` in active consumer repos (zen/zenavif, pre-filter, imageflow). The zola hit is for `avif_parse::read_avif` (different upstream crate). 

## Flagged Items

### A: `ParseOptions` — missing `#[deprecated]`

**Evidence:** `ParseOptions` is in the default-features surface (line 572-587 of snapshot). Its only public use is as an argument to `read_avif_with_options`, which already carries `#[deprecated(since = "1.5.0")]`. The struct itself has only a doc comment saying "Prefer using `DecodeConfig::lenient()` with `AvifParser` instead" — no `#[deprecated]` attribute. 0 external consumer hits for `ParseOptions` across all active repos.

**Proposed action A:** Add `#[deprecated(since = "1.5.0", note = "Use `DecodeConfig::lenient()` with `AvifParser` instead")]` to `ParseOptions` at `/home/lilith/work/zen/zenavif-parse/src/lib.rs:728` (before the `#[derive(Debug, Clone, Copy)]` line).

**Impact:** Additive; emits a compiler deprecation warning for any code still using `ParseOptions`. No breakage.

## `zencodec` Feature — Deprecated No-Op, Correctly Kept

The `zencodec` feature is documented in Cargo.toml as "Deprecated no-op: zencodec is now a hard dependency. Kept so existing Cargo.toml entries with features = ['zencodec'] don't break." Active consumers found:
- `/home/lilith/work/zen/zenavif/Cargo.toml` — `zenavif-parse/zencodec`
- `/home/lilith/work/zen/gainmap-spec-status/tools/corpus-test/Cargo.toml` — `zenavif-parse = { features = ["zencodec"] }`

Keep as-is.

## Digest

- Snapshot: 598 (default) / 692 (all-features) items
- Flagged A: 1 (`ParseOptions` — missing `#[deprecated]`)
- Flagged B: 0
- ~0.2% of surface flagged
- Top finding: `ParseOptions` missing `#[deprecated]` annotation despite its only consumer being a deprecated function
- Note: 8 eager-feature items are already deprecated in source (correct); `cargo public-api --simplified` doesn't surface the `#[deprecated]` attribute, so they appear clean in the snapshot
