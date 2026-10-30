# Native MP4 timeline qualification — 2026-10-30

Remaining gates: portable git dependency alignment, independent edited-playback
inspection, and consumer adoption. These controls do not certify rendered pixel
parity, whole-operation memory bounds, or a release.

Native source `9cd12ba7a4d9cb2e45ef0939d7335bb2f5d79e1ec` against the core
archive SHA256 `9cda97a050adab7583961cae2b6fb98f4a89cdc2f864ec8964e9a2ce47c03e3e`
passed all-target tests: 6 unit and 32 integration tests, zero ignored/filtered.
Controls retain every edit, inspect raw movie-clock and edit records, distinguish
empty/unedited/open programs, preserve signed legacy rounding, and refuse
unsupported programs before header publication with clean retry. Packet fixtures
are opaque AV1 payloads; this is not independent video playback validation.

The first locked attempts refused unused patch-record ordering before compilation.
The first compilation exposed a new fixture treating `same_instant` as Result;
corrected to its boolean return. Runtime tests then passed, but strict Clippy
found a blank line after an attribute and a closure explicitly dropped solely to
end its borrow. Source `89d236001559c42ca0a75e34ac8e0adb414aa698` removes the blank
line and uses lexical scope. Library source is byte-identical to the runtime-tested
snapshot; assertions remain unchanged. Strict all-target Clippy and no-default
WASM check both pass on this snapshot.

`zenmp4/scripts/prepare-timeline-check.py` creates the isolated wrapper from
immutable native/core exports. It patches registry and git interface edges to
one canonical path per crate; it is explicitly not portable git-graph evidence.
The reused lock is unchanged. Run `just qualify-mp4-timeline` in an aligned
workspace; the isolated wrapper runs the same three Cargo commands.

Resource receipts (3G hard cap, jobs2, timeout900; not performance benchmarks):
- Runtime plus failing Clippy: rc101, 8s, peak RSS 0.25GiB,
  minimum available 14547MiB, peak load 0.16.
- Corrected Clippy plus WASM: rc0, 5s, peak RSS 0.25GiB,
  minimum available 14618MiB, peak load 0.19.

Full logs are retained under `~/tmp/native-timeline-20261030/` as `repair.log`
and `lint.log`, along with immutable source archives and earlier failure logs.

## Portable native graph

Source `8ca4b0a86d03f4f81cc02e572447671c398202c0`, with its resolved lock,
passes `just qualify-mp4-timeline`: all 38 tests (zero ignored/filtered),
strict all-target Clippy and no-default WASM. All five manifest roots now pin
core `41b1357e9bec50fb1c9508a310ba4b601605207b`. The root graph has one
copy of each shared interface; only zenresize's existing private edge retains
zenpixels 0.2.16. The lock adds the testkit's how-far-along/how-far-really and
critical-section dependencies, updates the shared family revisions and adds
core's libm edge. No unrelated existing package versions changed.

The updated dump example preserves the oracle's scalar `edit=` field and adds
a separate full `timeline=` field. The scalar is not a general edit executor.
Full portable run: rc0, 32s, peak RSS 0.32GiB, minimum available 14414MiB,
peak load 0.84; 3G/jobs2/timeout900. Log: `portable-check.log` alongside the
prior receipts. The supervisor and Rust processes were verified exited.

Standalone fuzz/downstream lock qualification, the broader AVIF package,
independent edited playback and the other shared container adapters remain
open. The API dependency is a review revision; this is a draft adoption.
