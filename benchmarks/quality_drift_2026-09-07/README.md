# Encoder quality-gate investigation — 2026-09-07

The persistent ladder failures are reproducible encoder-side changes, not
regressions introduced by the animation metadata/alpha integration. One real
quality defect is the wrapper's forced one-candidate intra-mode budget at speeds
9/10. The correction restores the preset's full mode search while retaining
CDEF, transform-domain choices, and the backend's explicit mode-budget facility.

## Baseline reconstruction and isolation

The envelope was introduced in canonical zenavif `ec2540b`, with a sibling
zenravif path dependency (its exact working revision was not captured) and
registry zenrav1e 0.1.4, whose packaged VCS revision is `30d37fcf`. Using the
July 5 owner `adb88ddc` with that registry backend reproduces **all 27 pinned
file sizes exactly**. This does not prove the unrecorded historical files had
identical contents. Scoring those files with the current decoder changes scores
by at most 0.077, within the original 0.5 tolerance.

The probe copies the gate's unchanged integer-only generators and uses its
8-bit, 4:2:0, quality, speed, threading and imazen feature configuration.
Decoder and fast-ssim2 0.8.2 stay fixed. The current owner `9585c67a`, backend
`60594682`, reproduces **all 33 non-timing canonical failure rows exactly**.
The standalone probe uses the canonical serializer for every arm. Its source
and output tables are preserved here; no gate threshold/envelope was changed.

Four complete 27-cell arms:

| Table | Owner | Backend | Preset changes |
|---|---|---|---|
| old.tsv | adb88ddc | registry 0.1.4 | Historical defaults |
| current.tsv | 9585c67a | 60594682 | Current defaults |
| unarmed.tsv | 9585c67a | 60594682 | Four dep-bump switches false, diagnostic only |
| backend.tsv | adb88ddc + default fields for API compatibility | 60594682 | Historical defaults |

The four diagnostic switches are S1_DEEP_ARMS_LIVE, S6_TX_SIZE_RDO_LIVE,
S6_PART_PRUNE_LIVE and S10_RETIER_LIVE. SMALL_PX_RDO_TX_LIVE remains enabled;
S6_INTRA7_LIVE was already false. **No production switch was disabled.**
Every complete AVIF in backend.tsv is byte-identical to its unarmed.tsv
counterpart (27/27), separating backend changes from the four preset changes.
The backend-only arm retains substantial speed-2 cost/quality changes.

For the speed-2 photo/q35 witness, complete AVIF SHA-256 is identical across
all tested revisions:
`95bad5b24c73025f8bbef2abb036654bdd024c9870b68da205f79445dd516f95`.
Recorded one-pass encode times (not stable latency bounds): registry baseline
~612 ms, dc0a1165 1102 ms, its child 6b3b0493 1952 ms, 4563cc5b 2707 ms,
its child b073182c 2911 ms, current backend ~3022 ms. The immediate-parent
comparisons establish added cost from removing the transform-type first-trial
exit and from deeper SPLIT estimation. They do not justify reverting those
search/correctness changes: other cells select different winners. Other
intervening changes also contribute; the timing drift is not fully corrected.

## Mode-budget correction

For speed-10 photo/q35, current defaults produce **1165 B / SSIM2 53.312**.
Removing only the one-candidate cap produces **1016 B / 59.913**. This preserves
the other re-tiered settings. `fullmode.tsv` covers nine synthetic speed-10
cells: eight improve both size and score; screen/q35 trades slightly less size
for a 0.109 score loss. Time increases. The original pre-bump photo reference
was 1064 B / 57.477.

The new consumer regression `tests/fast_mode_budget.rs` uses the original
quality lower bound and size upper bound, including the unchanged 0.5 / 2%
tolerances. With matching encode-imazen + encode-threading features it fails
before correction at **53.3124898991**. An earlier encode-only run lacked the
original imazen configuration; its 52.568 score is not the baseline comparison.
The test is gated on encode-imazen to match its contract. Improvements are
allowed; no existing assertion or benchmark envelope is weakened.

## Corpus verification

All 25 local GB82 photo PNGs and 10 GB82-SC screen PNGs are used, not a selected
winning subset. `corpus-sources.tsv` records original file hashes. Inputs are
converted to 8-bit opaque RGB (explicit white composite for alpha), resized to
at most 512 on the long edge using the image crate's triangle filter, and
encoded at q5/15/25/35/50/65/80/90/95, 4:2:0, 8 bits. This is an SDR,
size-bounded comparison, not HDR, alpha, or full-resolution evidence.
All normalized source pixels are byte-identical between arms; their hashes are
in the comparison table. Both arms use the same current decoder and metric.

