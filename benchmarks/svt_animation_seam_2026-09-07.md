# Canonical SVT animation wiring — 2026-09-07

The four public RGB8/RGBA8/RGB16/RGBA16 animation functions now dispatch to
SVT when selected. They previously returned a blanket refusal despite the
pinned upstream encoder's sequence support. The new integration test reached
that refusal before dispatch was added (`animation-seam-before-test-final.log`).

A shared `CodedSvtFrame` separates pixel encoding from container construction.
Still wrappers serialize those bytes as items; animation serializes them as
sync samples. There are no temporary still files and no encode/decode/remux
round trip. Existing color conversion, alpha quality, SVT parameter application,
thread budget and stop token paths are retained. Sequence mode configures full
headers via `with_image_sequence()` and derives the level from the fastest
frame interval. Empty sequences, zero durations, mismatched geometry, invalid
track dimensions and count/duration overflow are rejected. Compressed frame
and sample tables use fallible reservation; pixel conversion/serialization
still have allocation work remaining.

Both color and alpha get full sequence headers. Track samples omit temporal
delimiters; av1C includes the first complete sequence-header OBU. The existing
millisecond API uses timescale 1000, retaining totals and timestamps above
u32. Metadata currently available on EncoderConfig is forwarded: CICP,
ICC/Exif/XMP, orientation and CLLI/MDCV.

The new premultiplied-alpha test failed because the adapter retained the
premultiplied samples without signaling them (`animation-seam-alpha-before.log`).
Both still and animation muxing now write the flag for premultiplied RGBA;
the test checks decoded RGB/alpha against expected values within five 8-bit
levels, not merely the presence of a property.

Verification:

- Four native input types × two coded depths × three speeds × three frames:
  24 actual animations / 72 frames, at odd 65×67 dimensions. Includes an opaque
  first alpha frame followed by partial transparency. Tests assert frame count,
  dimensions, timing, depth, full headers, alpha track presence and changing
  rendered pixels.
- Layout/duration failures and an animation total exceeding u32 milliseconds.
- Exact XMP and CLLI values; premultiplied rendered pixel checks.
- Codec-trait animation encoding from RGBA8 into ten-bit color/alpha.
- Final focused run: all four animation tests and all 26 existing SVT
  still-backend tests pass. The animation matrix additionally compares every
  coded color/alpha plane through rav1d-safe and zenav1-aom: **108 sample
  pairs agree exactly**. Full-sequence and native-depth assertions pass.
  The final sequence-header walker uses checked u64-to-usize sizing for
  32-bit hosts; the focused run and clippy include this last change.
- Full workspace all-feature nextest: **871/871**, nine existing skips.
- Default workspace tests and doctests: **472 passed**, ten existing ignores.
- Workspace all-feature library clippy with warnings denied passes.

Required broader gates exposed pre-existing zenravif baseline drift:

- Determinism passes before and after: five cells × five thread legs.
- Reference conformance passes before and after: **56/56** AVIF cells. The
  optional palette/IntraBC sibling CLI is absent, so its armed leg was explicitly
  not run; this is not claimed as armed-tool coverage.
- Ladder: **49 tolerance failures before and after**, across 27 cells. All
  **33 byte/quality failure rows are identical**; remaining misses are timings.
  The checked-in envelope was added at `ec2540b` on July 5 and has not changed.
  This is evidence of an old baseline mismatch, not proof that the intervening
  encoder changes are correct. Do not simply re-pin it to make the gate pass.
- Monotonicity reports the same two new screen/q80 inversions before and
  after (speed 6 dominated by speeds 7 and 8). Investigation remains required.

Logs are in `~/tmp/animation-metadata/animation-seam-*.log`. Initial compile
failures in the new tests/adapter are separate from the executed before/after
failures and passing runs listed above. CI and another push remain deferred
while the broader baseline failures are unresolved.

Remaining scope: expose repetition/options and exact non-ms timing in the
canonical API, monochrome animation, explicit lossless policy, streaming and
bounded allocations, inter-picture compression, and the complete metadata,
transparency, grid and video inventory. Inspection also found the codec job
stores but never forwards its loop-count request, and its MDCV conversion uses
scales unlike the container's 50000/10000 units; both need reproductions and
correction in the next integration work. This is not full-goal completion.
