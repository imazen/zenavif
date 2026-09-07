# Still alpha conversion and identity decoding — 2026-09-07

The animation alpha-policy audit exposed three defects in the still paths:

- zenravif's RGBA8 conversion divided color by alpha and cleared opaque
  pixels to transparent black, despite its unassociated-input contract.
- Canonical RGBA16 passed unassociated color to raw-plane APIs while signaling
  premultiplication. Multiplication now happens in full input precision before
  narrowing to the requested coded depth; the alpha plane is unchanged.
- AOM identity-color decoding allocated RGB storage even when alpha was present,
  so attaching alpha failed. It now allocates RGB/RGBA in the correct depth,
  using the configured fallible allocator, and retains existing range expansion.

`tests/still_premultiplication.rs` failed before correction with opaque alpha 0
instead of 255 (RGBA8) and an invalid alpha-buffer error (RGBA16/AOM). Both tests
now pass across 8/16-bit input, 8/10-bit coding, alpha 0/128/255 and both managed
and AOM decoders. A mutation disabling only the RGBA16 multiplication fails with
red 149 rather than 75 at half alpha; it was restored before final verification.
No tolerances or existing tests were relaxed.

Independent libavif 1.3.0 decodes all twelve exported files to PNG. A CRC-checked
PNG reader verifies every pixel, with visible color error at most 3/255 and exact
alpha. Invisible RGB is ignored only for fully transparent pixels. The test's
unchanged limits are RGB <=5/255 and alpha <=1/255. Logs and the reference
reader are under `~/tmp/animation-metadata/still-prem-*` and
`check_still_prem_reference.py`.

Owner fix: published cavif-rs review revision
`9585c67a6be5f32418924610084ca7cc8e91c449`. Its all-feature tests/doctests pass
86, with five existing ignored doctests; all-feature/all-target clippy passes
with warnings denied. Local-source canonical all-feature nextest passes 897/897
with nine existing skips, default tests/doctests 484 with ten existing ignores,
and all-feature library clippy passes with warnings denied. The canonical
manifest now pins the published owner revision; Cargo.lock changes only that
source revision. The fetched manifest and eight Rust source files are byte-identical
to the verified owner package. Published-Git all-feature nextest also passes
897/897 (nine existing skips), and library clippy passes. Determinism passes
25 legs; conformance passes 56 AVIF cells, with its optional armed CLI leg
explicitly unrun. The final ladder has 49 failures: its 33 non-timing rows
match the prior published integration exactly, plus 16 timing misses. Thresholds
and envelopes remain unchanged. Monotone retains the same two screen/q80
inversions: speed 6 is dominated by speeds 7 and 8. These are unresolved,
not new alpha regressions and not dismissed as harmless.

User authorization includes merging main once ready, then reading all open
GitHub issues to reconcile remaining gaps. Main merge and CI remain deferred
while known quality-envelope failures are unresolved. The review branch can
still be published under the user's earlier explicit push instruction.

The full animation/video objective remains active. In particular, the owner's
animation path still hardcodes 4:2:0, full range and BT.601, omits film grain,
and does not forward the per-superblock stop token to its context. Exact timing,
other alpha policies, additional metadata forms and video require further work.

Final focused test after adding explicit alpha-association assertions passes
2/2 against the published Git revision with encode + AOM enabled
(`still-prem-signaling-final.log`). Scoped formatting and diff checks pass.
