# zenmp4

MP4 (ISO base media file format) for `zencodec-media`, sans-I/O: the
demuxer asks for byte ranges, and any `zencodec-io` driver (slice,
`ReadAt`, `Read`, async) answers them.

`Demuxer::step`, `Muxer::write_packet` and `Muxer::finish` accept an optional
borrowed Pulse. `None` adds no per-call control; parsing, limits and timing
validation still apply. This source migration targets zencodec `a7c2a6ca` and
has not been compiled or tested with the still-pinned dependency graph.

- Progressive files, `moov` before or after `mdat`.
- Fragmented files (`moov` with `mvex`, then `moof` + `mdat`).
- Packets in file order, with pts, dts, duration and random-access
  evidence.
- Seeking by time in both layouts. A fragmented file's first seek indexes
  every `moof` (range requests that skip each `mdat`), and later seeks
  reuse that index.
- Edit lists carried as the track's edit delay, not applied.
- Configuration records as `codec_private`: `av1C`, `vpcC`, `avcC` and
  `hvcC` as they are; Opus `dOps` as an `OpusHead`; AAC `esds` as its
  AudioSpecificConfig; FLAC `dfLa` as `fLaC` plus its blocks.

`scripts/ffmpeg-oracle.sh` checks both directions (`just mp4-ffmpeg-oracle`
from the zenavif root). ffprobe must read FFmpeg-written files the same way
zenmp4 does. zenmp4's stream copies, in both layouts, must read the same in
ffprobe and decode to the original's frames in FFmpeg. Seeks into every
file must land where ffprobe's packet list says they should.

The muxer writes either layout:
- Progressive: `mdat` streamed, `moov` at the end, one patch for the
  `mdat` size.
- Fragmented: for destinations that cannot seek back, `moov` first and a
  fragment per video random-access point, with no patches.

Durations come from the next sample's decode time, so they are exact. An
AV1 track without an `av1C` gets one built from its first sequence header.

Unpublished: `zencodec-media` is not on crates.io yet.

For exact supplied packet timing, use `Mp4Muxer::begin_exact_timing(tracks,
hint)`. It prepares the actual fragmented writer before exposing its header,
including for a seekable destination, so nonzero decode origins survive.
Every packet must supply an explicit duration; unrepresentable clocks, ranges,
decode gaps/overlaps and missing durations refuse before that packet changes
output. Missing DTS uses PTS as decode time. Preparation does not promise that
unknown future packets will fit; drivers retain and report prior publication
on failure. Metadata and workspace guarantees are separate.

The legacy `Muxer::begin` still allows inferred durations and progressive output.
The shared exact-timing trait hook is being qualified on the zencodec API review;
this inherent method lets the native implementation be tested without changing
the existing workspace's shared dependency revision.
