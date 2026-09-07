# Public animation color-format wiring

The public encoder already forwarded color model, chroma subsampling and
pixel range into zenravif, but its animation implementation ignored them.
Pinning owner `7f55b540` connects those settings through pixel preparation,
codec contexts, sequence headers and container signaling. Its backend
dependency `1447c200` also fixes forced 4x4 mono/444 inter partitions and
partial chroma-edge distortion. Both revisions are published Git commits;
no local dependency patch is needed.

`tests/animation_encode_color.rs` reproduces the old behavior with the
previous owner pin: the limited-range request reads back as full range.
It passes with the new dependency across 40 two-frame files:

- RGB8, RGBA8, full-range RGB16 and RGBA16 input storage;
- explicit 8-bit and 10-bit coding, including promotion and narrowing;
- YCbCr 420/full, 420/limited, 444/full, 444/limited and RGB identity/full.

The test checks profile, coded depth, subsampling flags, both poster and
track color metadata, alpha presence, timing totals, and managed decoding.
The second frame changes every RGB channel so inter coding is exercised.
All files use a 30000 timescale and two 1001-tick samples.

`scripts/verify_animation_color.py` checks all 40 files with libavif and
decodes their color streams with libaom. All eight RGB streams match the
expected G/B/R samples after the public API's documented depth conversion.
All 20 alpha streams also match their coded source planes exactly. This
does not claim that YCbCr conversion or 16-to-8/10-bit narrowing preserves
the original RGB precision. Native depth conversion is independently
represented in the expected plane files, rather than taken from encoder
reconstruction.

The manifest `animation_color_2026-09-07.tsv` records all file and decoded
plane hashes. Raw artifacts and logs are under
`~/tmp/slower-preset-probe/animation-color-canonical*`.

Default animation bytes change because the configured default 4:4:4 is
now honored. Explicit 420/full retains its previous conversion arithmetic;
the backend correction can change edge decisions. The backend independently
verified 144 lossless configurations with rav1d-safe and libaom, passed its
54-cell reconstruction gate, and updated exactly three intentional
odd-dimension fingerprints before passing all 81 identity cells and 360
toggle cells. Its full suite passed 229 tests with six existing ignored
doctests. See zenrav1e's `benchmarks/sub8_inter_2026-09-07.md`.

The all-feature workspace suite passes 925 tests including doctests, with
13 existing ignored tests/doctests. Library and color-test clippy pass.
The color regression also passes with no default features and only
`encode-imazen` enabled. The separate wrapper quality investigation still
prevents a main merge; this pin does not change quality floors or
speed-ladder expectations.
