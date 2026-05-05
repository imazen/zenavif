# zenavif recovery register — 2026-05-08

## Verdict table

| branch | commit | date | item | what it adds | verdict | files |
|---|---|---|---|---|---|---|
| main | (current) | 2026-05-07 | post-yank state | live HEAD; no shipped 0.x crate (0.1.7 yanked) | starting point | — |
| fix/stitch-tiles-saturating-sub | — | 2026-05-06 | tile-stitch arithmetic fix | guards against width/height underflow when stitching | kept (correctness fix) | tile stitcher |
| user-wip-preserved-2026-05-06 | `a5b0042` | 2026-05-06 | rav1e InternalParams API adapt | follow-up for ravif 0.1.3 partition_range/lrf/fast_deblock plumbing changes (signature updates, non-breaking) | merged to main | — |
| feat/expert-internal-params (worktree `zenavif--expert`) | `80b884a` | 2026-05-07 | expert knob adapter layer | re-routes 4 deepest knobs (`partition_range`, `complex_prediction_modes`, `lrf`, `fast_deblock`) through `ravif::expert::InternalParams` (no bake bundling — caller can supply); Phase 0.5 individual setters removed | **CURRENT path forward** | `src/encoder.rs`, `examples/predictor_sweep.rs`, `examples/phase2_oat.rs` |
| feat/v0.5-picker-2026-05-04 | (recent) | 2026-05-04 | picker v0.5 retraining (4× larger corpus, 192³ MLP) | held-out overhead 4.07% → 3.88% (89k pairs vs 23k v0.4) | kept (training artifact) | `benchmarks/picker_v0.5_holdout_ab_2026-05-04.{md,tsv}`, `benchmarks/zenavif_picker_v0.5_2026-05-04.bin` |
| feat/v04-picker-2026-05-04 | — | 2026-05-04 | picker v0.4 baseline | superseded by v0.5 | superseded | `benchmarks/...v0.4*` |
| release/0.1.7 | — | 2026-05-02 | **YANKED release** | shipped baked rav1e picker v0_1_1 (ZNPR MLP) + 4 InternalParams expert knobs via `include_bytes!` | **YANKED** — bake bundling caused versioning brittleness | `src/auto_tune.rs`, `src/models/rav1e_picker_v0_1_1.bin` |
| feat/expert-internal-params (other worktree) | (same) | 2026-05-02 | initial InternalParams | foundation for v0.5 | superseded by 80b884a | — |

## Re-release path (Phase 2)

The 0.1.7 yank reason: bundled bake → tight version coupling → any model retraining = new binary bloat + version bump. Path forward (per user direction "caller-supplied"):

1. **Merge `feat/expert-internal-params` (`80b884a`) to main**.
2. **Remove `include_bytes!`** in `src/auto_tune.rs`. Drop the bundled `.bin`.
3. **Add public API:**
   - `with_baked_model(bytes: &[u8])` — caller supplies bake bytes
   - `with_model_path(path: impl AsRef<Path>)` — caller supplies path
   - Errors at runtime (`AutoTuneError::ModelNotBaked`) when picker is invoked without a model.
4. **Integration tests**: assert (a) bake supplied at runtime works, (b) no bake → picker is bypassed (returns default-encode plan), no error spam.
5. **CHANGELOG**: document as "picking now caller-supplied" — not regression, feature.
6. **Tag as 0.2.0** (minor bump for new API surface; not breaking since 0.1.7 was yanked).
7. **DO NOT publish** until ZNPR v3 is finalized + zenpredict 0.2.0 is shipped (per user; everyone re-bakes to v3).

## Cherry-picks for main (anti-bloat: each is shipped or required)

1. `fix/stitch-tiles-saturating-sub` → main (correctness fix).
2. `feat/expert-internal-params (80b884a)` → main as replacement for 0.1.7's bundled bake.
3. `feat/v0.5-picker-2026-05-04` artifacts (the `benchmarks/zenavif_picker_v0.5_2026-05-04.bin` + the held-out A/B markdown) → preserve under `benchmarks/recovered/`.

## Drop / archive

- `release/0.1.7` branch: archive (yanked).
- `feat/v04-picker-2026-05-04`: superseded by v0.5; archive.
- Older `salvage/...` branches and dependabot branches: archive or close.

## Notable docs to preserve

- `benchmarks/picker_v0.5_holdout_ab_2026-05-04.md` — held-out A/B numbers, parity validation.
- The yank-reason explanation belongs in CHANGELOG.md under [Yanked] section (or in a new `docs/RELEASE_HISTORY.md` if the project starts tracking yanks systematically).
