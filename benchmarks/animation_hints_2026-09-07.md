# Animation quantizer-map integration — 2026-09-07

Owner pin: `5b7c50b95bc5071bf29a46b0b5e3a6b2801f07d1`.
Previous pin: `ecbf41ec42f05568e0f36074edc7f946ebdb12bf`.

The canonical builder already forwarded the configured superblock quantizer
map to the owner, but its animation frame submissions discarded it. The owner
now attaches the map to every color frame and leaves alpha parameters at their
defaults. Backend application remains limited to non-lossless intra frames.
The builder supplies one shared map, not a different map per frame; inter
hint support and per-frame map APIs remain unfinished.

`tests/animation_encode_hints.rs` tests no map, six neutral scales and six
alternating 0.5/2.0 scales through two-frame RGBA animation at both coding
depths. It decodes every complete file, requires neutral-byte identity,
non-neutral output changes, and unchanged alpha pixels. Against the previous
owner it fails because the non-neutral map is ignored at Eight. The corrected
published pin passes all six files / twelve decoded frames.

The owner has a separate raw-packet test and libaom verifier. Twelve streams /
24 frames decode at the expected dimensions/depth; neutral maps preserve
pixels, alpha pixels remain identical and hinted color pixels differ at
both depths. Both color packets change, which does not itself establish
inter hint support because changed reference frames can affect later packets.
Owner all-feature tests pass 97 (five existing ignored doctests), no-default
passes 58 (four existing ignored doctests), and all-target Clippy passes.
See its `benchmarks/animation_hints_2026-09-07.md` and matching hash TSV.

Published owner manifest and both edited source files match the locally
verified owner byte-for-byte. Logs: `~/tmp/slower-preset-probe/animation-hints-*.log`.
All heavy jobs run serialized with the shared 16G/four-job wrapper and
RUST_TEST_THREADS=4. No quality floor, envelope or test skip was changed.
The independent quality investigation still prevents a wrapper main merge.

Published-Git workspace validation passes 904/904 tests (nine existing skips)
and all-feature workspace library Clippy with warnings denied. Commands:

```sh
cargo test --all-features --test animation_encode_hints
cargo nextest run --workspace --all-features
cargo clippy --workspace --all-features --lib -- -D warnings
```
