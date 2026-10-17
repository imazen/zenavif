# Exact animation timing, admission, cancellation, and 12-bit native encoding

Validated with the portable Git dependencies in Cargo.lock and zenpixels 0.3.1.
The pre-change checkout was b5be33bd244e48187dd4a0b3a1605876109b0640.
No benchmark baseline or tolerance was changed.

| Check | Result |
|---|---|
| Release workspace tests, `encode-imazen,encode-threading` | 687 passed, 11 existing ignored |
| All-target Clippy for those features, warnings denied | passed |
| Rust 1.93 check for those features | passed |
| Three public API snapshot generators | passed |
| Determinism | 25 legs, no failures |
| Reference decoder conformance | 56 AVIF cells plus 4 armed-tool cells; no failures |
| Rate–distortion ladder | baseline 60 failures; after 61; all 35 size/quality failures identical; timing failures 25 → 26 |
| Speed monotonicity | same six inversions before and after |

The ladder and monotonicity gates remain **failing**, not waived or rebaselined.
The metric owner stays at its original revision plus the manifest-only zensim
PR 63; updating the metric implementation during a comparison would invalidate it.
The reference decoder is the existing libaom `aomdec`; armed encodes use the
zenrav1e CLI pinned in the coordinated series.

Targeted coverage includes exact rational duration field bounds and mixed clocks,
atomic rejection before canvas/format/clock admission, retained memory and total
pixel/duration limits, job and per-call cancellation, source ICC/CICP preservation,
60 animation color/depth/storage combinations, and lossless 12-bit RGB/alpha
low-code reconstruction. The broader native encoder source tests live in
cavif-rs PR 7 and zenrav1e PR 44.

The current adapter still buffers animation frames and the encoded container.
Borrowed stop tokens reach native preparation and packet boundaries; set the
owned job token for deep codec-kernel cancellation. No global allocator hard cap
or forward-only AVIF muxing is claimed. External codec-corpus tests were explicitly
excluded with `ZENAVIF_NO_CODEC_CORPUS=1`; checked-in and locally installed parser
vectors were exercised. Optional AOM/SVT feature suites are separate checks.
