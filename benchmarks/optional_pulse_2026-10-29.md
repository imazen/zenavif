# Optional Pulse and paired parser qualification — 2026-10-29

This is focused correctness and dependency qualification, not a performance
benchmark, total-memory bound, full-feature claim or release certification.

## Sources and graph

- Native source: `dee1a361a9902beea3fd1b84df64f6e80666c080`.
- Serializer fixture correction: `e5760d1c` (only the large-dimension test changed).
- Core: `feb09f9fd25e90e49a30d7161db8f3b558dfb2a6`.
- Pixels/converter: `283655c5424e0ad7ebc6d2acd94670fac1e29158`.

Incremental resolution started from the committed lock, outside parent Cargo
patches. The portable audit requires one core, pixel and Pulse family. It first
caught the serializer's old parser0.6 bringing in core0.1. Replacing that dev edge
with the paired workspace parser0.7 removed the split. All existing roundtrip
assertions and independent avif-parse/mp4parse oracle dependencies remain.

The strict audit retained its initial refusal pending review of explicit source
changes: UltraHDR10a→7e9, PNG0.1.4→0.2git7b6, zenflate0.3.6→0.4.1gita0,
rustix1.1.4→1.1.6 (PNG requires^1.1.5), and removal/disambiguation of the old
parser/core IDs. No other unreviewed changes remain. The tested lock is committed.

## Commands and results

Each command used a serialized3GiB/jobs2 run-heavy wrapper, a900-second command
timeout and a2GiB disk-floor check between gates. Native tests ran with the
`media-encode` feature; assertions and feature coverage were not removed.

```sh
cargo test --locked -p zenavif --features media-encode --test media_session --test sansio
cargo clippy --locked -p zenavif --features media-encode --lib --test media_session --test sansio -- -D warnings
cargo test --locked -p zenavif-serialize --lib
cargo clippy --locked -p zenavif-serialize --lib --tests -- -D warnings
```

- First run: native21+3 tests pass. Disk floor then stops before Clippy.
  Wrapper **rc1,140s,peakRSS1.18GiB,minavail13699MiB,peakload1.93**.
- After deleting inactive Cargo output: native strict Clippy passes; serializer
  runs100 passing tests and one failure at its70,000×70,000 fixture. The new
  parser's default512-megapixel budget refuses the header. Wrapper
  **rc101,43s,peakRSS0.86GiB,minavail14353MiB,peakload1.40**.
- Fixture-only correction preserves dimensions, saturating-width assertions and
  frame-count roundtrip. It adds default-budget refusal and explicitly admits
  4,900megapixels for the header-only parse; no raster is decoded or allocated.
  Serializer101/101 tests and strict Clippy pass. Wrapper
  **rc0,14s,peakRSS0.46GiB,minavail14613MiB,peakload0.78**.

The last run does not erase either earlier failure. Native source and lock are
unchanged by the fixture correction, so the passing native gates were not rerun.

Native regressions require None to retain owned timeout/cancellation in every
media operation and incremental decode, before rows or finish. Existing context,
picture admission and packet ownership controls remain. This does not add native
progress reporting or certify a whole-operation workspace bound.

Remaining: standalone downstream/fuzz graph qualification, generated API audit,
full feature/platform matrix and release gates.
