# Canonical workspace verification — 2026-09-07

The real workspace now resolves its sibling dependencies. The missing
`../zenanalyze` checkout was cloned at `b102fa5f`; no dependency source was
modified. `cargo check --workspace --all-targets` passes.

`cargo test --workspace --no-fail-fast -- --test-threads=4` initially recorded
466 passes, six failures and ten existing ignores. All six failures came from
the missing `weld_sato_12B_8B_q0.avif` fixture in `negotiation_matrix`.
After restoring that fixture, the entire target passes 10/10. Combined with
the unaffected targets, this accounts for 472 passing tests and ten existing
ignores, with no unresolved default-workspace failures. This includes the
seven animation timing/count/HDR integration tests in the actual workspace,
136 parser public tests and 89 serializer unit tests. No assertions or test
selection were changed.
The vector downloader now also checks for the 12-bit fixture before treating
an existing libavif corpus as complete; its old gain-map-only guard accepted
the incomplete 1.3.0 corpus.

Corpus provisioning preserved existing files:

- AOM corpus at the submodule pin `bf4c18d1f3971069b75e87d6ee469790589f4f09`.
- link-u corpus at `c666a368b73006246694919b5dbcc078317af6cc`, also copied to
  the root test-vector directory (156 missing files).
- 49 missing libavif fixtures from the previously verified libavif 1.3.0
  source tree.
- The newer 12-bit fixture from
  `https://raw.githubusercontent.com/AOMediaCodec/libavif/main/tests/data/weld_sato_12B_8B_q0.avif`,
  SHA-256 `fa41d615d244d50fc99d71c1fea14561e0814382a748d1b8b672c3fd5a595dbe`.

The first all-feature/all-target check exposed two non-exhaustive bit-depth
matches in `examples/encode_sweep.rs`. The correction combines depth mapping
and output labeling and returns a descriptive error for unsupported variants;
8-bit, 10-bit and automatic mappings are preserved. The CLI continues to
accept only those three modes, matching the zenravif backend's capabilities.
`cargo check --workspace --all-targets --all-features` now passes, including
the legacy assembly decoder and alternate backends. Existing development
example warnings remain. This checks the manifest's pinned backend revisions;
it does not substitute the unpublished SVT working copy for the older SVT pin.

Logs are in `~/tmp/animation-metadata/`: `canonical-workspace-check.log`,
`canonical-workspace-test.log`, `canonical-negotiation-restored.log` and
`canonical-all-features-check.log` (before) and
`canonical-all-features-check-fixed.log` (after). Optional-feature runtime tests remain
outstanding. No CI or push was run. The main encoder's temporary serializer
path dependency still needs a verified canonical git pin before landing.
