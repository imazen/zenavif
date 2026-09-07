# SVT capability integration — 2026-09-07

The canonical adapter now pins SVT `4e688a545847b3e6a40a3b46fc158dfd46331c3d`,
which merges the animation branch with main `74d92430`. A fresh fetch found
no additional main commits. Upstream merge verification is recorded in
`zenav1-svt/rust/benchmarks/main_merge_2026-09-07.md`.

The old adapter rejected odd mono/alpha geometry and partial superblocks
at low speeds, and rejected ten-bit mono/alpha below speed 7. Those gates
are removed from the shared validation/encode predicates. Empty images
and unsupported depths remain rejected. Existing actual pipeline entry
points handle padding, native ten-bit planes and typed errors.

The first new 65×67 all-speed/depth test failed at the old dimension gate
(`current-svt-before.log`). Updating only SVT then failed to compile:
registry magetypes 0.9.28 lacks the integer SIMD operations used by its
variance code. Root patches and lock entries now use upstream's matching
archmage, archmage-macros and magetypes revision `cc24398c`. Adding the
patch table alone was insufficient: the lock retained 0.9.28 until the
three packages were explicitly updated. No SIMD code was removed.

Focused verification: **26/26 SVT tests pass**, with no ignored or filtered
tests. The new all-speed test covers 65×67 grayscale and RGBA at Eight/Ten
(40 actual image encodes). Five former geometry/depth refusal tests now
require positive round trips, preserving their original 38/40 dB quality
floors and extending those checks to formerly refused cells. The QP-0
refusal test now requires exact luma/chroma reconstruction for 8-bit mono,
10-bit mono and 10-bit color, at 64×64 and 65×67, presets 0/7/9: 18 streams,
checked with both rav1d-safe and zenav1-aom. Native sources vary their low
bits; dimensions and monochrome/chroma geometry are asserted too.

Full workspace all-feature nextest: **867/867 passed**, nine existing skips
(`current-svt-nextest.log`). This rebuild includes all backend combinations
and the new SIMD revision; existing development-example warnings remain.

Default workspace tests and doctests: **472 passed**, ten existing ignores
(`current-svt-default.log`). Workspace all-feature library clippy with
warnings denied passes (`current-svt-clippy.log`). Scoped rustfmt and
whitespace checks pass. No quality floor or existing skip was changed.

Logs are under `~/tmp/animation-metadata/`: `current-svt-before.log`,
`current-svt-after.log`, `current-svt-after-patches.log`,
`current-svt-patched-final.log`, and `current-svt-suite.log`.

This change does not expose public lossless or animation encoding in the
canonical adapter. Its quality ladder still clamps QP to at least 1;
RGB-to-4:2:0 cannot promise image-lossless output. Upstream native ten-bit
monochrome mode decisions still use the upper eight source bits. The wider
AVIF/video feature inventory remains open, and CI remains deferred.
