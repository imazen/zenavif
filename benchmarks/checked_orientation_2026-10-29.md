# Checked orientation representation

Native source d07d51618970185875df4b7bde69cf0d17724a98; portable manifest
candidate eec6128283c042d3e285a4fdf8ff8e07efde4b2e; shared core c6df5d5e,
paired zenpixels f23ac753. Core proposal: https://github.com/imazen/zencodec/pull/179.

An isolated export resolved four roots and ran the commands now in
`just checked-orientation`: three native tests pass, strict library/test Clippy
passes. Existing default-baking assertions are unchanged. The new controls cover
all eight stored orientations with padded input, typed and erased jobs, regular
and Send registry preparation, exact stored-sample/encoded-byte parity,
conflicting native overrides and typed animation-bake refusal.

Wrapper: run-heavy --mem 3G --jobs 2 -- timeout 900, rc0, 139s,
peak-RSS 0.99GiB, min-avail 10704MiB, peak-load 1.83. These are qualification
resource observations, not controlled compile-time or performance comparisons.

Lock audit: core-family source changes and paired pixels 0.2.17 -> 0.3.1;
unchanged packages only gain disambiguated zenpixels dependency labels. Root
resolution additionally retains registry zenpixels 0.2.16 through zenresize 0.3.1
under zensim-regress, a dev dependency. `cargo tree --locked -i zenpixels@0.2.16
--edges normal,build,dev --features encode-imazen` verifies that path. It does not
cross the production native/core pixel boundary, but complete development graph
unification remains open. No unrelated package version/source moved.

Same-codec parity is not independent reader conformance. Required animation
baking refuses; it is not implemented. Shared retained-plan orientation facts,
row/replay acquisition and memory bounds remain unqualified. The default still
behavior stays baked; default animation behavior stays carried.
