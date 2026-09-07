# Animation output depth and input stride — 2026-09-07

Auditing exact-timing options exposed a more immediate encoder-option failure.
The pinned zenravif animation methods hard-code coded depth from their input
method (8-bit input -> 8-bit AV1; 16-bit input -> 10-bit AV1), bypassing the
EncoderConfig bit-depth request. A real encode requesting Eight from RGB16
reported 10 in av1C and failed the executed regression in
depth-before-enabled.log. The initial default-feature test command ran zero
tests because encoding is optional; depth-before.log is not bug evidence.

The canonical seam now resolves the requested coded depth before choosing an
upstream animation entry point. RGB/RGBA8 -> Ten promotes samples by 257 and
uses the 16-bit animation path; RGB/RGBA16 -> Eight uses the shared narrowing
rule and the 8-bit path. Auto keeps 8-bit output for 8-bit input and 10-bit
output for 16-bit input. Unsupported depths still fail through the existing
guard. Conversion stages reserve fallibly, check size multiplication and poll
cancellation every 1024 pixels. Existing 16-to-10-bit staging now uses the same
logical-pixel iterator, excluding stride padding instead of treating it as
image pixels. Durations and the caller's configuration are forwarded to the selected entry
point. This does not prove that upstream honors every other animation option.

The native regression encodes 24 two-frame files across explicit Eight/Ten,
Auto, packed/padded storage and RGB/RGBA8/16 input: 48 color samples and 24 alpha
samples. It checks actual sequence-header depths and av1C, frame count/timing,
identical full files for equivalent input storage, and identical full files
with/without padding. A deliberate logical-pixel -> backing-buffer mutation
fails on "row padding changed encoded pixels" (depth-stride-mutation.log),
then is restored. The test exports 16 unique depth/input/layout AVIF artifacts
when ZENAVIF_DEPTH_ARTIFACTS is set; Auto overlaps the explicit-depth filenames.
Assertions always run regardless of export settings.

The codec regression additionally requests both output depths from both RGBA
storage types, checks color/alpha bitstreams and decodes both variable-duration
frames with managed/AOM backends. This test was added after the initial full
workspace run and is checked in the final focused run.

The first complete all-feature workspace nextest passes 891/891 (nine existing
skips); default workspace tests/doctests pass 484 (ten existing ignores).
Library clippy passes with warnings denied. After the codec test addition,
final focused checks pass 2/2 with all features and 2/2 with encode-only features.
No production code changed after the full suite except documentation. Libavif
1.3.0 independently decodes all 32 frames of the 16 exported files and reports
the requested 8/10-bit depths. Scoped formatting and diff checks pass.

Determinism passes five cells × five thread legs; reference conformance passes
56 AVIF cells. The optional armed CLI leg is explicitly unrun. Ladder retains
the identical 33 byte/quality rows from the previous sidecar/spatial baselines
plus 16 timing misses (49 total). The same two speed-6 screen/q80 inversions
against speeds 7 and 8 remain. No thresholds were changed; push/CI stay deferred.
Logs under ~/tmp/animation-metadata: depth-nextest.log, depth-default.log,
depth-final-focused.log, depth-final-encode-only.log, depth-clippy.log,
depth-libavif.log and depth-{determinism,conformance,ladder,monotone}.log.

Exact non-millisecond encoding timing remains open. Canonical frame structs
carry duration_ms; SVT derives level from 1000/shortest-duration and writes a
1000 Hz container. The pinned zenravif animation implementation hard-codes
Rational(1,1000) for both encoding and its separate sequence-header context,
with bitstream timing disabled. Extending timing requires deliberate upstream
and seam wiring, including level/rate behavior; changing a container clock
alone has not been accepted as a complete solution. Other animation options,
metadata forms, auxiliary images, and video coding remain open.

Additional read-only audit: pinned zenravif animated.rs::assemble_animation
sets only color/alpha codec configuration before serialization; it does not
call the metadata setters used by still encoding. This needs an executed
regression and correction next. Its frame encoder and separate sequence-header
context also hard-code several options, including timing and film-grain absence.
These findings expand the remaining audit; they are not covered by depth tests.