At speed 10, 315 cells per arm yield **35/35 improved rate curves**, median
**-3.332%**, mean -3.461%, range -8.635% to -0.630%. The median paired encoding
time ratio is 1.233 (one pass, indicative only). These are piecewise-linear
log-rate integrals on per-image Pareto fronts, over their common SSIM2 range
clipped to [30,90]. This is not a cubic/PCHIP BD-rate estimator; the precise
method is in `analyze_curves.py`. No image lacks an overlapping range.
See corpus-current.tsv, corpus-fullmode.tsv and corpus-comparison.tsv.
Speed 9 also improves all 35 curves: median -5.202%, mean -5.120%, range
-8.809% to -2.044%, with median paired encoding-time ratio 1.493. These are
another 315 cells per arm, with identical normalized source pixels. See the
corpus9-* tables. The extra search cost at speed 9 is substantial and explicit.

Independent libavif 1.3.0 successfully decodes **751 files** from the four
27-cell arms, four parent/child single-cell probes, nine mode-budget synthetic
cells and both 315-cell speed-10 corpus arms. File hashes are in
reference-files.tsv. This is independent decoder acceptance, not an independent
SSIM metric or encoder-reconstruction comparison. Another 630 files from both
speed-9 arms independently decode successfully (reference-speed9-files.tsv),
for 1381 total checked files. No decode failures or silently omitted cells.

## Reproduction and pending work

The harness lives at `~/tmp/quality-drift-probe` during this session. Copy
Cargo.toml.template to a separate Cargo.toml, substitute @OWNER@ with the
selected owner's ravif directory and @ZENAVIF@ with the canonical root, and
copy probe.rs to src/main.rs. `probe.rs`
is its complete main source. Dependencies: zenravif at the selected owner path
(no defaults, features threading/imazen/stop), canonical zenavif at abcd16c8
for decode, imgref 1.12, rgb 0.8, fast-ssim2 0.8.2 (imgref/rayon), image
0.25.10 (PNG only). Patch zenavif-serialize from crates.io and the canonical
Git source to the same canonical workspace member. Use release builds and the
shared run-heavy wrapper (16G / four jobs), serially. Capture Cargo.lock for
each arm; the session's locks and full terminal logs remain beside the harness.

`probe OUTPUT_DIR` runs the 27-cell synthetic ladder by default. PROBE_CELL
selects one named cell; PROBE_SPEED selects an explicit speed. PROBE_MANIFEST
is a tab-separated name/path list; PROBE_CURVE=1 selects the nine-quality
curve. Every source and encoded file is written under OUTPUT_DIR. The corpus
manifest contains every PNG in the two named groups, sorted by filename.
Run `analyze_curves.py ROOT corpus 10` on the captured logs/source files;
`check_reference.py` verifies the corresponding explicit file counts with
avifdec. These tools do not repin or modify product tests.

The older gate-flip RD claims in cavif-rs were invalidated by an ARM decoder
bug; see fa877406 and gate_flip_summary_2026-08-06.tsv.meta. That history was a
reason to re-measure, not evidence for dismissing today's x86 failures.
The remaining speed-6/7/8 inversion and broader backend quality/timing drift
still require work. Main merge and CI remain deferred; the user authorized
merging once ready, then reading every open GitHub issue to reconcile gaps.

The matching-feature regression passes after correction (`fast-mode-after.log`).
Owner all-feature tests/doctests pass 86 with five existing ignored doctests.
Local-source integration passes 898/898 (nine existing skips). Owner all-target
clippy and canonical all-feature library clippy pass with warnings denied.
Determinism passes 25 legs; conformance passes 56 AVIF cells, with the optional
armed CLI leg explicitly unrun. The shell conformance gate used a process-local
Cargo wrapper to pass the source override to its builtin Cargo commands; no
user Cargo configuration was modified.

Final ladder: 52 failures, comprising 32 byte/quality changes and 20 timing
misses. Its symmetric bounds flag the corrected photo for being smaller and
higher-scoring too; failure count is not a count of quality regressions.
Monotone now has six inversions: the existing screen/q80 speed-6 cases versus
7/8, plus photo/q40 speeds 6/7/8 and photo/q80 speed 5 dominated by improved
speed 10. Slower-preset gaps remain unresolved. No threshold/envelope changed.

The owner correction is published as cavif-rs
176ad8ee5686fcadff36d1a28478d7c61c3f9442 on animation-avif-complete. This
consumer pins that Git revision. The fetched manifest and eight Rust source
files are byte-identical to the locally verified package; Cargo.lock changes
only that dependency source revision. Published-Git all-feature nextest also
passes 898/898 (nine existing skips), and all-feature library clippy passes
with warnings denied, without local owner overrides. Logs: nextest-git.log and
clippy-git.log under the session harness. Review pushes are authorized; main
merge and CI remain deferred.
