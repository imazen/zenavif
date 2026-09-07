# Animation color and geometry provenance — 2026-09-07

Three independent regressions are corrected locally:

1. Codec animation probing and frame-decoder info used the primary poster's
   dimensions, depth, chroma layout and color metadata. Track HDR had already
   been corrected separately. A differing nclx fixture executed red with poster
   1/13/6 versus track 9/16/9 (`track-color-before.log`).
2. Managed and AOM animation pixel conversion selected the poster's color
   property, including the MC=2 fallback matrix. A colored 150×150 SVT sample
   makes BT.601 and BT.2020 conversion produce different pixels. Its AV1 CICP
   fields are changed to unspecified, retaining all other bits, so independent
   item and track nclx values obey AV1-ISOBMFF §2.3.4. Eager and both lazy
   backends now match every row of the track control. Temporarily restoring
   poster selection makes the same test fail on rendered rows
   (`track-matrix-poster-mutation.log`); this is a targeted mutation witness.
3. Decoder construction applied frame limits to the poster even for animation.
   A 150×150 8-bit track with a separately encoded 220×230 10-bit poster was
   rejected as ImageTooLarge at a 150×150 limit (`track-geometry-before.log`).
   Track-aware construction and probing now accept that animation. The test
   also uses a smaller 32×34 10-bit poster and rejects a 149-pixel width cap.
   Final call-site audit also reproduced the same erroneous poster rejection
   through decode_animation_with (`track-eager-limit-before.log`). That eager
   convenience entry point now uses track-aware construction too; its test
   accepts the exact pixel boundary and rejects one pixel below it.
   The poster itself is independently decoded to verify fixture dimensions
   and depth before checking animation info and pixels.

The ICC test additionally exposed reverse inheritance: absent poster colr
fell back to the color track's ICC. The parser now preserves primary-item
absence, while the existing no-poster branch continues to expose track fields.
Both directions of absent ICC are tested against a real RGB ICC profile.

Implementation:

- Add animation_color_info() to the parser, independently of poster metadata.
- Add AV1Metadata CICP fields, using the existing parsed sequence-header values.
- Add ManagedAvifDecoder::probe_animation_info(), deriving geometry/depth/chroma
  from the first actual sample and using track color/HDR/alpha. Codec animation
  construction applies limits to these values. Native still probing retains
  primary-item semantics; posterless probing selects the animation path.
- Select Primary versus Animation explicitly for managed/AOM frame conversion
  and fallback-matrix resolution. Existing premultiplication routing uses the
  same selection; still and sink paths continue to select Primary.

Fixture notes: the first matrix witness attempt used the flat timing sample,
which did not distinguish rendered matrices. It was replaced with a real
colored sample, not a weakened assertion. Early geometry-test compile/setup
errors are not counted as bug evidence. Empty configOBUs in the geometry
fixture are permitted by AV1-ISOBMFF §2.3.4; nclx is present and every sync
sample includes its own sequence header. This test uses the low-level serializer
because try_serialize currently insists on a nonempty sequence-header argument.
Reference: https://aomediacodec.github.io/av1-isobmff/#av1codecconfigurationbox-semantics

Focused all-feature exact-timing/metadata suite: 11/11 passes. Full all-feature
workspace nextest: 880/880, nine existing skips (`track-color-nextest-final.log`).
The initial run exposed the older maximum-finite-count unit test's fake
"sample"/"header" bytes: constructor sequence-header limit validation correctly
rejected them. Replaced those placeholders with the real 150×150 timing sample,
keeping both maximum-count assertions. Default tests/doctests: 475 passed,
ten existing ignores. All-feature library clippy passes with warnings denied;
scoped formatting and whitespace checks pass. Determinism passes five cells ×
five thread legs; reference conformance passes 56/56 AVIF cells. The optional
armed CLI leg remains explicitly unrun because the sibling CLI is unavailable.
The final ladder has 48 tolerance failures: 33 byte/quality plus 15 timing.
All 33 byte/quality rows are identical to the pre-change baseline. Monotonicity
retains exactly the same two screen/q80 inversions, speed 6 dominated by 7 and 8.
These unresolved historical failures are not excused or re-pinned.
Logs are under ~/tmp/animation-metadata/track-color-*.
No thresholds or ignore flags changed.

Still required: track orientation/clean aperture/pixel aspect ratio and Exif/XMP
association versus poster properties; source-encoding detail provenance; full
canonical animation options/timing/mono and AVIF/video feature inventory.
This change does not establish those other properties' correctness. Existing
zenravif ladder/monotonicity failures remain under investigation; CI is deferred.

Code audit for the next chunk: read_stsd retains only AV1 config, one colr and
HDR; read_tkhd skips the matrix; read_trak skips track-local meta entirely.
The serializer writes clap/irot/imir through write_transform_properties in
both ipco and av01, and sidecars through track-local meta. These locations
need independent specification and reference-decoder verification before a
parser-only round trip is treated as spatial/sidecar support. Also audit
coexisting ICC and nclx: the current single ColorInformation enum can discard
one representation (track parsing takes the last colr, primary find_prop the
first), potentially losing an MC=2 matrix hint when ICC is present.

The 880-test workspace run and 475-test default run preceded the final one-line
eager convenience constructor routing. Final focused tests pass 16/16 and library clippy passes; these
cover that routing and its strengthened limit assertions; see
track-color-final-focused.log and track-color-final-clippy.log. The earlier
full suite already covers the common track-aware constructor and conversion.
