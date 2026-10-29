# AV1 session contract gaps

The current PR75 source migration accepts `Option<&dyn zencodec::Pulse>` for
AV1 session calls, codec incremental steps and MP4 container work, matching
[zencodec `a7c2a6ca`](https://github.com/imazen/zencodec/commit/a7c2a6ca).
`None` adds no per-call control. Existing owned stops remain active, and the
incremental wrapper forwards absence to its job-owning helper. Native progress
reporting is unchanged. These source changes have not been built or run.

The committed root, fuzz and downstream locks still contain zencodec at both
`f73a7e0e` and `7cc3d3ec`: ultrahdr-core `10a79c3a` and its zenmeta retain the
latter git edges. The MP4-fuzz lock has only the former. Manifests, locks and
pixel pins remain unchanged by this source migration; matching core/media/IO/
testkit pins and a unified intermediary graph are prerequisites for
qualification. The current pixel pin is `1d3ddd21`; the target core pairs with
`283655c5`. The dated findings below describe their original revisions.

This records unresolved findings from the post-landing review on 2026-10-18.
The reviewed implementation is main commit `d235ae23`. It is not an assertion
that the whole media contract is implemented.

## Limits and admission

The AV1 adapter does not expose shared `MediaLimits` or `ResourceLimits`.
`src/media.rs` concatenates configuration bytes with packet payload before
sending them to rav1d, and `copy_and_send` allocates encoder frames without
shared resource-limit checks. The decoder's existing `with_settings` can set
rav1d's pixel limit; that is separate from shared limit plumbing.

The configured queue cap is also a soft pull trigger when zenrav1e needs more
lookahead. See [the measured queue audit](media-queue-contract.md) for exact
overruns, its executable probe, and the pending capacity-policy decision.

## Trim configuration

The reviewed shared `CodecConfig` in `zencodec-media/src/session.rs` carries
codec bytes, epoch, clock and declared video shape. Its conversion from
`TrackInfo` omits codec delay, seek pre-roll and edit delay. The decoder cannot
apply those track-level trims through its current configure interface.
Unsupported packet discard is refused before consumption. Applying trims
exactly once requires a shared configuration decision.

## Picture identity and packet metadata

`Av1VideoDecoder::frame` wraps every presentation in a new `Av1Picture` Arc;
it does not intern pictures or call `Frame::represent`. The pinned rav1d
managed frame exposes presentation flags and planes but no stable picture
identity/reference-slot interface used by this adapter. Shared interpretation
Arcs do not establish shared picture identity.

That function also retains only offset-map ordinals greater than the output
ordinal. It removes the current packet's input offset after the first output,
although one packet may produce multiple presentations. Correct retirement
must preserve metadata for all applicable outputs while bounding its lifetime.
This is a source finding; a real-stream reproduction has not been measured.

## Epochs and error categories

Frame interpretations snapshot the configured epoch. In-band sequence changes
do not independently advance it. The current shared contract states that
`configure` starts an epoch; deciding whether in-band changes also advance it
requires an explicit interpretation of that contract.

The MP4 muxer's unsupported codec/time-base refusals use `SessionError::Codec`.
`zenmp4/tests/mp4.rs::what_mp4_cannot_carry_is_refused` requires those variants.
Changing them to `SessionError::Unsupported` changes existing expectations and
needs a contract decision first.

## Dependency alignment

The reviewed root and consumer graph retain zencodec revision `7cc3d3ec`.
Upgrading only the root patches leaves git-pinned intermediaries using another
zencodec revision and can split `GainMapParams` into incompatible types. Align
the intermediary graph before moving the pin; verify one zencodec and one
enough in each root, fuzz, downstream and MP4-fuzz lockfile.

With root pins moved to zencodec `d327434b0826a3e39633665c76f5081c0ceb34cc`
and ultrahdr `cf20843d792222be23e4ccef417a0d59aeccd064`, the locked
`cargo check --features media-encode,zenav1-svt --locked` on 2026-10-18 fails
with E0308 at `src/codec/gain_map.rs:99`: ultrahdr expects the `GainMapParams`
from zencodec `7cc3d3ec`, while the caller supplies the newer crate's type.
That trial resolves two zencodec copies and one enough. Those pins were not
landed; the root/consumer locks retain their single-copy graph.

A root Git-source patch of `zenmeta` to that same repository URL at the newer
revision is also rejected by Cargo because the patch points to the same source.
Aligning the intermediary pin, or deliberately choosing a different-source
override, is required; registry patches alone do not replace this Git edge.
See [Cargo's dependency override reference](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html)
for the workspace-root patch mechanism.

The owned cancellation builders are proposed separately in
[PR 71](https://github.com/imazen/zenavif/pull/71). That proposal does not resolve
the queue, trim, picture-identity or shared-limit decisions above.

## Accepted-packet context (2026-10-25)

The historical offset finding above now has native reproductions and an
implementation. Against main `a7638f6e` with rav1d-safe `dfd01e32`, a queued
frame accepted at 40 ticks on the 1/1000 clock was returned on a subsequently
configured 1/90000 clock. Combining three temporal units in one packet also
lost the packet's input offset after the first presentation. See
[issue 73](https://github.com/imazen/zenavif/issues/73).

The adapter now snapshots the accepted `CodecConfig`, ordinal and input offset
in an immutable native packet context. [rav1d-safe PR 537](https://github.com/imazen/rav1d-safe/pull/537)
retains that owner through reference pictures, delayed output and presentations.
The adapter no longer guesses metadata retirement from output ordinals. A
configuration change cannot mutate an accepted frame's interpretation, and
identical ordinals from different epochs cannot replace one another's metadata.
The context has the native reference lifetime; end-of-input alone does not
release reference pictures. Reset/drop and the last retained frame determine
release. This does not establish shared presentation-picture identity.

`just test-media-context` exercises native 8/10-bit samples, one and multiple
decoder threads, alternating clocks and container colors, repeated ordinals,
multiple presentations in one packet and frames retained after reset/drop.
The expected samples are the original lossless encoder input. The native
context's delayed/reordered/show-existing and film-grain ownership checks live
with rav1d-safe; they also run in checked and untracked storage modes.

Native decode errors previously cloned only the underlying enum and discarded
its `whereat` trace. `DecodeTraced` retains the original traced error in an Arc;
`Error::source` exposes it through both typed and erased media sessions. The
old `Decode` constructor remains source-compatible. `just test-media-errors`
feeds malformed native input, checks the original error type and locations,
and verifies that cloning the adapter error retains the trace.

The shared zencodec pin remains `7cc3d3ec`; this change does not require the
checked-operation proposal. The native decoder pin is `5707a2e9`. These fixes
do not implement policy suspension, in-band epoch numbering, container-specific
color conflict checks, track trims, hard queue admission or shared native
workspace bounds.

At `ae51b7e1`, the normal root and fuzz graphs each resolve one zencodec, one
enough and one rav1d-safe. The full native session regressions, decoder unit
checks, strict lib/test Clippy, fuzz-target compilation and API snapshots pass.
The FFmpeg oracle also passes all of its comparisons: lossless encoder output
against the source samples, lossy zenrav1e and zenav1-svt output against
libdav1d/libaom, and libaom input against libdav1d samples and ffprobe timestamps.
The tested layouts cover 8/10/12-bit, gray, 4:2:0, 4:2:2 and 4:4:4 where the
respective encoder supports them. [Recorded evidence](../benchmarks/media_packet_context_2026-10-25.txt)
includes commands, revision, output inventory hash and the resource-wrapper
receipt: rc=0, 135 s, peak RSS 0.99 GiB, minimum available 13935 MiB, peak load
3.75. This is build/test-process usage, not a native workspace or speed claim.

API review note: the new traced error variant retains the original `At`;
`Av1SessionError` therefore no longer automatically implements `UnwindSafe` or
`RefUnwindSafe`. Its existing Clone/Eq traits remain. The media API is still
unpublished. API snapshots also move the existing media feature surface out of
the internal-feature snapshot and record current rustdoc auto-trait spelling.

## Native picture-plane admission (2026-10-25)

The native pin `b19aec65` adds `rav1d_safe::Settings::max_picture_bytes`,
available through the existing `Av1VideoDecoder::with_settings` constructor.
It reserves the full requested native plane allocation before the first plane
is allocated, including row padding and alignment. References, reconstruction,
film-grain copies and frames held by the caller share that allowance. A charge
survives reset or decoder destruction until the last native owner releases it.
Bounded mode frees plane buffers instead of returning them to the uncharged
cache. The default `None` retains the existing pooled path.

This setting does not establish shared `MediaLimits`, a bound on codec
workspace, input concatenation, metadata, encoder lookahead or process RSS.
The pixel-count setting remains independently available. A publication policy
callback still runs after reconstruction; a caller needing this native storage
requirement sets it before opening the session.

The adapter's native regression exercises refusal on one and four workers,
requires that no `Frame` is returned, and inspects the original
`whereat::At<rav1d_safe::Error>` in the source chain. Refusal currently uses the
native decoder's existing `Error::Other` category; it does not introduce a
structured shared resource-limit error. Native boundary, retained-owner,
film-grain and independent pixel-oracle gates live in
[rav1d-safe's admission contract](https://github.com/imazen/rav1d-safe/blob/b19aec6561204ff310250bf2bc1b2c6faa42c3b5/docs/PICTURE_STORAGE_LIMIT.md).

The normal dependency export passes the native session tests, decoder unit
checks, strict lib/test Clippy, fuzz-target compilation and all three generated
API snapshots. Root and fuzz locks each contain one rav1d-safe, one zencodec
and one enough. [Validation receipt](../benchmarks/media_picture_budget_2026-10-25.txt):
run-heavy rc=0, 220 s, peak RSS 1.17 GiB, minimum available 14093 MiB,
peak load 3.58. This is build/test-process usage, not a decoder RAM bound.

### Downstream dependency resolution, 2026-10-25

The native picture-budget pin also requires refreshing the independent
`dev/downstream-probe/Cargo.lock`. CI caught the omitted update under `--locked`.
`media-context-resolve` and `qualify-media-context` now include that consumer,
so the standalone graph and its runtime probe remain part of native pin changes.
The repaired graph passed `just resolve-downstream && just check-downstream`
outside workspace patches: run-heavy rc=0, 12 s, peak RSS 1.12 GiB, minimum
available 15067 MiB, peak load 1.64. No test expectation changed.
