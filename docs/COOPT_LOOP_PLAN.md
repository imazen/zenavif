# The co-optimized-loop program — a month-long re-engineering plan (drafted 2026-07-05)

**Premise (measured, not assumed):** the piecewise era is complete. Twenty-odd programs
proved mechanisms port but constants don't, and COEFF_RD_STACK.md proved the stronger
claim: aom's coefficient-level edge is an internally-coherent valuation loop that cannot
be entered piecemeal. Our remaining residuals — the s4-tier cpu2iq column (+2.8), the
iq-AQ trio, 6096 no-skip, the 8-photo s1 class, 5000 full-tx — are all owned by
co-designed loops, not by missing levers. The next structural gain requires rebuilding
OUR loops as jointly-calibrated systems against OUR objective, using assets aom doesn't
have: metrics we own (ssim2 harnesses, zensim profile-B diffmaps incoming), a 68k-row
label store, the cell cache + fleet (each candidate configuration is cheap to evaluate),
FrameHints, per-image features, and the §A gates as the safety rail.

**The one-sentence plan: stop fitting levers one at a time against a frozen loop; fit
the loop's three currencies (λ, D, R) and its two allocators (quantizer, spatial AQ)
jointly against the mass-weighted metric objective, then re-derive the speed ladder as
budget points on the new loop.**

## Phase 0 (½ week) — the objective + the decision trace
- Formalize the evaluation policy into ONE scalar training objective: bytes minimized
  subject to metric floors; ssim2 primary, butteraugli veto as a constraint (not an
  afterthought), zensim-B when profile-B ships; families mass-weighted per the
  representative-not-diverse policy; sizes per the long-edge classes.
  **LANDED 2026-07-06** — `scripts/rd_gap/objective.py` (`--selftest` + palette-A/B
  real-data validated) + `docs/COOPT_LOOP_OBJECTIVE.md`. Generalizes `bd_arm.py`
  with family grouping, the +1.0% butteraugli veto as a hard constraint, and
  cluster_size mass-weighting; returns the one scalar a joint fit minimizes
  (incumbent = 0). Remaining: the canonical `family→cluster_size` manifest (equal-
  weight default until then; per-family table is policy-correct now).
- **Decision-trace instrumentation** in zenrav1e (feature-gated): per-block log of
  (λ, D terms per candidate, R estimates vs actual tells, chosen vs runner-up, quant
  decisions). This is the dataset generator for every fit below — the census tools
  from COEFFRD/SSIMRD generalized into one format. Without traces, joint fitting is
  blind; with them, most fits are OFFLINE against cached encodes.
- Gates green before and after every phase (ENGINEERING_BASELINE §A); pre-registered
  decision rules per phase (the discipline that kept every verdict honest).

## Phase 1 (1½ weeks) — the valuation core: one calibrated λ–D–R triangle
The program's scars show the three legs are locally fitted and mutually inconsistent:
the trial-SPLIT bias, tx-domain rate so wrong at fast tiers it was better amputated,
the fork trellis dead below Q80 on its own private λ, SSIMRD's triple-counting.
- **D — one distortion, everywhere.** Today: cdef_dist psy weighting in some paths,
  raw SSE in others, tx-domain at fast tiers, the QM-ratio composed on top. Rebuild as
  a single psy-distortion kernel with per-context weights, and fit it so that ΔD
  *predicts Δmetric* — regress candidate D definitions against measured ssim2/zensim
  deltas from the label store + traces. (This is the co-optimization aom structurally
  cannot do: they calibrate against PSNR-family proxies; we calibrate against the
  shipping objective on the shipping corpus.)
- **R — exact tells where affordable, fitted tables where not.** Exact in-trial tells
  are our strength (keep). Fast tiers get a table-rate estimator FITTED AGAINST OUR OWN
  EXACT TELLS (not aom's tables), so cheap rates stay consistent with the slow-tier
  currency instead of forcing tool amputation (the txdr lesson).
- **λ — one composed function, jointly fitted.** The q→λ curve plus every per-context
  modulation (variance boost, masking, QM-ratio, size ramps) refit as ONE parameter
  vector via coordinate descent / small evolutionary search over cached cells —
  killing the independently-fitted-multiplier stacking that caused overshoot.
- Acceptance: ladder-wide non-regression + any measured win; the fits are severable —
  land D alone if R/λ stall.

## Phase 2 (1 week) — quantization as a loop, not a step
Rebuild coefficient decisions in the SAME currencies as phase 1 (the coherence aom has
and our fork lacked): always-on descent driven by exact tells at slow tiers and the
fitted tables at fast tiers, Valin offsets as the seed (they independently equal aom's
zbin — keep them), per-TU zero-out and skip valued in the same D/R, one λ. No aom
constants — the *shape* is ours already; the work is unifying the currency.
- Acceptance tests: 6096 no-skip class (+15.8 today), the 8-photo s1 class vs cpu0,
  near-lossless rescans; photos-primary per policy; butteraugli constraint throughout.

## Phase 3 (1 week) — spatial allocation v2: metric-in-the-loop AQ
Replace the stacked per-SB machinery (variance boost + masking + QM-ratio + ad-hoc
hints) with ONE per-SB allocation model: zenanalyze features (+ zensim-B/butteraugli
ANALYSIS maps — zero extra encodes, the surviving path from the two-pass verdict) →
per-SB λ/q offsets through the existing FrameHints/delta-q channel, trained offline
against the phase-0 objective. The heads (per-image) and hints (per-SB) merge into one
feature-extraction pass: per-image knobs + per-SB allocation from the same analysis.
- Acceptance: the iq-AQ trio (1236/9100/9118), 6018, the o_6629-class smooth-gradient
  failures; the anti-boost doc-chart gate folds in here (sample ready).

## Phase 4 (1 week) — the ladder re-derived as budgets on the new loop
With coherent valuation, speed = budget, not toolset: re-derive s1–s10 as measured
budget points (the pruning philosophy completed), delete the amputation remnants,
refit the per-tier heads, collapse ravif into the SpeedPolicy table (the
ENGINEERING_BASELINE refactor lands here naturally, AFTER the flip). Full ladder +
JPEG-anchored s10 re-measure; the parity criteria re-scored.

## Method, risks, and honesty
- **Joint fits, not lever A/Bs**: parameter vectors over cached cells (the cell cache +
  fleet make one full-corpus evaluation minutes, not hours — this plan was not
  affordable at program start; it is now).
- **Local optima**: multi-start; the shipped config is always the incumbent to beat
  under the pre-registered rule; per-family veto rows kill lopsided "wins".
- **Overfitting**: LSD splits, held-out families, the diverse-vs-representative policy,
  and the drift lesson (baselines regenerate per encoder rev — labels go stale).
- **Scope truth**: four phases in a month is tight; each phase lands standalone value
  and the plan survives partial execution. Phases 1–2 are the highest-certainty RD
  gains (they attack the refuted wall with the coherent-loop answer); phase 3 is the
  highest-variance/highest-ceiling; phase 4 is guaranteed cleanup value.
- **Payoff estimate (honest)**: the named residual columns sum to low-single-digit
  BD at the tiers where they bind — the larger prize is COMPOSABILITY: a coherent base
  where future levers stop triple-counting and every subsequent fit gets cheaper.
- Executable by non-frontier sessions: the gates, the harness, the trace format, the
  pre-registration discipline, and this doc are the scaffolding; each phase is a
  sequence of fit→measure→gate steps with numeric acceptance criteria.
