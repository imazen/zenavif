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

`scripts/ffmpeg-oracle.sh` checks it against ffprobe on FFmpeg-written
files (`just mp4-ffmpeg-oracle` from the zenavif root).

The muxer is not written yet.

Unpublished: `zencodec-media` is not on crates.io yet.
