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
