# Legacy decoder runtime findings — 2026-09-07

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
