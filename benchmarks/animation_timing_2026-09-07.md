# Exact animation timing integration — 2026-09-07

Owner pin: `23522fff0947ae05aa6c1f341f5ee55fb7b78d84`.
Previous pin: `5b7c50b95bc5071bf29a46b0b5e3a6b2801f07d1`.

The four new `encode_animation_*_timed` entry points consume owned
`TimedAnimationFrame<P>` values with u32 tick durations and a u32 timescale.
Legacy millisecond functions and timed functions share private generic input
paths; adapting timing does not clone source images. Depth promotion,
narrowing and native 10-bit scaling preserve durations exactly. The result
contains exact total ticks and timescale, plus a compatibility millisecond
total rounded down. Zero timescales are rejected before depth conversion.

SVT uses the supplied clock for both sample tables and its fastest-frame
rate/level derivation. Its total uses checked addition and u128 millisecond
conversion. Zenravif consumes the published owner's exact-tick API. Its
nominal coding cadence is bounded separately from presentation timing, as
recorded in the owner's timing benchmark; this is not a realtime-throughput
or unrestricted AV1-level claim for extreme frame rates.

The concrete codec adapter adds `push_frame_ticks`. Mixed clocks use their
least common multiple, with checked u32 clock/sample rescaling. Validation
precedes frame append; old durations and the stored clock change only after
the new frame is accepted. Equal clocks avoid rescanning buffered durations.
Per-call and owned cancellation are checked before timing validation. The
generic zencodec trait remains millisecond-based and delegates through the
same path with timescale 1000; no trait change is claimed.

The native matrix covers two backends × four input formats × two coded
depths × four clocks: 64 files / 128 decoded frames. Clocks/durations are
30000:1001,2002; 1000000:1,7; u32::MAX:MAX,MAX; and 1000:20,30. It verifies
all media headers and stts durations, exact result totals, decoded frame
counts and alpha presence. SVT requires explicit 4:2:0 in this test; the first
attempt correctly rejected the default 4:4:4 setting, and the fixture was
corrected without changing its assertions or matrix.

The adapter matrix covers both backends, all four storage formats and both
coded depths: 16 files / 48 frames. Combining 1001/30000 seconds, 23ms, and
1001/60000 seconds produces exact ticks [2002,1380,1001] at 60000. Subsequent
clock and sample overflow requests fail without appending frames or changing
those durations. Zero clocks fail. Every complete output decodes.

`scripts/verify_animation_timing.py` independently decodes all 80 files / 176
frames with libavif, requiring exact integer timescales, durations and PTS,
expected dimensions and coded depth. All pass. Hashes are in the matching TSV;
raw files and detailed decoder output remain under
`~/tmp/slower-preset-probe/animation-timing-canonical/`.

Published-Git workspace validation passes 906/906 tests (nine existing skips).
All-feature workspace library and timing-test Clippy passes with warnings
denied. The final focused test includes the compatibility millisecond-total
assertion and passes. No floors, envelopes or skips changed. Commands use the
shared 16G/four-job wrapper with RUST_TEST_THREADS=4:

```sh
cargo test --all-features --test animation_encode_timing
cargo nextest run --workspace --all-features
cargo clippy --workspace --all-features --lib --test animation_encode_timing -- -D warnings
python3 scripts/verify_animation_timing.py "$HOME/tmp/slower-preset-probe/animation-timing-canonical" --manifest benchmarks/animation_timing_2026-09-07.tsv
```

Set ZENAVIF_TIMING_ARTIFACTS to export the complete fixture matrix. Logs:
`~/tmp/slower-preset-probe/animation-timing-canonical-*.log`.
The independent quality investigation still prevents wrapper main merges.
Broader animation color formats, auxiliary metadata, inter-frame hints and
video remain unfinished; exact timing does not complete the full objective.

The same two public tests also pass with `--no-default-features --features
encode` (zenravif only: 32 native and eight adapter configurations). The
published owner manifest and edited animation/lib sources match the locally
verified checkout byte-for-byte.
