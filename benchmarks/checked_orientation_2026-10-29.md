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
baking refuses; it is not implemented. The follow-up below qualifies shared retained-plan orientation facts.
Row/replay acquisition and memory bounds remain unqualified. The default still
behavior stays baked; default animation behavior stays carried.

## Retained planning follow-up

`plan_encode` and `begin_declared` share validation and report whole-raster input
plus applied/carried orientation. Startup preserves retained dimensions and
pixel descriptor; encoding rejects later geometry, descriptor or color changes.
Unknown native transform overrides remain unknown facts. Canonical RGB8/RGBA8
sRGB is supported; explicit color contexts, exact-depth assertions and other
layouts refuse during preflight. These refusals do not poison startup.

Eight native tests cover plan/start agreement and ordinary-byte parity for every
orientation and both representations, the existing stored/baked regressions,
changed declarations, unsupported declarations, typed resource-limit errors,
repeated startup, exact-depth and late color-context refusal. The original pixel
and EXIF/XMP assertions remain unchanged. No native allocation bound is claimed.

Portable git graph: core ab133d2c, pixels f23ac753; four roots resolved,
eight tests and strict library/test Clippy pass. Wrapper rc0/41s,
peak RSS 0.45GiB, minimum available 11327MiB. Lock identity deltas are
12/4/8/6 records respectively (old plus new core-family identities); no unrelated
package version/source changed. The initial media pixel split failure is retained;
matching shared pixel floors repairs it without excluding media or tests.
