# Explicit MP4 fragment publication

2026-11-08. Correctness and source-compatibility qualification, not a measured
end-to-end streaming latency or memory bound. Base source zenavif `69914c73`.
The native and independent oracle graph pins zencodec/media `1816b4d0` and the
owner's zenpixels source. No pixel codec or default fragment policy changes.

The original muxer closes an open fragment before the next video random-access
packet. Even a known-duration single-frame fragment therefore need not publish
until another packet arrives. The additive `Mp4Muxer::flush_ready_fragment`
allows callers to publish all ready samples explicitly and drain the output
without finishing the stream. It neither guesses an unknown duration nor waits
for missing tracks. Progressive and unbegun writers refuse; cancellation is
checked before mutation. Existing automatic boundaries retain their behavior.

The frozen original source passes34 native tests, strict all-target clippy and
207 independent FFmpeg oracle checks. The candidate passes38 native tests and282 independent oracle checks, with
strict all-target clippy passing. Its native gates cover
publication before another packet or finish, exact nonzero origins/durations,
continuation after publication, empty flushes, retained unknown-duration tails,
audio/video packets and stop/refusal state preservation. No existing assertion
is weakened and no test is ignored.

The independent oracle additionally remuxes ordinary inputs with publication
after each packet. It checks packets, decoded frames and seeks against FFmpeg
for progressive, automatic-fragmented and explicit-ready layouts. A separate
single-frame VP9 prefix ends before mux finalization: ffprobe reads matching
PTS/DTS, flags, size and payload SHA256; FFmpeg decodes the same first frame.
The written `trun` duration is checked separately against the source packet.
This FFmpeg build omits packet duration at that prefix's final VP9 sample;
the native test still requires exact recovered duration, and the field-level
check is mandatory. The initial over-strict ffprobe duration probe and its
failure remain in the retained evidence.

Reproduction is `cargo test --release -p zenmp4 --all-targets`,
`cargo clippy --release -p zenmp4 --all-targets -- -D warnings`, and
`zenmp4/scripts/ffmpeg-oracle.sh NEW_OUTPUT`. Freeze only this package and the
owning dependency patches to avoid changing other workspace sources. Native
artifacts, resolved locks, frozen sources and full logs are retained under
`output/imij/video_latency_20261108/`; the owning IMI video-latency plan tracks
container integration separately from the codec and transport work.

Original test wrapper: rc0,10s,peakRSS0.32GiB; strict clippy rc0,2s,0.26GiB;
original independent oracle rc0,47s,0.21GiB. Candidate native v2 rc0,6s,0.31GiB;
strict v3 clippy rc0,0s,0.16GiB. Corrected v4 oracle rc0,11s,0.21GiB. All use
8GiB hard caps; the oracle is confined to4 CPUs. These are correctness job
resources, not codec throughput measurements.
