# COOPT Phase-1 decision-trace corpus — 2026-07-11 (pointer)

**SUPERSEDED 2026-07-12** by `/mnt/v/output/cooptloop/traces-scored-2026-07-12/`
(adds commit rows = surviving-scope marking, decodable IVF scoring — ssim2+mse
columns — and a denser 5-quantizer grid). Keep this dir only until the scored
corpus' verdicts land; it lacks row-3 commits so `fit_trace_d.py`'s surviving
columns read zero on it. Regenerable either way.

Block-storage payload (traces are 3–45 MB each; never committed):

- **Path:** `/mnt/v/output/cooptloop/traces-2026-07-11/`
- **Contents:** 72 traces (train26 24 images × zenrav1e quantizers {60, 100, 160},
  speed 6, threads=1, `Tune::Ssimulacra2`) + `manifest.tsv` (per-encode: trace file,
  rows, encoded bytes) + `summary.tsv` (per-encode analyzer row) + `d_aggregates.tsv`
  (per-encode winner-D/R reductions from `fit_trace_d.py`).
- **Producer:** `scripts/rd_gap/gen_trace_corpus.sh` @ zenavif cooptloop branch,
  zenrav1e master `310148ec` (`cooptloop_trace` feature; the dump example),
  ravif--cooptloop not involved (traces are zenrav1e-native encodes).
- **Regenerable:** fully deterministic from the corpus PNGs + the pinned commits
  (single-threaded encodes, no wall-clock in the trace) — no Tower mirror needed;
  the committed `summary` + `d_aggregates` TSV copies beside this pointer are the
  durable analysis artifacts.
- **Next-iteration note:** regenerate with `--ivf-out` (zenrav1e@310148ec) to add
  decodable streams per trace, then score them (fast-ssim2) and join on
  (image, quantizer) for the Phase-1 D-vs-metric regression.
