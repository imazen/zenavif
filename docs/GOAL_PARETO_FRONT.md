# GOAL: AVIF at the cutting-edge Pareto front — RD and RD-time (charter 2026-07-12)

**Mission, one sentence:** a registry user encoding a still AVIF with zenavif gets a
(bytes, quality, encode-time) point that no other production AV1 still encoder
Pareto-dominates — at the quality tip AND at every time budget of the ladder — and the
claim is proven by the gates below, not asserted.

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
- **G6 — shipped.** The release train has run: every crate published with the full
  ceremony (tests, CI green on ALL platforms incl. windows-11-arm / macOS Intel /
  i686, tag, GitHub release, publish), the dep-bump flip landed on main, and the G1/G2
  measurements REPRODUCE on a clean registry build (no path deps). Cutting-edge on a
  branch is not cutting-edge.
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

**Status (2026-07-12):** COOPT Phase 0 complete; the composed flip measures −25.7%
mass ssim2 BD vs the registry chain on TRAIN (no aggregate veto; 9094 bamax flag
open → G4 work); G1 near at the tip vs aom (SVT unmeasured — first G2 prerequisite);
G2 blocked on Phase 4 re-tiering; G6 blocked on the release train (user-gated).
