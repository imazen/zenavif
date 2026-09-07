# Legacy decoder runtime findings — 2026-09-07

## Final conversion and color-grid correction

The legacy decoder now uses the same strip kernels as managed and raw-OBU
decoding for monochrome, 4:2:0, 4:2:2 and 4:4:4 at 8/10/12 bits, including
RGBA output. It also uses the shared H.273 resolver, preserving container
hints and chromaticity-derived coefficients instead of silently treating
unknown/unsupported matrices as BT.601. Identity remains a separate reorder.
Output allocation honors the decoder's allocation preference.

Color grids now decode their individual AV1 tiles through the legacy decoder
and use shared canvas assembly, with output-size limits, tile-count checks
and cancellation polling. Grid descriptors are no longer sent as raw OBUs.
Transparent grids remain explicitly unsupported, consistently with the
managed/AOM paths; implementing alpha-grid stitching is still required.

All 11 focused conversion/quality/parser comparisons passed unchanged.
The full suite then exposed a fragile mutation witness: the existing two-byte
corruption decoded at 38.43 dB, above its 38 dB quality floor. The revised
test preserves those exact flipped bytes and requires exact decoded-pixel
comparison to detect them. It additionally zeros sixteen coded-tail bytes
and requires the original 38 dB quality gate to reject that damage. The
uncorrupted control passes at 44.03 dB. No quality threshold was relaxed.

Final local evidence:

- Workspace all-feature nextest: **866/866**, nine existing skips.
- Default workspace tests and doctests: **472 passes**, ten existing ignores.
- Explicit `encode,zenav1-aom` product-path tests without `unsafe-asm`:
  **7/7**, including animations, grids, alpha and 10-bit comparisons. The
  earlier `zenav1-aom`-only invocation selected zero tests and is not evidence.
- All-feature library clippy passes with warnings denied. Two pre-existing
  lint issues were corrected using a config builder and a sweep factory alias.
- Scoped formatting and whitespace checks pass.

Logs: `ffi-kernels-focused.log`, `ffi-kernels-grid-nextest.log` (mutation
witness failure), `ffi-mutation-after.log`, `ffi-kernels-final-nextest.log`,
`ffi-kernels-default.log`, `ffi-kernels-managed-aom-final.log`, and
`ffi-kernels-clippy-final.log`, under `~/tmp/animation-metadata/`.

These runs use the canonical manifest's existing backend revisions, including
SVT `2d75a105`; they do not yet verify integration with the newly merged SVT
animation branch. CI remains deferred. The broader AVIF/video goal remains
open, including transparent grids and the other recorded feature gaps.

## Initial findings and drain/alpha correction

The real workspace all-feature nextest run completed 853 tests successfully,
reported nine assertion/fixture failures, and had three grid tests terminated
by signals (865 executed tests total, nine existing skips). The outer wrapper
returned 143; the source of that termination was not established. The grid
tests printed `Unknown OBU type 0 of size 7` before termination.

Confirmed and corrected in the legacy `unsafe-asm` wrapper:

- A valid temporal delimiter OBU with no frame spun forever on `EAGAIN`.
  `packet_without_frame_returns_error` reached execution and timed out before
  the fix (`ffi-no-frame-before.log`, rc 124). It now returns an error.
  The pinned rav1d 1.1.0 implementation uses the first `get_picture` call to
  enable draining and the second to wait for delayed frame-thread output;
  another EAGAIN requires more input, not a busy wait.
- The wrapper borrowed packet bytes behind a no-op free callback even though
  rav1d can retain references beyond the call. Packets now use rav1d-owned
  storage with an RAII unref on every return path.
- `add_alpha16` already scales native alpha to full u16 and expects RGB in
  that domain. The legacy caller scaled the whole buffer afterward, scaling
  alpha twice and unpremultiplying against mismatched domains. Color scaling
  now precedes alpha insertion. The existing 10-bit SVT alpha-quality test
  failed at 8.75 dB before and passes unchanged after.

The missing `seine_sdr_gainmap_srgb_icc.avif` fixture was restored from upstream
libavif main, SHA-256
`63fa6580a4cc215debc1229e35450c8144207218cc7ce6f5bfe5596efa357d55`.
Its roundtrip test now passes. The vector downloader also checks for this
fixture before accepting an older corpus as complete.

Verification after these corrections:

- All-feature root library: 308 passes, two existing pixel-conversion failures,
  one existing skip (`ffi-lib-after.log`).
- Focused no-frame, high-depth alpha and ICC gain-map cases pass. Two grid
  alpha cases now fail promptly with an incorrect error instead of hanging
  (`ffi-drain-alpha-focused.log`, three passes / two failures).
- The AOM high-depth container PSNR failure remains (`ffi-drain-alpha-after.log`).

Remaining failures include mono/4:2:2 conversion differences, two parser pixel
hashes under the legacy decoder, high-depth container conversion, SVT low-bit
quality ratio, zensim-loop quality, and grid handling. The legacy decoder still
feeds grid descriptors to the AV1 decoder; bounding the drain does not add grid
support. No assertion was relaxed and no test was disabled. This is partial
repair evidence, not an all-feature green result. CI remains deferred.

Logs reside in `~/tmp/animation-metadata/`; the full initial run is
`canonical-all-features-nextest.log`.
