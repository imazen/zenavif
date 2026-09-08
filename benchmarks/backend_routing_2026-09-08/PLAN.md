# Backend-owned still-image routing and comparative calibration

User scope: preserve useful SVT extensions, route reasonable still features
outside pinned C SVT's envelope through zenrav1e or zenav1-aom, and route
performance-dominated regions elsewhere using backend-reported suitability.
Run distributed experiments through zenfleet, not a separate scheduler.

Implementation progress: zenmetrics now has `benchmarks/av1-compare`, a single
statically linked Linux x86-64 executable with public-API C wrappers and all
three standalone Rust encoder arms. All five pass independent libaom decode
in the initial 8-bit I420 smoke envelope. API lifecycle timing and binary/source
hashes are recorded. Its DesiredJob adapter, wider-format arms and real-source
calibration are still pending; it does not yet replace the existing fleet
executor's oracle/replay path or supply backend optimality estimates.

## Contract

1. Each backend owns a side-effect-free configuration support query and its
   performance/quality evidence. Query dimensions, coded precision, chroma,
   monochrome/alpha requirements, lossless, requested tools and speed/quality
   preference. The production validator and query must share predicates.
2. zenavif intersects the backend report with the actual wrapper's ability to
   prepare pixels, preserve metadata, code auxiliary planes and mux the result.
   Backend support alone is not wrapper support.
3. Hard requirements are masks, never score penalties. In particular, never
   silently change chroma, precision, color model, range, alpha, lossless or
   requested coding tools to make a backend fit.
4. Automatic routing is an explicit config mode; an explicitly named backend
   retains its current contract. The selected plan reports candidate refusals,
   chosen backend/settings, estimate basis and uncertainty. Unsupported input
   must error before allocation/encoding; encoder failures are not fallback
   opportunities to hide defects.
5. Rank eligible candidates using the queried speed/quality balance and
   calibrated, matched-quality rate/time observations. Unknown estimates remain
   unknown. Preserve a supported established default in an unmeasured region;
   route required formats to a supported alternative without claiming a
   performance win. Do not use equal speed-dial numbers as equal work.
6. Reuse the existing backend tuner and resolved-plan plumbing. Its existing
   `cell_is_viable` only filters availability and AOM alpha: extend validation
   through the new queries rather than create a contradictory second map.

## Initial useful coverage

- Keep SVT's 8/10-bit monochrome/alpha, partial dimensions and lossless paths.
- zenrav1e/zenravif already supplies useful 4:4:4 and RGB identity; route those
  requests there when SVT cannot satisfy them.
- Standalone zenav1-aom accepts mono/420/422/444 at 8/10/12 bits. Its current
  zenavif seam exposes only 420 limited-range RGB at all three depths and
  8-bit monochrome. 444, alpha and full-range support need real consumer wiring,
  not a capability flag change. RGB identity additionally needs truthful AV1
  color signaling: its sequence header currently pins limited range.
- Keep still routing separate from the incomplete public SVT video API.

## Calibration protocol

Use distinct names: `libaom`, `zenav1-aom`, `zenav1-svt`, `zenrav1e` (with
wrapper revision separately recorded). The existing zenmetrics `aom-rs` path
runs a C bootstrap/oracle and replay; it cannot supply standalone Rust timing.
Replace that experiment path with the standalone key-frame API and time C
independently via the existing FFI reference. Preserve old job identities;
new protocol/backend-build identity must not reuse old cached work.

- First smoke-test one photo and one screen/text source through all four
  encoders, checking produced format and independent decode.
- Then use a declared stratified CID22/CLIC/screen subset at thumbnail,
  approximately 1 MP, and native sizes, with explicit source hashes.
- Sweep each backend's useful native effort range and a quality ladder; use
  SSIMULACRA2 plus Butteraugli/zensim corroboration, not PSNR. Separate HDR,
  high-depth, alpha and RGB-lossless measurements from 8-bit opaque SDR.
- Time in-memory APIs, with conversion/muxing costs reported separately from
  codec-only cost. Interleave backend arms on the same worker, repeat process
  starts, control thread count and record CPU/ISA/compiler/build revisions.
  Do not compare total subprocess or oracle+replay duration as encoder time.
- Use zenfleet's content-addressed jobs, claims, retries and ledger. Persist
  encode timing in the job result alongside the encoded artifact; score saved
  files separately so metric work does not contaminate encode timing.
- Derive Pareto fronts and matched-quality rate/time comparisons on held-out
  sources. Backend-owned reports include calibration revision, metric, tested
  source/size/format envelope and uncertainty. No extrapolation disguised as
  measurement; no quality-baseline relaxation.

## Access/build findings

This host has no installed zenfleet controller/worker or zenmetrics binary.
The configured SSH identities on two existing development/fleet hosts reject
this session's key, but the existing Nomad controller is reachable and reports
six ready, eligible nodes and no registered jobs. SSH is therefore not the
current dispatch dependency. Canonical zenmetrics
source was cloned at `168bd59b` for preparation. A normal workspace metadata
query fails on absent sibling `heic`; do not treat that as a completed build.
LAN/R2 credential files exist; use the canonical store resolver and existing
fleet tools without exposing credentials. No sweep has been dispatched yet.

## Fleet workflow verified against repository documentation

zenfleet is maintained in the zenmetrics workspace (`crates/zenfleet-*`). Read
`docs/RUNNING_JOBS.md`, `docs/PLAN_SWEEPS.md`, `scripts/jobsys/README.md`,
`docs/status/fleet-orchestration-2026-08.md`, and `fleet-tools.json` there;
site deployment details stay in the private homefleet repository.

- Nomad owns LAN worker lifecycle only. Declare cells through zenmetrics'
  planner and `zenfleet-ctl declare-encodes`; zenfleet owns claims, retries,
  coverage and the Parquet ledger. Use `scripts/jobsys/fleet` for monitoring.
- The private pilot jobspec explicitly overrides `ZEN_EXEC=/bin/cat`. It is
  an orchestration smoke test, not an encoder deployment. A comparison run
  needs the real `zenmetrics jobexec` executor with all required backend arms.
- Build and smoke-test a revision-pinned executor image with
  `scripts/jobsys/build_executor_image.sh`. Overlay the current worker and
  entrypoint as well as zenmetrics; the base image can contain stale code.
  Test inside the actual image to catch runtime linkage and feature omissions.
- The older August orchestration defect register is superseded on heterogeneous
  chunk duplication by RUNNING_JOBS section 6b and the August 30 fleet evidence:
  fleet-uniform chunk boundaries and contention-aware idle handling shipped.
  Verify the deployed worker contains those fixes; do not infer this from a
  mutable image tag or add an independent scheduler.
- Keep image/source/backend/protocol provenance in declared work so existing
  oracle-and-replay results cannot satisfy standalone-encoder comparison jobs.
  Ledger row timestamps are pass clocks, not per-encode duration measurements.
