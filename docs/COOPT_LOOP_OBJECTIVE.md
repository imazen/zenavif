# The scalar objective — COOPT_LOOP Phase 0 (part 1)

Status: **landed 2026-07-06** (`scripts/rd_gap/objective.py`, selftest + real-data
validated). This is the first deliverable of `docs/COOPT_LOOP_PLAN.md` Phase 0:
"Formalize the evaluation policy into ONE scalar training objective." The trace
instrumentation (Phase 0 part 2) is the next chunk.

## Why one objective

Today ~10 `analyze_*.py` scripts each re-derive the verdict — per-image BD here, a
median there, a hand-applied butteraugli veto somewhere else. That was fine for
reading one A/B at a time. It does not survive a **joint fit**: coordinate descent
or an evolutionary search over cached cells scores thousands of candidate configs,
and every one must be scored the *same* way or the fit optimizes the harness's
inconsistencies instead of the codec. So the policy becomes one function.

## The objective (the 2026-07-05 evaluation policy, made executable)

Minimize, over a candidate config (an "arm") vs the incumbent (a "base"):

- **Primary: ssim2 BD-rate** — bytes at matched quality, negative = the arm needs
  fewer bits (wins). The Bjøntegaard core is `bd_arm.py`'s `frontier` + `bd_rate`,
  reused verbatim (one BD implementation, per the drift lesson).
- **Aggregation: per-family FIRST, then cluster_size mass-weighted.** The k-means
  corpus subset is DIVERSE, not REPRESENTATIVE — one member per cluster regardless
  of cluster mass — so a plain median over images over-weights rare classes and
  dilutes photo-dominant effects. The scalar weights each family by its recorded
  `cluster_size`; the per-family table is always printed (photo-parity claims stay
  per-family; a lever with merit only on photos is KEEPABLE, not rejected).
- **Constraint: a HARD butteraugli veto.** An arm that improves ssim2 by regressing
  butteraugli past +1.0% BD (the pre-registered threshold, both the 3-norm and the
  max norm) is INFEASIBLE, not merely worse — the metric-gaming guard. A vetoed arm
  scores `VETO_PENALTY + max(butteraugli BD)` so any minimizer treats it as
  unreachable but can still rank two infeasible points by violation severity.

The scalar the fit minimizes: the mass-weighted ssim2 BD-rate when feasible, the
penalty when vetoed. The incumbent (arm == base) scores exactly 0.0 by construction.

## Two veto scopes, both reported

- **Per-family veto** (`per_family[f].veto`): flags each family whose butteraugli
  regressed past the threshold — the fine signal a human needs ("palette helped
  overall but hurt butteraugli-max on families 1000/6000").
- **Aggregate veto** (`vetoed`): the mass-weighted butteraugli BD over families
  past the threshold — the coarse constraint the scalar enforces. An arm can have
  a per-family veto without an aggregate veto (a localized regression the mass
  outweighs); the fit sees the aggregate, the human sees both.

## Usage

```
objective.py BASE.tsv ARM.tsv [--manifest cluster_sizes.tsv] [--veto 1.0] [--json]
objective.py --selftest
```

Input is the raw per-cell `run_gap.sh` schema (`image w h family encoder fmt q bytes
bpp ssim2 enc_ms butteraugli_3n butteraugli_max`) — the same `bd_arm.py` consumes,
so any existing A/B pair scores without re-encoding. `--manifest` is a TSV of
`family<TAB>cluster_size`. As a Python import, `score(base, arm, weights, veto_pct)`
returns the full dict for a fitter's inner loop.

## Validation

- **Selftest** (`--selftest`, no data files): a genuine bit-saving arm scores < 0; a
  metric-gaming arm (ssim2 win bought with a butteraugli regression) is vetoed; the
  incumbent scores 0; mass-weighting amplifies a photo-only win that equal-weight
  dilutes. (Gotcha baked into the fixture: butteraugli must VARY across the frontier
  — a constant is one quality level, so its BD is undefined.)
- **Real data**: palette off-vs-auto (`rd-gap-palette-ab-2026-07-03`) scores
  −0.60% mass ssim2 BD, wins on screen families (8000 −3.18%, 5300 −2.28%, 7000
  −0.94%), neutral on photos, per-family-vetoes 1000/6000 on butteraugli-max — the
  expected palette signature.

## Known limitation → the immediate next chunk

The default aggregate is **equal-family-weighted** (labeled `weighting=equal`) until
a canonical `family → cluster_size` manifest exists. Generating it is a separable
data task with its own policy call: which corpus defines the masses (the imazen-26
TRAIN split's per-class counts, NOT the diverse k-means subset, whose counts are ~1
each by construction). Until then, pass `--manifest` explicitly for a policy-compliant
aggregate; the per-family table — the primary policy directive — is correct regardless.

Then Phase 0 part 2: the decision-trace instrumentation in zenrav1e (the dataset
generator), so the fits in Phases 1–3 run OFFLINE against cached encodes + traces.
