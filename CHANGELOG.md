# Changelog

All notable changes to zenavif are documented here. zenavif is an AVIF encoder
and decoder built on the excellent work of the [rav1d-safe](https://github.com/imazen/rav1d-safe)
decoder (our fork of [dav1d](https://code.videolan.org/videolan/dav1d) via
[rav1d](https://github.com/memorysafety/rav1d)),
the [zenrav1e](https://github.com/imazen/zenrav1e) encoder (our fork of
[rav1e](https://github.com/xiph/rav1e)), and the
[zenavif-parse](https://github.com/imazen/zenavif-parse) container parser.

## Workspace

### [Unreleased]

#### Added — backend + knob auto-tuning (`auto-tune`, off by default), 2026-09-04

- **`zenavif::backend_tuner`** — choose an `Av1Backend` *and* its knobs for
  one image at a quality target inside a time budget, and report the
  expected cost. `AvifTuning` trait with two implementors: `AvifTuner`
  (a ZNPR v3 bake the **caller** supplies — no `include_bytes!`, no
  bundled weights) and `StubTuner` (measured defaults, no model), with
  `AvifTune::source()` always reporting which answered. New public items:
  `AllowedBackends`, `AvifTune`, `AvifTuner`, `AvifTuning`, `StubTuner`,
  `TuneRequest`, `TuneSource`, and the `backend_tuner` module
  (`TuneContract`, `TuneCell`, `TuneHead`, `stub::WallTimeModel`).
  Purely additive; no existing signature changed. (`04fa3093`)
- Wall-time table transcribed from the committed speed instrument
  (`speed_alpha_beta.tsv`, sha256 `c7f63157…`), 20 `(backend, speed)`
  `alpha + beta*MP` rows. A test reproduces the backend campaign's own
  published iso-time row from them. `zenav1-aom` has **no** rows because
  it was never measured — every aom lookup returns `None` (NOT MEASURED),
  never an analogy.
- Knobs are **backend-scoped**: `tune=still|psycho` (rav1e) vs
  `svttune=<u8>` (SVT), and the QM window (`qmmin`/`qmmax`) exists only on
  SVT. A knob on the wrong backend is a load error, and a cell declaring
  SVT knobs on a build without `__expert` is refused rather than encoded
  with the knobs dropped — the `zenav1-svt#17` defect class.
- Bake contract documented at `docs/AUTOTUNE_CONTRACT.md`.
- 21 unit tests + 7 integration gates with real encodes, `av1C` decode
  read-back, and the model path driven end to end by a hand-baked
  contract-carrying ZNPR. (`cdfe7b46`)
- Adds `zenpredict-bake` as a dev-dependency (the canonical ZNPR
  serializer, for the test bake).


- **2026-08-29 — Known issue: this repository cannot be resolved by anything that
  clones only this repository. That breaks `Dependabot Updates`, and it blocks
  `cargo publish`.** Investigated as part of a workspace-wide sweep of six repos
  showing a red `Dependabot Updates`. Both symptoms have the same single cause, and
  both were reproduced rather than inferred.

  **Cause.** `Cargo.toml:105` declares
  `aom-decode = { package = "zenav1-aom-decode", path = "../zenav1-aom/crates/aom-decode", optional = true }`
  — a path that escapes the repository root. `cargo` must load a path dependency's
  manifest during resolution even when the dependency is optional and its feature
  (`aom-backend`) is off, so in a fresh standalone clone of `main` **every** cargo
  command fails before it starts:

  ```
  error: failed to load manifest for dependency `zenav1-aom-decode`
  Caused by: failed to read `.../zenav1-aom/crates/aom-decode/Cargo.toml`
  Caused by: No such file or directory (os error 2)
  ```

  Verified with `cargo metadata`, `cargo metadata --no-deps` and
  `cargo generate-lockfile`, all exit 101 in a clean clone.

  **Why CI is green anyway.** Every job in `ci.yml`, `fuzz.yml`, `linku-corpus.yml`
  and `release.yml` runs `./.github/actions/clone-siblings` immediately after
  checkout, which clones `imazen/zenav1-aom` (and `zenanalyze`, `cavif-rs`,
  `zenav1-svt`, `zenrav1e`, `zensim`) into `../`. Dependabot is a GitHub-managed
  job with no checkout step of ours, so it has no equivalent and cannot be given
  one. That asymmetry — green CI, red Dependabot — is the whole story.

  **This also blocks publishing, which is why `Release` has never succeeded.**
  `cargo publish --dry-run --no-verify -p zenavif` fails at manifest verification:

  ```
  error: failed to verify manifest at `.../Cargo.toml`
  Caused by: all dependencies must have a version requirement specified when
    publishing. dependency `zenav1-aom-decode` does not specify a version
  ```

  `zenav1-aom-decode` is unpublished on crates.io, so it cannot be given a version
  requirement without publishing it first. It is not the only blocker: `Cargo.toml`
  carries **six** `git = "https://…"` dependencies (including `rav1d-safe`), none
  with a `version` key, and `cargo publish` rejects those for the same reason. The
  `Release` workflow does run `clone-siblings`, so its failure is **not** a
  checkout problem — the crate is simply not in a publishable state, and no CI
  change can make it one.

  **Note for whoever picks this up:** the `aom-decode` path dep was intended to be
  temporary. Its own comment (`Cargo.toml:104`) reads *"Return to a git-rev pin on
  `imazen/zenav1-aom` before this branch lands"* — it landed on `main` on 2026-08-06
  without that revert. Restoring the git-rev pin would make the repo resolvable
  standalone again (git dependencies resolve fine in a lone checkout) and would
  remove one of the two publish blockers. It is **deliberately not done here**: it
  reverses a stated intent to let the decode-backend work and the in-repo
  zenav1-aom work land together without a publish round-trip, which is a call for
  the owner, not a drive-by edit.

  **UPDATE 2026-09-02 — the AV1 half of this is now done** (`85af725`). Both
  backend deps are git-rev pins again, so `../zenav1-aom` and `../zenav1-svt`
  are no longer needed to resolve this repo, and the new CI job
  `resolve-standalone` keeps them that way (it clones only `../zenanalyze`,
  asserts the two AV1 siblings are absent, and requires `cargo metadata` to
  succeed; proved able to fail on the previous manifest). The judgement below
  was reversed by a second measurement the original entry did not have: the
  path dep was also making the svt seam build against a **dirty** sibling
  working copy — 71 uncommitted lines in `svtav1-encoder` on 2026-09-02 — so it
  was not only costing Dependabot, it was making local results
  non-reproducible. `zenanalyze` / `zenpredict` remain escaping path deps and
  are now the sole residual cause of standalone-resolution failure; that is a
  separate decision. The publish blockers are unchanged (six `git =` deps with
  no `version` key).

  **Decision on Dependabot (2026-08-29, superseded above): not "fixed", and not
  silently tolerated.** Making it work means giving up the sibling-path
  arrangement, which is not worth an automated PR bot. Two things make that acceptable: this repo has **zero open
  Dependabot alerts**, and — the correction that matters — **alerts are unaffected
  by the updater failing.** Alerts come from GitHub's dependency graph, which
  parses committed lockfiles and never runs cargo, so advisories are still
  reported here; only the automatic pull request is lost.

  **Owner action, if the red mark should stop:** Dependabot **security** updates
  cannot be disabled by any file in the repository — `.github/dependabot.yml`
  configures *version* updates only. The switch is Settings → Code security →
  "Dependabot security updates" (currently `{"enabled": true, "paused": false}`).
  Turning it off ends the red mark and does **not** stop the alerts.

  (The April 2026 failures predate the `aom-decode` path dep and their logs are
  past 90-day retention, so that specific cause is unrecoverable; the advisory
  behind them, GHSA-cq8v-f236-94qc on `rand`, reached `state: fixed` on 2026-05-08.)

- **2026-08-29 — CI: pushes to `main` now cancel their superseded runs.**
  `ci.yml` and `linku-corpus.yml` keyed their concurrency group on
  `${{ github.head_ref || github.run_id }}`. `github.head_ref` is populated only
  for `pull_request` events, so on a push it was empty and the group fell through
  to `github.run_id` — unique per run, so no two pushes ever shared a group and
  `cancel-in-progress` could never fire. Both matrices carry `macos-latest` plus
  `macos-26-intel`, so the wasted runs landed on the scarcest runner pool. Both
  now key on `${{ github.ref }}`, which is set for both event types
  (`refs/heads/main` on push, `refs/pull/N/merge` on a PR): PR cancellation is
  unchanged and consecutive pushes supersede each other. `linku-corpus.yml` keeps
  its distinct `linku-` group prefix so the corpus run and `ci.yml` still never
  cancel one another.
- **2026-08-29 — rav1d-safe pin `140f9145` -> `66f58fa6`; the
  `row_sink_decode_is_byte_identical_to_buffered_decode` CI flake is fixed at
  the source (rav1d-safe#524 / `3426ebf7`), and this repo's diagnosis of it was
  wrong.** The note in `Cargo.toml` and the Known Bugs entry in `CLAUDE.md`
  both read the panic (`& _[73721..73729]` vs `&mut _[73728..74112]`, one
  shared index) as another `fdd6a35`-shaped over-reservation — "an 8-byte
  window where the filter reads 7 taps". It was not a false positive. The
  guard was HONEST: the 8bpc x86_64 H kernels load all 8 bytes.
  `loop_filter_4_8bpc_wd6_simd_h`'s `load_row_hi` pulls a 4-byte chunk at `+1`
  and `..._wd16_simd_h`'s `load_chunk(_, 5)` pulls one at `+5`, and only lanes
  0/1 of each survive `transpose4` (`hi[2]`/`hi[3]`, `c3[2]`/`c3[3]` are never
  bound) — the trailing lanes were genuinely read and discarded, so narrowing
  the guard would have made it lie. The defect the geometry actually points at
  is a second, compounding one: the H window was sized from the **plane's**
  worst case ((3,5) chroma / (7,9) luma) rather than from the run's mask
  (`lf_run_reach`). `default_picture_alloc` pads a stride only when it is a
  multiple of 1024, so a 768-wide 4:2:0 frame's chroma plane (stride 384 = its
  width) gets none — and `tests/vectors/libavif/kodim03_yuv420_8bpc.avif`, the
  fixture this test decodes, is exactly 768x512 4:2:0. At the last 4-column
  group `edge + 5` is the first pixel of the NEXT picture row, which
  `owned_recon::stitch_sbrow` legitimately holds `&mut` over for the next
  superblock row. Upstream fixed both halves: the kernels now load only what
  they consume (dead lanes zero-filled — bit-identical by construction, since
  `transpose4`'s outputs 0 and 1 depend only on input lanes 0 and 1), and both
  dispatchers size the H window from `lf_run_reach` through one shared
  `lf_compact_window`. The SIMD-vs-scalar fallback predicate deliberately keeps
  its old 9/5 values so the SIMD/scalar decision — and therefore every output
  byte — is unchanged. The affected module is `cfg(x86_64|wasm32)`; aarch64
  routes through `loopfilter_arm`, which was never exposed. Notes corrected in
  `Cargo.toml`, `CLAUDE.md` (which also had a stale claim that rav1d-safe is
  supplied via `[patch.crates-io]` — it is a direct git-rev dependency), and
  `fuzz/Cargo.toml`'s mirrored pin.

  The bump spans ~96 upstream commits; two of them change behaviour zenavif
  sits on top of, and neither moved any gate here:

  - **`Settings::strictness` now defaults to `Strictness::Strict`**
    (rav1d-safe `2e0f7e8`). Every zenavif decode path builds
    `Settings::default()`, so non-conforming streams — dav1d's
    `strict_std_compliance` checks plus the AV1 6.10.8 `segment_id` bound —
    are now `Error::InvalidData` instead of concealed garbage pixels. The
    deprecated `strict_std_compliance` flag still works (stricter of the two
    wins) and zenavif never set it. Upstream surveyed 315 AVIFs/OBUs from
    codec-corpus + this repo's `tests/`: identical verdicts except a
    deliberately corrupted vector, which Strict correctly rejects.
  - **`Decoder::flush()` drains before it resets** (rav1d-safe `59eb17b`,
    rav1d-safe#423) instead of resetting first and dropping frames it still
    owed. zenavif calls `flush()` at end of input in `src/decode_av1.rs` and
    `src/decoder_managed/decoder.rs`.

  Everything else in the range is docs, benchmark records, guard/tracker
  bookkeeping, or SIMD load-narrowing that is bit-identical by construction
  (`ee07356` x86 16bpc MC, `6f6081f` aarch64 16bpc bilinear MC, `08245d9`
  negative-stride block guards, `d973628` `owned_recon` band width). No
  committed decode MD5 reference value changed anywhere in the range — the
  only edits to `tests/decode_md5_committed.rs` are additions plus
  `Strictness::Lenient` pins on the cross-arch parity references.

  Verified locally on aarch64 at the new pin, every CI feature set green:
  `cargo test --workspace` 390 passed / 0 failed; `--no-default-features` 206;
  `-p zenavif-parse --features eager` and `--all-features` 151 each;
  `--features aom-backend,encode,encode-imazen,encode-svt-rs,target-quality`
  416; `--features two-pass-butteraugli,two-pass-zensim,auto-tune` 434; clippy
  (root + both members) and `cargo fmt --check` clean; `gate_kit determinism
  --ci` PASS (3 cells x 5 thread legs, 0 failures); and `linku_decode_all`
  156/156 — the corpus most exposed to the new `Strictness::Strict` default,
  with zero newly-rejected samples. (`linku_pixel_parity` reports 12 pixel
  failures + 3 `irot` size errors on this box, but an A/B at both revs gives
  identical tallies — it is a homebrew-vs-Ubuntu `avifdec` difference, now
  documented in `CLAUDE.md`.) The flaking test itself
  passed 50/50 consecutive runs — but note that is a **regression gate for
  this platform, not proof of the fix**: `safe_simd::loopfilter` is
  `cfg(x86_64|wasm32)`, so the repaired kernels cannot execute on aarch64 at
  all. The fix's runtime proof is CI's x86_64 legs.

- **2026-08-29 — `encode-svt-rs` seam adapts to three retired/changed upstream
  shapes; CI red -> green (was run 33226351115).** All three are the seam
  catching up to `imazen/zenav1-svt` movement on 2026-08-28/29, and none of
  them relaxes a gate:

  - **QP 0 is IMPLEMENTED upstream, not refused** (`aeb619cd8` + `75cf7b0f7`,
    zenav1-svt#5 chunk 2; `129d45494`, issue #9 items 6-7 — `encode_yuv420`
    emits a real AV1 bitstream and `with_lossless` is honoured on 4:2:0). This
    is the first CAPABILITY refusal that port has RETIRED (inventory 15 -> 14),
    and it is what turned zenavif CI red on every platform:
    `svt_rs_direct_qp0_rejected_typed` asserted a refusal that no longer
    exists. REPLACED — not deleted, not loosened — by
    `svt_rs_direct_qp0_codes_lossless_420`, which demands the STRONGER
    property the refusal stood in for: a qp0 stream must decode under our own
    decoders with recon == source EXACTLY, on all three planes, at 64x64 +
    128x64 x SVT presets {6,7,9}, with aom-rs byte-agreeing with rav1d-safe.
    Mutation-verified (re-run at qp 1, it fails at the first luma pixel). The
    arms upstream still refuses keep typed assertions in
    `svt_rs_direct_qp0_typed_refusal_outside_420_8bit` (monochrome + 10-bit,
    each pinned to its own `lossless_config_error` arm by message). The seam's
    QP >= 1 clamp is RETAINED, now documented as a product choice rather than a
    corruption guard — quality 100 must encode, and RGB -> 4:2:0 means
    coded-lossless AV1 is still not a lossless image round-trip.
  - **The partial-superblock preset floor is removed for the 4:2:0 colour
    path.** It rested on "presets 0-5 are not C-identical on a partial SB",
    which upstream retired on 2026-08-04 by making the PD1 refinement walk
    edge-aware and dropping its `full_sb` gate; `partial_sb_gate.sh` gained a
    23-cell presets-0-5 block, all byte-identical to real SvtAv1EncApp v4.2.0
    (146/146 aarch64, 145/145 x86-64 CI — the delta is an ISA-scoped C-side
    divergence, upstream SUSPECTED-C-BUGS #9). The residual upstream names is
    `screen` content at p0/p1/p2 (+4 at p4), issue #71's palette/IntraBC RD
    class, which also fires on 64-ALIGNED frames this seam always accepted —
    so it is not dimension-conditioned and a dimension gate never addressed
    it. `PARTIAL_SB_MIN_PRESET` -> `MONO_PARTIAL_SB_MIN_PRESET`, gating only
    the Cs400 mono path (nothing upstream measures mono partial SBs below
    preset 6). New gate `svt_rs_partial_sb_roundtrip_at_low_presets` (96x96 /
    65x72 / 100x37 x speeds 1-4 = presets 0/1/3/4; measured 49.3-50.9 dB vs a
    38 dB floor) — extended in `20e158e6` with a direct-pipeline arm where
    rav1d-safe and aom-rs must BYTE-AGREE on all three planes (53.1-55.2 dB
    luma across the same 12 cells), because a single-decoder PSNR floor cannot
    tell a correctly-coded edge superblock from one that decoder merely
    tolerated — the distinction that caught the mono edge-leaf bug. The mono
    floor keeps a refusal gate in
    `svt_rs_mono_partial_sb_still_refused_below_preset_6`, which also asserts
    the same geometry validates without alpha.
  - **10-bit post-filter doc corrected.** `src/encoder_svt_rs.rs` claimed the
    deblock / CDEF / Wiener searches "still decide on MSB-truncated planes".
    Stale: upstream hbd chunk 2 (`f319ec298`, on chunk 1 `35743ebd5`) threads
    the caller's native u16 into all three, and an unconsumed native source is
    now a typed refusal rather than a silent truncation
    (`bd10_hbd_src_gate.sh` 100/100 byte-identical to C).

  Also: the sibling path dep needed no `use` path changes for the 6 -> 4 crate
  consolidation (`bfae1b690`, zenav1-svt#3) — the facade re-exports
  `svtav1::tables` / `svtav1::entropy` — verified by building the full
  `aom-backend,encode,encode-imazen,encode-svt-rs,target-quality` set. The
  Cargo.toml dep comment is rewritten to match (there is no rev to bump: CI
  clones the sibling from `origin/main`).

  Commits: `4560ceaf` (the seam + gates, closes #41), `6e8694a1` (a `style:
  cargo fmt` of `examples/zensim_cq_rd.rs` — the Format job had been red on
  main since `e50a983` independently of this work, and was blocking every
  push), `e421d5c2` (BACKEND_SUPPORT_MATRIX + README sync), `20e158e6` (the
  cross-decoder arm above).

- **2026-08-28 — lockfile `zenav1-svt` entries: restored, then un-restored.**
  Retraction. Re-resolving for the zenanalyze-api change also collapsed
  `zenav1-svt-entropy` / `-tables` into their parents, and I read that as a
  sibling path dep capturing another session's uncommitted refactor, then
  "restored" the old shape. Wrong on both counts: the fold is
  `zenav1-svt@bfae1b69` and it is **on that repo's `origin/main`**, so the
  collapsed lock was the correct resolution and the restore pinned a stale
  pre-fold shape. Reverted; the lock again matches the pushed sibling.

  The hypothesis was tested and disproved rather than assumed: the restore
  shipped, CI ran (33224361675), and `svt_rs_direct_qp0_rejected_typed` failed
  exactly as before.

- **2026-08-28 — `zenanalyze-api` unified to a crates.io version + one
  `[patch.crates-io]`; the shared-Offer reuse paths are now version-pinned per
  feature.** Owner directive: "zenanalyze-api should be the sole contract and
  intermediary so different zenanalyze versions can compile together"
  (`docs/sole-contract.md` in imazen/zenanalyze).

  The dep was a **git-rev pin** (`47b4d0f5`) on the theory that pinning every
  codec to the same rev keeps the contract type unified. It doesn't: zenjpeg
  carried that rev, zensquoosh a different one (`7b84d53c`), and
  zenpipe/zencodecs the registry+patch form — three Cargo sources for one crate,
  so a graph combining them got three incompatible `Offer` types (zenpipe hit
  exactly this and recorded the E0308). Worse, the pinned rev was old enough
  that zenavif was compiling against a **superseded contract API**
  (`Request::new(names, analyzer_version, defs_version, config_hash)`), which
  simply does not exist in the published crate — so this crate could not build
  in the same graph as any correctly-pinned consumer. Now
  `zenanalyze-api = "0.1.0"` plus one workspace-root patch, which rewrites every
  edge (including zenpicker's and zenpredict's) to a single source.

  Migrating to the current contract changed the reuse gate for the better. The
  old one was global — analyzer version + `feature_defs_version` + config hash,
  all-or-nothing. The new one is **per feature**: `reuse_pinned` (`auto_tune.rs`)
  builds each want's qualified `name@hex8` from THIS build's
  `feature_version_hash_by_name`, so a single re-defined column misses and the
  caller runs its own pass, instead of one upstream bump invalidating every
  offer. Applied to `auto_tune::reuse_from_offer`, `palette_gate_for_rgb8`,
  `fast_heads::{fast_tier_budgets_for_rgb8, monotone_speed_gate_for_rgb8}`, and
  `q0_head::predict_q0_for_rgb8`. `reuse_from_offer` additionally keeps the
  model's own `analyzer_version` / `config_hash` stamps as a pre-gate.

  One asymmetry is now explicit in code: a shipped bake can name a feature that
  was culled upstream (the rav1e picker names `text_likelihood`), and the
  own-pass path fills those with 0.0. Reuse now does the same — pinned for every
  column the build still defines, 0.0 for the ones it doesn't — so offer-reuse
  and own-pass stay identical, which is what
  `auto_tune_offer_reuse_matches_own_pass` gates. No behaviour change for any
  column the build defines.

  Verified: `cargo test --features auto-tune --lib` (199 pass) and
  `cargo clippy --features auto-tune --all-targets -D warnings` clean.


- **2026-08-27 — svt-rs: alpha/grayscale (Cs400) streams take partial
  superblocks at speed ≥ 5 (SVT preset 6), same as the colour path.** The
  seam had held mono at preset ≥ 7 because the port's mono path mis-coded
  partial SBs at preset 6 (`encode_fixed_tree` coded a one-false edge leaf
  as a PARTITION_NONE square; 96x80 18 dB garbage, 128x80 / 200x136
  undecodable). zenav1-svt `b6a1737a` fixes the mono arm, and `1ed7db46` the
  second defect that exposed (the now-rect edge block straddled a thin right
  edge and its recon store wrapped into the next row — 200x136 decoded at
  27.9 dB with the first SB row clean and every later row wrong from column
  0; aomdec still decoded it), so
  `MONO_PARTIAL_SB_MIN_PRESET` is gone and `svt_rs_dims_error` applies one
  preset rule (multiples of 8 still required for mono — no TRUE→ALIGNED
  padding there). The canary
  `svt_rs_direct_mono_partial_sb_preset6_still_broken` — which had panicked
  in the "AV1 backend seams" CI step on every platform since it landed,
  because the pack's `debug_assert` fires in the dev profile where the
  release measurement had shown garbage — is now the round-trip gate
  `svt_rs_direct_mono_partial_sb_preset6_roundtrips` (7 geometries,
  rav1d-safe + aom-rs byte-agreeing, 96x80 56.18 dB at qp 10). Tests
  `svt_rs_rgba_partial_sb_needs_8_aligned_dims_at_speed_5` /
  `svt_rs_gray8_partial_sb_needs_8_aligned_dims_at_speed_5` now encode
  96x80 at speed 5 and keep the speed-4 refusal.

- **2026-08-27 — `zenravif` is a git-rev dep, so the `encode` chain resolves
  for git consumers.** `ravif` (package `zenravif` 0.2.0, unpublished) was the
  sibling path `../ravif/ravif`, which escapes this repo and made any
  `zenavif = { git = … }` consumer fail at resolution (zencodecs / zenpipe CI
  had been red at `cargo test`'s resolve step since 2026-08-01). Now pinned to
  imazen/cavif-rs `f6c883b6`, where zenravif's own unpublished deps are
  git-rev pins as well — zenrav1e 0.2.0 (was path-only, `09a0dba3`) and
  zenavif-serialize 0.2.0 (was a root `[patch]` no consumer inherited,
  `f6c883b6`; same archived-repo rev `990bd6d` it already built against) —
  so git consumers need no patch of their own. Manifest + lockfile only; the
  compiled sources are the same shas the sibling checkouts were at. A new
  `[patch."https://github.com/imazen/zenavif-serialize"]` folds zenravif's
  copy onto the workspace member (one zenavif-serialize in `encode` + test /
  `encode-svt-rs` builds; cargo reports the entry unused, as a warning, in
  builds without `encode`).

- **2026-08-14 — decode negotiation converged across all five entry points
  (zenavif#39, #38, #37, #36).** `preferred = [Rgb8]` over an HDR file no
  longer panics: `negotiate_format` reduced formats through
  `PixelBuffer::to_rgb8()` / `to_rgba8()`, which `expect` on
  `RowConverter::new`, and the arm was selected by the CICP read out of the
  file — untrusted input, reachable from an ordinary caller preference. 72 of
  990 (fixture x `preferred` x entry point) cells panicked; now 0, with all
  918 previously-working cells byte-identical. The same defect on a second
  surface: the five `decode_into_*` convenience methods, which take no
  `preferred` list at all, panicked on 18 of 90 (fixture x method) calls and
  now return `Error::Unsupported`. The streaming decoder and the row sink now
  run the `preferred` reduction per strip (40 of 374 format disagreements with
  the buffered decode -> 0), describe their pixels with the container's CICP
  (15 of 34 lost tags -> 0; an HDR PQ image was handed over labelled
  `transfer: Unknown`), and refuse `GainMapRender::ReconstructHdr` instead of
  silently returning the SDR base as `Ok`. New gate
  `tests/negotiation_matrix.rs` (10 tests) sweeps the whole product; new fuzz
  target `fuzz_decode_negotiate` reaches the negotiation layer, which no
  existing target did.

- **2026-08-07 — zensim CQ target-hitting loop harness + ADDITIVE per-SB
  hint hook (zensim campaign appendix AC.4).** New example
  `zensim_cq_rd` (features `encode-imazen,two-pass-butteraugli`):
  seed-CQ encode → decode → folded-944 zensim scoring → the jxl-adopted
  proportional controller (exp 1.0 / per-step clamp 2.0), arms baseline /
  h3-mag (per-64px-superblock attribution steering) / outer CQ-bisection
  comparator; pre-registered study
  `benchmarks/zensim_avif_loop_2026-08-07.md` (G-AV2 smoke MEASURED and
  committed; the G-AV3 matrix runs when the wave-12 candidate bake
  lands, shipped C as control) + runner
  `scripts/zensim-loop/run_avif_loop.sh`. New public API:
  `EncoderConfig::with_sb_q_scale` / `sb_q_scale_value`
  (`two-pass-butteraugli`) — external access to the per-superblock AC
  quantizer scale map the butteraugli two-pass driver already forwards;
  release-gated inert until zenravif `FRAME_HINTS_LIVE` (the zenrav1e
  >0.1.4 dep bump), gated by `tests/sb_q_scale_hint.rs` which asserts
  the real behavior of BOTH gate states. Dev-deps: renamed `zensim03`
  path dep on sibling zensim 0.3.0 (registry 0.2.4 deps untouched); CI
  `clone-siblings` now also clones imazen/zensim.
- **2026-08-07 — `hdr_encode_cell` example stamps MEASURED clli (zensim
  campaign appendix AA).** The example previously wrote a hardcoded
  `content_light_level(1000, 250)` on every file regardless of content;
  it now measures MaxCLL/MaxFALL from the decoded PQ pixels via the
  zenpixels owner (`zenpixels_convert::CllMeasure::measure_max`, MaxRGB
  per CTA-861.3, ST-2084 EOTF → cd/m²), writes no clli for non-PQ inputs
  rather than a guessed one, and its self-check asserts the container
  echoes the measured values. The mdcv stand-in stays declared (ST 2086
  describes the mastering display — not measurable from pixels). The
  library's clli/mdcv setters remain caller-supplied passthroughs
  (declared-by-necessity; nothing is invented when absent).
  zenpixels/zenpixels-convert min 0.2.16 (+`hdr-experimental`).
- **2026-07-16 — absorbed the zenavif-parse and zenavif-serialize
  repositories.** Both crates now live here as workspace members
  (`zenavif-parse/`, `zenavif-serialize/`) with their complete git
  histories rewritten under those paths, and all tags imported under
  lineage prefixes: `zenavif-parse-v*` / `zenavif-serialize-v*` (fork
  releases), `avif-parse-v*` (kornelski upstream), `mp4parse-v*` (Mozilla
  lineage). GitHub releases were recreated here from the source repos.
  Releases are per crate via crate-prefixed tags (release.yml routes on
  the prefix to `cargo publish -p <crate>`); the sections below remain
  the zenavif crate's changelog.

## zenavif-parse

History and future entries: [`zenavif-parse/CHANGELOG.md`](zenavif-parse/CHANGELOG.md).
Queued at absorb time: 0.6.3 must publish from pre-break commit `c36b822`
(rewritten equivalent in this repo's history); next from head is 0.7.0
(breaking `At<Error>` returns, already on main).

## zenavif-serialize

History and future entries: [`zenavif-serialize/CHANGELOG.md`](zenavif-serialize/CHANGELOG.md).
Queued at absorb time: next release is 0.2.0 (breaking `At<SerializeError>`
write-path returns + gain-map interop additions, already on main).

## [Unreleased]

### BREAKING — version bumped `0.1.8` -> `0.2.0` (user-approved 2026-09-03)

The breaks ship together, as the QUEUED BREAKING CHANGES section required.
Nothing is published: the crate is still not in a publishable state (see the
Workspace section), so this is a version in tree, not a release.

- **`EncodeBitDepth` gained `Twelve` and is now `#[non_exhaustive]`.** Both in
  one break, exactly as queued: retrofitting `#[non_exhaustive]` later would
  itself have been a break, so every future depth or policy variant is additive
  from here. Downstream exhaustive matches need a `_` arm.
- **`EncodeColorModel`, `EncodeChromaSubsampling` and `EncodeAlphaMode` are now
  `#[non_exhaustive]`.** Each names a strict subset of what the format or the
  policy space allows — `EncodeChromaSubsampling` names two of AV1's four
  (4:4:4 / 4:2:2 / 4:2:0 / 4:0:0), and the other two are policy sets rather
  than closed spec domains — so they will grow, and this is the release to pay
  for it in. **`EncodePixelRange` is deliberately NOT marked** — it mirrors H.273's
  `video_full_range_flag`, a single bit, so the domain is genuinely closed and
  forcing a `_` arm on every consumer would buy nothing.
- **`Av1Backend`, `DecodeBackend` and `TargetMetric` `#[non_exhaustive]` +
  `TargetMetric::ZensimC` + `ValidationError::BackendUnsupportedParam` ship
  here.** These were already in tree and queued for "the next 0.x minor bump";
  this is that bump.

`docs/public-api/*` is **not** regenerated for this break. Those snapshots can
only be produced on Linux (`just api-doc` builds rustdoc JSON over all manifest
features, which includes `unsafe-asm`, whose rav1d `.S` sources Apple `cc`
refuses to assemble) and were already stale on `main` before this change. No CI
job runs `api-doc`, so nothing gates on them. The API delta is: one new
`EncodeBitDepth` variant plus four `#[non_exhaustive]` attributes — verified by
diffing every `pub fn` / `pub struct` / `pub enum` / `pub const` signature in
`src/encoder.rs` against `ec6728b`, which comes back **empty**, and by
`dev/downstream-probe` compiling against the result.

**Deliberately NOT cleared from the queue:** the deprecated feature aliases
(`encode-svt-rs`, `aom-backend`) and enum aliases (`Av1Backend::SvtRs`,
`DecodeBackend::AomRs`, `EstimateArm::SvtRs420`) were marked "removed in 0.2"
and are **kept**. A live consumer was still building against the old spelling
when 0.2.0 was cut, and removing the names would have broken that build for no
benefit. Their `#[deprecated]` notes and Cargo comments now say the removal is
deferred and is not tied to a version. `tests/deprecated_backend_aliases.rs`
still gates them. Also not cleared: removing
`Error::ColorConversion(yuv::YuvError)` — the only remaining constructor is in
`src/decoder.rs`, which is behind `unsafe-asm` and is compiled by nothing (not
CI, not this box), so removing it would be an edit made blind. It stays queued.

### Fixed — `validate_for_input` accepted alpha the aom seam refuses (zenavif#44)

`8395e86` fixed one instance of a class — `validate_aom_scope` missing the
encode path's lossless refusal. zenavif#44 found the second, in
`validate_for_input` rather than `validate`: an **alpha** input validated and
then failed at `encode_rgba8`, because the aom seam does not build the Cs400
`auxl` alpha item. `input_has_alpha` is exactly the config x input property
that method exists to check, and the zenav1-svt backend needs no such arm
because it *implements* alpha. Now refused there with
`BackendUnsupportedParam { param: "alpha input" }`.

The issue's other observation is also addressed, in the opposite direction:
16-bit input used to be rejected for this backend only **incidentally**, by the
generic identity-RGB-has-no-4:2:0 rule, with no aom-specific check behind it.
That would have become the wrong answer the moment the seam grew a 16-bit entry
point — which it just did, so the aom backend is now excluded from that rule
and 16-bit RGB validates, with a comment saying why.

Gated in `validate_agrees_with_the_encode_path`: alpha input must fail
`validate_for_input`, the same config without alpha must pass, and 16-bit RGB
must pass. Mutation-verified — neutering the new arm turns that test red.

### Verified — the 7-platform Test matrix COMPLETED, for the first time

Run 33710032112 on `820947a`: **15/15 jobs green**, `Fuzz` and `link-u corpus`
green on the same commit. Worth recording because the matrix had never finished
before — consecutive pushes kept superseding it under
`cancel-in-progress`, so every "green" before this was a partial.

The gate is confirmed to have RUN, not merely to have not failed. All six
native Test platforms — ubuntu-latest, ubuntu-24.04-arm, macos-latest,
macos-26-intel, windows-latest, windows-11-arm — print
`aom_encode_backend ran: 25 tests passed` from the assert-the-gate-ran step.
(`Test (i686 cross)` is a separate `cross` job with its own reduced step list
and has never carried that step; its absence there is by design, not a silent
skip.) The downstream-consumer step printed its encodes on ubuntu-latest:
`aom Eight: 364 bytes`, `aom Ten: 542 bytes`, `aom Twelve: 415 bytes`, and the
zenravif 12-bit refusal text.

### Added — `dev/downstream-probe/`, the `cargo semver-checks` substitute

`cargo semver-checks` cannot run on this crate. `dev/downstream-probe/` is an
out-of-crate consumer with its own `[workspace]`, run with
`CARGO_TARGET_DIR=../../target cargo run --release` (seconds — it shares the
parent's target dir) and wired into CI on `ubuntu-latest` only, since the break
it detects is platform-independent and it rebuilds the dependency graph in its
own feature unification. It exercises the `zencodec` surface imageflow uses, the
`EncoderConfig` builder at all three depths, and **exhaustive matches on every
enum that became `#[non_exhaustive]`** — the only place that break's shape shows
up, since in-crate matches are unaffected by the attribute. It also asserts that
`EncodePixelRange` still matches WITHOUT a wildcard, which is the claim behind
leaving it unmarked.

Measured blast radius of the 0.2.0 bump, from running it:

- Every downstream `zenavif = "0.1.x"` requirement must move to `"0.2.0"`.
  zenpipe pins `0.1.7`, imageflow `0.1.4`.
- Neither zenpipe nor imageflow matches on the newly `#[non_exhaustive]` enums —
  both go through `AvifEncoderConfig` / `AvifDecoderConfig` — so beyond the
  version requirement the enum break costs them nothing.
- **Pre-existing, unrelated to this bump:** imageflow requests
  `features = ["zencodec", "encode"]`; `zencodec` is a required dependency here,
  not a feature, and has not been one on `main` for some time, so that request
  already fails to resolve.
- **Any consumer needs its own `[patch.crates-io]`** for `zenavif-serialize`
  (0.2.0, unpublished) and `zenanalyze-api` (0.1.1, unpublished): `[patch]`
  tables in a dependency's manifest are ignored, so this crate's root patch
  table does not reach downstream. Also pre-existing.

It carries two more binaries, both measurement tools rather than gates:
`emit` writes one AVIF per coded depth for an external decoder to read, and
`bd8_anchor` prints the 60-cell length+hash table the 8-bit byte-identity
benchmark diffs. `default-run` is set so the bare `cargo run` CI uses stays
unambiguous.

### Added — 10- and 12-bit 4:2:0 through the zenav1-aom encode backend

`Av1Backend::Zenav1Aom` codes **8, 10 and 12 bits** on its RGB -> 4:2:0 colour
path, from both the 8-bit (`encode_rgb8`) and 16-bit (`encode_rgb16`) entry
points. Before this it was 8-bit only and `encode_rgb16` never reached the seam
at all.

**The two blockers the refusals named were both already cleared, and the
refusal text was stale:**

- *"this seam has no u16 forward RGB->YUV path"* — `src/yuv_convert.rs` has
  shipped `rgbx_to_yuv420_u16` since the zenav1-svt seam needed it, and
  `encoder_svt_rs::Yuv420Planes::convert` already called it. It is depth-generic
  (`FwdConsts::for_depth` scales the studio swing by `<< (depth - 8)` per
  H.273, so 10- and 12-bit needed no new constants) and dispatched per ISA
  tier through `#[magetypes(v4x, v4, v3, neon, wasm128, scalar)]` + `incant!`
  — the body is a scalar loop inside each tier's `target_feature` region, so
  that is per-tier auto-vectorization rather than hand-written intrinsics.
  `ForwardPixel` is implemented for `Rgb<u8>`, `Rgba<u8>`, `Rgb<u16>` and
  `Rgba<u16>`, so the 16-bit entry point needed no new impl either.
- *"profile-2 `av1C` signalling is not wired"* — `KeyFrameConfig::profile()`
  already returns 2 at `bit_depth == 12`, and `zenavif-serialize` already
  derives `high_bitdepth` / `twelve_bit` / `pixi` and raises `seq_profile` from
  the depth argument that `mux_aom` was passing a hardcoded `8`.

So the change is wiring: the three hardcoded `8`s
(`key_frame_config.bit_depth`, `mux_aom`'s `try_to_vec` depth, and the blanket
depth refusal) are gone.

**Depth resolution is now one function.** `EncoderConfig::coded_bit_depth_bits`
serves `validate`, all three encode seams and `resolve_plan`; the duplicated
`match config.bit_depth` copies in `encoder_svt_rs.rs`, `validation.rs` and
`encode_plan.rs` are gone. A duplicated match is what would let
`EncodeBitDepth::Twelve` reach a backend that cannot code it.
`EncodePlan::bit_depth` is now the coded depth in bits (8/10/12), not a
`ravif::BitDepth` narrowed to 8-or-10.

**Depths a backend cannot code are refused by name, never coded at a different
depth.** `Av1Backend::Zenravif` refuses `Twelve`
(`encoder::reject_unspellable_coded_depth`) because `ravif::BitDepth` has no
12-bit representation and would silently code 10 — the silent-wrong-pixels
class. `Av1Backend::Zenav1Svt` refuses it in `svt_rs_depth_error`, matching
C SVT-AV1 v4.2.0's own 8/10 check.

**Still refused at the aom seam, each naming what is unimplemented:** alpha
(`encode_rgba8` / `encode_rgba16` — the Cs400 mono encode an `auxl` item needs
exists, the item itself is not built), 4:4:4, 4:2:2, full pixel range, gain
maps, animation, and **high-bit-depth grayscale** — `encode_gray8` takes `u8`
samples and the seam passes them through as the coded luma, so promoting them
to a 10/12-bit swing would need a value-scaling rule nothing here measures.

**Gates** (`tests/aom_encode_backend.rs`, 25 tests):

| gate | measured | bound |
|---|---|---|
| bd10 coded luma, 3 sizes, q90 s6 | 49.58-49.69 dB | 40 dB |
| bd12 coded luma, 3 sizes, q90 s6 | 50.24-50.57 dB | 40 dB |
| bd10 flat luma vs longhand H.273, q99 | worst **0** code values | 0 |
| bd12 flat luma vs longhand H.273, q99 | worst **1** code value | 1 |
| bd10/bd12 container RGB round trip, 3 sizes x q80/q90 | 43.56-48.12 dB | 38 dB |

Every cell decodes with **rav1d-safe** — a different port from the one that
encoded it — which reports the requested coded depth and whose luma plane is
compared against an H.273 expectation written longhand in the test, not against
`yuv_convert.rs`. bd12 is off by one because `--cq-level 1` is not
`base_qindex 0`; the bound is the measured worst, not a padded number. A
studio/full range mix-up moves these by hundreds to thousands of code values.
`aom_backend_12_bit_signals_profile_2` reads `seq_profile` / `high_bitdepth` /
`twelve_bit` back out of the `av1C` box.

`two_independent_decoders_agree_at_10_and_12_bits` holds the high-bit-depth
path to the bar the 8-bit path already had: rav1d-safe and the zenav1-aom
DECODER (a different crate from the encoder) must agree **bit-exactly** on
every plane at both depths, not merely each decode successfully. They do.

`aom_hbd_decodes_correctly_through_the_container` closes the other half:
`zenavif::decode` — the path a caller actually uses, which applies `colr`,
undoes the studio swing and converts back to RGB — recovers the source across
3 sizes x 2 depths x 2 qualities. That is a different code path from the
raw-OBU comparison, so a mux-level colour error that a coded-luma check cannot
see would land here. It also pins the OUTPUT PIXEL TYPE, which follows the
coded depth: `Rgb<u16>` at 10 and 12 bits, `Rgb<u8>` at 8, and a caller that
assumed otherwise gets `None`.

Mutation proofs are `#[test]`s using `catch_unwind`, matching the pattern the
8-bit gate set: `hbd_gate_can_fail_on_wrong_content_and_wrong_depth` (wrong
source, wrong claimed depth, wrong flat value, **and** that the flat bound
itself is load-bearing — a bound of 1 must still reject a wrong source) and
`the_studio_swing_expectation_scales_with_depth` (the longhand helper cannot
degenerate to 8-bit constants).

**Three existing assertions were deliberately INVERTED, not slipped in:**
`"10-bit must be refused"` and `"16-bit must be refused"` in
`aom_backend_refuses_what_it_does_not_implement`, and `"10-bit must fail
validate()"` in `validate_agrees_with_the_encode_path`. 16-bit **RGBA** takes
over the 16-bit refusal case, because alpha genuinely is still refused.

**8-bit output is unchanged, MEASURED** (`benchmarks/aom_bd8_identity_2026-09-03.*`):
a `git archive` of `ec6728b` and this tree each drive the same emitter over 60
cells (6 geometries x 5 quality/speed pairs x gradient/flat) and the results are
**60/60 byte-identical** by length and hash. Anti-vacuity: a full-range mutation
on the same harness moves 60/60. `aom_bd8_output_is_unchanged_by_the_hbd_wiring`
is the cheap forward anchor on one of those cells so a later regression shows up
in the normal test run.

That benchmark also **corrected a premise this changelog first stated wrongly**.
`color_planes_420` keeps the `rgb8_to_yuv420` u8 kernel for the
8-bit-source-at-depth-8 cell, and the first draft said that split is what keeps
8-bit output stable. It is not: routing that cell through the depth-generic
`rgbx_to_yuv420_u16` recipe instead changes **0 of 60** cells — the two kernels
agree byte-for-byte at output depth 8. The split is kept as conservatism plus a
lane-width argument (u8 packs twice the lanes), with no speed number claimed
because none was measured.

`sweep.rs`: `EncodeBitDepth::Twelve` gets a new fingerprint discriminant (3) and
a new cell-id token (`-bd12`), so **no id or fingerprint minted before 0.2.0
moves**. `aom_bit_depth_resolves_into_the_fingerprint` measures — rather than
assumes — that 8/10/12/Auto aom cells hash apart, which they do because depth
is hashed in the shared pixel-path block even though `aom_resolved_identity`
carries only `(cpu_used, cq_level)`.

### Measured — a decoder outside this workspace reads all three depths

`sips`, Apple's own AVIF decoder, sharing no code with this workspace, was
pointed at 192x128 gradients encoded through the aom backend at quality 88 /
speed 5 (emitted by `dev/downstream-probe`'s `emit` binary; `file(1)` reports
all three as "ISO Media, AVIF Image"):

| file | `sips` `bitsPerSample` | mean per-channel delta vs source | max |
|---|---|---|---|
| 8-bit | 8 | 0.919 | 5 |
| 10-bit | 10 | 0.690 | 3 |
| 12-bit | 12 | 0.674 | 4 |

`sips` reports the DEPTH, not just the dimensions, so this independently
confirms the `av1C` `high_bitdepth` / `twelve_bit` signalling from outside —
the in-tree gate reads those bits out of our own container, which cannot catch
a shared misunderstanding. 73,728 8-bit channel values per file after `sips`
transcodes to PNG. The high-bit-depth files score better, which is the expected
direction (more coded precision on the same source), not evidence of anything
else.

### Found — `--cq-level 0` (quality 100) PANICS in the aom port on flat content

Pre-existing, **not** caused by the high-bit-depth wiring: quality 100 maps to
`--cq-level 0`, and `encode_key_frame` then panics with
`assertion failed: depth <= MAX_VARTX_DEPTH`
(`crates/aom-dsp/src/entropy/partition.rs:675` at the pinned rev `c3e1b4ab`).
It is a plain `assert!`, so it fires in release builds too, and it crosses the
seam as a process panic rather than an `Err` — the zenavif CLAUDE.md "Backend
seam" obligation 2 case.

MEASURED 2026-09-03, 64x64, speed 6:

| quality | cq | bd8 flat | bd8 grad | bd10 flat | bd10 grad | bd12 flat | bd12 grad |
|---|---|---|---|---|---|---|---|
| 100 | 0 | PANIC | ok | PANIC | ok | PANIC | PANIC |
| 99 | 1 | ok | ok | ok | ok | ok | ok |

It is **content-dependent**, so this seam does not blanket-refuse `cq 0`: a
refusal would reject the gradient cells that encode correctly today. Whether to
clamp (as the zenav1-svt seam clamps QP 0) is a product call for the owner.
Pinned meanwhile by `aom_cq0_still_panics_on_flat_content`, which also asserts
the boundary — quality 99 encodes the same content at all three depths — so the
day upstream fixes it, the canary says so instead of going quietly green. The
`src/encoder_aom.rs` module docs claiming "no clamp away from the endpoint"
were describing a mapping that panics, and now say so.

Tracked as **zenavif#45**, which lays out the four options (clamp to cq >= 1
like the svt seam; typed refusal; `catch_unwind` at the seam; fix upstream) and
why none of them is the seam's to pick unilaterally.


### Fixed — the assert-the-gate-ran step died on Windows before cargo ran

- The `Assert the aom encode gate actually ran` step captured the rerun with
  `tee /dev/stderr`, and the windows-11-arm runner's Git Bash has no
  `/dev/stderr` — under `pipefail` the step failed with
  `tee: /dev/stderr: No such file or directory` before cargo started. Run
  33700746090 was the FIRST run on which a Windows Test job ever reached the
  step (every earlier run's Windows job was superseded by the next push
  first), so the breakage was invisible until then. The rerun output now tees
  to a regular file and the count is parsed from it.

### Added — CI builds the deprecated alias features, which no job enabled

- `aom-backend` and `encode-svt-rs` (the deprecated 0.1.8 alias spellings)
  appeared in no CI job: cargo validates that an alias's target feature exists,
  but nothing ever built with either alias enabled, so an alias rewired to the
  wrong target would have kept CI green while breaking consumers on the old
  spelling. The `feature-check` job now runs one
  `cargo check --features aom-backend,encode-svt-rs` — the same
  feature-in-no-job class the series itself fixed for `__expert` and
  `encode-mono`.

### Fixed — the gray8 feature-off refusal contradicted itself and named no switch

- With `encode-mono` on and `zenav1-aom-encode` off, `encode_gray8` on
  `Av1Backend::Zenav1Aom` fell through to `reject_aom_backend`, whose message
  says the backend "does not support encode_gray8" while listing "8-bit
  grayscale stills" as in scope — and never names the missing feature (the
  rgb8 entry point already did). Same wrong-switch class as the a4ba11c
  refusal fix. Now the gray8 dispatch mirrors rgb8's feature-off arm and the
  refusal names `zenav1-aom-encode`; gated by
  `without_zenav1_aom_encode_gray8_names_the_feature` (proved able to fail),
  which runs in the diffmap CI step — `encode-mono` joined that step's feature
  list because the seam step has both features ON and so can never observe
  this build.

### Fixed — the aom scope's `validate()` accepted a lossless config the encode path refuses

- `validate_aom_scope` documents itself as carrying the same predicates as the
  encode path, but the `encode-imazen`-gated lossless refusal
  (`src/encoder_aom.rs` `reject_unsupported_config`) had no `validate()` twin —
  the zenav1-svt scope has always had one. So with `encode-imazen` +
  `zenav1-aom-encode`, `.backend(Zenav1Aom).with_lossless(true)` returned
  `Ok(())` from `validate()` and then failed at `encode_rgb8`. Proved from a
  real downstream consumer during adversarial review, fixed with the mirror
  `BackendUnsupportedParam { param: "lossless" }` check, and gated in
  `validate_agrees_with_the_encode_path` (proved able to fail: reverting the
  validation hunk reds it). (8395e86)

### Added — a third AV1 encode backend: `Av1Backend::Zenav1Aom` (`zenav1-aom-encode`)

- **The one function the "there is still no aom ENCODE backend" entry below
  named as the concrete ask now exists, so the seam is wired.** That entry
  (`Investigated (there is still no aom ENCODE backend, and why)`, audited at
  `14124356`) ended: *"The concrete ask for zenavif to gain an `encode-aom-rs`
  backend is one function — something on the shape of `encode_key_frame(planes,
  cfg) -> Vec<u8>` that authors its own sequence + frame headers and wraps a
  temporal unit."* `aom_encode::key_frame::encode_key_frame` is exactly that,
  and it lands the two things that entry called missing: the port authors its
  own sequence header (no C bytes copied) and emits its own
  `OBU_TEMPORAL_DELIMITER`. That entry is **superseded**, not deleted.

  Verified here rather than taken on report: the sibling gate
  `crates/aom-encode/tests/self_contained_key_frame.rs` was re-run 2026-09-02
  at `c3e1b4ab` — 6/6 tests, **186/186 cells byte-identical to real aomenc**
  (mono / 4:2:0 / 4:2:2 / 4:4:4, bit depths 8/10/12, 16x16-512x512, 20 crops
  incl. 1x1, all four CDEF x loop-restoration combinations, `--cpu-used`
  0..=9, multi-tile), 20 decode cells under both real libaom and the in-repo
  decoder.

- **Scope: KEY FRAME / STILL ONLY, and it says so.** `encode_key_frame`
  encodes one AV1 KEY frame; there is no inter prediction, no reference
  management and no multi-frame state in it. So animation is not "unwired at
  the seam", it is absent from the encoder, and `reject_aom_backend` refuses
  it by name. Wired: 8-bit RGB -> YCbCr 4:2:0 BT.601, and 8-bit grayscale ->
  monochrome Cs400 (`encode-mono`). Refused with a message naming what is
  unimplemented: animation, alpha (`encode_rgba8`/`encode_rgba16`), 16-bit
  input, 10/12-bit output, 4:4:4 and 4:2:2, the identity/RGB colour model,
  full pixel range, gain maps, lossless. Nothing falls back to zenravif.

- **Feature-name collision, resolved deliberately.** `zenav1-aom` was already
  taken by the DECODE backend because it names the *repository* — unambiguous
  while that repository supplied one backend crate. It now supplies two
  (`zenav1-aom-decode`, `zenav1-aom-encode`), so the encode feature takes the
  *crate* name `zenav1-aom-encode`, and an additive synonym
  `zenav1-aom-decode = ["zenav1-aom"]` lets the decode side be spelled by
  crate name too. `zenav1-aom` is **not** deprecated and stays canonical for
  decode (every existing consumer, benchmark, doc and CI line uses it);
  nothing is overloaded to mean two things.

- **Dependency shape: a git-rev pin, not a sibling path dep.** `aom-encode =
  { package = "zenav1-aom-encode", git = ".../zenav1-aom", rev = "c3e1b4ab" }`.
  A path dep escaping the repo root makes every cargo command fail in a
  standalone clone — the failure the `resolve-standalone` job exists to gate,
  and the one that kept zencodecs / zenpipe from taking zenavif as a git
  dependency for most of August. The only escaping path deps in `Cargo.toml`
  remain the pre-existing `../zenanalyze` pair that job already clones;
  `cargo metadata` (full resolve) exits 0 with the new dep in place.

  The encode rev is **newer** than the decode pin (`14124356`) because
  `encode_key_frame` landed after it. Cargo resolves the two revs as two
  sources, so the decode backend keeps the rev it was gated at. The cost is
  one extra `zenav1-aom-dsp` compilation, not a correctness risk: the seam
  passes only `&[u16]` planes and `usize` across it, sharing no types. Unify
  the two the next time someone re-gates decode.

- **Colour signalling: the mirror image of the zenav1-svt seam, and MEASURED.**
  The port pins the AV1 sequence header to `color_range = 0`
  (`AOM_CR_STUDIO_RANGE`, real aomenc's default) where zenav1-svt pins
  `color_range = 1`. So this backend converts **limited** range and muxes
  `full_color_range = false`, and requesting `EncodePixelRange::Full` is
  refused rather than mis-signalled.

  Measured 2026-09-02, and it corrects the obvious assumption: `zenavif::decode`
  takes range **and** matrix from the **sequence header**, not from the
  container `colr` — `src/decoder.rs` reads `seq_hdr.color_range` /
  `seq_hdr.mtrx`, and flipping the `colr` nclx `full_range` bit in an encoded
  file leaves the decoded pixels bit-identical to six decimals. The sequence
  header's `matrix_coefficients` is 2 (unspecified), whose `to_yuv_matrix`
  fallback is BT.601 — what the seam converts with, so they agree. `colr` is
  written to agree with the bitstream, not to override it.

- **It produces an AVIF file, not raw OBUs.** The retired `Av1Backend::Svtav1`
  draft is rejected by `EncoderConfig::validate` precisely because it returned
  raw OBUs; this backend muxes through `zenavif-serialize`, and
  `assert_is_avif_container` in the new gate asserts the `ftyp` box, a primary
  item, and a payload starting at `OBU_TEMPORAL_DELIMITER`.

- **Dials.** quality 1..=100 -> `--cq-level` 63..=0 linear with **no clamp**
  (both endpoints are byte-gated upstream, unlike the zenav1-svt seam which
  must avoid QP 0); speed 1..=10 -> `--cpu-used` 0..=9 one-to-one (the whole
  range is byte-gated, so the dial advertises no distinction the encoder does
  not have). `--enable-cdef=0` / `--enable-restoration=1`, real aomenc's
  ALLINTRA defaults. **Encode wall time is unmeasured from this seam and no
  number is quoted.**

- Additive only: a new variant on an already-`#[non_exhaustive]` enum, a new
  feature, a new module, a new test. No existing variant, feature or signature
  changed; the 0.1.8 deprecated aliases (`encode-svt-rs`, `aom-backend`,
  `SvtRs`, `AomRs`, `SvtRs420`) are untouched and
  `tests/deprecated_backend_aliases.rs` still compiles.

### Added — the feature-OFF contract for the aom encode backend is now gated

- Nothing asserted what happens when `zenav1-aom-encode` is **off**. The aom
  gate is `#![cfg(feature = "zenav1-aom-encode")]`, so it never observes that
  path — a build with the feature off could have silently served an
  `Av1Backend::Zenav1Aom` request with zenravif and no test would have
  noticed. Three tests in `tests/deprecated_backend_aliases.rs` close it:
  the feature-on config validates; with the feature off `validate()` returns
  `BackendUnavailable` naming **`zenav1-aom-encode`** (not `zenav1-aom`, which
  is the decode backend and would send a caller to the wrong switch); and
  `encode_rgb8` itself refuses rather than falling through.

- They run in CI already, in the existing diffmap step
  (`--features two-pass-butteraugli,two-pass-zensim,auto-tune`), which
  transitively enables `encode` but not `zenav1-aom-encode` — verified by
  running that exact command, not by reading the feature graph.

- Proved able to fail, both directions: renaming the feature in the
  `BackendUnavailable` error to `zenav1-aom` reddens the first, and making the
  entry point fall through to zenravif reddens the second.

### Measured — encode wall time, replacing the "unmeasured" note

- `src/encoder_aom.rs` shipped saying encode speed was unmeasured rather than
  quoting a number nobody had run. `examples/aom_backend_bench.rs` (committed)
  now runs it: 144 cells, 4 sizes x 6 qualities x 3 speeds x 2 backends,
  results in `benchmarks/aom_backend_2026-09-02.{tsv,meta}`.

- **The speed ladders are misaligned**, the same shape of finding the
  zenav1-svt seam records: the aom backend is 2.5-3.2x faster than zenravif at
  speed 1 and 3.9-8.0x faster at speed 9, but 2.0-3.2x **slower** at speed 5.
  zenavif speed N does not mean comparable work across backends.

- **Per-pixel cost is not constant** — ms/MP falls 7-25x from 64² to 1024² for
  both backends — so no single ms/MP figure is quoted, and the
  `alpha + beta*MP` fit is deliberately omitted rather than reported: it is
  badly conditioned on this grid (it predicts a 555 ms intercept at speed 1
  where 84 ms was measured at 64²). The `.meta` carries the per-size table.

- **Fixed container overhead is 262 bytes** for this seam (242 for zenravif),
  constant across all 144 cells — 89% of a 64² q10 file. A bitrate model over
  these backends needs `header_bytes + content_bpp * pixels`, not a bpp alone.

- The byte/quality columns are explicitly **not** an RD comparison and the
  `.meta` says so: only 6 of 24 speed-5 cells land within 0.15 dB, PSNR is not
  a perceptual metric, and it is one synthetic image. Those six all favour the
  aom backend at 0.32-0.46x the payload bytes at equal PSNR, which is recorded
  as six data points, not a claim about the codecs.

### Fixed — the new variant broke the `__expert` build, and no CI job would have said so

- `src/sweep.rs` carries two **exhaustive** `match config.backend` arms (the
  cell-id token and the `fingerprint` backend byte). `#[non_exhaustive]` does
  not apply inside the crate, so adding `Av1Backend::Zenav1Aom` broke that
  build with two E0004s. Measured, not hypothetical: `cargo check --features
  __expert,encode` failed while every other build and the whole test suite
  passed.

- **`__expert` appeared in NO CI job**, so the break would have shipped green.
  `feature-check` now checks it in three combinations (alone, with the encode
  backends, with the expert knobs), and the `test` job runs the `--lib` tests
  that gate the sweep fingerprint.

  The `--lib` step went into `feature-check` first and reddened CI (run
  33697870049): `--lib` includes two `decode_av1` tests that read
  `tests/vectors/` **fail-loud**, and only the `test` job provisions those
  vectors. Nothing was relaxed to fix it — the tests are right to fail loudly
  on a missing corpus, so the step moved to the job that has the corpus. A
  local run had passed because this box already had the vectors on disk, which
  is exactly the difference CI exists to catch.

- A second, quieter bug in the same place: `zenravif_mediated` decides whether
  the zenravif quantizer/speed mediators are hashed into the fingerprint, and
  the aom backend reads neither — so with only the `match` arms fixed, an aom
  cell would have hashed **nothing** quality- or speed-dependent and every aom
  quality would have fingerprinted alike. The sweep planner would then merge
  cells that encode differently, which is silent RD-data corruption rather
  than a missing feature. `encoder_aom::aom_resolved_identity` supplies the
  resolved `(cpu_used, cq_level)` and the fingerprint hashes it, mirroring
  `svt_resolved_identity`. The svt block's guard also changed from
  `!zenravif_mediated` to an explicit `== Zenav1Svt`, because that expression
  is no longer a synonym for "is svt-rs"; svt behaviour is unchanged.

- Gated by `aom_quality_resolves_into_the_fingerprint`: qualities resolving to
  different `--cq-level` must fingerprint apart, qualities resolving to the
  same one must merge, no two speeds may collide, and an aom cell must never
  share a fingerprint with the zenravif cell at the same dials. Proved able to
  fail — dropping the two `h.u8` calls reddens it.

  Its merge case was written on a guess (99 and 100 both resolve to cq 0) that
  the test's own premise assertion rejected: `cq(q) = round((100-q)*63/99)`
  gives cq 1 for q99 and cq 0 for q100. 36 of the 99 adjacent quality pairs do
  alias; 98/99 (both cq 1) is the highest-quality one, and that is what the
  test now uses. Recorded because the premise assertion is the reason a wrong
  constant did not land.

### Measured — a third-party reader accepts the aom backend's output

- The in-repo gate decodes with rav1d-safe, which is a different port from the
  encoder but still in this workspace. Outside it entirely: a 192x128 gradient
  encoded at quality 88 / speed 5 through `Av1Backend::Zenav1Aom` is reported
  by `file(1)` as "ISO Media, AVIF Image", and macOS `sips` — Apple's own AVIF
  decoder, sharing no code with anything here — reads it as 192x128 and
  transcodes it to a PNG whose pixels match the source to **mean 0.57 / max 3**
  per channel over 4608 sampled values.

- That also confirms the studio-range signalling from outside the workspace:
  the top-left source pixel is (0, 0, 0), it codes to studio luma 16, and
  Apple's decoder returns (1, 1, 1) — not the (16, 16, 16) a decoder ignoring
  `color_range = 0` would give.

### Fixed — an aom-backend refusal named the `zenav1-svt` feature

- `reject_svt_rs_backend` delegates to `reject_aom_backend` with the same
  `entry` string, and four call sites had the svt-specific hint baked into
  that string. So an aom refusal read *"Av1Backend::Zenav1Aom does not support
  encode_rgb16 (requires the `zenav1-svt` cargo feature)"* — naming a feature
  unrelated to this backend and pointing the caller at the wrong fix. The call
  sites now pass the bare entry name and each rejector adds its own hint.

- **Found by compiling a real downstream consumer, not by a same-crate test.**
  A scratch crate that path-deps zenavif (carrying the two `[patch]` entries a
  path dep does not inherit), exhaustively matches `Av1Backend` with a `_`
  arm, uses the deprecated `SvtRs` alias in expression *and* pattern position,
  and encodes then decodes a 64x64 image through the new backend — 290-byte
  AVIF, decoded 64x64. That crate is the substitute for `cargo semver-checks`,
  which still cannot run here (packaging drops the in-repo patch supplying
  unpublished `zenavif-serialize` 0.2.0; it aborts at `cargo update`,
  unchanged and pre-existing).

- Gated: `aom_backend_refuses_what_it_does_not_implement` now asserts no aom
  refusal contains `zenav1-svt`, and that the 16-bit refusal names the
  limitation rather than only the backend. Proved able to fail — re-adding the
  hint to the `encode_rgb16` call site reddens it (11 passed, 1 failed).

### Added — `tests/aom_encode_backend.rs`, a decode gate proved able to fail

- 12 tests (11 without `encode-mono`). The claim is **not** "encode returned
  `Ok`": every gate decodes the produced AVIF with an **independent decoder**
  — rav1d-safe, a different port from the one that encoded it — and compares
  pixels. RGB round trip through `zenavif::decode` on four sizes including a
  partial superblock and odd dimensions (33x47) and across all ten speeds;
  flat content decodes **exactly** (no tolerance) at the plane level on
  rav1d-safe and to within 1 at the RGB level; rav1d-safe and the zenav1-aom
  decode backend agree bit-exactly on the aom encoder's own output when both
  features are on.

- **The mutation proofs are `#[test]`s, not a one-off.**
  `gate_can_fail_on_wrong_content` and `gate_can_fail_on_a_corrupted_payload`
  run the SAME assertion helpers against deliberately broken input under
  `catch_unwind` and REQUIRE them to panic, so a later edit that makes an
  assertion vacuous turns them red in CI.
  `limited_range_signalling_is_load_bearing` asserts the studio swing codes
  235 as 218, that the decoder returns 235, that 218 is far enough away for
  the tolerance to distinguish them, and that the assertion fails when handed
  the full-range misreading.

  Additionally proved able to fail by mutating the **production source** three
  ways and watching it go red (source restored and verified byte-identical
  afterwards): `YuvRange::Limited` -> `Full` in the conversion reddens 6/12;
  `mux_aom` returning the raw payload instead of the AVIF reddens 9/12;
  `cq_level`/`cpu_used` pinned to constants reddens 6/12.

- **Threshold provenance.** Every PSNR bound was measured on this content
  before being written, with roughly 5 dB of headroom over the worst measured
  cell: q90 across four sizes measured 43.3-45.3 dB, bound 38; q80 across four
  sizes x ten speeds measured 37.9-41.0 dB, bound 33. A broken decode is
  nowhere near either: wrong content measures 6.4 dB, and a two-byte payload
  corruption fails to decode at all.
  `aom_backend_tracks_the_production_backend` adds the bound an absolute floor
  cannot give — at q90 the aom backend measures 1.6 dB behind zenravif to
  0.2 dB ahead of it, and the gate allows 5 dB.

- **A correction to how the test content was first written.** The initial
  generator added independent per-channel noise, which made 4:2:0 subsampling
  — not the encoder — the dominant error term: measured, that variant scores
  22.4 dB for the aom backend and 22.4 dB for zenravif on the same image, i.e.
  it cannot tell the two apart and would have been graded as a backend defect.
  The committed generator adds the noise equally to all three channels so it
  lives in luma. This is recorded because the first reading of the low number
  looked like a bug in the new backend and was not.


### Changed — backend names now match the crates they name (`0.1.7` -> `0.1.8`)

- **`Av1Backend::SvtRs` -> `Av1Backend::Zenav1Svt`, `DecodeBackend::AomRs` ->
  `DecodeBackend::Zenav1Aom`, `heuristics::EstimateArm::SvtRs420` ->
  `EstimateArm::Zenav1Svt420`.** The variants carried pre-rename names from
  when the sibling crates were `svtav1-rs` and `aom-rs`; those repos are now
  `imazen/zenav1-svt` and `imazen/zenav1-aom`, so the `-rs` spellings named
  nothing that exists. `Av1Backend::Zenravif` — the PascalCase of crate
  `zenravif` — was already the convention in the same enum; these follow it.

  `EstimateArm::SvtRs420` was renamed with them because it is public API
  (`pub mod heuristics`) named after the variant being renamed: leaving it
  would have kept a live, non-deprecated `SvtRs` spelling exported after the
  variant it names was deprecated.

  **This is ADDITIVE, not a break — hence 0.1.8 and not 0.2.0.** Each old
  spelling survives as a `#[deprecated(since = "0.1.8")]` associated
  constant (`pub const SvtRs: Self = Self::Zenav1Svt`). An associated
  constant is usable in a *pattern* only when its type is structural-match;
  all three enums derive `PartialEq` + `Eq` rather than hand-implementing
  them, so **both expression and pattern position keep working**.
  `tests/deprecated_backend_aliases.rs` exercises every alias in both
  positions and fails to COMPILE if either regresses — the compatibility
  claim is proved by the build, not asserted. A scratch downstream consumer
  built against this tree with the old spellings compiles, emits one
  deprecation warning per alias, and round-trips to the same values.
  (`144bda27`)

- **Features `encode-svt-rs` -> `zenav1-svt` and `aom-backend` ->
  `zenav1-aom`**, same reasoning. Cargo has no `#[deprecated]` for features,
  so the old names are kept as **alias features that enable the new ones**
  (`encode-svt-rs = ["zenav1-svt"]`, `aom-backend = ["zenav1-aom"]`); this
  entry and the `Cargo.toml` comments beside them are the deprecation
  notice. **Both alias features are removed in 0.2.**

  That the aliases *gate* the same code, rather than merely resolving, was
  measured rather than assumed: `cargo test -- --list` under
  `zenav1-aom,encode,encode-imazen,zenav1-svt,target-quality` and under
  `aom-backend,encode,encode-imazen,encode-svt-rs,target-quality` enumerate a
  byte-identical 448-test set, and the tests gated on the *new* feature names
  are present in the *old*-spelling build. (`144bda27`)

- **Retired the dead `svtav1-rs` / `aom-rs` crate names from current-state
  docs** — 124 lines of rustdoc, doc files, `Cargo.toml` comments, example
  prose and *runtime error strings* still called the backends by the names
  their crates had before the repos became `imazen/zenav1-svt` and
  `imazen/zenav1-aom`. This includes user-visible text such as
  `Error::Encode("svtav1-rs encode failed: ...")` and the aom decode-error
  messages, which now name the crate that produced them; no error variant or
  structure changed, and no test asserted on those strings. `CHANGELOG.md`,
  `benchmarks/` (records of what shipped and what was measured under the old
  names), the generated `docs/public-api/`, and the two rustdoc lines that
  explain the rename itself were deliberately left as written. (`97c94947`)

- **Corrected six over-replacements from that sweep** (`3c5a47b9`). Replacing
  `svtav1-rs` / `aom-rs` everywhere also hit places where those strings named
  something other than a crate: the **real** origin branch `svtav1-rs-backend`
  (cited by `CLAUDE.md`, `docs/BACKEND_SUPPORT_MATRIX.md` and
  `docs/TEST_COVERAGE.md`), a `CLAUDE.md` line whose whole point was to record
  the *old* `/root/aom-rs/...` paths, the reference-box path defaults in
  `scripts/decode_4way_c_refs.sh` and `examples/decode_4way_bench.rs`, a path
  inside the upstream repo, and the dated `docs/REPO_HYGIENE_2026-07-24.md`
  (restored wholesale — a historical snapshot, like `CHANGELOG.md` and
  `benchmarks/`). The unverifiable `/root/...` defaults were restored rather
  than guessed at.

  One genuine fix was kept: `src/encoder_svt_rs.rs` pointed at
  `https://github.com/imazen/svtav1` with an `svtav1-rs/` subdir. Per
  `Cargo.toml:151-158` and the git dep at `:211` — which call that the "dead
  `imazen/svtav1` URL" — the repo is `imazen/zenav1-svt` and the facade crate
  lives in `rust/svtav1`. That line was wrong before this work and now names
  both correctly.

- Not renamed, deliberately: `Av1Backend::Svtav1` (a retired draft that no
  build can select and `EncoderConfig::validate` already rejects — already
  `#[deprecated]`), and `DecodeBackend::Rav1dSafe` / `Rav1dFfi`, which match
  their crates. The `svtav1` dependency alias and the `src/encoder_svt_rs.rs`
  / `tests/svt_rs_backend.rs` file names are internal and unchanged.
  (`144bda27`)


### Fixed (main did not compile with `encode` on any platform)

- **`EncoderConfig.svt` named a type that only exists behind `__expert`.**
  `dd61d45` added the field plus `with_svt_params` / `svt_params`, all spelled
  `crate::expert::SvtParams`, but `pub mod expert` is
  `#[cfg(feature = "__expert")]` and **nothing in CI enables `__expert`**. Every
  build that compiles `mod encoder` — that is, every `--features encode*`,
  `target-quality`, and both diffmap loops, plus the CI backend-seam
  combination — died with four `error[E0433]: cannot find `expert` in `crate``.
  Only the default and `--no-default-features` builds survived, because they do
  not compile the encoder at all. CI run `33657668239` is red on
  ubuntu-latest, macos-latest, windows-latest and the Gate A3 determinism job
  with exactly these errors.

  The fix could not be "gate the four items too": `apply_svt_params`
  (`src/encoder_svt_rs.rs:264`) runs on the ordinary `encode-svt-rs` path and
  needs the type where `__expert` is off. So `SvtParams` moved to
  `src/svt_params.rs`, a private module gated on
  `any(encode-svt-rs, __expert)` — precisely the features that consume it —
  and `src/expert.rs` re-exports it, leaving `zenavif::expert::SvtParams` the
  only public spelling. The two accessors gained `#[cfg(feature = "__expert")]`,
  matching `with_internal_params` immediately below them and the "expert-only"
  wording already in their own docs; neither existed in any *building*
  non-`__expert` configuration, so nothing that worked was removed. No public
  API added. `apply_svt_params` and the sweep planner now share a `pub(crate)`
  `EncoderConfig::svt_params_resolved()`. (`2a1f24f`)

- **`cargo fmt` on `src/sweep.rs`**, which had left the `Format` job red since
  `bcd7978`. Verified formatting-only before committing: the two revisions are
  identical once whitespace is stripped and rustfmt's added trailing commas are
  normalised away. (`30c8ca1`)

### Changed (both AV1 backends: sibling path deps → git-rev pins)

- **`aom-decode` and `svtav1` are git-rev pins again**, on
  `imazen/zenav1-aom` @ `14124356` and `imazen/zenav1-svt` @ `ef0b122b` — the
  return each dep's own comment had been asking for since 2026-08-06. Two
  measured reasons, not one:

  1. *Resolution.* A path dep that escapes the repository root must have its
     manifest loaded during resolution even when its feature is off, so one such
     dep makes **every** cargo command fail in a plain clone — the
     `Dependabot Updates` failure and the git-consumer breakage recorded in the
     Workspace section above.
  2. *The svt seam was building against a dirty working copy.*
     `git -C ../zenav1-svt diff --stat origin/main` on 2026-09-02 showed **71
     uncommitted lines** in `svtav1-encoder` (`inter_md_arm.rs`,
     `pipeline.rs`) — the exact crate the seam links. Every local
     `encode-svt-rs` result was therefore measured against code that existed on
     one developer's disk and nowhere else, and could not agree with CI by
     construction. A rev pin makes the two measure the same encoder.

  **The "UNFETCHABLE" premise in the old `svtav1` comment was stale and was
  re-measured rather than inherited.** What 404'd was the *renamed*
  `github.com/imazen/svtav1` URL, not a git dep on this repo:
  `imazen/zenav1-svt`, its `reference/svt-av1` submodule
  (`imazen/zenav1-svt-c`) and `imazen/zenav1-aom`'s `imazen/libaom-mirror` are
  all public, and `cargo metadata` on a scratch crate git-depending on these
  exact revs resolves clean, submodules included.

  **Measured cost, stated rather than waved at.** Cargo recurses those
  submodules where `clone-siblings` deliberately did not: about 250 MB
  (libaom-mirror) + 85 MB (zenav1-svt-c) of git db and ~420 MB of checkouts on a
  cold `Swatinem/rust-cache`. That is the same order as the `rav1d-safe`
  (396 MB) and `aom` (403 MB) git deps this graph already carries, and it is
  cached; the two now-redundant clone steps come back off the other side.

- **`clone-siblings` no longer clones `zenav1-aom` or `zenav1-svt`** — cargo
  fetches them itself now. It still clones `zenanalyze`, `ravif`, `zenrav1e` and
  `zensim`.

- **New CI gate `resolve-standalone`** clones *only* `../zenanalyze`, asserts
  `../zenav1-aom` and `../zenav1-svt` are absent, and requires both
  `cargo metadata --no-deps` and a full `cargo metadata` to succeed. Proved able
  to fail as well as pass on 2026-09-02: with the same siblings absent, the
  *previous* manifest exits non-zero with
  ``error: failed to load manifest for dependency `zenav1-aom-decode` ``, and
  the current one exits 0.

  **This does not yet make a bare clone resolve.** `zenanalyze` and
  `zenpredict` (`Cargo.toml:205-206`, and the `zenanalyze` dev-dep) are still
  escaping path deps and are the whole residual cause; they are outside the AV1
  backend scope and need their own decision. When they go, delete the clone step
  inside that job and it becomes the full standalone-resolution gate. `cargo
  publish` stays blocked regardless — six `git =` deps carry no `version` key.

### Documentation (the svt-rs dimension envelope was documented backwards)

- **`Av1Backend::SvtRs`'s rustdoc and the `encode-svt-rs` feature comment both
  contradicted the gate they describe.** Both said dimensions must be multiples
  of 64 except at speed >= 5. `svt_rs_dims_error`
  (`src/encoder_svt_rs.rs:351`) says the opposite for the colour path: multiples
  of 64 are always accepted, **any other size is accepted on 4:2:0 colour at
  every speed** (the partial-superblock floor was removed 2026-08-29), and only
  an alpha/grayscale Cs400 item at a non-multiple-of-64 size additionally needs
  speed >= 5 and multiples of 8. The feature comment was also still claiming
  8-bit only and "bitstream identity vs C-SVT is NOT yet asserted", both long
  since false. Corrected against the code, with the gate named as the source of
  truth. (`c284f99`) The *behaviour* was never wrong and stays pinned by
  `svt_rs_partial_sb_roundtrip_at_low_presets` and
  `svt_rs_mono_partial_sb_still_refused_below_preset_6`; only the prose drifted.

### Measured (both backends run, not just built)

- **`benchmarks/backend_sweep_2026-09-02.{tsv,meta}`** — 8 CID22-512 images x
  sizes {64, 128, 256, 512} x quality 5..=100 step 5 x speed 6 x
  {zenravif-420, zenravif-444, svt-rs-420}, 1920 rows. The **cross-decoder gate
  is 1920/1920 byte-identical**: every AV1 payload either encoder produced
  decodes to identical planes under `DecodeBackend::Rav1dSafe` and
  `DecodeBackend::AomRs`. Bytes at *matched* ssim2 (log-interpolated per image,
  median across images) put svt-rs at 0.94-1.05x zenravif at 64-128 and
  1.03-1.16x at 256-512; encode wall time runs 17.3x / 23.6x / 26.8x / 28.2x in
  svt-rs's favour by size. Median bytes against median ssim2 over a quality
  sweep is not an RD comparison and is deliberately not quoted.

- **`benchmarks/backend_sweep_partialsb_2026-09-02.{tsv,meta}`** — the same
  harness at sizes {65, 100, 200, 333, 500}, none a multiple of 64 and two not
  multiples of 8. **600/600 svt-rs cells encoded with no dimension refusal**,
  1800/1800 decoder-identical, RD and speed ratios inside the aligned dataset's
  range. This is the evidence behind the dimension-envelope correction below —
  the claim now rests on encoded bitstreams, not on a reading of the gate.

  Limits stated in the `.meta` files: the `encode_ms` columns come from a
  4-thread run and are load-polluted in absolute terms (the arm *ratio* is
  not); speed 6 is SVT preset 7, so this grid does not exercise the
  below-preset-6 half of the 2026-08-29 floor removal, which stays pinned by
  `svt_rs_partial_sb_roundtrip_at_low_presets`; and `backend_sweep_2026-07-22`
  is **not** comparable (different pins, box, speed set, image count).

- These are the first backend measurements in this repo whose encoder is
  identified by a commit rather than by whatever was checked out in
  `../zenav1-svt` — see the dep change above. `benchmarks/README.md` gains an
  "AV1 backend datasets" table, which it did not have for any backend dataset.
  (`e088692`)

### Added (backend spike measurements rescued from a stale branch)

- **`benchmarks/av1_backends_spike_2026-05-23.md` + the three
  `backends_*_2026-05-23.tsv` matrices**, lifted from
  `origin/abandoned/spike-av1-backends-2026-05-23` (`669de3c`) (`41f3f28`), the one branch
  in the repo carrying measurements that exist nowhere on `main`: 150 vectors x
  4-5 decode backends on Linux (ffmpeg 4.4 / libdav1d 0.9.2) and Windows 11
  (ffmpeg 7.1.1), with the finding that hardware AV1 decoders land within ±20%
  on internal time but 4-7x slower wall once GPU init and sysmem readback are
  paid, and reject mono / 4:4:4 / 4:2:2 / 12-bit outright. The harness
  (`examples/bench_backends.rs`) and the superseded backend-trait design it
  needs were deliberately NOT brought over — they cannot compile against the
  current `DecodeBackend` seam — and the salvaged `.md` carries a header saying
  where to read them.

### Investigated (there is still no aom ENCODE backend, and why)

> **SUPERSEDED 2026-09-02.** `encode_key_frame` — the one function this entry
> named as the concrete ask — now exists, and `Av1Backend::Zenav1Aom` is wired.
> See the `Av1Backend::Zenav1Aom` entry at the top of `[Unreleased]`. Kept as
> written: it is the audit that specified the ask.

- **`zenav1-aom-encode` cannot back a frame-level encode today.** Audited at
  `14124356` because the wiring is asymmetric — aom gives zenavif *decode*
  (`aom-backend`), SVT gives it *encode* (`encode-svt-rs`) — and the obvious
  question is whether the fourth quadrant is reachable. It is not, and no
  backend was faked around it:

  - The crate's public surface is block-level. `crates/aom-encode/src/lib.rs`
    re-exports nothing; its root items are `xform_quant*`,
    `encode_block_coeffs` (`:818`), `encode_block_coeffs_full` (`:865`),
    `encode_coding_block_plane` (`:931`), distortion and RD-cost helpers. There
    is no `Encoder`, `Sequence` or `Frame` type anywhere in `src/`, and no
    function returns a bitstream. The highest driver is `pack::pack_tile`
    (`src/pack.rs:1588`), tile-level.
  - **The port never authors a sequence header** — the upstream repo's own
    words (`docs/CONFIG_AXIS_INVENTORY_2026-07-30.md:477`, carried into its
    `CLAUDE.md:25`), and there is no OBU_TEMPORAL_DELIMITER writer at all.
  - `crates/aom-encode/tests/avif_parity.rs` looks like an end-to-end AVIF gate
    and is not one. It calls real C libaom first (`:211`), parses that stream's
    sequence (`:226`) and frame (`:291`) headers, has the port derive only the
    tile payload (`:481`) and the loop-filter level (`:513`), then **copies C's
    sequence header verbatim** into the shipped stream (`:528`). What it proves
    is real — the coded payload is byte-exact vs aomenc (`:611`) and the muxed
    file decodes pixel-identically (`:648`) — but it proves nothing about
    authoring headers. Its envelope is also non-default
    (`enable_cdef=false, enable_restoration=false`). The only full-frame entry
    in the whole repo, `crates/aom-bench/src/lib.rs:1150`, takes a `bootstrap:
    &[u8]` argument and lives in a `publish = false` crate.
  - Missing, per that repo's own gap table (`coverage-audit/COVERAGE.md`):
    `av1_get_seq_level_idx` (libaom `level.c`) is the one genuinely absent
    *algorithm* (`:143`); the rest is wiring already-byte-exact writers —
    base_qindex composition, tile-count choice, CICP echo, temporal-unit /
    TD-OBU assembly (`:142`), plus the sequence-header authoring chain
    (`init_seq_coding_tools`, `set_sb_size`, profile derivation,
    `set_bitstream_level_tier`) and a `write_uncompressed_header_obu`
    field-*setting* side. Separately, loop restoration is on by default in
    `--allintra` but the (byte-exact) LR search is wired only behind
    `--enable-restoration=1`, so even a bootstrap-fed encode does not match a
    stock `aomenc --allintra` stream (`:53-56`, `:140`).

  This is a shell-shaped gap, not an empty crate: the RDO search,
  transform/quant, entropy coding, tile packing and the CDEF / loop-filter /
  loop-restoration searches are all byte-exact against C. **The concrete ask
  for zenavif to gain an `encode-aom-rs` backend is one function** —
  something on the shape of `encode_key_frame(planes, cfg) -> Vec<u8>` that
  authors its own sequence + frame headers and wraps a temporal unit. Until
  that exists there is nothing for a seam to call, and `Av1Backend` gains no
  variant.

### Branch and PR inventory (2026-09-02 audit)

- `svtav1-rs-backend` and `svtav1` are literal ancestors of `main` — 0 commits
  ahead. `backup/svtav1-rs-backend-pre-rebase-2026-07-23` is 24 ahead but 100%
  superseded: 16 of 24 are exact patch-id matches upstream, the other 8 differ
  only by dropped `Cargo.lock` hunks or rebase line-offsets, and **all nine of
  its benchmark files are byte-identical blobs on `main`** — including
  `backend_sweep_2026-07-22.tsv`, so no bench record is at risk.
  `preserve/2026-07-25-svtav1-rs-backend` re-pins svtav1 to a rev that this
  change supersedes. All four are recommended for deletion by the repo owner;
  none were deleted here.
  `abandoned/spike-av1-backends-2026-05-23` is the one branch that must NOT be
  deleted casually — its measurements are now on `main` (above) but its harness
  is not.
- **PRs #11 and #10** (`feat(picker)`, both from 2026-05-04) are fully
  superseded: `git cherry` marks every commit already upstream and all 10 blobs
  are byte-identical to `main`, which took them as `b2a0c48` and `f9fd1a3`.
  Their green checks are 1584 commits stale. Recommended for closing as merged;
  left open here.

### Fixed (silent depth coercion on the 16-bit encode entry points)

- **`encode_rgb16` / `encode_rgba16` now honour `config.bit_depth`.** Both
  scaled every sample with `scale_from_u16(.., 10)` and called
  `encode_raw_planes_10_bit` unconditionally, reading `config.bit_depth` *not at
  all* — so `EncoderConfig { bit_depth: Eight, .. }` plus a 16-bit buffer
  produced a **10-bit file with no error and no warning**, reachable from the
  generic zencodec route for any `Rgb16` / `Rgba16` input
  (`src/codec/encoder.rs:225`, `:255`, `:399`, `:411`). The mechanism:
  `encode_raw_planes_*` takes the coded depth as an argument, which overrides
  the encoder's own `with_bit_depth` — so `build_ravif_encoder`'s already-correct
  `resolve_bit_depth` result was discarded one line later.

  `EncodeBitDepth::Eight` now takes a narrowing route (16-bit buffer → 8-bit
  coded stream) rather than being refused, because narrowing is a real
  capability the generic codec path exists to serve. `Ten` and `Auto` are
  unchanged — `Auto` keeps its documented "16-bit input → 10-bit AV1" contract,
  and the produced files are **byte-identical** pre/post fix (measured: rgb16
  and rgba16, both `Auto` and `Ten`, four files, sha256 unchanged).

  Narrowing uses the crate's existing owner, `convert::scale_from_u16(v, 8)`,
  wrapped as `convert::narrow_to_u8` for the `[u8; 3]` plane API. That rule is
  the exact inverse of the widening rule (`scale_to_u16`, LSB replication), so
  8-bit content promoted to 16 bits round-trips to the original bytes, and it
  matches the decode side's `downscale_to_8bit` ("high byte of each channel").
  Half-up rounding was rejected on measurement, not preference: it leaves the u8
  domain at `0xFFFF` (→ 256) and corrupts the 8→16→8 round-trip for **128 of 256
  bytes**. Both facts are pinned as tests rather than left as claims.

  The svt-rs backend already honoured the request
  (`encoder_svt_rs::effective_bit_depth`); this brings the zenrav1e path level
  with it. No public API change (verified: `cargo public-api` diff is empty
  across 1,834 items). Registered as a defect in zenmetrics
  `benchmarks/bitdepth_capability_matrix_2026-09-02.md` §2.

  Gates: `tests/bit_depth_request.rs` (5 tests — depth read back from the
  **bitstream** via `zenavif::detect::probe`, never from the request; 3 of the 5
  fail before the fix with `left: 10, right: 8`) and
  `convert::narrow_16_to_8` (3 unit tests, one exhaustive over the whole u16
  domain).

  Not done, and why: `EncodeBitDepth::Twelve` stays unimplemented. The enum is
  not `#[non_exhaustive]`, so adding a variant breaks every downstream
  exhaustive match — a 0.1.7 → 0.2.0 bump that the workspace rules require to be
  real, unavoidable and user-approved. It is queued below instead.
  **SUPERSEDED 2026-09-03: approved and landed — see the BREAKING section at the
  top of [Unreleased].**

### QUEUED BREAKING CHANGES
<!-- LANDED 2026-09-03 in the 0.1.8 -> 0.2.0 bump, and removed from this queue:
     `EncodeBitDepth::Twelve` + `#[non_exhaustive]`; `#[non_exhaustive]` on
     `Av1Backend` / `DecodeBackend` / `TargetMetric` (plus
     `TargetMetric::ZensimC` and `ValidationError::BackendUnsupportedParam`).
     See the BREAKING section at the top of [Unreleased]. -->
- Remove `Error::ColorConversion(yuv::YuvError)` — the last public-API tie to
  the `yuv` crate. In-house kernels no longer construct it (they are
  infallible); the legacy `unsafe-asm` decoder still does. **Deliberately NOT
  taken in 0.2.0**: the only remaining constructor lives in `src/decoder.rs`,
  which is behind `unsafe-asm` and is compiled by nothing — not CI, not the dev
  box (Apple `cc` refuses the rav1d `.S` sources) — so removing it would be an
  edit made blind, with no build to catch a mistake. Ships when either that
  file is compilable somewhere or the variant's last constructor goes.
- Remove the deprecated aliases `encode-svt-rs`, `aom-backend`,
  `Av1Backend::SvtRs`, `DecodeBackend::AomRs`, `EstimateArm::SvtRs420`. Their
  notes said "removed in 0.2" and they were **deliberately kept** in 0.2.0: a
  live consumer was still building against the old spelling when the bump was
  cut. The notes now say the removal is deferred and is not tied to a version.
  `tests/deprecated_backend_aliases.rs` fails to compile if they regress.

### Changed
- **`src/yuv_bilinear_fix.rs` retired; `yuv` moved 0.8.12 → 0.8.17.** Upstream
  0.8.17 fixes the even-height 4:2:0 bilinear row-pair defect the wrapper
  existed to repair, so the wrapper, its module declaration and all eight of its
  call sites in `src/decoder.rs` are gone, replaced by direct converter calls
  shaped exactly like the sibling `Cs422` arms already there.

  **Byte-identity gate, run before the deletion** (`benchmarks/yuv_bilinear_retirement_2026-08-29.{tsv,meta}`;
  harness `dev/yuv-bilinear-retirement/`): 2,304 cells — all eight converter
  variants the decoder uses × 16 geometries (2×2 … 2048×2048, both height
  parities, odd widths) × 2 ranges × 3 matrices × 3 content classes.
  **Wrapper-on-0.8.15 and direct-on-0.8.17 were byte-identical in 2,304 of
  2,304 cells**, so the retirement moves no output byte. The gate is not
  vacuous: the same harness run direct-on-0.8.15 differs in **1,836** cells,
  and writes the final row in **0** of 1,872 even-height cells versus 1,836 for
  the other two rounds — the defect's exact signature.

  **Scope correction to the 2026-08-29 note below, which said this touched
  "eight live 4:2:0 chroma-upsampling call sites".** They are not live: every
  one is in `src/decoder.rs`, which is `#[cfg(feature = "unsafe-asm")]`, and
  per this repo's CLAUDE.md that feature is compiled by nothing — not CI, not
  the dev box. The wrapper was dead code in every configuration anyone builds.
  The gate above proves the stronger property anyway (the output would have
  been identical had it been live), which is what makes the deletion safe if
  `unsafe-asm` is ever revived. `src/decoder.rs` was type-checked for this
  change using the documented technique — temporarily dropping `"rav1d/asm"`
  from the feature so Apple `cc` need not assemble rav1d's `.S` sources.

  **The retired reverse tripwire is replaced, not merely deleted.**
  `tests/yuv_upstream_bilinear.rs` now pins the positive property — upstream
  must write every row on even-height 4:2:0 bilinear input — so a regression
  upstream is caught rather than silently reintroducing the original defect.
  It is mutation-verified: against `=0.8.15` it fails naming the exact row,
  while its odd-height companion still passes (odd heights were never
  affected). It runs unconditionally, unlike the code it guards.
- **Third-party lockfile refreshed within the existing requirements** (`ac849ce`).
  `Cargo.lock` only — no manifest requirement moved. 55 third-party packages
  advanced, notably `libc` 0.2.186 → 0.2.189, `cc` 1.2.64 → 1.4.4, `zerocopy`
  0.8.52 → 0.8.56, `thiserror` 2.0.18 → 2.0.20, `bytemuck` 1.25.0 → 1.25.2,
  `imgref` 1.12.2 → 1.12.3, `wide` 1.5.0 → 1.7.0 and `exr` 1.74.0 → 1.74.2;
  nine transitive crates joined the graph (`pulp`, `reborrow`, `regex`,
  `zlib-rs`, `num-complex` and friends), none left it. Every git-sourced and
  zen-family package was held byte-identical — `rav1d-safe` stays at rev
  `66f58fa6`, `zensim` / `zensim-regress` at `a390a182`, `zenanalyze-api` at
  `47c3c69e`, `zenbench` at 0.1.8 — so this does not move the zen-family graph.

  **`yuv` was deliberately held at 0.8.15.** 0.8.17 (2026-08-08) *fixes* the
  upstream even-height 4:2:0 bilinear defect that `src/yuv_bilinear_fix.rs`
  exists to repair, which trips that module's reverse tripwire
  (`upstream_drops_last_row_pair_on_even_height`, whose failure message reads
  "upstream yuv fixed the dropped-last-pair bug — this wrapper (and its call
  sites) can be retired"). Retiring the wrapper is a change to eight live 4:2:0
  chroma-upsampling call sites in `src/decoder.rs`, so it wants its own commit
  with a byte-identity check against the wrapper's current output — not a
  dependency refresh. Left for that follow-up; see the tripwire's doc comment,
  which currently records the defect as "verified against yuv 0.8.12 and 0.8.16".

  Verified on aarch64-apple-darwin at 0.8.15: `cargo test --workspace` and
  `--no-default-features` green; clippy (default + both member legs) clean;
  `cargo fmt` (package-scoped) clean; `cargo hack check --rust-version
  --workspace` clean at 1.93; **`gate-determinism` PASS** (5 cells × 5 thread
  legs, 0 failures) and **`gate-conformance` PASS** (56 AVIF cells byte-agreeing
  with `aomdec`, 0 failures; the `ZENRAV1E` armed leg was skipped deliberately —
  that CLI is not built here). `gate-ladder` / `gate-monotone` were not run: they
  are timing envelopes and the box was at load ~30 under concurrent agents.
- **The decoders no longer opt out of strict container validation, and a file
  carrying an unknown property marked `essential` is now refused instead of
  decoded with a log line.** `AvifDecoder::new` (`src/decoder.rs`) and
  `ManagedAvifDecoder::new` (`src/decoder_managed/decoder.rs`) both passed
  `zenavif_parse::DecodeConfig::default().lenient(true)`, overriding a parser
  default that is documented as *"Default: false (strict validation)"*. This
  reached shipped code: imageflow's `create_avif`
  (`imageflow_core/src/codecs/zen_decoder.rs:295`) builds
  `zenavif::AvifDecoderConfig::new()`, which routes through
  `src/codec/decoder.rs` into `ManagedAvifDecoder::new`.

  **How the reason got lost, recorded here so it is not lost twice.** The
  `.lenient(true)` was originally justified in place by the comment *"Use
  lenient parsing to handle files with non-critical validation issues"*.
  Commit `0a6606a` ("switch to zenavif-parse with zero-copy AvifParser")
  replaced that comment with an unrelated note about zero-copy borrowing
  **while keeping the `.lenient(true)` itself**. The behaviour outlived its
  justification, and every later reader saw an unexplained opt-out. What it
  silently bought was four downgraded conformance checks: non-zero reserved
  flags in boxes required to have none, and three `essential`-flag rules —
  including *unknown property marked essential*, where zenavif-parse's own
  warning says the item "will be unusable".

  **What was actually load-bearing, measured rather than assumed.** Parsing all
  227 AVIF files in this repo's corpus under both settings, leniency changed the
  outcome for exactly **two**, and not for the reasons folklore recorded:
  `tests/vectors/libavif/extended_pixi.avif` failed strict on the *flags* check
  (`expected flags to be 0`) — its `pixi` box carries `flags = 0x000001` plus 6
  bytes of extension payload, so the trailing-bytes tolerance was never the
  first blocker — and `tests/vectors/libavif/clap_irot_imir_non_essential.avif`
  failed on `property must be marked essential`. A blanket restore of all four
  checks would therefore have regressed the second file; that outcome is
  mutation-verified.

  Both deviations are now handled narrowly inside `zenavif-parse` (see its
  changelog), so leniency is load-bearing for **zero** of the 227 files and the
  decoders run strict. Decoded output is unchanged: all 214 decodable corpus
  vectors produce byte-identical pixels (208 decode OK with identical
  fingerprints, the same 6 refusals with the same messages). New gate:
  `tests/parser_leniency_scope.rs` (5 tests, each mutation-verified) pins both
  halves — the two files still decode byte-identically, and the silenced checks
  are enforced again. Re-adding `.lenient(true)` to either decoder fails three
  of them.

  The six `examples/` that still parse leniently now say why in place
  (diagnostic tools and sweep harnesses want malformed files to load);
  `tests/cross_backend_decode.rs` was switched to strict, since every container
  it parses was produced by our own encoder moments earlier. (`e63fbcd`,
  rationale wording corrected in `fca0a31`; the lost justification was dropped
  in `0a6606a`.)
- **`zencodec` / `zenpixels` / `zenpixels-convert` requirements now span the
  published minor and the next one**, across all four workspace manifests:
  root `zencodec >=0.1.26, <0.3.0`, `zenpixels >=0.2.16, <0.4.0`,
  `zenpixels-convert >=0.2.16, <0.4.0`; `fuzz/` the same for its two;
  `zenavif-parse` `zencodec >=0.1.26, <0.3.0`; `zenavif-serialize`
  `zenpixels-convert >=0.2.13, <0.4.0`. For a `0.x` crate Cargo treats the minor
  as the major, so the plain `"0.1.26"` meant `^0.1.26` = `>=0.1.26, <0.2.0` and
  a `zencodec 0.2.0` release would have been invisible until every one of those
  manifests was hand-edited — the coordinated wave the 0.1.26 rollout already
  cost. Floors are unchanged and nothing newer is published, so resolution is
  identical today (`cargo metadata --all-features`: one `zencodec 0.1.26`, one
  `zenpixels 0.2.16`, one `zenpixels-convert 0.2.16`). The standing
  current-plus-next rule is documented in the zencodec repo's `CLAUDE.md`.
  `[patch.crates-io]` is untouched — a patch replaces the source regardless of
  the requirement.
- **`zensim` re-pinned from registry 0.2.4 to git `main` (0.3.0)** for the
  diffmap API the closed loop needs (`compute_with_ref_and_diffmap`,
  `PrecomputedReference` reuse, `Zensim::with_stop`). Consequences:
  `zensim-regress` is pinned to the same git branch (registry 0.3.1 depends
  on zensim 0.2.x, which put two incompatible `Zensim`/`ImageSource` types
  in the graph and broke `tests/linku_corpus.rs`), and all six
  `ZensimProfile::latest()` call sites moved to
  `ZensimProfile::codec_target()` — `latest()` is deprecated in 0.3.0.
  **`TargetMetric::Zensim` scores on a different profile than before**:
  `latest()` was `PreviewV0_2` at 0.2.x and is `B` at 0.3.0, so a given
  target lands on a different quality. Unpin both at the zensim 0.3.0
  publish.
- zenav1-svt pin bumped `3e25f52b` → `2d585bb2b` (upstream master
  2026-07-24; zero seam API breaks). The pin is past the upstream repo
  restructure that moved the C reference into a git submodule — the
  `zenav1-svt*` crates are pure Rust and unaffected (crate names + pub API
  unchanged). Brings the typed QP-0 rejection
  (upstream #5: `try_encode_frame*` now returns `UnsupportedConfig` for
  base_qindex 0 instead of emitting garbage — the seam's quality→QP clamp
  keeps quality 100 encoding at QP 1 and is now composition-tested from
  both sides), the IBC screen-content vertical, the bd8/bd10 real-photo
  p0–p3 exchange-sort parity closures (photo p0 bd8 135/135, bd10 p0–p3
  187/187 upstream), and the nz-map SIMD port. Seam drift fixed in the
  same change: stale "no C bitstream identity yet" / "non-conformant"
  claims rewritten to the verified envelope, the 64-multiple dimension
  gate re-documented as a zenavif-side envelope choice (upstream pads +
  codes partial SBs since task #95), and `EncoderConfig::threads` is now
  threaded into the pipeline (`thread_count`; byte-inert, single-tile
  today).

### Added
- **`Av1Backend::SvtRs` encodes 10-bit (#33).** `encode_rgb16` /
  `encode_rgba16` and `EncodeBitDepth::Ten` on 8-bit input now route to
  the svtav1-rs backend instead of being refused: RGB → YCbCr (BT.601 full
  range, 4:2:0) runs at 10-bit precision through a new depth-generic
  forward kernel (`yuv_convert::rgbx_to_yuv420_u16`, the same f32 recipe
  quantized at the output depth — an 8-bit source keeps its chroma-average
  fraction bits), and the u16 planes go through the port's native
  `EncodePipeline::try_encode_frame_420_hbd` (imazen/zenav1-svt#6, landed
  upstream 2026-08-04). The container carries profile 0 / 10-bit av1C,
  BT.2020/PQ/HLG CICP, and `clli` / `mdcv` from
  `EncoderConfig::content_light_level` / `mastering_display` (HDR static
  metadata is box-level; the port emits no metadata OBUs). Envelope:
  10-bit **alpha and grayscale** Cs400 streams need speed ≥ 7 (SVT preset
  ≥ 9, the port's only bd10 monochrome level producer;
  `encoder_svt_rs::svt_rs_depth_error`, shared with `validate_for_input`,
  which also stops applying the zenravif identity-path "16-bit + 4:2:0"
  exclusion to this backend). Measured on the pinned rev (aarch64, q85):
  RGB16 → 10-bit 96x80 54.9 dB (10-bit domain), RGBA16 at speed 7 54.7 /
  62.9 dB colour / alpha, RGB8 + Ten 54.1 dB; at the QP floor the 10-bit
  path's 10-bit-domain RMS error is 1.07 vs 2.33 for the 8-bit path from
  the same 16-bit source (the low bits are coded, not truncated); 4 cells
  (64-aligned, partial-SB, odd, preset 0) decode byte-identically on
  rav1d-safe and aom-rs as 10-bit streams. Known band limit (upstream hbd
  chunk 2): the deblock / CDEF / Wiener searches still decide on
  MSB-truncated planes. Not measured: the encode memory model
  (`heuristics::EstimateArm::SvtRs420`) was fit on 8-bit cells and is
  reused for 10-bit as-is. Tests:
  `svt_rs_rgb16_roundtrip_10bit_pq_with_hdr_metadata`,
  `svt_rs_rgb8_bit_depth_ten_codes_10bit_stream`,
  `svt_rs_10bit_path_keeps_low_bits_vs_8bit_at_qp_floor`,
  `svt_rs_10bit_alpha_needs_speed_7`,
  `svt_rs_gray8_bit_depth_ten_needs_speed_7`,
  `svt_rs_10bit_output_decodes_identically_on_both_backends`,
  `yuv_convert::tests::forward_u16_kernel_matches_8bit_recipe_at_depth_10`.
- **`Av1Backend::SvtRs` accepts non-64-multiple dimensions at speed ≥ 5
  (#32).** The seam's blanket 64-multiple gate is now one predicate
  (`encoder_svt_rs::svt_rs_dims_error`, shared with
  `EncoderConfig::validate_for_input`): multiples of 64 at any speed;
  arbitrary dimensions — odd, partial on both axes — at SVT preset ≥ 6
  (speed ≥ 5) on the 4:2:0 colour path, where the port codes partial
  superblocks C-identically and signals the true size; with an alpha or
  grayscale Cs400 stream, multiples of 8 at speed ≥ 6 only. Presets 0–5
  keep the 64 rule (the port's partition search is not C-identical on a
  partial SB there). Measured on the pinned rev: every 4:2:0 partial-SB cell
  at speeds 5–10 round-trips at 47.8–50.8 dB with rav1d-safe and aom-rs
  byte-agreeing; the mono path at preset 6 is mis-coded upstream (12–26 dB
  or undecodable — CLAUDE.md Known Bugs), hence the stricter alpha/gray
  gate and the canary `svt_rs_direct_mono_partial_sb_preset6_still_broken`.
  Tests: `svt_rs_partial_sb_roundtrip_at_preset_ge_6`,
  `svt_rs_rgba_partial_sb_needs_8_aligned_dims_at_speed_6`,
  `svt_rs_gray8_partial_sb_needs_8_aligned_dims_at_speed_6`,
  `svt_rs_partial_sb_output_decodes_identically_on_both_backends`.
- **`zensim_c`: scoring and steering on zensim generation-C
  (`ZensimProfile::C`), plus the additive `TargetMetric::ZensimC`.**
  `C` is a 944-input MLP over the folded-720+append+append2 regime, which
  the standard 372-feature `Zensim::compute` pipeline does not produce, so
  it needs its own front end: `compute_folded720_features_streaming` +
  `score_features_with_profile(C, …)`. Feeding `C` through `compute` fails
  with `ModelForwardFailed` — except on a byte-identical pair, which
  short-circuits to 100 before the forward pass, so a naive smoke test
  passes and proves nothing. `c_via_compute_fails` pins both halves.
  - **`C` has a per-pixel steering map, and it is NOT a diffmap.**
    `AttributionResult` is a real row-major `w × h` f32 plane (trimmed to
    the logical image, no SIMD padding) plus a summed-area table for O(1)
    rectangle queries. Its semantics differ from `DiffmapResult` in every
    way that matters to pooling: it is **signed**, its unit is **score
    points**, and it is absolutely normalized and mass-conserving —
    `query_rect(B)` is the linearized score gain from re-encoding block
    `B` at reference quality. The B rule
    `clamp(e_b / geomean(e_b), 0.4, 2.5)^(−strength)` therefore does not
    transfer: its whole justification is that SSIM error has no absolute
    unit, and a geometric mean is undefined over a sign-mixed quantity.
    `sb_q_scale_from_attribution` re-derives on the area-weighted
    arithmetic mean (the quantity is additive) with non-positive blocks
    pinned neutral. DERIVED, NOT FITTED; nothing enables it by default.
  - **HDR is refused twice, because one silent-wrong-number path is real.**
    `sdr_guard` rejects CICP transfer 16 (PQ) / 18 (HLG) with a typed
    `Error::Unsupported`, and `ZensimC::features` rejects
    `ImageSource::is_hdr()` before extraction — zensim's folded-944
    extractor does not error on an HDR-flagged `LinearF32Rgba` + opaque
    pair, it silently auto-routes to the PU/HDR front end and returns
    944 HDR-domain features, which `score_features_with_profile` has no
    domain guard against. `profile_for_transfer` names `BHdr` for HDR and
    says plainly that this crate cannot drive it (no absolute-luminance
    PU-linear front end).
  - **Honest costs.** No `PrecomputedReference` for the 944 extraction
    (zensim removed the prepared-reference forms; only `V2Scratch`
    allocation is reused), `Zensim::with_stop` is not checked by the v2
    extraction walks, and `steer()` is a second full walk plus two bake
    forwards per live column — not the one-call score+map the B loop gets.
    Measured numbers: `benchmarks/zensim_c_*_2026-08-07.*`.
- **`two-pass-zensim`: a zensim-diffmap-driven closed loop
  (`encode_rgb8_zensim_loop`).** Encode → decode → ONE
  `Zensim::compute_with_ref_and_diffmap` per pass → correct the quality
  **globally** from the score and the per-64×64-superblock quantizer
  **spatially** from the diffmap → re-encode. The reference pyramid is
  precomputed once (`precompute_reference`) and reused by every pass, and
  the caller's stop token is threaded into zensim itself
  (`Zensim::with_stop`), so a long scoring pass is interruptible.
  - **Global half is live.** It steps along the fitted population curve,
    `q_next = q + (Q(target) − Q(score))`, rather than a constant-slope
    Newton step — the score-vs-quality curve saturates, so the same score
    error needs a very different quality move at 40 than at 90. Once the
    target is bracketed it hands off to the same clamped secant
    `encode_rgb8_with_target` uses.
  - **Spatial half is release-gated** behind `zenravif::FRAME_HINTS_LIVE`
    (re-exported as `SPATIAL_HINTS_LIVE`) and reported per call as
    `ZensimLoopResult::spatial_applied`, so a computed map can never be
    mistaken for an applied one. Unlike `two-pass-butteraugli` — purely
    spatial, so it refuses to run at all while the gate is shut — this loop
    runs and converges regardless.
  - **The diffmap → quantizer-scale mapping is derived, not transplanted.**
    libaom's `tune=butteraugli` valuation `min(mse/distance, 5) + K` cannot
    take a zensim diffmap: the map is unitless SSIM error, so typical photo
    blocks land at ratios of 10²–10³ and the clip saturates on every block.
    Derived instead from scale-invariance + equal-error allocation:
    `q_scale_b = clamp(e_b / geomean(e_b), 0.4, 2.5)^(−strength)`,
    `strength = 1/γ` under `e ∝ step^γ`, default 1.0. DERIVED, NOT FITTED,
    and not honestly fittable until the gate opens (an applied map has no
    bitstream effect today).
  - **Measured against the existing secant baseline**, same images, same
    targets, same options on both arms (12 sources x long edges
    {64, 256, 1024} x zensim targets 20-90 step 5, plus a 4-source x 2048
    leg on a step-10 grid; `benchmarks/zensim_loop_ab_2026-08-06.tsv` and
    `..._tol1_...`, summaries alongside):

    | tolerance | arm | n | mean encodes | converged | 1 encode | <=2 encodes |
    |---|---|---|---|---|---|---|
    | 0.5 | secant | 572 | 4.14 | 66.3% | 8.0% | 14.7% |
    | 0.5 | loop | 572 | **3.75** | 64.9% | **12.9%** | **29.4%** |
    | 1.0 | secant | 360 | 3.35 | 82.2% | 17.8% | 31.7% |
    | 1.0 | loop | 360 | **2.90** | **82.8%** | **23.3%** | **51.4%** |

    Paired per-cell at tolerance 0.5: -0.39 encodes on average (fewer on
    43.4% of cells, more on 23.3%), achieved-vs-target error a wash (closer
    26.9%, further 26.9%, median delta 0), identical file sizes. The gain
    holds at every size and target band. At tolerance 0.5 the loop trades
    marginally lower convergence (64.9% vs 66.3%) for the encode saving --
    a tight early bracket runs out of lattice resolution where the
    baseline's coarse extrapolation gets more chances to land on a lattice
    point; at tolerance 1.0 that trade disappears.
  - Anchor-curve fit and its 1,008-encode sweep:
    `benchmarks/zensim_anchor_2026-08-06.tsv` (+
    `scripts/hyperparam/fit_zensim_anchor.py`, deterministic).
  - New public API: `two_pass_zensim` module, `ZensimLoopOptions`,
    `ZensimLoopResult`, `encode_rgb8_zensim_loop`,
    `anchor_quality_for_zensim`, `SPATIAL_HINTS_LIVE`.
- **The achievable-score lattice is now a measured, documented limit**
  (`benchmarks/zensim_score_lattice_2026-08-06.tsv`). `quality` resolves to
  an integer AV1 quantizer index, so the reachable zensim scores form a
  discrete lattice no search can land between: adjacent achievable scores
  are 0.82–1.05 apart at the median and ~50% of gaps exceed 1.0 over
  quality 50–80. A ±0.5 `tolerance` is therefore unreachable about half the
  time for reasons unrelated to the search — relevant to
  `TargetOptions::tolerance` as much as to the new loop.
- **CI now builds and tests the diffmap closed loops.** Neither
  `two-pass-butteraugli` nor `two-pass-zensim` appeared anywhere in
  `ci.yml`, so `tests/two_pass.rs` ran on no platform at all; a new test leg
  covers both. `scripts/gauntlet.sh` gains a `zloop` combo and adds
  `two-pass-zensim` to `allsafe`.
- **Memory-adaptive encode concurrency: encodes fit their thread count to
  the memory budget instead of blowing past it.** The zencodec encode
  pre-flight (still + animation paths) now checks the CALIBRATED
  thread-aware peak estimate (`heuristics::estimate_encode_threaded`) —
  not just the raw input-buffer size — against an explicit
  `ResourceLimits::max_memory_bytes`, or, absent one, against an implicit
  budget of 80% of detected available RAM (Linux `/proc/meminfo`
  `MemAvailable`; no implicit cap elsewhere). When the budget requires it,
  the encoder walks its thread count down (floor 1) and pins the reduced
  count on the native config — including under `ThreadingPolicy::Parallel`,
  which previously always kept the machine-wide default — and records the
  reduction on the `EncodeOutput` as a `String` extra
  (`output.extras::<String>()`); reductions are never silent. Only when
  even the single-threaded estimate exceeds the budget does the encode
  error (the memory-limit error; the implicit-budget message says to set
  `max_memory_bytes` to override — a clean error beats the kernel OOM
  kill measured on 32 GB boxes). `estimate_encode_resources` now returns
  thread-aware figures too: peaks carry the measured per-thread
  working-set term and wall time is divided by the fitted Amdahl speedup
  (with `cpu_ms` carrying the single-thread work). All helpers are
  crate-private — no new public API. (zensysbench CODEC-MEMORY-PLAN
  wave 2.)
- `DecoderConfig::decode_backend` — the aom-rs decoder is now a selectable
  PRODUCT decode backend (feature `aom-backend`, experimental): non-grid
  stills decode through zenav1-aom across the full still surface — 8/10/12
  bit, 4:2:0/4:2:2/4:4:4 + mono, RGBA alpha items, identity (MC=0),
  exotic matrices, ICC/CICP/HDR-metadata passthrough (upstream
  `FrameDecode` gained the sequence CICP fields), gain-map item decode
  (`decode_av1_obu_with_config`, both zencodec gain-map sites routed), and
  the prefer_8bit downscale — byte-identical to the rav1d-safe path by
  construction (both run the same canonical in-house kernel recipe; pinned
  across the whole grid by `tests/product_aom_backend.rs`). ANIMATION now
  decodes too: zenav1-aom landed the animated-AVIF inter envelope (8/8
  tracks / 40/40 frames byte-exact vs aomdec upstream, pin a06bce15) and
  `AnimationDecoder` on AomRs eagerly decodes each track in one
  `decode_frames` pass (DPB/CDF state spans samples) — all 5 libavif
  animated vectors byte-identical to the rav1d path, frame-by-frame
  (pixels + durations). Grid AVIFs decode too (each grid cell is an
  independent AV1 still; the byte-stitch is shared with the rav1d path) —
  identity pinned on the sofa/mixed-alpha grid vectors. Only row-sink
  streaming still returns honest `Unsupported` on AomRs.
- `decode_av1_obu_yuv_with` — the raw-OBU decode seam gains a config-carrying
  twin threading `DecoderConfig` + a stop token into the backends
  (seam obligation 3): `frame_size_limit` reaches rav1d-safe's managed
  `Settings` and aom-rs's `DecodeLimits::max_pixels`, the stop token is
  polled in-loop by aom-rs (SB-row/tile/frame cadence), and `alloc_pref`
  maps onto aom-rs's `AllocMode` (fallible pre-flight by default).
  Liveness pinned by `tests/cross_backend_decode.rs::config_threading`.
- Cross-backend validation + calibration (2026-07-22): every encode-backend
  bitstream is now cross-decoded on rav1d-safe AND zenav1-aom with plane
  byte-compare (`tests/cross_backend_decode.rs`, incl. 10-bit; 7018/7018
  sweep cells identical — first conformance coverage of OUR encoders'
  output), the ssim2 target search is pinned to converge over
  `Av1Backend::SvtRs` (the unified cross-backend quality mechanism), and
  the RD/speed calibration sweep is committed
  (`benchmarks/backend_sweep_2026-07-22.{tsv,meta}`, harness
  `examples/encode_backend_sweep.rs` + `examples/decode_backend_bench.rs`).

- The canonical YUV numeric recipe is now a documented FIXED-POINT formula
  (P8): single rounding of a 2^-16-accurate value, pure-integer arithmetic
  (platform-exact on every arch incl. wasm — no FMA/rounding-mode variance),
  offsets derived from the rounded coefficients so gray decodes to exactly
  R=G=B at every depth. d<=12 decode paths (all real AV1) run it; measured
  2-10x over the f32 kernels (benchmarks/yuv_fixedpoint_2026-07-20.csv:
  8-bit 420 490 Mpx/s, 444-rgba 3.4 Gpx/s, 10-bit 420 484 Mpx/s). Accuracy
  pinned within ±1 of exact rational conversion by test.
- One unified in-house YUV kernel family (`src/yuv_convert.rs`): strip-first,
  generic over sample depth (8/10/12/16-bit) and output pixel
  (RGB/RGBA/Gray x u8/u16), separable auto-vectorized chroma passes, one
  canonical f32 numeric recipe (reciprocal-mul normalize, chained FMA,
  round-ties-even), AVX-512/AVX2/NEON/wasm/scalar tiers. Replaces four
  independent kernel implementations; the managed decoder now converts every
  planar/mono path in-house (BT.601/709/2020 + FCC/SMPTE-240M/derived via
  explicit Kr,Kb), and the SvtRs encode path uses the in-house forward
  RGB(A)->YUV420 kernel (box-averaged f32 chroma; measured +1.2 dB on the
  q85 gradient round-trip). 8-bit decode conversion measured 2-6x faster
  (benchmarks/yuv_kernel_unify_2026-07-20.csv); new default-on `avx512`
  feature. The `yuv` crate remains only in the legacy `unsafe-asm` decoder
  and the public error type (queued above).
- SvtRs backend: RGBA8 encode (color 4:2:0 item + straight-alpha Cs400
  `auxl` auxiliary item honoring the `alpha_quality` fallback contract) and
  grayscale encode (monochrome Cs400 color item, `encode-mono` feature) —
  the backend now covers all three 8-bit still input shapes; round-trip +
  contract tests in `tests/svt_rs_backend.rs`
- `DecodeBackend` (renamed from the decode-seam `Av1Backend` to avoid
  colliding with the encoder enum of the same name in `encode` builds)
  gained `Rav1dFfi` (upstream rav1d 1.1.0 with full asm, `unsafe-asm`
  feature) as a third raw-OBU decode-seam arm; the 4-way decode benchmark
  interleaves it when built with `unsafe-asm`
- EXPERIMENTAL svtav1-rs AVIF encode backend: `Av1Backend::SvtRs` behind the
  new default-off `encode-svt-rs` feature (git-branch dep on imazen/svtav1
  `wave2/entropy-c-parity`, pinned rev conformance-verified upstream at
  525/525 mono + 700/700 4:2:0 aomdec cells). 8-bit 4:2:0 stills with
  64-px-aligned dimensions; BT.601 full-range; muxed in-crate via
  zenavif-serialize; out-of-scope configs rejected honestly at validate()
  and encode time. Bitstream identity vs C-SVT is NOT yet asserted — that
  parity gate lands with svtav1-rs decision-layer bitstream identity.
  (`src/encoder_svt_rs.rs`, `tests/svt_rs_backend.rs`)

- **Adopt the `zencodec` `CategorizedError` taxonomy (PR #103, final API).**
  `Error` now `impl zencodec::CategorizedError` with
  `codec_name() == Some("zenavif")` — a `&self` method (not an associated const,
  so the trait stays dyn-compatible) — and an exhaustive `category()` mapping
  every variant to one coarse `zencodec::ErrorCategory`, so consumers route on
  the category (HTTP status, retry policy, logging) without naming the enum. The
  `Parse` arm **delegates** to `zenavif_parse::Error`'s own `CategorizedError`
  (zenavif-parse PR #17): a malformed container stays `MalformedImage`, a
  truncated one `UnexpectedEof`, a parser cap `LimitsExceeded`. Other arms:
  `Unsupported` → `UnsupportedImageFeature`; `ImageTooLarge` →
  `LimitsExceeded(Pixels)`; `ResourceLimit` → `LimitsExceeded(Memory)` (the
  `String` variant is a catch-all, so a representative kind is reported, with
  the precise limit in `Display`); `OutOfMemory` → `OutOfMemory`;
  `Cancelled(r)` → `r.category()`; `UnsupportedOperation(op)` → `op.category()`;
  `Decode` / `ColorConversion` (foreign `yuv`) / `Encode` → `Internal`. `Decode`
  is a grab-bag the decode pipeline reuses for decoder setup/flush faults and
  internal invariant checks (e.g. "expected 8-bit planes") as well as some
  malformed-input cases; with no structural signal to split it, the conservative
  `Internal` is its best single category. Behind **temporary `[patch.crates-io]`
  path pins** to the sibling `../zencodec` checkout (the taxonomy is
  post-0.1.25, unreleased) and `../zenavif-parse` (0.7.0, unreleased) until both
  publish. Additive (`#[non_exhaustive]` enum + opt-in trait).

### Changed
- Backend pins: zenav1-aom ed29932f -> 7b972e50 (structured `DecodeError`
  + `DecodeConfig`, bd8 lowbd perf pipeline), svtav1 wave2/entropy-c-parity
  -> master 3e25f52b as `zenav1-svt` (bd10 palette panic gate, palette
  byte-parity #71, SB128 fix #91). Both seams now map every backend error
  variant onto the matching zenavif `Error` variant, the svt seam uses the
  fallible `try_encode_frame*` entries (no more `is_empty()` heuristic),
  and the caller's stop token is threaded into the svt pipelines.

- **Reshape `ErrorCategory` onto zencodec's origin-first, two-level taxonomy
  (zencodec PR #116, `caterr-reshape`), superseding the flat 17-variant shape
  the PR #103 entry below describes.** New shape: `Image(ImageError) /
  Request(RequestError) / Resource(ResourceError) / Policy(PolicyKind) /
  Stopped(enough::StopReason) / Io(CodecIoKind) / Internal(InternalKind)`.
  `error_from_rav1d` now classifies rav1d-safe's real cause instead of
  discarding it: `InvalidData` → `Malformed`, `NeedMoreData` → `UnexpectedEof`,
  `OutOfMemory` → the dedicated `OutOfMemory` variant; only the genuinely
  opaque `InvalidSettings`/`InitFailed`/`Other` setup faults still construct
  `Decode` (→ `Internal(Bug)`, not attributable to the input bitstream).
  `Unsupported(&str)` split by origin across `cicp_resolve.rs` (image-feature),
  `GainMapRender` mode (request), an internal grid invariant (internal), and
  alpha-size mismatch (request-buffer) — previously one string conflating four
  origins. `ErrorCategory::Lifecycle` (the original name in this reshape) was
  further renamed to `Stopped` for the same `StopReason` payload — naming
  only, done crate-wide across every zen codec in the same pass.
  `tests/whereat_trace_preservation.rs`'s `av1_decode_error_preserves_trace`
  updated to accept `Malformed` (the now-correct classification for a
  corrupted AV1 payload) alongside `Decode`, preserving its real assertion
  (non-empty trace across the `decoder_managed` boundary). Additive at the
  type level (new `#[non_exhaustive]` enum shape); `ErrorCategory` itself is
  still unreleased, so this is not a break of any published zencodec API.

### Fixed
- **Both fuzz-regression harnesses could pass while replaying nothing**
  (`tests/fuzz_regression.rs`, `zenavif-parse/tests/fuzz_regression.rs`,
  `Cargo.toml`, `zenavif-parse/Cargo.toml`). Both call
  `zenutils_fuzz::RegressionSuite`, whose published `0.1.0` `run()` treats a
  missing *or empty* seed directory as a silent no-op and returns `()`, so
  neither harness had a guard of any kind. Concretely: the root suite replays
  one seed (`fuzz/regression/fuzz_decode_animation/crash-cdef-tile-overlap.avif`,
  the rav1d-safe CDEF tile race) through five entry points — delete that single
  file and the test stayed green while replaying nothing; and
  `zenavif-parse`'s corpus holds only its `README.md`, so that suite was
  **already replaying zero seeds and reporting success**, indistinguishable in
  the log from a corpus that ran clean. Both now declare a seed expectation up
  front and print a `RegressionReport`: a missing or unreadable seed directory
  is a hard failure naming which it was, and the count is pinned exactly —
  `1` for the root crate, and a deliberate, documented `0` for `zenavif-parse`
  (`min_seeds(0)`, which still requires the directory to exist, so "no seeds
  yet" cannot silently become "the seeds were deleted"; the corpus README now
  records that adding the first seed must bump the constant in the same
  commit). `zenavif-parse`'s empty corpus is the honest state, not an
  oversight: its harness and README were added together as a template ported
  from zenwebp *before* any crash existed, `zenavif-parse/fuzz/artifacts/` does
  not exist, and no fuzz-found fix appears in its changelog. The guards mirror
  the `min_seeds` / `RegressionReport` API landing in `zenutils-fuzz` but are
  kept in-file, because that API is **not published** — crates.io still has
  `0.1.0`; the dev-dependency is retained (with a comment at each site) so
  migration is deleting the local module and restoring the import, with the
  builder chain unchanged. Mutation-verified, each failing only as intended and
  each restored: renaming either `fuzz/regression/` now fails with "does not
  exist" where both previously passed; deleting the root crate's one seed fails
  with "1 seed(s) went missing"; and dropping a file into `zenavif-parse`'s
  empty corpus fails with `left: 1, right: 0` — which also demonstrates that
  the two parse entry points really are wired up and would replay a seed the
  moment one exists. Note for whoever adds the next seed: `fuzz/artifacts/`
  still tracks six raw crash/OOM files that predate the `fuzz/artifacts/`
  `.gitignore` entry; they are unminimized fuzzer output, not minimized
  fixed-bug seeds, so the gate deliberately does not replay them — promoting
  one means `cargo +nightly fuzz tmin` first, then moving it under
  `fuzz/regression/` and bumping the pin.
- **Three silent-corruption decode paths from the 2026-08-26 ultracode sweep
  (issue #40).** (1) Grid AVIFs carrying alpha auxiliary items — per-tile
  `auxl` alpha or an alpha grid item — decoded the colour grid only and
  returned OPAQUE pixels with `Ok` on every backend and entry point (buffered,
  `decode_full`, streaming sink, aom). Alpha-grid stitching is not implemented,
  so these files are now refused with `Error::Unsupported` naming alpha; the
  detection is container-wide (`AvifParser::has_alpha_aux_items`) because the
  primary-only `alpha_data()` filter cannot see either shape. (2) The grid
  stitch placed every tile by its OWN decoded dims while sizing the canvas
  from tile 0, so a crafted grid with one differently-sized tile silently
  scrambled/zero-filled the canvas; tiles are now validated uniform (dims +
  descriptor) and must cover the declared output, and placement comes from
  the single validated tile size — in the buffered stitch and the streaming
  sink. (3) An alpha item coded with the same width but fewer rows than the
  primary left the bottom rows at the converter's opaque default (`zip`
  stopped at the shorter plane) — the default rav1d-safe path now requires
  alpha dims == primary display dims, matching the aom backend's existing
  check. Regressions: `tests/sweep_40_geometry.rs` (real libavif grid+alpha
  vectors; a crafted 1204x800/1204x799 container muxed with zenavif-serialize
  from the link-u fox vectors; each mutation-verified) +
  `decoder_managed::grid::stitch_tests` uniformity cases.
- **Lossless no longer pessimizes at slow speeds (issue #8).** On the
  zenrav1e release the dep chain currently resolves (0.1.4, whose
  "lossless" is qi=1 lossy — zenrav1e#9), speeds 1-4 measurably produce
  +5..+19% larger lossless files than speed 8 at 4.6-11x the wall time,
  and speed 10 is larger still on most content (up to +42%): the slow
  tier's RDO optimizes against phantom distortion the fixed encoder
  wouldn't emit. Lossless encodes now clamp their running speed preset
  into the empirically size-optimal [6, 8] band
  (`LOSSLESS_REGISTRY_SPEED_BAND`); lossy encodes are untouched, the
  encode plan and sweep fingerprint mirror the clamp, and the residual
  in-band s6-vs-s8 inversion is ≤ ~1.1%. Measured before/after:
  `benchmarks/lossless_speed_clamp_2026-07-23.tsv` (requested s1: −5.0
  to −15.2% bytes at 3.2-3.8x faster; worst-case pixel delta at s1-2 on
  paris drops 8 → 2). REMOVE the band (set the const to `None`) at the
  zenrav1e >0.1.4 dep bump — the fixed encoder is bit-exact and
  byte-monotonic (slower = smaller), see the dep-bump checklist in
  CLAUDE.md and `EncoderConfig::speed_effective`.
- svtav1-rs QP-0 corruption gate: quality >= ~99.3 mapped to QP 0, which
  emits valid-syntax bitstreams decoding to garbage pixels on the pinned
  rev (imazen/zenav1-svt#5). The seam clamps QP to >= 1
  (`quality_to_qp_gated`), so quality 100 now encodes at the best verified
  tier instead of corrupting.
- **Decode corruption: the bottom two rows of every even-height 4:2:0 decode
  that routed through the `yuv` crate's bilinear converters came back
  unwritten (black / alpha-0).** Upstream defect in yuv 0.8.12–0.8.16
  (`yuv420_*_bilinear` / `i0xx_*_bilinear` pair luma row-pairs with
  overlapping chroma `windows()`, dropping the final pair on even heights;
  odd heights and 4:2:2 unaffected). Affected zenavif paths: RGBA composite
  decode (`decoder_managed`), the exotic-matrix RGB fallback, raw-OBU gain
  map decode (`decode_av1`), and all 8/10/12/16-bit 4:2:0 paths of the
  legacy `unsafe-asm` decoder. Fixed by `src/yuv_bilinear_fix.rs`: run the
  upstream converter, then re-run it on a tight 2-row sub-image with the
  last chroma row duplicated (the crate's own odd-height clamp semantics) —
  already-written rows stay byte-identical. Found by the SvtRs RGBA
  round-trip test (color PSNR 20.10 dB → 50.20 dB); regression-guarded by
  unit tests including a canary that fails when upstream fixes the bug.
- `cargo test` / `cargo test --features encode` failed to compile on any
  feature set that leaves `zencodec` or `encode` off: the
  `gainmap_render_probe`, `gainmap_reencode`, `twelvebit_probe`,
  `hdr_encode_cell`, and `hdr_fidelity_probe` examples landed without
  `required-features` gates (pre-existing; same batch as the red main CI)
- Executable engineering-baseline gates (`docs/ENGINEERING_BASELINE.md` A2/A3/A6):
  `examples/gate_kit.rs` (`determinism`/`cells`/`ladder` subcommands on pinned
  integer-synthetic content) + `scripts/gates/gate_conformance.sh` (the PALCONF
  protocol: aomdec-clean + aomdec==rav1d-safe raw md5) + justfile targets
  `gate-determinism`/`gate-conformance`/`gate-ladder`(-pin)/`gates` + a CI job
  for the determinism `--ci` subset + the machine-scoped ladder envelope
  `benchmarks/gate_ladder_envelope.tsv`. zenrav1e's halves (A1 identity, A5
  recon) landed as `zenrav1e@e0b5b44b`.
- SSIMRD (the TUNER2 "what remains" item (a) prosecuted; record
  `docs/RD_GAP_VS_LIBAOM.md` "SSIMRD"): aom's per-16×16 ssim-rdmult λ curve
  (`av1_set_mb_ssim_rdmult_scaling`, the LAST unported iq/ss2 rdmult
  mechanism) ported to zenrav1e (`zenrav1e@57de2815`,
  `EncoderConfig::ssim_rdmult_strength`, default-off byte-identical) and
  measured as a monotone HONEST NEGATIVE under the composed tune (strength
  ladder 0.25→2.0: mass-weighted ssim2 BD +2.11→+6.85, butteraugli agreeing
  from the aom-verbatim point; no photos-merit; the single train winner 6018
  refuted by its val class-sibling 6091 at +4.77/+2.85/+6.50). The composed
  tune's own distortion-side masking + variance-boost subsume the curve.
  1236/9094-class movement vs cpu2iq-ai: flat-to-worse — the iq-AQ residual
  owner is NOT this curve; the only iq machinery left unported is the
  CDEF_ADAPTIVE adaptation schedule. First program run under the 93b83401
  evaluation policy (per-family first, cluster-mass-weighted, pre-registered
  amended rule). Infra: `scripts/rd_gap/chain_ssimrd.sh` +
  `scripts/hyperparam/analyze_ssimrd.py`;
  `benchmarks/rd_gap_ssimrd{,_val}_2026-07-05.tsv` + raw-sweeps pointer.
- TUNER2 (the two P3 "Near-lossless rescans" handoffs prosecuted; record
  `docs/RD_GAP_VS_LIBAOM.md` "TUNER2"): FOUR measured honest negatives — the
  per-image boost-strength head (train-LOOCV + label-drift + val-transfer all
  negative; the 14-origin valstr label set fills the head's named data gap),
  the deeper-boost-curve ramp (never fires on the deep-AQ class: 1-bit scans /
  gradient illustrations are not low-8×8-variance), the anti-boost OFF-gate
  (blocked by a named corpus gap: document-charts absent from train26 while
  val charts pay +5.8/+7.3 at str1), and the 6096 dead-zone/rounding probe
  (QROUND=128 aom-parity: med +2.67, 20/23 butteraugli vetoes — the constant
  does not transplant without aom's whole valuation stack; zenrav1e#30 item-1
  closed). PLUS the drift discovery: the 2026-07-02 strength labels are STALE
  under the composed tune (qmdist+lfsharp subsumed 2-4 BD points of the
  boost's marginal). Boost default 1.0 stands (18/23 current-binary train
  wins). Infra: `scripts/rd_gap/chain_tuner2.sh` + `fetch_tuner2.sh`,
  `scripts/hyperparam/{refit_boost_strength_p3,fit_boost_gate,analyze_tuner2}.py`,
  label store sources `valstr-2026-07-04` + `tuner2-2026-07-04` (64,338 rows);
  benchmarks `tuner2_valstr_2026-07-04.tsv` +
  `hyperparam_boost_{refit,gate}_2026-07-04.tsv`. Conformance: 1,872 cells
  (1,488 knob-armed incl. the quantizer-rounding arms) all PALCONF-clean,
  0 CELLFAIL/CONFFAIL; byte-continuity 96/96 vs the speedladder store rows.
  zenrav1e-side knobs (`variance_boost_strength` / `variance_boost_deep` /
  `quant_rounding_bias`, zenrav1e@6435e6f9) stay default-None byte-identical.

- S4TIER (FAST_TIER_PARITY_PLAN, the last open fast-tier column): the
  s4-equivalent-tier operating point + the program's final scoreboard. The
  fast_heads tx D bound refit per-tier (`dcty<23.69` at requested speed 4..=5,
  LOOCV 22/24-stable; W + partition gates unchanged; `src/fast_heads.rs`
  s4-tier gates, release-gated recommend-only) + the NEW zenrav1e intra
  mode-RDO budget knob (`num_modes_rdo_override`, zenrav1e@071e9844 — the "no
  top-5 knob exists" gap from the P2 report; default None byte-identical,
  6/6-cell local md5 + 288/288 box chain byte-continuity). Measured (12q,
  PALCONF-clean): **v3+top5 = 6.26× plain-s6 solo (0.97× aom cpu2iq-allintra's
  wall): photos median +2.80 ssim2 / +4.14 ba3n vs cpu2iq-ai — the column
  closed from +4.40/+4.04; top-5 DOMINATED top-7 at mode level** (7.61× for
  +2.84/+4.04); cpu4iq deepens to −1.53/−2.62. The residual is measured
  structural (intraBC screens 8414 +22.5; iq-AQ interiors/illustrations
  1236/9100/9118; near-lossless rescans 6096/6018; ungated full-tx headroom —
  oracle extras +2.36/+2.04 at 10.1×). CDEF/LRF hi-q force probes measured
  null/adverse. Plan verdict: parity ±1% MET at s6+s8, NOT met at the s4 tier
  (structural, per-family quantified); beat ≥2 tiers MET; quality tip KEPT.
  Design fully offline (`scripts/hyperparam/fit_s4_tier.py` over the label
  store); record `benchmarks/rd_gap_s4tier_2026-07-04.tsv` + raw
  `/mnt/v/output/zenavif/s4tier-20260704/`.
- P2HEADS (FAST_TIER_PARITY_PLAN Phase P2, "prediction replaces search"): the
  per-image hyperparameter fast mode. Two new deterministic descriptor heads in
  `src/fast_heads.rs` (release-gated recommend-only, the palette-gate pattern;
  wired into `auto_tune`): a TX budget head ({Largest|Size1|Min} via
  `patch_fraction>0.8505 && dct_compressibility_y>100` withhold /
  `pf<=0.8505 && dcty<8.352` deepen, s6-s8) and a partition budget head
  ({Ship|Max32} via `gradient_fraction_smooth<0.4105`, s6). Fit from the label
  store's fastwins/p1part per-image surfaces (veto-adjusted, LOOCV), the
  conjunctive bounds from a val attribution factoring that convicted the
  pf-only withhold (8103: (none,ship) +18.1 vs (size1,m32) −1.9). Composed s6
  fast mode measured at 12q (box, zenrav1e 39f0ecdd): train26 −4.38 med /
  −7.07 mean vs the s6+size1 base (23/24) vs global-ship −2.89/−4.80; on the
  11 deviating images −5.13 mean vs ship (10/11); VAL (14 held-out origins)
  −3.98/−5.19 vs base, deviators −2.41 mean (6/8, worst real loss +0.32).
  Parity movement (photos vs cached aom-allintra refs): vs cpu4iq-ai
  +2.88/+0.91 (ship) → **+0.57 ssim2 / −0.94 ba3n median — inside the ±1%
  band** (−0.35/−1.70 with the ravif intra arm); below the curve vs
  cpu4def/cpu6iq/cpu6def; composed-v2 (measured wall 3.45× plain-s6)
  strictly dominates cpu2def-ai. Head-3 (intra budget)
  measured NOT-a-head: top-7 keyframe intra (ComplexKeyframes +
  `filter_intra=Some(false)`) is a small broad global win (s6 −0.56 / s8
  −1.17 med, composition-stable) with no per-image structure; no top-5 knob
  exists in zenrav1e (`num_modes_rdo` hardcoded 7|3) — recorded as a ravif
  SpeedTweaks arm candidate. All 0 CELLFAIL / 0 CONFFAIL (PALCONF every
  cell). Fits `benchmarks/hyperparam_{tx,partition}_budget_2026-07-04.tsv`;
  record `benchmarks/rd_gap_p2heads_2026-07-04.tsv` (+ pointer, Tower
  mirror); harness `scripts/rd_gap/chain_p2heads.sh`; label store
  `p2heads-2026-07-04` sources.
- FASTWINS P0 (FAST_TIER_PARITY_PLAN): both speed-ladder cheap wins measured and
  landed upstream — (1) the cavif default-tiling byte hazard is FIXED live on
  ravif main 55f8c935 (tiles capped to ≥1 MP each; old 48-core default cost
  +7.4% median ssim2 BD at s6 with 0/24 images better at any tile count;
  `--threads 1` and 1..48-core defaults byte-identical, 18/18+18/18 md5); (2)
  the s4→s6 rdo_tx cliff is decomposed via new default-off zenrav1e knobs
  (d82c16ba: `rdo_tx_size_override`/`rdo_tx_type_override`/`rdo_tx_size_depth`)
  and the winning size-half depth-1 arm landed release-gated at s6-s8 (ravif
  7baad5f9, `S6_TX_SIZE_RDO_LIVE`): 51% of the s6→s4 RD step at 1.67× solo
  (full-grid s6 −2.78/−3.95/−6.01 ssim2/ba3n/bamax median), s8 −2.89 at 1.43×;
  type-half standalone butteraugli-max-vetoed, reduced_tx_set standalone a
  measured null. 4,176/4,176 armed cells aomdec+rav1d-safe conformance-clean.
  Harness `scripts/rd_gap/chain_fastwins.sh`; verdicts
  `benchmarks/rd_gap_fastwins_2026-07-04.tsv` (+ pointer, Tower mirror); label
  store `fastwins-2026-07-04` sources; zenavif consumes both at the zenravif
  dep bump (see CLAUDE.md "FASTWINS P0").
- SPEED-LADDER GAP MAP: first fast-tier measurement of the RD-gap program —
  zenrav1e s{2,4,6,8,10} × {tune-ss2+palette, off} vs libaom `--allintra`
  cpu{2,4,6,8,9} × {default, `--tune=iq`}, train26 + legacy, per-cell aomdec +
  rav1d-safe conformance (5,520 cells clean), solo wall-time pass. Verdict:
  aom's allintra ladder pareto-dominates every zr arm at matched wall-time on
  photos (no fast-tier crossover); tune mandatory + nearly free at fast tiers;
  mechanism-liveness audit + ranked wedge list seed the fast-mode program.
  Harness `scripts/rd_gap/chain_speed_ladder.sh` + `analyze_speed_ladder.py`;
  `docs/SPEED_LADDER.md` + `benchmarks/rd_gap_speed_ladder_2026-07-04.tsv`;
  label store +9,776 fast-tier rows (speedladder-2026-07-04 sources)
  (5aacaca1, f4924263).
- **`examples/hang_stress.rs` — the zenavif#30 futex-hang repro/stress loop**
  (encode→decode→butteraugli→encode, `fast`/`full`/`decode`/`butter` modes).
  Found the root cause of the two-pass conformance hang: a rav1d-safe
  tile-worker `overlapping DisjointMut` panic (loop-filter compact-COW guards
  vs CDEF) wedged the decode wait forever. Fixed upstream in
  rav1d-safe@49df1fc0 (release-gated until the dep bump past 0.5.7 — see
  CLAUDE.md Known Bugs); verified 613/613 full-stack cells clean on the
  patched chain. Closes zenavif#30.
- **`encode-mono` feature — true monochrome (Cs400) encode for Gray8 input**
  (zenavif#6): `encode_gray8` / the codec Gray8 path route through zenravif's
  new `Encoder::encode_gray8`, coding a luma-only bitstream (no chroma planes,
  no chroma RDO — measured 2–3× faster at output-byte parity vs the gray→RGB
  expansion, `benchmarks/mono_encode_ab_2026-06-11.txt`). End-to-end gate
  `tests/mono_encode_roundtrip.rs` proves the sequence header signals
  `mono_chrome=1` and the file decodes back to native Gray8 via rav1d-safe.
  TEMPORARY non-default gate: requires zenravif ≥ cavif-rs@89668f13 (CI's
  clone-siblings provides it); fold into `encode` at the next zenravif bump.
  Without the feature, Gray8 keeps the pixel-safe RGB-expansion path.
- **HDR contract tests + conformance evidence**: 10-bit PQ/HLG encode→decode
  roundtrips assert CICP echo, clli/mdcv (verbatim G,B,R wire order), re-encode
  chain preservation, and measured pixel-fidelity bounds
  (`tests/hdr_roundtrip.rs`, 5720e260); 21-cell aomdec + rav1d-safe md5
  cross-decoder grid, all clean/agreeing, incl. a 12-bit cell
  (`benchmarks/hdr_conformance_2026-07-03.tsv`, ebc8f525). `examples/ivf_raw`
  now dumps 10/12-bit raw (2-byte LE, aomdec-compatible).
- **Gain-map render evidence**: `examples/gainmap_render_probe` — ReconstructHdr
  envelope verified on 4 SDR-base vectors × 3 headrooms; HDR-base (10-bit)
  reconstruction is an honest documented refusal (dfc878e5).
- `docs/HDR_GAINMAP_STATUS.md`: verified capability tables, measured roundtrip
  gaps, release gates (66ee0be5; finalized 302be267).
- `EncoderConfig::with_gain_map_alt_color` / `with_gain_map_alt_icc` +
  payload-derived gain-map `av1C` (subsampling/monochrome from the AV1
  sequence header; declared dims/depth validated against it — a mismatched
  or unparseable payload is now an honest encode error). Real-vector
  roundtrip contracts: `tests/gainmap_roundtrip.rs`, 4 classes (SDR-base
  4:4:4 multichannel + PQ alt, HDR-base backward + sRGB alt, small-map,
  ICC-base + ICC alt), asserting byte-carry, metadata normal-form equality,
  alt-colr roundtrip, and av1C honesty. Cross-validated with libavif 1.4.1:
  avifgainmaputil printmetadata identical 4/4 vs the original vectors
  (needed the zenavif-serialize tmap-brand/altr/ispe/pixi + seq_profile
  fixes and zenravif GainMapData carriers, all landed on their mains)
  (302be267).

- **Float YUV→RGB output is now byte-identical across arch, SIMD tier, and
  image width.** The 4:2:0/4:2:2 float kernels computed three subtly
  different pipelines: fused `mul_add` on x86-64-v3/NEON lanes, unfused
  mul+add on the scalar tier (what i686 runs — its CI leg had been red on a
  1-LSB green-channel mismatch since 2026-07-06) and on wasm128 (which has
  no FMA at all), and division-normalized ties-away rounding in
  `yuv_to_rgb` (the width-remainder path), so the same image could decode
  to different bytes per platform or per width%8. All paths now compute
  one spec — unfused multiply-add, reciprocal-multiply normalization,
  ties-to-even rounding — which every tier can produce exactly. Rare
  single-pixel ±1 shifts vs. the previous x86-64 fused output are the
  cost of cross-platform determinism; the libavif bilinear-parity suites
  (`test-pixels`, `test-linku`) should be re-run against references as
  follow-up confirmation. (Superseded for d<=12 decode by the fixed-point
  recipe below — all real AV1 depths now use platform-exact integer
  kernels; the one-spec f32 path survives only for 16-bit API entries,
  as scalar `f32::mul_add`, which is correctly-rounded on every target.)
- **rav1d-safe pinned to a git rev (now 398b0bfa, 0.6.0-staged) — registry
  0.5.7 wedge.** Registry 0.5.7 carries a tile-worker guard race whose panicked worker parks
  `rav1d_decode_frame`'s completion wait forever (zenavif#30 — on a
  28-thread box the animated codec roundtrip test hung ~3 of 4 runs). The
  dependency is now a direct git dep at `398b0bfa` (0.6.0-staged: the
  wedge fix 49df1fc0, the #14 aarch64 16bpc SGR rounding fix, and the
  current safe-SIMD state; the earlier f6aed27e `[patch.crates-io]` pin is
  gone — a 0.6.0-versioned rev cannot patch a 0.5.x requirement). Return
  to a registry dep at the 0.6.0 release. The pinned rev's managed API attaches `whereat::At` locations and
  adds cooperative cancellation, so rav1d-safe decode errors now keep
  their upstream trace frames (`At::map_error` at the boundary instead of
  rewrapping) and `Error::Cancelled` maps rav1d-safe's own `Cancelled`.
- **Default-feature `cargo test` compiles again**: four auto-discovered
  examples that call `encode`-gated APIs (`gainmap_reencode`,
  `hdr_encode_cell`, `hdr_fidelity_probe`, `twelvebit_probe`) had no
  `[[example]]` declarations, breaking every platform's CI test job since
  2026-07-12; they are now declared with `required-features = ["encode"]`.

- **Vector/corpus tests no longer silently skip** (no-graceful-skips
  policy, c1533f48): libavif-vector loaders fail loud (CI provisions the
  vectors; locally `just download-vectors`), and the codec-corpus animation
  roundtrip — whose `../codec-corpus` path never resolved, so it silently
  no-oped everywhere since inception — now really runs, with corpus-less
  environments declaring `ZENAVIF_NO_CODEC_CORPUS=1` in the visible CI chain.
- `MasteringDisplayConfig::primaries` doc: the slot order is the `mdcv` wire
  order **GREEN, BLUE, RED** (ST 2086 / HEVC SEI), not R,G,B as previously
  claimed — values were always written verbatim, so following the old doc
  produced swapped primaries for conforming readers.
- **Untrusted-input hardening, decode path** (zenavif#18 items 2–3):
  `StripConverter::new` no longer `panic!`s on unsupported
  (bit_depth, chroma) combinations — replaced by `try_new`, whose
  unsupported case hands the frames back and the decoder takes the
  full-conversion fallback (defense in depth; both values come from the
  attacker-supplied bitstream). The RGB output byte length
  (`pixel_count * 3`) is now a checked multiply returning
  `Error::OutOfMemory` instead of wrapping and under-allocating on
  i686/wasm32 (`rgb_byte_len` + unit test). `DecoderConfig::cpu_flags_mask`
  rustdoc now states the knob is currently inert rather than implying it
  gates SIMD dispatch.
- **Decode allocations are now fallible on the managed path** (zenavif#21):
  the raw-OBU RGB/gray output buffers (`decode_av1.rs`, five sites) and the
  animation frame table reservation (`decode_animation`) route through
  `alloc_util` `try_reserve` helpers, returning a graceful error instead of
  aborting the process on OOM — completing the coverage started by the
  grid-stitch canvas and `convert_to_image` buffers.
- rd_gap harness: per-worker result appends are now flock-serialized (drvfs
  append races silently dropped 315/576 rows on cache-hit-fast runs) and the
  per-cell WORK dir defaults to local disk (drvfs transiently EIOs whole
  worker batches under WSL memory reclaim) (6884b7b, 17c428a).

- `two-pass-butteraugli` feature: `encode_rgb8_two_pass` — butteraugli-
  diffmap-guided second pass (spatial closed loop; libaom `tune=butteraugli`
  analog through zenrav1e's per-SB delta-q). Release-gated at runtime behind
  `zenravif::FRAME_HINTS_LIVE` (fails honestly until the zenrav1e dep bump);
  evaluate-first verdict (aom's own tune: −2.4..−3.5% median butteraugli-3n
  BD on photos, ssim2 neutral-to-better) + mechanism + dep-bump checklist in
  `docs/DIFFMAP_TWO_PASS.md`; A/B harness `scripts/rd_gap/zenavif_2p_cell.sh`
  + `run_2p_ab.sh` + `examples/two_pass_cell.rs` (2e8e9912).
- Size-decay NON-TUNE isolation A/B: driver
  `scripts/rd_gap/sizedecay_nontune_arms.sh` (9 arms over the quality-keyed
  ravif SpeedTweaks gates + Tune::Psnr + composites, per-armed-cell
  aomdec+rav1d-safe conformance), analyzer
  `scripts/hyperparam/analyze_sizedecay_nontune.py`, label-store source block,
  summary `benchmarks/hyperparam_sizedecay_nontune_2026-07-03.tsv`. Verdict:
  no coding default convicted; the tune-off small-px decay decomposes into
  zr's content-adaptive strengths fading on downscaled renditions
  (Psychovisual metric value 7.5->4.4 BD, segmentation AQ 4.1->1.8 BD,
  1024->256) — see RD_GAP_VS_LIBAOM.md. Found + upstream-fixed zenrav1e#34
  (TX_MODE_SELECT sliver gate) and found zenavif#29 (ravif 4:2:0
  non-conformance, open).

### Documentation
- README overhaul: clickable badge row (CI/crates.io/lib.rs/docs.rs/MSRV/dual-license), `## Quick start` with a `[dependencies]` block, absolute links throughout, regenerated crosslink footer (now last), split crates.io README (`README.crates.md` via `readme =` + `include`), and a `benchmarks/README.md` methodology index.

### Added
- **Speed-conditional palette-gate threshold** (follow-up A/B to the mechanism
  A/B; release-gated like the gate itself): `palette_gate(patch_fraction, speed)`
  now uses per-speed-tier thresholds — 0.197 at speed ≤ 5 (byte-identical to the
  prior rule) and 0.05 at speed ≥ 6 (`PALETTE_GATE_PATCH_FRACTION_FAST` +
  `palette_gate_threshold(speed)`); `palette_gate_for_rgb8` gains the speed
  param and `auto_tune` passes the speed it just picked. Measured: arms
  {0.197, 0.10, 0.05, fire-always} × s{2,6,8}, 391/481 BD cells derived offline
  from the label store + mech-A/B TSV (a threshold arm is a per-cell selection
  over already-measured off/always/auto outcomes) + one fresh 1,350-cell s8
  iso run (byte-continuity sha-proven, 0 conformance failures). s2 keeps
  0.197; s6 confirms 0.05 (deploy −0.047 train / −0.074 val vs 0.197, flips
  butteraugli-clean); s8 corroborates (−0.044 val). fire-always measured
  nominally best but rejected: its extra value sits inside the photo
  patch_fraction mass at 1.80×/2.13× (s6/s8) fired encode cost.
  `benchmarks/hyperparam_palette_speed_ab_2026-07-03.tsv` +
  `scripts/hyperparam/fit_palette_speed_threshold.py`; label store +1,350 rows
  (`palette-mech-iso-s8-2026-07-03`).
- **Size-decay isolation A/B (wedge #3) — verdicts + the qmdist size ramp**:
  leave-one-out Tune::Ssimulacra2 mechanism arms × {256,512,1024} renditions
  (`scripts/rd_gap/sizedecay_arms.sh` driver + `scripts/hyperparam/`
  `analyze_size_decay_ab.py`, train/val sample ladders in `scripts/rd_gap/`)
  acquitted 4 of 5 tune mechanisms for the small-size decay (the top suspect
  ss2-QM curves keeps ≥82% of its win at 256), convicted the QM-dist ratio
  (−3.48 → −0.96 median 1024→256), and shipped its half-strength long-edge
  ramp upstream (`zenrav1e@b0098eb1`, release-gated; train +1.03/+0.87 @
  256/512 vs full strength, VAL +1.12/+1.00, butteraugli agreeing,
  conformance 180/180). Most of the wedge decay measured as the tune-OFF
  baseline's own vs-cpu2 decay — the wedge #3 owner moves to non-tune
  small-px coding defaults. Record:
  `benchmarks/hyperparam_size_decay_ab_2026-07-03.tsv` (+ raw pointer),
  `docs/RD_GAP_VS_LIBAOM.md` "Size-decay isolation A/B".
- **Hyperparameter-expert label store + first threshold-rule heads**
  (FEATURE_HINTS_PLAN §E): `scripts/hyperparam/build_label_store.py` aggregates
  every mechanism fit sweep + the wedge dataset into one queryable parquet
  (14,880 rows / 50 arms, per-row arm knobs + encoder_rev + q_kind + LSD split
  + feature-join; block storage + Tower, pointer in `benchmarks/`), and three
  first-cut rule fits land their evaluations in
  `benchmarks/hyperparam_*_2026-07-03.tsv`: the zenanalyze palette gate
  (`patch_fraction > 0.197` → Always — the graduating head), the size-decay
  attribution (1024→512 decay is a high-quality-band loss; ss2-QM top suspect),
  and the per-image variance-boost rule (not deployable at n=24; global 1.0
  stands). Report: `docs/HYPERPARAM_FIRST_CUT_2026-07-03.md`.
- **Precise perceptual-quality targeting** (`target-quality` feature):
  `encode_rgb8_with_target` converges the encoder on a requested SSIMULACRA2
  or zensim score via an encode→decode→score bracketed secant search
  (typically 3–5 encodes at ±0.5 tolerance). New `TargetMetric` /
  `TargetOptions` / `TargetedEncode` types; selection policy returns the
  smallest file inside the target band and reports `converged` honestly
  when the target is unreachable. RGBA variant
  (`encode_rgba8_with_target`): zensim scores alpha natively; SSIMULACRA2
  composites both sides onto mid-gray. 16-bit variant
  (`encode_rgb16_with_target`, 10-bit AV1): ssim2 scored natively at
  16-bit, zensim on an identical 8-bit view of both sides. Contract tests
  in `tests/target_quality.rs`.
- **Honor `zencodec::AllocPreference` at zenavif's own decode allocations.**
  The full-image RGB(A) output buffer, the grid-stitch canvas, the crop
  destination, and the per-row YUV→RGB scratch now route through a 3-mode,
  per-site allocation helper (`src/alloc_util.rs`). Big buffers sized from the
  (untrusted) AV1 frame / grid dimensions default to the fallible
  `try_reserve` path (graceful `Error::ResourceLimit` instead of an allocator
  abort); the width-bounded per-row scratch defaults to the infallible `vec!`
  path. `ResourceLimits::prefer_fallible_allocations` (`Fallible` /
  `Infallible`) overrides every site; `CodecDefault` (the default) keeps each
  site's own default, so behavior is unchanged. Wired only at the `zencodec`
  `DecodeJob` boundary (`effective_config`) — the direct `zenavif::decode` API
  leaves it `CodecDefault`. The AV1 frame/tile buffers live in the
  `rav1d-safe` dependency and are out of scope (noted as a follow-up).
- **`DecoderConfig::estimate_decode_resources` (zencodec `estimate` API).**
  Mirrors `estimate_encode_resources`: delegates to the calibrated
  `heuristics::estimate_decode` (peak memory + time from the decoded
  bytes-per-pixel) and maps a new conservative `heuristics::decode_threading_info`
  onto `zencodec::estimate::ThreadingInformation` (AVIF decode is only partly
  parallel — tile decode parallelises, the YUV→RGB conversion does not).

### Changed
- **zencodec trait impls return the `At<CodecError>` envelope (Pattern B).** Every
  `zencodec` trait method (`EncoderConfig`/`EncodeJob`/`Encoder`/`AnimationFrameEncoder`
  + `DecoderConfig`/`DecodeJob`/`Decode`/`StreamingDecode`/`AnimationFrameDecoder`)
  now declares `type Error = whereat::At<zencodec::CodecError>` instead of
  `At<Error>`. A generic consumer driving zenavif through `Dyn*` dispatch — where
  the error is erased to `Box<dyn Error + Send + Sync>` — now recovers the coarse
  `ErrorCategory` **and** the `"zenavif"` codec name via `CodecErrorExt`
  (`error_category()` / `codec_error()`), which return `None` under the previous
  native-error type. The native `Error` enum (the detail + category source, with
  its `CategorizedError` impl including the `Parse` delegation) is **unchanged**,
  and zenavif's inherent rich public API still returns `At<Error>` for direct
  callers — only the zencodec trait boundary changed. Internally each trait method
  keeps its verbatim logic in a private `*_inner` helper returning `At<Error>` and
  re-wraps once at the boundary with `CodecError::of` (already-located errors,
  trace preserved) or the new `From<Error> for At<CodecError>` bridge (bare errors
  at reject / sink-wrap sites). Additive at the type level; visible only to
  generic `zencodec`-trait consumers.
- **`zencodec` is now a required (non-optional) dependency.** The optional
  `zencodec` cargo feature is removed; the codec-trait integration is always
  built (the `codec` module implementing `Encoder`/`Decoder` +
  `SourceEncodingDetails`, the `CategorizedError` impl, and
  `From<zencodec::AllocPreference>`). `ultrahdr-core` (gain-map `ReconstructHdr`
  math, previously pulled only by the `zencodec` feature) becomes a hard dep for
  the same reason. The `cov_zencodec` / `color_context_decode` / `mono_decode` /
  `gainmap_decode` integration tests are ungated (they run under default
  `cargo test`); CI drops the now-redundant `--features zencodec`
  test/check/clippy steps and provisions the downloaded AVIF vectors in the
  `i686` and `coverage` jobs, which now run those decode tests.
- **Migrate to zenavif-parse 0.7.0 `whereat::At<Error>` Result API.** zenavif-parse
  now returns `whereat::At<Error>` (location-tracing) from its parser entry points;
  zenavif's parse-error boundaries (`decoder_managed.rs`, `decoder.rs`,
  `detect.rs`) switch to the trace-preserving `ResultAtExt::map_err_at(Error::Parse)`
  / `At::map_error(Error::Parse)` (and `e.error()` for the probe-classification
  match) instead of dropping-and-rewrapping the inner error. Required to consume
  zenavif-parse's `CategorizedError`. Decode output is unchanged (error-path only).
- **Re-calibrate the ENCODE peak-memory model (was over-conservative).** A fresh
  VmHWM + heaptrack sweep (`examples/mem_probe_encode`, RGB8, threads=1, sizes
  256–2048 px × speed {6,8,10} × photo/screenshot × q {50,85}) showed the prior
  "2026-06-14" constants over-predicted typical peak by up to 2.0× at small sizes
  (the 8 MiB fixed term dominated a measured ~4.6 MiB intercept) and 1.35× at
  4 MP. Tightened `ENCODE_FIXED_OVERHEAD` 8 MiB → 5.5 MiB and `ENCODE_BPP`
  40 → 37 B/px in `src/heuristics.rs`. The TYP still clears the measured
  worst-case marginal + 10 % at every swept size (min margin 1.107× at 1 MP — it
  never under-predicts), and the MAX (1.8×) tier clears the heaptrack-requested
  heap by 1.6–1.9×; over-prediction is now 1.11–1.49×. Found that encode memory
  is driven mostly by **quality** (q85/q50 ≈ 1.28×) and **content** (photo/shot
  ≈ 1.26×), and only ≈ 1.09× by speed — so the model's speed-independence is
  conservatively fine (constants fit to the speed-10/q85/photo worst case). The
  alpha (+7 B/px) and 10-bit (×1.55) memory factors were not re-measured
  (RGB8-only sweep) and are carried forward unchanged. Provenance:
  `benchmarks/zenavif_encode_mem_2026-06-23.tsv`.
- **deps: migrate to published `zencodec 0.1.24` estimate API; drop git-rev
  patch.** Removed the temporary `[patch.crates-io] zencodec = { git, rev =
  "0f71295" }` now that `zencodec 0.1.24` is on crates.io. Migrated the
  `estimate_encode_resources` mapping in `src/codec.rs` for the refined
  `ResourceEstimate`: `new(peak, wall_ms: u64)` (was `f32`),
  `with_peak_max(max)` (the `min` arg is gone), dropped the removed
  `with_output_bytes`, and the encode `ThreadingInformation::parallel` is now
  1-arg (`parallel(max_useful_threads)`; the `fraction` / `mem_per_thread` args
  are gone). `cargo update -p zencodec` pulled published 0.1.24.
- **Preserve the encoder's whereat trace across the ravif boundary.** The
  `encode_*` paths now convert ravif (zenravif) errors with
  `.map_err_at(|e| Error::Encode(e.to_string())).at_crate(…)` instead of
  `at!(Error::Encode(e.to_string()))`. The old form started a *fresh* trace and
  discarded ravif's frames; `map_err_at` keeps the underlying `At<ravif::Error>`
  trace (the originating encode site) and `at_crate` annotates the zenavif
  boundary. Also bumps the `ravif`/`zenravif` dep to `0.2.0` (its `At<Error>`
  public-error API) — the conversion now takes the inner `ravif::Error` via
  `map_err_at`, which both preserves the trace and matches the new signature.
- **Decode is bounded by default (closes the decode fail-open part of #22).**
  `DecoderConfig::default()` now defaults `frame_size_limit` to `120_000_000`
  (120 MP, admits ~108 MP photos) instead of `0`. The pre-flight dimension
  check (`decoder_managed.rs` / `decoder.rs`) only fires when the limit is
  `> 0`, so `zenavif::decode(untrusted)` was previously unbounded by total
  pixels; it now rejects over-120-MP frames with `Error::ImageTooLarge`
  before any frame allocation. Non-breaking: `frame_size_limit(0)` still opts
  out (unbounded), and the parser already inherits zenavif-parse's own caps
  (512 MP / 1 GB peak). The frame-granularity cancellation sub-point of #22
  stays open pending a rav1d-safe Stop hook.

### Fixed
- **Preserve whereat traces across the decode strip-conversion / animation
  boundaries.** Three decode-path sites discarded the inner `At<Error>` trace
  with `.map_err(|e| e.decompose().0)?` (`codec.rs` streaming `next_batch` +
  `render_next_frame`, and `decoder_managed.rs` `decode_to_sink`). They now use
  `.at()?`, which keeps the originating trace (`StripConverter::convert_strip` /
  `AnimationDecoder::next_frame`) and adds a frame at the propagation site. New
  `tests/whereat_trace_preservation.rs` drives a corrupt and a truncated AVIF
  through `decode_with` and asserts the surfaced error carries a non-empty trace
  (`frame_count() >= 1`). The parser boundaries (`parser.frame` / `tile_data` /
  `primary_data`, etc.) keep `at!(Error::from(e))` — zenavif-parse `0.6.2`
  returns a *bare* `Error`, so `at!` correctly starts the trace there (matching
  the in-file `TODO(whereat)`); they switch to `map_err_at` only once
  zenavif-parse ships its `At<Error>` API.

### Added
- **vCPU-aware resource estimation via zencodec's unified `estimate` API.**
  `AvifEncoderConfig::estimate_encode_resources(&ImageCharacteristics,
  &ComputeEnvironment)` overrides the `zencodec::encode::EncoderConfig` default,
  delegating to the calibrated `heuristics::estimate_encode` (memory / time /
  output, keyed on the AV1 `speed` preset + input bytes-per-pixel) and folding
  in core count via `ResourceEstimate::at_cores`. The crate's local
  `heuristics::ThreadingInfo` is kept (decoupled from the optional `zencodec`
  dep so a decode-only build still compiles) and mapped onto the shared
  `zencodec::estimate::ThreadingInformation` only at the trait-impl boundary.
- **Sweep generator: trained-scalar-head + compute-budget surface**
  (`__expert`, VARIANT_GENERATION patterns 17–18). `SweepAxes::scalar_dense()`
  gives each CONTINUOUS knob a dense isolated ladder (speed `2..=10`,
  VAQ-strength, seg_boost) with every categorical axis pinned to its
  production default — the per-knob response curves a scalar regression head
  fits; pair with `with_max_deviations(1)`. `compute_tier(&EncoderConfig) -> u8`
  is the ordinal CPU-cost proxy, driven primarily by speed **inverted** (AV1
  `speed` is higher-is-faster, so tier = `10 − speed`; +2 trellis, +1 QM).
  `SweepBuilder::with_compute_limit(max_tier)` drops cells over budget (the
  fast/high-speed end survives), reported in the new
  `SweepPlan::compute_tier_skipped` field — no silent caps.
  `SweepBuilder::with_max_deviations(max)` scopes to main-effects-only.
  `scalar_dense` is resolvable via `SweepAxes::by_name`. All additive.
- `examples/heaptrack_decode.rs`: a reusable heaptrack/valgrind harness that
  decodes an AVIF file from bytes via `zenavif::decode(..)` in a loop, for
  profiling heap-allocation behaviour. Defaults to the committed
  `tests/vectors/libavif/kodim03_yuv420_8bpc.avif` (768×512, 4:2:0 8-bit) decoded
  8×; a path + iteration count can be passed. Driven by `just heaptrack-decode`.
  Profiled result: heap **size** and leaks are healthy — peak 2.41 MiB (O(image)),
  leaked pinned at ~28 across 2/8/16 iterations (one-time thread-pool statics, no
  per-decode growth). Allocation **count** is a pathology — ~37,000/decode, of
  which 99% are transient (0 B net) and concentrated in the **rav1d-safe** backend:
  `compact_read_per_row` (325,917 total across 9 decodes) stages a strided plane
  region into a fresh contiguous heap buffer per CDEF/loopfilter/intra-pred block.
  The churn originates in the rav1d-safe dependency's safe-SIMD filter path, not
  in zenavif's own container/YUV code. Tracked as a resource follow-up.
- **Calibrated resource-estimation module (`heuristics`).** New
  `zenavif::heuristics` with `EncodeEstimate` (min/typical/max peak memory +
  `time_ms` + `output_bytes`), `DecodeEstimate` (peak memory + time +
  output), and `estimate_encode(w,h,input_bpp,speed)` /
  `estimate_decode(w,h,output_bpp)` — mirrors the zen per-codec pattern
  (`zenwebp::heuristics`). Calibrated from real measurement, not guesses: a
  new `examples/avif_probe` measures the marginal working set (`VmHWM`
  delta) plus wall and user/sys CPU (`/proc/self/stat`, threads=1, one
  process per op), swept by `scripts/avif_resource_calibrate.py` over
  5 content classes × 256–2048 px × speed 4/6/8/10 × rgb/rgba × 8/10-bit
  (`benchmarks/avif_resource_{main,alphadepth}_2026-06-14.tsv`). Model
  captures that AVIF encode time is dominated by the AV1 `speed` preset
  (~7.6/2.0/1.2/0.55 us/px at speed 4/6/8/10, single-thread, ~14× spread),
  that AVIF is light on memory (encode ~40 B/px, decode ~18 B/px), and the
  alpha (+7 B/px, +30 % time) and 10-bit (×1.55 mem, +40 % time) deltas.
  Times are single-thread CPU; divide by thread count for wall latency.
  (7d00689)

### Added (sweep planner — dense-sweep program)
- **SCALAR ladder densification** for `zenpicker-train --scalar-axes` heads
  (zenmetrics `docs/PLAN_SWEEPS.md` §5 gaps): seg_boost probes
  {0.75, 1.5, 2.5, 4.0} (de-boost direction + validate endpoint) and
  vaq_strength probes {0.25, 2.0, 3.0} (+ the 0.5 vaq-axis stratum). New
  values ride the existing `-sb<f>`/`-vaqs<f>` id grammar and resolved-state
  fingerprint. Direct quantizer (qp) axis documented as **blocked on encoder
  knob** (no public quantizer setter in zenavif/zenravif; the resolved
  quantizer is already the picker mediator via `feature_row`).
- **Still-envelope equivalence finding (proven by encode, 28/28 × 3 pairs)**:
  `vaq_strength(x)` ≡ `seg_boost(x)` byte-identically on still encodes (both
  are the same log-domain exponent on the `spatiotemporal_scores` →
  `segmentation_scores` chain; zenrav1e `internal.rs:1379`). Curated ladders
  interleave values so the joint 8-point effective ladder is alias-free,
  with a test guarding disjointness; the fingerprint deliberately
  under-merges the spellings (animated/inter encodes may diverge). See
  `docs/VARIANT_GENERATION.md`.

### Added (SIMD platform parity — issue #2)
- **NEON bilinear AVG** (`src/simd/avg.rs`): 16 pixels/iteration via
  `vqrdmulhq_s16` (bit-exact pmulhrsw for the 1024 multiplier — the
  saturating corner is unreachable), wired into the runtime dispatch.
  Parity vs scalar verified under qemu (`cross test
  --target aarch64-unknown-linux-gnu`), including non-multiple-of-16
  tails and a loud failure if the NEON token can't summon. Closes the
  one production-relevant gap from the cross-platform audit: the
  remaining table rows (`yuv_convert_fast`, `yuv_convert_libyuv_simd`)
  are `_dev`/benchmark-only modules with no production call sites — the
  production YUV strip/full converters in `yuv_convert.rs` already
  cover wasm32 via `#[magetypes]` (v3/neon/wasm128).

### Investigated + fixed upstream (lossless speed inversion — issue #8)
- Root cause found and fixed in zenrav1e (c3567081, closes zenrav1e#9):
  "lossless" never reached qindex 0 — the rate path floored it at 1, so
  every lossless encode was qi=1 lossy with ±2 error on 7-28% of pixels,
  and the speed "inversion" was RDO rationally spending bits against
  that phantom distortion (slow speeds were 8-13% wrong pixels vs 17-27%
  at fast speeds — size inversely tracked exactness;
  `benchmarks/lossless_speed_sweep_2026-06-11.tsv`). With the fix,
  roundtrips are bit-exact (0 mismatched pixels) on every source at
  every speed and bytes are monotonically non-increasing with effort
  (`benchmarks/lossless_speed_sweep_fixed_2026-06-11.tsv`).
  `examples/lossless_speed_sweep.rs` is the permanent harness. The fix
  reaches zenavif when zenrav1e 0.1.5 releases and zenravif bumps;
  `tests/identity_roundtrip.rs` tolerances then tighten to exact.

### Changed (gray + RGB-ICC refinement)
- **Derivable RGB-class profiles no longer block native gray.** Gray
  files carrying RGB ICC profiles are common in the wild; when the
  profile's CICP is derivable (embedded `cICP` tag or normalized-hash
  identification — the same chain the load-bearing reduction uses), the
  decoder emits native Gray8/Gray16 with a **CICP-only context** (white
  point + transfer remain fully meaningful for single-channel data, per
  moxcms guidance) instead of expanding to RGB just to honor the
  profile. Only underivable RGB-class profiles keep the RGB layout. The
  class-gate fallback now also prefers the profile-DERIVED CICP over the
  signaled nclx (the profile outranked it per MIAF). New fixture:
  `mono_gradient_8b_p3icc.avif` (real Display P3 profile → gray + CICP
  (12, 13)).

### Changed (devil's-advocate follow-ups on self-describing buffers)
- **Streaming strips now carry the context** — the "known gap" was a
  plain drop bug: every emission path copies rows into a per-batch
  scratch buffer and returned slices of that. The class-gated context is
  now stored on the streaming decoder and re-attached to every emitted
  strip (baked/grid/converter paths); the converter path uses the
  frame-era info the converter already returned (previously discarded),
  fixing a probe-vs-frame CICP divergence the new test exposed.
- **Mono files carrying an RGB-class ICC no longer decode native gray**
  — the profile is the most accurate color description present, so the
  decoder keeps the RGB layout it validly describes instead of stripping
  it (a gray preference then resolves through the load-bearing ICC
  rules: swap when derivable, honest suppression otherwise). GRAY-class
  ICC mono files decode native gray with the profile riding along. New
  ICC-tagged mono fixtures cover both.
- **HDR reconstruction output now carries a synthesized linear CICP**
  (source primaries raw, H.273 transfer 8, identity matrix, full range)
  instead of being context-free — no SDR signaling carries over, but the
  linear output is describable and the raw primaries code point survives
  where the descriptor enums fold.

### Added (self-describing decoded buffers)
- **Decoded buffers now carry a `zenpixels::ColorContext`** — the
  authoritative source color, selected through zencodec's drop-dupe
  rules (`SourceColor::to_color_context`: ICC > nclx per MIAF, the
  non-authoritative duplicate dropped) and **class-gated** so an
  RGB-class ICC never rides a Gray-layout buffer (the raw H.273 CICP —
  code points the descriptor enums fold away — stays as the fallback).
  Attached on the buffered decode, streaming-bake, streaming-reconstruct
  (SDR base), and animation paths; conversions, orientation bake, and
  the load-bearing gray reduction all propagate it, and
  `downscale_to_8bit` now carries it across its rebuilds. HDR
  reconstruction output is deliberately context-free (no SDR profile
  describes linear f32; the descriptor is the honest carrier). Known
  gap, test-pinned: the strip-converter streaming path does not yet
  attach contexts to strips.

### Added (load-bearing ICC contract tests)
- `codec::tests::negotiate_gray` pins the gray-collapse ICC rules in
  zenavif's exact feature configuration (zenpixels-convert without
  `icc-db`): no-context collapse with exact channel equality; sRGB ICC →
  collapse with the RGB-class profile dropped to CICP-only (an RGB
  profile must never ride on a Gray buffer); underivable ICC →
  suppression that falls through to the next preference; and lying
  metadata (mono-flagged but colorful pixels) → byte-verified refusal.
  Upstream 0.2.13 audit: plan logic + all five signal outcomes tested,
  174/174 bundled gray profiles GRAY-class and byte-identical to fresh
  moxcms synthesis (zero-tolerance gate verified green locally — it is
  `cms-moxcms`-gated and not exercised by upstream CI; reported).

### Changed (load-bearing API integration)
- The mono-source gray negotiation arm now uses zenpixels-convert's
  `reduce_to_load_bearing_format_in_place` instead of `to_gray8()`: the
  R==G==B collapse is byte-verified (not metadata-trusted), rewrites in
  place with no allocation, and inherits the load-bearing module's color
  signaling rules (an RGB-class ICC profile can't describe a Gray layout
  — a gray-class variant is swapped in when derivable, otherwise the
  collapse is suppressed and negotiation falls through honestly).
- `ReconstructHdr` output now tags alpha `AlphaMode::Opaque` (the apply
  kernels emit constant 1.0 structurally) so downstream encoders know
  the lane isn't load-bearing without rescanning.

### Added (native grayscale decode — issue #5)
- **Monochrome AVIFs decode to native `Gray8`/`Gray16`** (1-2 bytes/pixel
  instead of the 3-8x RGB expansion) through the zencodec adapter:
  `native_gray` capability is now `true`, `GRAY8_SRGB`/`GRAY16_SRGB` join
  the supported descriptors, and an alpha-free mono source decodes gray
  when the caller passes no preference (gray IS the native format) or
  ranks a Gray descriptor first. Range expansion runs through the same
  `yuv` kernel as the RGB path — gray output equals the R channel of an
  RGB decode bit-for-bit (test-pinned, both ranges, 8/10-bit). Streaming
  emits gray strips identical to the buffered decode. Gray preferences on
  color images are never satisfied by luma synthesis; grid composition,
  animations, and the non-zencodec `decode_with` API keep their RGB
  behavior. Genuine Cs400 fixtures live at `tests/vectors/zenavif/`
  (generated by zenavif-serialize's `make_mono_avif` example).

### Added (HDR reconstruction — issue #17)
- **`GainMapRender::ReconstructHdr` is now honored** (`reconstructs_hdr()`
  capability is `true`): the decoder applies the ISO 21496-1 gain map to
  the SDR base via `ultrahdr-core`, producing linear f32 RGBA (1.0 = SDR
  white / 203 nits, base-image primaries). `target_headroom: Some(h)`
  renders for an h×-SDR-white display; `None` reconstructs at the map's
  encoded maximum. MaxCLL/MaxFALL are **measured** from the reconstructed
  pixels per the zencodec envelope obligation, and the gain-map components
  are still surfaced for transcode use. The streaming decoder reconstructs
  whole-image and emits fixed-height strips (same shape as the
  orientation-bake path); buffered and streaming outputs are bit-identical.
  Alpha-carrying and >8-bit bases are refused loudly (use `Components` and
  apply downstream); files without a gain map decode as honest SDR.

### Fixed (corpus integrity — issue #16)
- The libavif corpus test now asserts the two `unsupported_gainmap_*`
  fixtures are **rejected with their version-gate errors** — they are
  libavif's tmap version probes, designed to be refused; decoding one would
  be the bug. The other 10 corpus failures (all Apple-style HDR gain-map
  vectors with a size=0 `mdat`) were a zenavif-parse regression — fixed
  upstream in zenavif-parse f3c9f043 (45/57 → 57/57-equivalent validated via
  path-patch); the corpus goes fully green here when zenavif-parse 0.6.3 is
  released and the dependency bumps.

### Fixed (pixel corruption — issues #14 + #15, landed in lockstep)
- **Identity (MC=0) decode**: identity-RGB AVIFs were decoded through BT.601
  matrix math (`_ => Bt601` blind arms) — every pixel wrong (pure red came
  back R=111 G=0 B=0). Identity now takes a no-matrix GBR passthrough
  (8-bit + 10/12-bit, full + limited range; subsampled identity is rejected
  per H.273). The strip fast-path routes identity to the full-conversion
  path.
- **16-bit encode plane order**: `encode_rgb16`/`encode_rgba16` fed
  `[r,g,b]` plane tuples under identity signaling where the AV1 convention
  (and zenravif's own 8-bit path) is **G,B,R** — every 16-bit encode was
  channel-rotated for conforming decoders. Both bugs masked each other in
  self-roundtrips; `tests/identity_roundtrip.rs` pins them together
  (tolerance ≤2 documents zenrav1e#9: `with_lossless` is not bit-exact).
- **H.273 matrix resolution** (`src/cicp_resolve.rs`, the consumer-side of
  zenpixels#36's `Cicp::resolve_matrix` spec — migrates there when it
  lands): unspecified/reserved MC resolves through the container `nclx`
  matrix, else the documented AVIF-spec default (1/13/6) — never a silent
  guess; MC=12 derives coefficients from the colour primaries (CP=9 → the
  canonical BT.2020-NCL; P3 and friends decode **exactly** via the yuv
  crate's custom-KR/KB path — `cosmos1650_yuv444_10bpc_p3pq` previously
  decoded silently wrong through BT.601); MC=4 (FCC) now uses exact FCC
  coefficients instead of a 601 approximation; genuinely unimplemented math
  (YCgCo, BT.2020-CL, Y'D'zD'x, ICtCp, MC=13) errors loudly with the code
  point named. 8-bit RGB conversions for matrices outside the in-house SIMD
  tables (240M/FCC/derived) route through the yuv crate.

### Deprecated
- `Av1Backend::Svtav1`. The `encode-svtav1` feature was never shipped
  (svtav1-rs produces non-conformant bitstreams in most configurations, and
  the draft path returned raw AV1 OBUs in the `avif_file` field instead of an
  AVIF container), so no build could ever encode with it; `validate()` rejects
  it unconditionally. The variant stays for enum compatibility — a working
  svtav1 integration would land as a new variant.

### Removed
- The unreachable `encode-svtav1`-gated code: the `encode_rgb8_svtav1` draft
  path, the backend dispatch in `encode_rgb8`, three differential test files
  (`differential_svtav1.rs`, `differential_comprehensive.rs`,
  `differential_4k.rs` — all cfg'd on the never-shipped feature and
  referencing the commented-out path dep), the commented `svtav1` dependency
  and feature lines, and the `check-cfg` allowance. None of this compiled in
  any build; git history before this commit has the experiment if it resumes.
  Consequence: the `matrix_coefficients` CICP field is now consumed by no
  backend (zenravif derives the signaled matrix from `color_model`) — it is
  retained, documented as informational, and excluded from the sweep
  fingerprint on all backends.

### Added (pattern 7: cell ids as durable identity)
- `sweep::config_from_cell_id(base_id, q)` — reconstructs the exact
  `EncoderConfig` from a sweep cell id (the ledger contract: ids stored in
  TSV/parquet identity columns are regenerable years later). Parses through
  the same stratum builder the planner uses; grammar documented on the
  parser, additive-only, lossless numbers. `SweepAxes::by_name` resolves the
  named plans for executor wiring. The grammar-totality test
  (`cell_ids_roundtrip_to_their_configs`, fingerprint-exact over canonical +
  alias spellings of the full `modes_full_alpha × Step5` plan) caught a real
  tokenizer bug on its first run (`part4.16`'s separator dot eaten by the
  float scanner). The zenmetrics executor wiring (checklist step 8)
  landed the same day (zenmetrics 96a31b90): both execution models,
  e2e declare→jobexec→AVIF-bytes roundtrip + tampered-fp tripwire.

### Added (expert-knob parity + MLP training bridge)
- `sweep::feature_columns()` + `SweepCell::feature_row(PlanInput)` — numeric
  knob vectors for picker/MLP training (zentrain): one column per knob,
  resolved values where a mediator exists (quantizer, post-override search
  settings), bool 0/1 / small-int enum / −1-sentinel encodings, append-only
  columns.
- Alpha-plane sweep probes: `KnobProbe::AlphaQualityDelta` (a **delta against
  the grid q**, ±25 curated — deltas dodge the absolute-value-vs-moving-grid
  trap zenjpeg documents for `chroma_quality`) and `KnobProbe::AlphaMode`
  (Dirty / Premultiplied), in the new `SweepAxes::modes_full_alpha()` /
  `SweepAxes::alpha_probes()` presets for RGBA corpora. Kept out of
  `modes_full` (byte-inert without an alpha plane); the validation harness
  gained an RGBA leg proving each probe live on alpha content and
  non-coupling on color-only encodes (`benchmarks/sweep_validate_2026-06-11.tsv`).
- `EncodePlan.alpha_color_mode` — the alpha handling mode is pixel-changing
  resolved state and is now reported by the plan. `KnobProbe::apply` is
  public so harnesses can exercise single probes outside a full plan.

### Fixed
- **zencodec adapter now honors `OrientationHint` (`irot`/`imir`).** The
  `AvifDecodeJob` decode paths ignored the orientation hint, so `Correct` was
  silently treated as `Preserve` — pixels stayed in stored orientation. The
  adapter now bakes the container's intrinsic rotation/mirror into the decoded
  pixels on the bake path (`Correct`/`CorrectAndTransform`) via
  `zenpixels_convert::orient::apply_orientation`, reporting display dims +
  `Orientation::Identity`; `Preserve` (the default) is unchanged (stored dims +
  intrinsic tag). Covers the single-image, row-sink, streaming (grid + non-grid,
  baked after tile stitching), and animation paths. Adds
  `AvifDecoderConfig::with_orientation` + the `DecodeJob::with_orientation`
  override; the native (non-zencodec) API is unchanged. Requires
  zenpixels-convert 0.2.13 (the `orient` module). Pinned by
  `tests/cov_zencodec.rs::orientation_*`.
- **`alpha_quality` unset now follows the color quality, as its docs always
  promised.** zenravif's built-in default pins the alpha quantizer to the
  quality-80 equivalent and `with_quality` never touches it, so an
  alpha-bearing encode at `quality(30)` was silently encoding alpha at q80;
  zenavif now forwards `alpha_quality.unwrap_or(quality)` explicitly. Output
  bytes change for alpha-bearing encodes whose color quality ≠ 80. Pinned both
  directions by `tests/encode_contracts.rs::alpha_quality_unset_follows_color_quality`.

### Added
- **Variant-generation infrastructure** (the zenjpeg patterns, adopted per
  `docs/VARIANT_GENERATION.md`):
  - `EncoderConfig::resolve_plan(PlanInput) -> EncodePlan` — full static
    resolution of what an encode will do: quantizers (color + alpha), the
    qm×lossless gate, every speed-preset-derived search setting after
    overrides, chroma subsampling, resolved bit depth / color model / CICP
    matrix, and the tile count — including `TilesResolution::MachineDependent`
    when `threads` is unset, because zenravif substitutes the *host core
    count* into the tile formula and tile structure changes the bitstream
    (default-config encodes are not byte-reproducible across machines).
  - `EncoderConfig::validate()` now rejects `Av1Backend::Svtav1` when the
    backend isn't compiled in (previously a silent fallback to zenravif) and
    `Yuv420 × Rgb`; new `validate_for_input()` additionally rejects
    `Yuv420 × 16-bit input` (the 16-bit entry points force identity-RGB).
  - `EncoderConfig::chroma_subsampling(EncodeChromaSubsampling)` — 4:4:4
    (default, unchanged) vs 4:2:0. The biggest AVIF rate knob was previously
    hardwired to 4:4:4.
  - `zenavif::sweep` (behind `__expert`): `SweepAxes` (`rd_core` /
    `modes_full` + the `KnobProbe` single-deviation axis), `QualityGrid`,
    `SweepBuilder` with a no-silent-caps budget ladder, validity filtering,
    main-effects-first queue ordering, and a byte-identity `fingerprint`
    over RESOLVED state (quality mediated by quantizer; override==preset
    aliases merged; `vaq_strength` excluded when VAQ is off;
    `matrix_coefficients` excluded on the zenravif backend — every exclusion
    encode-proven). Sweep cells pin `threads(Some(1))` for cross-machine
    reproducibility.
  - `examples/sweep_validate.rs` — empirical axis validation (inert-step
    detection, fingerprint contracts on real encodes, tiles/threads claims,
    ssim2 sanity floor); results committed as
    `benchmarks/sweep_validate_2026-06-10.tsv`. Its first runs caught two
    real defects, both root-caused structurally and fixed: (1)
    `with_vaq(true, 1.0)` is byte-identical to VAQ off — the
    psychovisual/still tunes always compute the activity mask and zenrav1e
    skips the strength rescale at 1.0 (`api/internal.rs:1379`); the sweep
    vaq axis is `Option<f64>` now and the fingerprint hashes the active
    form. (2) `lru_on_skip` is byte-inert on still-image encodes at speeds
    2–8 (28/28 comparisons incl. skip-heavy flat content) — removed from
    the curated probe set with the evidence in the provenance table.
  - `tests/encode_contracts.rs` — encode-level pins for the alpha contract,
    quantizer mediation (q 80.0 ≡ q 80.2 ≠ q 81.0 at the byte level),
    subsampling liveness, and the new validate() rejections.
- Versioned public-API surface snapshot at `docs/public-api/zenavif.txt`,
  regenerated on every `cargo test` by `tests/public_api_doc.rs`
  (`ZEN_API_DOC=check` verifies in CI's clippy job, `=off` skips); justfile
  recipes `api-doc` / `api-doc-check`.
- `zencodec::GainMapRender` wired through the decode trait path:
  `BaseOnly` (default) ignores the gain map entirely — no extras attached
  (previously `GainMapSource` attachment was gated only on the legacy
  `with_extract_gain_map` flag, which still works); `Components` decodes the
  gain-map AV1 payload and surfaces BOTH `zencodec::decode::DecodedGainMap`
  (pixels + ISO 21496-1 params) and the raw `GainMapSource` (for transcode);
  `ReconstructHdr` downgrades to Components per the zencodec contract —
  zenavif surfaces, it does not apply (`reconstructs_hdr()` stays false), so
  the base is never silently SDR-labeled-HDR. Unknown future modes error.
  Tests in `tests/gainmap_decode.rs` (the stale always-on extras test was
  re-pointed at the opt-in Components contract; vectors via
  `just download-vectors`).
- zencodec 0.1.21 color-emit integration: `resolve_avif_color` drives the still and animation encode paths through `resolve_color_emit` (single source of truth); resolved CICP lowers to nclx on all three axes; AVIF declares CICP sole-safe (nclx is reader-authoritative per MIAF/HEIF), so a redundant ICC is dropped like JXL. Deps bumped to published zencodec 0.1.21 / zenpixels 0.2.11 / zenpixels-convert 0.2.12. Also aligns the zenanalyze/zenpredict path-dep reqs to the drifted 0.2.0 siblings, fixing the nightly Fuzz job's resolution failure (b3be82a6).

### Changed
- YUV420→RGB8 bilinear chroma upsampling now uses a direct truncating cast
  instead of `f32::floor` (libm `floorf`) for the per-pixel chroma index.
  Inputs are pre-clamped to `[0, dim-1]`, so the output is byte-identical —
  verified on x86 (AVX2, full lib suite) and aarch64 (NEON) via a byte-identity
  test against an independent libm-floor reference (6 sizes × 2 ranges × 3
  matrices). This is a correctness-preserving refactor that drops a libm call
  from the chroma-gather inner loop; a wall-time win was NOT demonstrated on ARM
  (the yuv_conversion_benchmark gates SIMD on an x86-only token, so on aarch64 it
  measures the scalar fallback, not the NEON path this change affects — measured
  no change there). See `benchmarks/zenavif_arm_yuv_floor_2026-05-30.{tsv,meta}`.
- `tests/fuzz_regression.rs` now uses the shared `zen-fuzz-regress`
  test-helper crate (DEDUP-J2). Behaviour is unchanged — same
  `fuzz/regression/` seeds, same four targets (`decode`,
  `decode_limited`, `decode_animation`, `probe`), same
  panic-propagation failure semantics. The ~60-line in-file
  scaffolding (`collect_seeds` walk + skip dotfiles + per-seed read +
  dispatch) is now provided by `RegressionSuite`.

### Added
- `tests/fuzz_regression.rs` regression-harness template ported from
  zenwebp (DEDUP-J). Walks `fuzz/regression/` (incl. per-target
  subdirs) and runs every seed through `decode_with`,
  `decode_animation_with`, `AnimationDecoder`, and
  `ManagedAvifDecoder::probe_info` on the stable toolchain — no
  nightly required. Drop minimized crash files into `fuzz/regression/`
  to gate future regressions of fixed bugs.

## [0.1.7] - 2026-05-02

### Changed
- Bump minimum `zenravif` dependency to 0.1.3 (published with the
  `__expert` + `InternalParams` surface). The local `[patch.crates-io]`
  override is no longer needed and has been removed.

### Added
- New `__expert` cargo feature exposing `expert::InternalParams`, an
  `Option<T>` struct of speed-preset overrides for the 4 deepest
  content-dependent knobs (`partition_range`, `complex_prediction_modes`,
  `lrf`, `fast_deblock`). Apply via
  `EncoderConfig::with_internal_params(InternalParams)`.
  `#[non_exhaustive]` + `Default` so callers tolerate field additions
  in any patch. Each `None` keeps the speed preset's default; each
  `Some(_)` overrides. Implies `encode-imazen` and pulls
  `ravif/__expert` for the underlying overrides. Used by the rav1e
  knob predictor MLP training harness; not for production code.
- Theory-of-operation docs on every `InternalParams` field covering
  the AV1 pipeline stage, why a caller might override it, the
  underlying mechanism, and the speed-preset interaction. Cross-
  references `zenravif::expert::InternalParams` for source-line
  citations into zenrav1e.
- `tests/expert_internal_params.rs` — 12 permutation, idempotency,
  combined-knob, default-as-baseline, reset, and forwarding-parity
  tests for `InternalParams`. Verifies that zenavif's wrapper produces
  byte-identical output to calling `zenravif::Encoder::with_internal_params`
  directly with the same values. Gated on `__expert`.

### Changed
- The 4 individual setters (`with_partition_range`,
  `with_complex_prediction_modes`, `with_lrf`, `with_fast_deblock`)
  added in Phase 0.5 are removed from the public API in favour of
  `with_internal_params(InternalParams)`. Never published, so no
  deprecation cycle. The other 11 `encode-imazen`-gated setters
  (`with_qm`, `with_vaq`, `with_seg_boost`, `with_cdef`, etc.) stay
  as-is — they're already in 0.1.6 published.
- `predictor_sweep` and `phase2_oat` examples now require the
  `__expert` feature (was `encode-imazen`); `phase2_oat`'s deep-knob
  perturbations rebuild on top of `ravif::expert::InternalParams`
  rather than the removed individual setters.
- `partition_range` doc and tests now reflect that zenrav1e
  debug-asserts `max <= 64×64`; `128` is invalid (would panic in
  debug builds). Valid values: `{4, 8, 16, 32, 64}`.

### Build
- `[patch.crates-io] zenravif` temporarily points at
  `../ravif--expert/ravif` while ravif's `feat/expert-internal-params`
  branch is unmerged. Revert to `../ravif/ravif` once that branch
  lands; remove the patch entirely once zenravif 0.1.3 publishes.

### Changed
- Bumped baked picker artifacts to `v0_1_1`: re-baked from a 4× larger
  Phase 1a corpus (448 images / 89,601 sweep rows vs. 116 / 23,200 in
  v0_1) with a wider 192³ MLP. Held-out student mean overhead drops
  from 4.07 % → 3.88 % on the same val split. ZNPR + per-(speed,
  size_class) encode_ms LUT + per-(cell, target_zq) quality LUT all
  rev v0_1_1; auto_tune.rs `include_bytes!` paths updated. Old v0_1
  artifacts dropped (git history preserves them).
- Dropped `composites` feature from zenanalyze path-dep — feature was
  removed upstream in zenanalyze@b1623ba.

### Added
- 11 new `EncoderConfig` builder methods exposing internal speed-preset
  overrides for content-aware encoding, all gated on `encode-imazen`:
  `with_cdef`, `with_rdo_tx_decision`, `with_sgr_full`,
  `with_lru_on_skip`, `with_segmentation_complex`, `with_encode_bottomup`,
  `with_seg_boost`, `with_trellis`, plus 4 deeper knobs newly plumbed
  through zenravif 0.1.3 — `with_partition_range`,
  `with_complex_prediction_modes`, `with_lrf`, `with_fast_deblock`. Each
  takes `Option<...>` where `None` keeps the speed-preset default.
- New `auto-tune` cargo feature + `EncoderConfig::auto_tune(...)` API
  that predicts optimal `(speed, quality)` knobs for a given image and
  target zensim score via a baked MLP picker. Supports time-budget
  constraints via `AutoTuneOptions::with_time_budget(Duration)` and
  Pareto blending between bytes and encode_ms via
  `with_pareto_weight(α ∈ [0, 1])`. The model file ships baked into
  the binary via `include_bytes!`; until the first bake lands the
  runtime returns `AutoTuneError::ModelNotBaked`. See
  `docs/RAV1E_PICKER_PLAN.md` for the training pipeline.
- `examples/predictor_sweep.rs` — multi-image, multi-size, multi-knob
  sweep harness for picker training; resumable via `--append`.
- `examples/extract_features.rs` — zenanalyze ~103-feature extractor
  for the same corpus (matches `zentrain/tools/train_hybrid.py`'s
  expected schema).
- `examples/auto_tune_smoke.rs` — end-to-end smoke test for the
  prediction path.
- `scripts/install_predictor_cron.sh` — installs nightly local cron
  jobs for incremental sweep growth + weekly retraining.
- `scripts/train_bake_pipeline.sh` — orchestrates train → bake →
  per-(speed, size) encode_ms LUT → per-(cell, target_zq) quality LUT
  in a single invocation.

## [0.1.6] - 2026-04-27

### Fixed
- QM (quantization matrix) quality across the encoding range is now monotonic
  and tracks QM-off within ±0.4 zensim from q=70 onward. Two bugs in zenrav1e
  fixed upstream and pulled in via the bumped minimum zenravif dep:
  (1) `qm_level_for_qindex` now uses libavif's still-image range `[4, 15]`
  instead of all-intra-video `[4, 10]`, so near-lossless qindex bypasses QM
  entirely instead of applying weights that multiplied the effective quantizer
  2-3× on high-frequency coefficients; (2) `using_qmatrix` is now cleared when
  the frame is coded-lossless or all selected QM levels are 15, fixing AV1
  spec 6.8.11 conformance and rav1d primary-frame decode at quality=100.
  Previously zensim collapsed from ~76 at q=95 (QM=on) to ~49 at q=100, and
  the whole q≥60 range was 11–22 zensim points worse with QM on. See
  imazen/zenrav1e#7. (0e7cefc, zenrav1e@30d37fc)

### Changed
- Bump minimum `zenravif` dependency to 0.1.2 (which requires zenrav1e 0.1.4)
  to pull in the QM and lossless-conformance fixes. The temporary q≥96 QM
  guard from 0e7cefc is removed — the fix lives upstream now.

## [0.1.5] - 2026-04-17

### Added
- Encoder `Auto` bit depth now matches input type: 8-bit input produces 8-bit
  AV1 and 16-bit input produces 10-bit AV1. Previously `Auto` always selected
  10-bit, which surprised callers decoding 8-bit sources back out as `Rgb16`
  (9bf934c).
- `DecoderConfig::prefer_8bit(bool)` (default `false`) for callers who want to
  downscale 10/12-bit AV1 to 8-bit RGB when decoding files produced by other
  encoders that default to higher bit depths (9bf934c).
- `RGBX8_SRGB` and `BGRX8_SRGB` pixel descriptors are now accepted by the
  encode dispatch. The padding byte is stripped before encoding and BGRX
  additionally swaps B/R channels; output is byte-identical to encoding the
  equivalent packed RGB8 (d9863f1).
- Encoder configuration guide in `README.md` covering speed/quality tradeoffs,
  quality parameter mapping, QM behaviour, and bit depth selection, with all
  numbers sourced from the committed sweep data on CID22-512 (a483b4a).
- Committed fine-grained encode sweep (q5-q100 step 5, speeds 1/2/4/6, QM
  on/off) under `benchmarks/` and a broader combinatorial sweep covering
  100 configurations, both measured with zensim-regress (53fff28, ef5cb08).
- `.workongoing` added to `.gitignore` for the main-with-lockfile agent
  workflow (d9863f1).

### Changed
- Default encoder profile for `[profile.test]` is now `opt-level = 2` so
  tests exercise optimised codec paths without requiring `--release`
  (9bf934c).
- Bumped `fast-ssim2` to `0.8.0`; `0.7.2` and `0.7.3` were yanked upstream
  after an accidental semver break from `yuvxyb` re-exports, which `0.8.0`
  removes (ef0500d).
- Minor documentation alignment between the zennode feature table in
  `README.md` and `Cargo.toml` (4b3a1df).
- Bump zencodec to 0.1.19

### Fixed
- `EncoderConfig::with_lossless()` and `with_lossless_mode()` (from the
  zencodec trait) now propagate `lossless = true` into the inner encoder
  config, so rav1e's lossless mode is actually engaged when requested via
  the trait API (ef5cb08).
- Quantization matrices (QM) are now auto-disabled when lossless encoding
  is selected. QM is still enabled by default for lossy quality levels
  (q5-q95), where it saves 9-13% on file size at speeds 4 and above with
  negligible quality impact, but combining QM with a quantizer of zero
  produced corrupt output (ef5cb08).
- `ColorAuthority::Cicp` is now set when the decoded image carries no ICC
  profile, reflecting the AVIF/MIAF precedence of `ICC > nclx > AV1 SPS
  CICP`. When no `colr` box is present, CICP (populated from nclx or SPS
  fallback) is the authoritative colour description (7d6b4e6, #3).
- `examples/encode_sweep.rs` — committed harness that regenerates the
  per-image TSVs under `benchmarks/`. CLI accepts `--image`, `--speeds`,
  `--qualities` (list or `START..=END:STEP`), `--qm {off,on,both}`, and
  `--force-bottomup {auto,off,on,both}` — the last is what reproduces
  zenrav1e#6's scenario now that ravif/40ddb66 disables bottom-up by
  default. `just sweep -- <flags>` is the shortcut. Gated on
  `encode-imazen,encode-threading`.

## [0.1.4] - 2026-04-05

### Added
- Generic SIMD YUV420-to-RGB8 path with autoversion dispatch covering NEON
  and WebAssembly in addition to x86, and 4:2:2 / 4:4:4 variants, built on
  the magetypes generic SIMD abstraction (0089d18, 0b7b333).
- Fuzz dictionary for AVIF and a nightly fuzz workflow that runs a short
  smoke fuzz on every push and a longer nightly run (b367344, ee107a7).
- Regression seed capturing the CDEF tile race fixed in rav1d-safe, so the
  race can never silently regress through the AVIF decode path (6d18489).

### Changed
- Replaced the platform-specific YUV-to-RGB SIMD implementations with a
  single magetypes-based generic dispatch. The x86-specific paths were
  collapsed into the generic implementation without measurable regression
  (0b7b333).
- Bumped `zencodec` to `0.1.13` (cfc1f7b).
- Committed `fuzz/Cargo.lock` for reproducible fuzz builds; `profraw` files
  and other tooling noise are now gitignored and excluded from published
  packages (193cbd0, ec244d8, 91912b7).

### Fixed
- Reverted the `max_frame_delay = 1` workaround added for a rav1d-safe CDEF
  threading race once the underlying race was fixed in rav1d-safe itself.
  The workaround served its purpose while the upstream fix was being
  developed (e089793, 1d1f838).

## [0.1.3] - Earlier

### Fixed
- Gated the `StopExt` import behind the `encode` feature so builds with
  `default-features = false` remain clean (812b817).

### Changed
- Bumped `zenavif-parse` to `0.6.0` and switched to the published `From`
  impl for gain map conversion (933db7a).
- Set correct minimum versions for `zenflate` and `linear-srgb`, and moved
  `linear-srgb` to a semver spec (d48d69b, a1c1131, 210c255).

## [0.1.1] - Earlier

### Changed
- Switched `rav1d-safe` from a git revision to the published `0.5.3`
  release on crates.io (55d009a).
- Removed local path overrides that broke CI (20500cd).

### Fixed
- Temporarily pinned `rav1d-safe` to a git revision containing the
  aarch64 panic fixes while the fix was making its way through a release
  (01c02d0).
