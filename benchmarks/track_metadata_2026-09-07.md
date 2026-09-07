# Animation track metadata independence — 2026-09-07

The codec animation probe and frame-decoder info took static HDR metadata from
the primary poster item. A fixture with poster MaxCLL 10 and track MaxCLL 1000
reported 10 (`codec-track-hdr-before.log`, executed failure). Both paths now
use the track's CLLI/MDCV, including authoritative absence: a missing track
property does not inherit a poster value. `hdr_metadata()` on the concrete
codec decoder retains AMVE/CCLV as well, in native AVIF units.

Expanding the same fixture to remove its poster exposed a real probe failure:
ManagedAvifDecoder parsed empty primary bytes instead of the first sample
(UnexpectedEOF at AV1 sequence-header parsing). It now probes the first actual
sample for posterless sequences and reports track alpha presence. Primary bytes
are resolved only once, preserving the multi-extent allocation behavior.

The transparency test then failed on missing premultiplication. The parser
read item `prem` references but discarded the color track's `tref/prem` link to
its associated alpha track. Track association now retains this boolean and
exposes it independently through `animation_premultiplied_alpha()`. No-poster
metadata also gets the track value. Managed and AOM animation conversion take
explicit track premultiplication; still conversion continues to take item
premultiplication. The shared pixel conversion helpers read that resolved value.

Focused verification:

- Eight exact-timing/HDR tests pass. The new fixture covers a different poster,
  absent track CLLI/MDCV, and no poster; AMVE values and signed CCLV primaries
  survive the concrete codec API. All rendered rows agree with the original.
- Five SVT animation tests pass. Alpha cases cover 8/10-bit, no poster,
  poster-straight/track-premultiplied, and poster-premultiplied/track-straight.
  Borrowed and owned parsers preserve the track flag. Managed and AOM codec
  decoders agree with separately encoded controls on every rendered row:
  24 decoded frames across the alpha scenarios, with true alpha and dimensions.
- Full workspace all-feature nextest: **877/877**, nine existing skips.
- Default workspace tests/doctests: **473 passed**, ten existing ignores.
- All-feature library clippy passes with warnings denied; scoped formatting
  and whitespace checks pass.
- Determinism: five cells × five thread legs pass. Reference conformance:
  **56/56** AVIF cells pass; the optional armed CLI leg was explicitly not run
  because the sibling CLI is unavailable.
- Post-change ladder: 48 tolerance failures (33 byte/quality plus 15 timing).
  All 33 byte/quality failure rows are identical to the pre-change baseline;
  its 49 total included 16 timing failures. Monotonicity reports the same two
  screen/q80 inversions (speed 6 dominated by speeds 7 and 8). This does not
  establish that the historical envelope drift is harmless.

Logs are under `~/tmp/animation-metadata/`: `codec-track-hdr-before.log`,
`codec-track-hdr-after.log`, `codec-track-hdr-expanded.log`,
`no-poster-alpha-before.log`, `no-poster-alpha-after.log`,
`track-metadata-final-expanded.log`, `track-metadata-nextest.log`,
`track-metadata-default.log`, `track-metadata-final-clippy.log`, and
`track-metadata-{determinism,conformance,ladder,monotone}.log`.
The expanded test initially had a private-field compile error; the executed
UnexpectedEOF and missing-premultiplication failures are separate evidence.

Remaining audit: track dimensions, CICP/ICC and spatial metadata versus poster
properties; canonical animation options/non-ms timing, mono animation, streaming
and full AVIF/video scope. Existing quality-envelope drift remains unresolved;
no thresholds were relaxed and push/CI remain deferred.
