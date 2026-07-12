# GOAL: AVIF at the cutting-edge Pareto front — RD and RD-time (charter 2026-07-12)

**Mission, one sentence:** zenavif encodes a still AVIF to a (bytes, quality,
encode-time) point that no other production AV1 still encoder Pareto-dominates — at
the quality tip AND at every time budget of the ladder — and the claim is proven by
the gates below on a build anyone can reproduce, not asserted.

This is a **completion-gate goal**: it is DONE only when every gate G1–G7 passes in one
composed measurement round, on held-out data, with the configuration registry users
actually receive. Partial passes are progress, never completion. Expected horizon:
multiple sessions/weeks (the COOPT_LOOP program is the vehicle; this charter is the
finish line it aims at).

## Definitions (fixed here so the gates can't drift)

- **Reference frontier:** libaom `aomenc` allintra (the measured frontier — its GOOD
  mode is off its own still pareto) across cpu-used 0..6 × {default, ssimulacra2/iq
  tunes}, AND SVT-AV1 (still/allintra path) across its presets — both at the LATEST
  release at measurement time (live-check versions each round; competitor encoders
  change like models do). JPEG (mozjpeg-class zenjpeg) anchors the ultra-fast end.
  Upstream rav1e is informational, not gating.
- **Corpora:** development on imazen-26 TRAIN (train26 + doccharts supplements);
  **gates score on held-out VAL/TEST origins only** (rd-corpus-split-hygiene). Sizes
  per long-edge classes {256, 512, 1024, 2048} — the size-decay lesson: a gate passed
  only at 1024 is not passed.
- **Scoring:** `scripts/rd_gap/objective.py` policy — ssim2 BD-rate primary,
  per-family FIRST, cluster-mass aggregate, butteraugli 3n+max veto at +1.0% BD as a
  HARD constraint. Time = solo wall ms/MP under the pinned harness (min-of-3,
  no `-C target-cpu=native`, threads policy documented per row).
- **"Matched time":** reference point time ±20% (the gate-monotone convention).

## Completion gates (all must hold in ONE round; each lands as a benchmarks TSV + .meta)

- **G1 — RD tip parity-or-better.** At the quality tip (our slowest shipped tier vs
  every reference's slowest-best config): mass-weighted ssim2 BD ≤ 0% against EVERY
  reference frontier point, butteraugli veto clean, and no family worse than +1.0% BD.
  (Position at charter time: s1-deep −0.97% vs aom cpu0 slowest-best; s2 ties
  cpu0-ss2tune at 0.86–0.98× its wall — the tip is near; families and SVT are the
  open halves.)
- **G2 — RD-time frontier, no domination anywhere.** Sample the reference ladders
  (aom cpu0..6 × tunes, SVT presets, JPEG anchor) across ~50 ms/MP to ~30 s/MP: for
  every reference point there exists a shipped zenavif config at ≤ its matched-time
  band with ssim2 BD ≤ 0% and veto clean. One dominated sample = gate fails. The
  ladder is re-derived as budget points (COOPT Phase 4) — tier names may change,
  domination may not.
- **G3 — internal monotonicity, empty envelope.** `gate-monotone` under the
  meaningful-margin definition (≥1% bytes OR ≥0.2 ssim2, plus an absolute-bytes floor
  for tiny cells) reports ZERO known inversions on the shipped default path — the
  envelope's documented goal state, reached by re-tiering/content-gating, never by
  widening margins without user sign-off.
- **G4 — zero downsides.** All §A gates green (determinism / conformance
  aomdec+rav1d-safe byte-agree / ladder / monotone); fidelity envelopes hold (PQ10
  class); per-family butteraugli vetoes ZERO on the ship config across held-out
  corpora — the 9094-class worst-spot tradeoff must be CLOSED (Phase 3 spatial
  allocation), not pinned.
- **G5 — the named structural residuals each closed or safely gated.** iq-AQ trio
  (1236/9100/9118), 6096 no-skip, 5000 full-tx headroom, the 8-photo s1 class,
  screens (intraBC completion incl. SB128), small-size decay: each either beaten
  outright or content-gated so the default never loses to the reference on that class
  beyond the +1% band. "Gated off and ignored" only counts if the gate is measured on
  held-out data.
- **G6 — integration-honest reproduction (NOT publish).** The passing G1/G2 round
  reproduces on a CLEAN build: fresh clone, pushed + pinned revisions only
  (git/registry deps — every repo in the chain publicly fetchable; the armed ravif
  clone must exist as a pushed branch, not a machine-local folder), no uncommitted
  state, the default feature policy, CI-green on the pinned revs. This is the gate
  that kills lab-config claims (feature-default drift, Cargo resolution surprises —
  the 0.6.2-lockfile class). **The crates.io release train is deliberately NOT a
  completion condition:** publishing is a user-gated ceremony on the user's timeline
  (README sign-off, semver, the full tag/release ritual) and adds no information to
  the RD/RD-time verdict. This goal hands the train a fully-verified artifact
  (docs/RELEASE_TRAIN owns the cadence concern); it does not wait on it.
- **G7 — durable and re-runnable.** One command (`just gate-pareto`, to be built)
  re-runs the G1/G2 sweep against freshly fetched reference encoders and emits the
  verdict; all round results committed under `benchmarks/` with commit hashes,
  encoder versions, and corpus manifests; docs/STATUS.md reflects the final position.

## Method constraints (standing; violations void the round)

- Pre-registered decision rules before each measuring round; no test/threshold
  relaxation without explicit user sign-off; measure — never extrapolate.
- Live-check reference encoder versions and re-baseline each round (stored labels go
  stale: the drift lesson).
- COOPT_LOOP phases are the engine (coherent λ–D–R loop, quantization loop, spatial
  allocation, ladder-as-budgets); the falsification ledger
  (`FUTURE_DIRECTIONS_AND_FALSIFICATIONS.md`) is binding — no re-trying refuted
  transplants without new evidence.
- Heavy work under run-heavy; sweeps persist encodes per the data discipline; every
  verdict TSV carries provenance.

## Cadence

Each session: advance the current phase → run the relevant gate subset → commit
TSV + docs → update this charter's status line. Re-measure G1/G2 fully at each phase
boundary. The charter closes when a single round passes G1–G7 together; the closing
commit links every gate's TSV.

**Status (2026-07-12, second update):** Phase 1 OPEN with its first pre-registered
verdict: the trace's surviving-R leg is frame-level EXACT (Σ winner-R/8 =
0.98–1.00× real bytes, was 5.9× before partition-outcome marking,
zenrav1e@8552e2f0), and **D DOES NOT BEAT MSE (0/5 quantizers)** — cross-image the
psy-D currency correlates with ssim2 at |r| 0.14–0.74 vs raw MSE's 0.74–0.85
(benchmarks/cooptloop_dfit1_verdict_2026-07-12.*): the D-refit is the top-priority
Phase-1 lever with quantified headroom. G2: SVT-AV1 live-checked (latest v4.1.0)
and built clean (~/work/zen/svtav1-v4.1.0); ladder cell wiring is next. G6: the
armed ravif branch is publicly fetchable (imazen/cavif-rs#cooptloop @146b3ed);
remaining = pinned-rev chain + clean-clone reproduction. G4: 9094 bamax flag open
(Phase 3). Flip A/B on TRAIN: −25.7% mass ssim2 BD, no aggregate veto.
