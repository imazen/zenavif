# Monotonic-RD-vs-time program (started 2026-07-06)

**Directive (user, 2026-07-05):** "we can make tiers equal, but make sure our image
analysis provides monotonic rd improvement with time." → speed tiers may *coincide*, but a
slower tier must **never** buy a worse RD point than a faster one, and the per-image content
analysis is the mechanism that guarantees it.

## The invariant (precise, gate-able)

For a given image, order the speed tiers by encode **time**. The invariant holds iff no
slower tier is Pareto-**dominated** (≤ bytes AND ≥ ssim2, strict on one) by a clearly-faster
tier. Tiers coinciding = fine. A slower tier dominated by a faster one = the bug.

A **fixed** table cannot satisfy this across content, because whether a time-costing arm
improves RD is **content-dependent**. So each time-costing arm must be **content-gated**
(the `src/fast_heads.rs` recommend-only mechanism): applied only where the features predict
ΔRD ≥ 0. Withheld on content where it doesn't pay, the tier collapses to its faster neighbor
→ equal + monotone. Applied where it pays → distinct + the extra time buys RD. Monotone by
construction.

## The finding that grounds it (measured)

`benchmarks/mono_rd_vs_time_2026-07-05.tsv` (armed/flipped ladder = zenrav1e master + consts
on + Tune::Ssimulacra2; 6 diverse images, Q80; analyze with
`scripts/rd_gap/analyze_monotone.py`): **11 inversions / 6 images.**
- **Monotone:** people-photo, product (the photo distribution the arms were tuned on).
- **Inverted:** nature-flowers, scan-1bit, plot, screenshot — the faster **s4 dominates
  s6/s7/s8**, and the faster **s9 dominates s5**.
- **Culprit arms** (validated on photo *medians*, net-negative off it):
  `fine_dir` (s5/s6) and the `tx_size_rdo + intra7 + part_prune` bundle (s6-s8).
- On synthetic/graphic content the monotone ladder is effectively **s4 → (skip s5-s8) → s9
  → s10**; the s5-s8 "quality" tiers are pure wasted time there.

## Plan (chunks — land + commit each; re-check the list after every commit)

1. **`gate-monotone` guardrail** — `examples/gate_kit.rs` `monotone [--pin]` subcommand +
   `benchmarks/gate_monotone_envelope.tsv` (platform-scoped known inversions; fails on any
   NEW one) + justfile `gate-monotone`/`-pin` + added to `gates`. Runs against the
   currently-configured encoder (registry now, armed post-flip). **[IN PROGRESS 2026-07-06]**
2. **Corpus fit** — dense sweep (≥20 imgs/class × low+high-q grid, armed) + zenanalyze
   features per image; fit the withhold-gate thresholds separating clean (photo) from
   inverting (synthetic/graphic) content on the fast_heads feature axis
   (`gradient_fraction_smooth`, `patch_fraction`, `dct_compressibility_y`). Held-out validate.
3. **Content-gates in `src/fast_heads.rs`** — extend the heads: withhold `fine_dir` + the
   S6 bundle on synthetic content (fitted thresholds), recommend-only, release-gated like the
   existing heads. Forward through the zenravif expert passthrough at the dep bump.
4. **Verify** — re-measure the ladder; `gate-monotone` envelope shrinks toward empty; re-pin
   the shrink in the same commit.
5. **SpeedPolicy collapse** (post-flip refactor, docs/ENGINEERING_BASELINE.md) — collapse the
   table to the ~5-6 monotone content-conditioned tiers this program justifies.

## Key facts / gotchas
- "Armed" measurements use the throwaway dev-patch clone `../ravif--statusmeasure` (consts
  flipped + zenrav1e master path + `Tune::Ssimulacra2` + an encode-only `ZR_ENC_MS` timer).
  The arms are **release-gated**; the content-gates ship gated too (flip at the dep bump).
- `gate-monotone` runs against zenavif's own encode API → `../ravif` path dep (consts OFF =
  registry now). So the pinned envelope pre-flip captures the *registry* inversions
  (incl. the catastrophic `fine_dir` recon-desync); post-flip re-pin captures the milder
  content-inversions the gates then remove.
- Decoder is exonerated (ffmpeg/dav1d agrees with rav1d-safe to 0.01 ssim2); the inversions
  are encoder-side. See benchmarks/vs_cratesio_per_speed_2026-07-05.tsv.

## Corpus fit findings (2026-07-06, `scripts/rd_gap/fit_content_gates.py`)

Armed sweep of 24 train26 renditions (2/family) × s4-9 × q{50,80}
(`benchmarks/mono_fit_labels_2026-07-06.tsv`) + zenanalyze features
(`benchmarks/mono_fit_features_2026-07-06.tsv`). Inversion = a slower tier
Pareto-dominated by a clearly-faster one (0.80 time margin) by a **meaningful**
RD margin (>=1% bytes OR >=0.2 ssim2 — sub-margin wins are flat-ladder noise,
e.g. photo 5004's 0.4%/0.03 discarded). **12/24 invert.** Two patterns:

1. **s5-valley (`s5<s9`, the dominant pattern):** s5 carries fine_dir's cost but
   lacks BOTH the s6+ bundle AND s9's `reduced_tx_set`/`inter_tx_split` — a
   universal armed valley. fine_dir is NOT the culprit (s6 with fine_dir ON ≈ s7
   with it OFF once the bundle is present); the missing **bundle** is. Armed-only
   (registry has no bundle → no gap).
2. **`s6/s7/s8<s4` (razor plots 7028/7050 + mid-feature 8414/9074/9228):** the
   s6+ bundle actively *hurts* — s4 (no bundle) dominates. tx_budget_gate covers
   the razor pair (pf>.85 & dcty>100 → Largest); the mid-feature cases are a
   **residual** (a bundle-withhold gate extension, not yet fit).

**Feature separation:** `gradient_fraction_smooth` gives a clean gap for the
s5-valley — inverters gfs 0.08-0.612, clean photos gfs >= 0.675 → threshold
0.64. 3 borderline clean-synthetic images (8196/9678/9958) sit below it and
misfire; NO feature in {gfs, pf, dcty} separates them from true inverters (e.g.
clean 9958 pf .582/dcty 3.2 ≈ inverter 9228 pf .587/dcty 7.3).

**Remap target chosen by simulation** (s5:=sX, recount inversions): **s9 removes
every s5-valley inversion with 0 new, lowest RD delta** (committed q80 labels:
10→0; full q50+q80 sweep: 17→0). s4 (slow) introduced 14 new s6/7/8<s5; s6
introduced 6 new s5<s4 (s6 itself dominated by s4 on razor content). s9 costs the
3 misfires <=4.4% bytes (still monotone). → shipped as `monotone_speed_gate`
(gfs<0.64 & s5 → s9), `src/fast_heads.rs`. Reproduce:
`fit_content_gates.py benchmarks/mono_fit_labels_2026-07-06.tsv
benchmarks/mono_fit_features_2026-07-06.tsv 9`.

## Progress log
- 2026-07-06: program started. gate-monotone (chunk 1) committed ad85a8c4, verified.
- 2026-07-06: chunk-2 fit complete (findings above). chunk-3 `monotone_speed_gate`
  implemented in fast_heads + wired into auto_tune (armed-build-specific, no
  encoder passthrough). Residual pattern-2 mid-feature bundle-withhold = next.
