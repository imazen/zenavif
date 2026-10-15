# zenmp4

MP4 (ISO base media file format) for `zencodec-media`, sans-I/O: the
demuxer asks for byte ranges, and any `zencodec-io` driver (slice,
`ReadAt`, `Read`, async) answers them.

- Progressive files, `moov` before or after `mdat`.
- Fragmented files (`moov` with `mvex`, then `moof` + `mdat`).
- Packets in file order, with pts, dts, duration and random-access
  evidence.
- Edit lists carried as the track's edit delay, not applied.
- Configuration records as `codec_private`: `av1C`, `vpcC`, `avcC` and
  `hvcC` as they are; Opus `dOps` as an `OpusHead`; AAC `esds` as its
  AudioSpecificConfig; FLAC `dfLa` as `fLaC` plus its blocks.

`scripts/ffmpeg-oracle.sh` checks both directions (`just mp4-ffmpeg-oracle`
from the zenavif root). ffprobe must read FFmpeg-written files the same way
zenmp4 does. zenmp4's stream copies, in both layouts, must read the same in
ffprobe and decode to the original's frames in FFmpeg.

The muxer writes either layout:
- Progressive: `mdat` streamed, `moov` at the end, one patch for the
  `mdat` size.
- Fragmented: for destinations that cannot seek back, `moov` first and a
  fragment per video random-access point, with no patches.

Durations come from the next sample's decode time, so they are exact. An
AV1 track without an `av1C` gets one built from its first sequence header.

Unpublished: `zencodec-media` is not on crates.io yet.
