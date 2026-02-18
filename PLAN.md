# zenavif Thorough Update Plan

## Current State

- **Version:** 0.1.0
- **CI:** Broken — references nonexistent `managed` feature everywhere
- **Clippy:** 30 warnings with `-D warnings`
- **Fmt:** Not passing
- **Path deps without versions:** zenavif-parse, zencodec-types, ravif
- **README:** Outdated (says no animation, no encoding, references removed feature names)
- **CLAUDE.md:** Stale investigation notes

## Blocking Dependencies

| Dep | Published? | Needed For |
|-----|-----------|------------|
| zenavif-parse 0.3 | Yes (crates.io) | Core decode — add version to path dep |
| zencodec-types 0.1.0 | **No** — no GitHub repo | Core — PixelData type used everywhere |
| ravif 0.14 | **No** (0.13 on crates.io) | `encode` feature only (optional) |

## Steps

### Phase 1: Housekeeping (no dependency changes)

1. `cargo fmt` — fix all formatting
2. Fix all 30 clippy warnings
3. Commit: `chore: cargo fmt + clippy fixes`

### Phase 2: Fix CI

4. Remove all `--no-default-features --features managed` → use default features (managed decoder is already the default with `default = []`)
5. Remove `submodules: recursive` (path deps resolve from local paths, CI needs crates.io versions)
6. Commit: `ci: fix broken managed feature references`

### Phase 3: Publish zencodec-types

7. Create GitHub repo `imazen/zencodec-types`
8. Commit the uncommitted changes in zencodec-types (src/format.rs, src/pixel.rs)
9. Push to GitHub
10. Tag v0.1.0
11. `cargo publish`

### Phase 4: Update zenavif dependencies

12. `zenavif-parse`: add `version = "0.3"` alongside path
13. `zencodec-types`: add `version = "0.1"` alongside path
14. `ravif`: leave as optional path-only for now (not published at 0.14)
15. Commit: `deps: add versions to path deps for crates.io compat`

### Phase 5: Fix CI for crates.io resolution

16. CI won't have local paths for zenavif-parse and zencodec-types. With version+path, cargo falls back to crates.io when path is unavailable? Actually — need to verify this works. If not, need to remove paths entirely for CI or use workspace-level patching.
17. The `encode` feature won't work in CI since ravif is path-only. CI should only test default features (decode).
18. Test by pushing and checking CI

### Phase 6: Rewrite README

19. Update to reflect current capabilities:
    - Animation decoding AND encoding
    - Still image encoding with all features
    - Remove "❌ Animated AVIF" and "❌ Grid-based collages"
    - Add encoding features section
    - Fix feature names (managed→default, asm→unsafe-asm)
    - Update code examples
    - Keep credits and license

### Phase 7: Clean up CLAUDE.md

20. Remove stale investigation notes from 2026-02-06/07/08
21. Remove "Recent Changes" from 2026-02-06 (no longer recent)
22. Keep only current known bugs and TODO items
23. Update quick commands if needed

### Phase 8: Push and verify

24. Push all changes
25. Verify CI passes
26. Tag v0.1.0 (or decide on version)
