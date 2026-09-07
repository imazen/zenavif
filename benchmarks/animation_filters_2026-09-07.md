# Animation filter integration — 2026-09-07

Owner pin: `b829a9ee172b90a8cbe8491b4e3e9fa1fc4559a6`.
Previous pin: `939fa5a3e6f0717837ff7cd6d314a5644b9abff3`.

The canonical builder forwarded explicit CDEF and loop-restoration settings,
but the animation owner ignored both when deriving its speed configuration.
The owner now applies the color overrides after the preset in both animation
context construction paths. The independently configured alpha track retains
its preset behavior, matching the still encoder's policy.

`tests/animation_encode_filters.rs` drives the public RGBA8 animation API at
coded depths 8 and 10. It compares neither-filter, CDEF-only, restoration-only,
and both-filter outputs, and decodes every complete AVIF with the managed
decoder. Against the previous pin it fails the CDEF output assertion. Against
the published correction it passes: eight files, two decoded frames each.
The owner regression additionally verifies unchanged alpha packets and fails
when only restoration forwarding is removed.

Owner validation: 92 all-feature tests and 58 without default features;
all-feature/all-target clippy passes. Existing ignored doctests remain five
and four respectively. Independent libaom accepts ten owner raw streams
(20 frames) at their expected dimensions and depth. Decoded bytes differ from
the off arm for CDEF at both depths. Restoration-only decoded pixels are
identical on that fixture: its regression establishes changed packet syntax
and option forwarding, not that restoration wins the search on this content.
No encoder-reconstruction equality claim is made for this run.

Canonical published-Git workspace validation passes 901/901 tests (nine
existing skips), plus all-feature workspace library clippy with warnings
denied. The fetched owner manifest and eight Rust sources match the locally
validated owner byte-for-byte. Commands run through the shared 16G/four-job
wrapper with RUST_TEST_THREADS=4.
Raw logs: `~/tmp/slower-preset-probe/animation-filters-*.log`.
No quality envelope, expected output or skip was relaxed. Other animation
options, exact non-millisecond timing, auxiliary tracks, and the separate
quality-gate investigation remain open; wrapper main is not advanced here.
