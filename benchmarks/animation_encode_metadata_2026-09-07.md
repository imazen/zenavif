# Upstream animation metadata integration — 2026-09-07

The pinned zenravif animated muxer omitted the metadata configured for still
encoding. The consumer regression failed on missing track Exif, and a separate
premultiplication regression failed on the absent track association. Both
reproduce after updating from f6c883b6 to current cavif-rs main a7e9fcc5; the
animation module was unchanged over that range. Main additionally integrates
zenrav1e 60594682 block-handling fixes. Its measured determinism/conformance
passes and the identical 33 quality-envelope rows are recorded in
~/tmp/animation-metadata/cavif-main-{check,determinism,conformance,ladder,monotone}.log.
Those measurements do not prove every encoded output across the revision range
is identical.

The owner fix is published on cavif-rs animation-avif-complete as
6974b5a8dd5ed38d416ee4455a2b990692c3d4d8. It wires ICC/Exif/XMP, rotation/mirror,
CICP, CLLI and MDCV to track and poster, and multiplies animation RGB by alpha
before color conversion when requested at 8/10 bits. The matching association
is signaled only when alpha is present. Checked serialization retains invalid
metadata errors through SerializationError. The serializer is the published
canonical revision 98c8a501; its manifest and nine source files match the local
workspace member exactly. The consumer now uses the published upstream Git
revision and unifies the canonical serializer source with its workspace member.
No local cavif path override remains in its manifest.

Owner validation: 86 tests/doctests with assembly disabled and 86 with all
features, five existing ignored doctests each; both all-target clippy runs pass
with warnings denied. See the owner's benchmarks/animation_metadata_2026-09-07/README.md.
The review branch is excluded from push CI by its checked workflow filter.
Remote ancestry confirms the published commit; the Actions API reports zero
runs for the branch. No PR or workflow dispatch was created.

Consumer tests exercise all four RGB/RGBA storage types at both coded depths,
checking exact track/poster ICC/Exif/XMP and spatial/HDR fields. Metadata-only
changes preserve all 16 color and 8 alpha sample strings and exact timing.
Native managed/AOM metadata is checked too. Premultiplication tests decode
alpha levels 0/128/255 at both depths with both backends. A signaling-only
mutation fails at alpha=128 with red 149 rather than 75; the restored code
passes unchanged RGB <=5 and alpha <=1 tolerances. Invalid rotation/mirror
metadata returns a serialization error. Focused tests pass 3/3.

Independent libavif 1.3.0 decodes all 22 frames in ten exported files. Its PNG
exports preserve exact 596-byte ICC, 14-byte Exif and 37-byte XMP. A CRC-checked
PNG reader verifies all 1024 half-alpha pixels at each depth, with maximum
RGBA errors [1,1,1,0]. Reference logs: encode-meta-libavif.log and
encode-meta-reference-details.log under ~/tmp/animation-metadata.

Local-source all-feature integration passes 895/895 (nine existing skips), and
library clippy passes with warnings denied. An earlier invocation placed Cargo
configuration before the external nextest/clippy commands, which did not forward
it and selected unfixed Git source; that nextest run was stopped. Those logs
(encode-meta-nextest-local.log / encode-meta-clippy-local.log) are not evidence
for the fix. Corrected invocations put --config after the subcommand, explicitly
log the local package source, and pass (encode-meta-nextest-corrected.log and
encode-meta-clippy-corrected.log). Published-Git verification, without local cavif overrides, passes 895/895
all-feature workspace tests (nine existing skips), 484 default tests/doctests
(ten existing ignores), 3/3 encode-only tests, and library clippy with warnings
denied. The fetched upstream manifest and eight source files match the verified
local package exactly. Cargo.lock changes only the zenravif and zenrav1e source
revisions. Logs: encode-meta-{nextest,default,minimal,clippy}-git.log.

Final determinism passes 25 legs; conformance passes 56 AVIF cells, with the
optional armed CLI leg explicitly unrun. Ladder retains the exact same 33
byte/quality rows as the current-main and old-pin baselines, plus 16 timing
misses (49 total). Monotone retains the same two screen/q80 speed-6 inversions.
See encode-meta-{determinism,conformance,ladder,monotone}.log. No thresholds were
changed. The user's existing review-branch push request is honored while CI
remains deferred; these branches are excluded by the checked push filters.

The existing quality-envelope failures are unresolved; no thresholds or skips
were added. CI remains deferred. Exact non-ms timing, other ignored animation
coding options, additional metadata/auxiliary forms, and video remain open.
A separate read-only finding in the owner's still convert_alpha_8bit helper
needs an executed regression and correction; this change fixes animation only.
