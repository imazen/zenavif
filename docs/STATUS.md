# STATUS — the current state in one page (2026-07-05)

The onboarding doc per ENGINEERING_BASELINE. Everything here is measured; history and
methods live in the program docs this page points at. **Registry users see none of the
gated items until the release train runs: `docs/RELEASE_TRAIN_2026-07-05.md`.**

## The ladder (photos, ssim2-BD at matched wall unless noted; gated = at the dep bump)

| Rung | Position | Record |
|---|---|---|
| s1-deep | −0.97% vs aom cpu0 slowest-best | RD_GAP "s1 deep mode" |
| s2-tune | ties cpu0-ss2tune at 0.86–0.98× its wall; −12.3% vs cpu2 | RD_GAP "CURRENT POSITION" |
| s4-class (v3 heads + i5) | crossed cpu2def-ai; +2.8 vs cpu2iq-ai (structural residual, owners named) | rd_gap_s4tier TSV |
| s6-composed (+i7) | **below** cpu4def-ai AND cpu4iq-ai | FAST_TIER_PARITY_PLAN §P2 |
| s8 | crossed cpu6iq-ai (−3.6/−5.1) | rd_gap_p1part TSV |
| s10′ (re-tiered) | beats old s10 at 0.95× its time; **0.69–0.78× JPEG-moz bytes at 4.3× its encode time** | docs/S10_PROGRAM.md |

Screen content rides palette(+UV) + intraBC A+B (legacy plots −26..−29%; fam-7 ladder
+169%→+57%→hash cuts further). Target-quality mode converges median ~3 encodes
(q₀ head, mean 3.75→2.72) with honest `converged` reporting.

## Defaults & policies (shipped zenavif-side)
- Tune::Ssimulacra2 stack (chroma dq, ss2 QM, variance boost s1.0 + size-ramp,
  QM-dist ratio, LF schedule) — gated; tune-default decision at the bump.
- Palette gate: speed-conditional τ (0.197 / 0.05 at s≥6); heads (tx/partition/intra
  budgets) recommend-only via fast_heads; q₀ seed under `auto-tune`.
- Monotonicity guarantee — now AUTOMATIC on the default `encode_rgb8`/`encode_rgba8`
  paths (per user directive). Two parts: (a) `monotone_speed_gate` remaps the armed s5
  RD-vs-time valley → s9 on synthetic (gfs<0.64); (b) a SELECTIVE probe resolves
  pattern-2 (`s6/7/8<s4`) at near-1× — photo-like content (pf≤0.45) skips it, structured
  content probes anchor s4 and keeps the Pareto-better. `encode_rgb8_once`/`_rgba8_once`
  are the non-probing primitives (search + two-pass use them). Release-gated
  `MONOTONE_GATE_LIVE=false` (inert on registry: one encode, no score); flip at the dep
  bump — docs/MONOTONICITY_PROGRAM.md. Armed-validated end-to-end (plot→s4, photo→skip).
- Tiles: ≥1 MP per tile default (bytes core-count-independent) — LIVE on ravif main.
- Evaluation policy: per-family first, cluster-mass weights, photos-only merit
  keepable (RD_GAP "EVALUATION POLICY").

## Executable gates (run before/after every refactor commit)
`just gates` in zenavif (determinism/conformance/ladder + **monotone**) +
`gate-identity` / `gate-recon` in zenrav1e. CI runs fast subsets. ENGINEERING_BASELINE §A.
`gate-monotone` (~30 s of fixture encodes) = RD improves monotonically with encode time
(empty envelope now; teeth post-flip when the arms create the valley).

## Open residuals (all with named owners)
- s4-tier cpu2iq column +2.8: iq-AQ trio (1236/9100/9118) + 6096 no-skip (coefficient
  valuation — transplant REFUTED, docs/COEFF_RD_STACK.md; needs novel/per-image work),
  5000 full-tx headroom, 8-photo s1 class.
- Ultra-fast residual: 5000-nps 1.01–1.12× JPEG at s10′.
- Decoder follow-ups: rav1d-safe #423 (flush drops frames), #414 (NEON conformance).
- Corpus: doc-chart anti-boost gate fittable (sample_doccharts.tsv); zensim-B hints
  await profile-B.
- Monotonicity pattern-2 (`s6/7/8<s4`): RESOLVED by the selective probe (above) — not
  feature-separable, so gate on the safe-to-skip property (photo pf≤0.45) and MEASURE the
  rest. Now default on encode_rgb8/rgba8, release-gated. Only τ validation on a larger
  corpus remains. docs/MONOTONICITY_PROGRAM.md.

## The two structural facts a newcomer needs
1. aom's GOOD mode is off its own still pareto — the frontier is **allintra**; we sit
   at-or-below it at s6/s8 (gated), with the quality-tip crown and a JPEG-beating s10′.
2. Mechanisms port, constants don't, and aom's coefficient core doesn't transplant at
   all — measured across ~20 programs (docs/RD_GAP_VS_LIBAOM.md is the ledger).

## Next (in order)
1. **The release train** (user-gated, 5 cars) — docs/RELEASE_TRAIN_2026-07-05.md.
2. The dep-bump flip (verified by the gates) + ladder re-confirm on registry deps.
3. The refactor pass — docs/ENGINEERING_BASELINE.md (SpeedPolicy table AFTER the flip).
4. Unified (target, effort) entry point; zensim-B/FrameHints consumers; residual heads
   round 2 under the representative-evaluation policy.
