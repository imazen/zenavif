# AV1 encoder queue contract under review

`Av1EncodeSettings::max_queued_frames` currently triggers an attempt to pull
output. When zenrav1e needs lookahead, the adapter accepts another input frame.
It therefore does not enforce this setting as a hard admitted-frame bound.

Run `just audit-media-queue` to reproduce the admission measurement. The dev-only
probe sends constant-128, 32×32 U8 YUV420 frames, one encoder thread, 40 ms
presentation intervals and durations, and otherwise default encoder settings.
It does not receive until the first `ReceivePending`, then verifies output is
available. At zenrav1e revision `9057c4bacf1d8a23974e32de09fd4446a1cc525a`,
the measurements on 2026-10-18 were:

| Speed | Configured cap | RDO lookahead | Accepted before first backpressure | Excess over cap |
|---|---|---|---|---|
| 10 | 1 | 10 | 19 | 18 |
| 10 | 3 | 10 | 19 | 16 |
| 6 | 2 | 20 | 27 | 25 |
| 6 | 64 | 20 | 64 | 0 |

These counts describe this fixture, not a universal capacity formula. The backend
requires `rdo_lookahead_frames + 1` ready coded-frame invariants while input length
is unknown; separate input, keyframe-analysis and reorder lookahead also apply.
RDO lookahead alone cannot establish the required input capacity.

Existing session tests require small queues to construct successfully and make
progress. Preserve those expectations until the contract is decided. The
recommended resolution is to derive the backend-required capacity, refuse
undersized queues at construction, and enforce admission against both that
capacity and shared media/resource limits. Allowing small values as soft triggers
instead requires an explicit distinction from a hard queue bound. This review
changes neither policy nor existing session-test expectations.

## Current paired graph and caller spelling (2026-10-29)

The unchanged audit on source122b1474 with corefeb09f9f/pixels283655c5
reproduces all four counts above. Receipt: rc0,4s,peak RSS0.37GiB,minimum
available10664MiB,peak load0.96. This is an admission count, not a memory bound.

Use `Av1EncodeSettings::with_output_pull_threshold` for this soft output trigger.
The misleading `with_max_queued_frames` builder is deprecated immediately and
delegates unchanged; the public `max_queued_frames` field remains its compatibility
storage. Native session tests retain small-threshold progress, and one explicitly
retains the old builder. A separate hard admission bound still requires native
capacity evidence and pre-effect refusal; this rename does not provide it.

The replacement builder is qualified on source0c6a7507: the four-case queue
audit preserves the counts above; all21 native media-session tests pass with
zero ignored/filtered tests, and strict downstream queue Clippy passes.
Receipt: rc0,29s,peak RSS0.87GiB,minimum available10371MiB,peak load1.11.
The existing default-speed test still exercises the deprecated builder; other
queue matrix cases and the downstream probe use the new spelling.
