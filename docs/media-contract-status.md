# AV1 session contract gaps

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
