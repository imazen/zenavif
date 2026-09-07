# Animation cancellation integration — 2026-09-07

Owner correction: cavif-rs `294cd9ee`; published dependency plus output evidence:
`2b9d4335950a952b9faa0bcd624eebf7820e714a`. Previous pin: `176ad8ee`.

The canonical wrapper already forwarded StopToken, but the animation owner
never installed it on zenrav1e or polled it. The regression in
`tests/animation_encode_cancellation.rs` lets the public-entry check succeed,
then requests cancellation. Against the old dependency it returns a successful
file; with the correction it returns the canonical Error::Cancelled. The test
covers RGB8 and RGBA8 without an intervening input-depth conversion.

Owner regressions cover all four storage formats with the legacy cancellation
token, direct stop and timeout. One deadline spans validation/preparation,
alpha inspection, row conversion, both tracks and pre/post serialization.
With the stop feature, the same combined control reaches backend superblocks;
without it, wrapper/row checks still operate. An executed mutation removing
only Context::set_stop fails the deterministic per-superblock test, which
covers 8/10-bit color and alpha. Source planes and encoding settings are not
changed by this correction.

Both before/after runs of `animation_encode_metadata` pass all three tests.
All ten exported AVIF files are byte-identical, covering the existing metadata,
input/coded-depth and premultiplication cases. Their hashes are recorded in
`animation_cancellation_identity_2026-09-07.tsv`. This preserves the prior
independent decoder evidence for those exact files; no new independent decode
claim is needed to establish byte identity.

Local integration: 899/899 all-feature workspace tests, nine existing skips;
all-feature library clippy passes with warnings denied. Owner all-feature
unit/integration/doc tests pass 90, five existing ignored doctests;
no-default-feature tests pass 58, four existing ignored doctests.
The fetched dependency's manifest and eight Rust source files are byte-identical
to the locally verified owner. Published-Git verification also passes 899/899 tests (nine existing skips)
and library clippy. Determinism passes 25 legs and reference conformance passes
56 cells; the optional armed CLI leg is explicitly not run. Ladder retains
52 failures: the same 32 non-timing rows, exactly equal to the prior audit,
and 20 timing misses. Monotone retains the same six inversions. These are
recorded failures, not an all-passing engineering-gate claim.

Commands use the shared run-heavy wrapper with 16G and four jobs. Raw executed
red, green, mutation, feature, output-pair and integration logs are under
`~/tmp/slower-preset-probe/animation-*.log`. No threshold/envelope or test skip
was changed. The separate RGB quality-gate investigation remains open.
