# Animation spatial metadata — 2026-09-07

The parser discarded clap, irot, imir and pasp from the animation sample entry.
A real SVT sequence with a 65×67 coded canvas, crop (3,5,49,53), quarter-turn
rotation and mirror reproduced incorrect codec dimensions: expected 53×49,
received 65×67 (`spatial-before.log`). An initial fixture compilation error was
corrected before this executed failure; it is not evidence of the production bug.

Parser AnimationInfo and legacy AnimationConfig now carry independent track
AnimationSpatialMetadata. Native DecodedAnimationInfo exposes that metadata
while preserving coded-canvas frames. Managed/AOM frame metadata and native
animation probes select track spatial properties rather than poster properties.
The codec probe and animation decoder resolve integral clean apertures exactly,
using checked rational arithmetic, then crop before orientation correction.
The concrete decoder exposes original spatial_metadata(). Frame limits apply
to coded dimensions before cropping. Crop allocation is fallible/configurable,
and cancellation is checked per row. Invalid or unsupported apertures leave
input ImageInfo dimensions unchanged; the unit test covers equivalent rationals,
zero denominators, zero sizes, extreme offsets and fractional origins/sizes.

The integration test covers two 8/10-bit SVT RGBA sequences, all four rotations,
three mirror states, and three poster/track layouts (conflicting poster,
posterless, track properties absent). Both Preserve and Correct orientation
policies and managed/AOM backends check every output pixel including alpha
against mathematically transformed decoded controls: 576 codec frames. Native
metadata and unchanged canvas pixels are checked for another 144 frames.
Controls add four decoded frames. Focused final tests pass 19/19, including
previous exact-timing, dual-color and SVT animation regression tests.
The transactional crop unit passes separately.

Independent libavif 1.3.0 avifdec --info reads the track's exact 49×53 aperture,
offsets -10/2 and -4/2, irot=1, imir=1 and pasp=1/1 despite different poster
properties. It decodes both frames at exact 17/33 ticks of a 1000 Hz timescale.
The initial posterless fixture failed reference decoding because it removed
meta while retaining avif/mif1 still-image compatibility brands. The shared
remove_poster helper now retains sequence brands and adds msf1, replacing
unused ftyp bytes with a free box so offsets stay unchanged. The final fixture
passes independent decoding (`spatial-libavif-final-no-poster.log`). This also
corrects the earlier timing, dual-color and SVT animation posterless fixtures;
no assertions were removed or weakened. Parser strict brand validation itself
is not changed by this fixture correction.

The installed avifdec CLI warns that PNG output does not apply clean aperture
or orientation. Both PNG exports remain 65×67; they are not independent proof
of rendered crop/orientation. A proposed Python comparison could not run because
Pillow is absent; it provides no pixel evidence. Independent verification here
is limited to metadata and successful decoding. Integration pixel mapping is
verified against untransformed decoded controls within our own implementation.

Validation logs are under ~/tmp/animation-metadata/:

- spatial-nextest.log: 884/884 workspace all-feature tests, nine existing skips.
- spatial-default.log: 478 default tests/doctests, ten existing ignores.
- spatial-clippy.log: all-feature workspace library clippy, warnings denied.
- spatial-determinism.log: five cells × five thread legs pass.
- spatial-conformance.log: 56 AVIF cells pass; optional armed CLI leg unrun.
- spatial-ladder.log: 49 failures, comprising the identical 33 non-timing
  byte/quality rows from animation-seam-before-ladder.log plus 16 timing misses.
- spatial-monotone.log: the same screen/q80 speed-6 inversions against 7 and 8.

No quality floors or envelopes were changed. Push and CI remain deferred.
Fractional aperture resampling, non-square pixel presentation, tkhd matrices,
other orientation-policy variants, track-local Exif/XMP and auxiliary metadata
remain open. This is a bounded correction, not completion of all AVIF features.
