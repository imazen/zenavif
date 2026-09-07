# Codec playback and HDR wiring — 2026-09-07

The codec job accepted and stored `with_loop_count` but dropped it when creating
the animation encoder. A real zenravif test requesting one playback decoded as
infinite (`codec-loops-before.log`). The count now survives through finish.
The generated container's version-1 mvhd/tkhd durations and single-edit repeat
flags are updated without resizing boxes or touching samples, offsets or other
metadata. The entire generated layout and duration product are validated before
any mutation. This is a bounded adapter for the known serializer output, not a
general input AVIF editor.

Actual zenravif and SVT animations pass counts 0, 1, 3 and u32::MAX. Color and
alpha sample bytes and media ticks match the format-default controls; explicit
infinity is byte-identical to the complete default file. Decoding preserves two
frames and alpha. A structural overflow witness requires both an error and an
unchanged output buffer, then verifies a supported count can still be applied.

The MDCV audit found two independent errors:

- Codec float primaries are RGB, while native AVIF/ST 2086 primaries are GBR.
  The still/animation encoder and decoded-info adapter all copied indices
  unchanged. Independent expected wire values and a correctly ordered native
  fixture failed both directions (`codec-hdr-before.log`).
- Animation still used chromaticity ×65535, max luminance ×256 and min luminance
  ×16384. An ordering-only fix made the probe pass but the animation wire values
  still failed (`codec-hdr-order-only.log`). All animation scales now match the
  still path: chromaticity ×50000 and both luminances ×10000.

The tests verify explicit P3/D65 GBR wire values, 1000 cd/m² -> 10,000,000,
0.005 cd/m² -> 50, track metadata after actual animation decode, and correctly
ordered RGB floats when probing a separately constructed native fixture.

Full workspace all-feature nextest passes **875/875**, nine existing skips.
Default workspace tests/doctests pass **472**, ten existing ignores. The
encode-only (no SVT) loop integration test executes and passes **1/1**.
All-feature library clippy with warnings denied and scoped formatting pass.
Logs live under `~/tmp/animation-metadata/`, including `codec-hdr-after.log`,
`codec-loops-after.log`, and `codec-playback-hdr-nextest.log`.

This does not close the broad goal. Canonical exact non-ms timing/repetition
options, mono animation, HDR properties beyond the current config surface,
streaming/inter-picture compression and the full AVIF/video inventory remain.
The existing 49 ladder misses and two speed/quality inversions remain under
investigation; no envelope was re-pinned, and push/CI remain deferred.
