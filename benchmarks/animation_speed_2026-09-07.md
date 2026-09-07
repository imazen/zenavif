# Animation speed override integration — 2026-09-07

Owner revision: `a8eaf99730fca1b7e98c3f9c731ab4189918339b`.
Previous pin: `b829a9ee172b90a8cbe8491b4e3e9fa1fc4559a6`.

The shared animation speed derivation now consumes the eight explicit speed
controls that were already forwarded by the canonical builder but dropped
by the owner: transform RDO, SGR complexity, restoration-on-skip, segmentation
complexity, bottom-up partition search, partition range, prediction modes and
fast deblocking. Explicit prediction modes clear the preset's top-seven arm.
Alpha keeps its independent preset policy.

Owner tests verify both values at speeds 1/6/8/10, the final backend speed
settings and unchanged alpha configuration. A public-owner packet regression
fails before the correction because a forced 4×4 partition range changes no
bytes, and passes after it at both 8-bit and native 10-bit input. Owner suites
pass 94 all-feature tests (five existing ignored doctests) and 58 tests without
default features (four existing ignored doctests); all-target clippy passes.

The canonical regression exercises each of the eight controls as on and off
at both coding depths through RGBA8 animation encoding. It enables restoration
while testing its search controls and decodes each complete two-frame AVIF.
It requires the partition-range arm to change output, but does not require
every search control to change pixels on every image. Against the old pin,
the partition assertion fails after the first 16 encodes/decodes.

Published-Git validation passes all 32 public-API cases (64 decoded frames),
902/902 workspace tests (nine existing skips), and all-feature workspace
library clippy with warnings denied. The fetched owner manifest and eight
Rust sources match the locally verified owner byte-for-byte.
Raw logs: `~/tmp/slower-preset-probe/animation-speed-*.log`.
Tests use the shared 16G/four-job wrapper and RUST_TEST_THREADS=4. No threshold,
envelope or test skip was relaxed. VAQ/trellis/tune/lossless, per-frame hints,
pixel-format options and exact public timing remain separate work. The
wrapper quality investigation remains open and its main branches are not
advanced by this integration.
