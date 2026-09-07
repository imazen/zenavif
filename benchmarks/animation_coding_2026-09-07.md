# Animation coding controls — 2026-09-07

Owner pin: `ecbf41ec42f05568e0f36074edc7f946ebdb12bf`.
Backend pin: `1d5a6e04e80216b1063faa3baa7cbb2189d132f3`.
Previous owner: `a8eaf99730fca1b7e98c3f9c731ab4189918339b`.

Animation now consumes lossless for both color and alpha, and VAQ, its
strength, segmentation boost, StillImage tuning and trellis for color.
Both the encoder and container-header contexts receive the same settings.
Alpha retains its separate tuning policy.

The combined lossless-plus-trellis arm exposed a backend defect: trellis
modified exact WHT residual coefficients. Disabling VAQ did not fix it.
The backend correction leaves WHT coefficients untouched; ordinary lossy
transform optimization is unchanged. The backend source-equality regression
covers 72 configurations / 108 frames, at 8/10/12 bits, mono/420/444,
one/two frames, two tuning modes and trellis off/on with VAQ and boost enabled.
Before/after identity gates pass 81 pinned baselines and 360 option arms;
before/after reconstruction gates pass 54 cases with both decoders.

Owner validation passes 96 all-feature tests (five existing ignored doctests),
58 no-default-feature tests (four existing ignored doctests), and all-target
Clippy. Its durable external verification script decodes 16 streams / 32
frames with libaom and requires all eight lossless streams to equal the
original coded source planes, including the combined tuning arm. Artifacts
and hashes are recorded in the owner's matching benchmark document.
This establishes coded-plane losslessness; RGB-to-420 conversion is still
lossy and does not provide an exact RGB round trip.

The canonical regression `tests/animation_encode_coding.rs` encodes seven
option arms at both coding depths: baseline, lossless, VAQ, StillImage,
trellis, VAQ plus boost, and all controls together with lossless. All 14
AVIFs / 28 frames decode, and both lossless arms preserve binary alpha
exactly. The previous owner pin failed that assertion with alpha 53 versus 0.
The published corrected pins pass the focused test, 903/903 workspace tests
(nine existing skips), and all-feature workspace library Clippy with warnings
denied. No test threshold, envelope or skip was changed.

Commands (serialized through the shared 16G/four-job wrapper with
`TMPDIR=$HOME/tmp RUST_TEST_THREADS=4`):

```sh
cargo test --all-features --test animation_encode_coding
cargo nextest run --workspace --all-features
cargo clippy --workspace --all-features --lib -- -D warnings
```

Logs: `~/tmp/slower-preset-probe/animation-coding-canonical-*.log`.
The wrapper quality investigation remains unresolved; these review commits
do not advance its main branches. Exact non-millisecond timing, per-frame
hints and broader pixel-format options remain separate work.
