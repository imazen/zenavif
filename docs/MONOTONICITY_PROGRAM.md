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

1. **`gate-monotone` guardrail** — DONE (ad85a8c4). `monotone [--pin]` subcommand +
   platform-scoped envelope + justfile + added to `gates`. Tests the RAW ladder.
2. **Corpus fit** — DONE (7211deb2). 24 armed origins + features → the s5-valley finding +
   the gfs<0.64 gate + the s5→s9 remap (simulation-chosen).
3. **Content-gate `monotone_speed_gate`** — DONE (7211deb2), **release-gated** (8d2f1cf2,
   `MONOTONE_GATE_LIVE=false` — registry-safe; flip at the dep bump). The fine_dir framing was
   REFUTED (fine_dir ≈ neutral once the bundle is present; the missing bundle is the cause) —
   the fix is a speed remap, not a fine_dir withhold.
4. **Verify** — DONE. Held-out (e154252, 15 doccharts: 0 new inversions, recall 9/10) +
   deep-tier s1-s3 exclusion validated (5b9a34ad) + the registry-regression catch (8d2f1cf2).
5. **Pattern-2 residual** (`s6/7/8<s4`) — SCOPED, not shipped (c574b91/9a5669a/c4d2e36c): not
   separable at 1 or 2 features on 39 origins (LOOCV 6/39); turnkey dense-fit recipe + the
   probe-encode direction documented above. RISKY (touches s6/7/8 wins).
6. **SpeedPolicy collapse** (post-flip refactor, docs/ENGINEERING_BASELINE.md) — collapse the
   table to the ~5-6 monotone content-conditioned tiers this program justifies.

## Key facts / gotchas
- "Armed" measurements use the throwaway dev-patch clone `../ravif--statusmeasure` (consts
  flipped + zenrav1e master path + `Tune::Ssimulacra2` + an encode-only `ZR_ENC_MS` timer).
  The arms are **release-gated**; the content-gates ship gated too (flip at the dep bump).
- **`monotone_speed_gate` is RELEASE-GATED (`MONOTONE_GATE_LIVE = false`).** The valley is
  armed-only: on registry s5 is NOT dominated — measured, it often *beats* s9 (6096:
  registry-s5 170337/90.16 vs s9 198950/89.29), so applying the remap pre-flip would REGRESS
  synthetic content (+17% bytes / −0.87 ssim2). The gate fires on `gfs` regardless of build,
  so its APPLICATION is held off until the arms flip (dep-bump checklist). Guarded by
  `monotone_gate_release_held_off_on_registry`. Unlike the budget heads (forwarded by ravif at
  the bump) this is a zenavif-side speed change, so it needs its OWN live flag.
- **Two layers — do not conflate them:**
  - `gate-monotone` tests the **RAW ladder** (`encode_rgb8` at *explicit* speeds — it does
    NOT go through `auto_tune`). It documents the encoder's intrinsic RD-vs-time inversions.
    Pre-flip (consts OFF) the fixture envelope is empty; **post-flip it will GROW** (the
    armed raw ladder carries the s5-valley on synthetic fixtures) — re-pin then.
  - `monotone_speed_gate` fixes the **POLICY layer** (`auto_tune`'s picks avoid the valley).
    It changes which speed auto_tune *chooses*, not the raw ladder — so it does **not**
    shrink gate-monotone's envelope. The two are complementary.
  - Consequence: to verify `monotone_speed_gate`'s *effect* you need an auto_tune-path check
    (sweep quality targets → assert the PICKS' (time, RD) are monotone), which needs a baked
    picker (MODEL_BYTES). That check is the natural follow-up; the fit simulation
    (`fit_content_gates.py`) is the current stand-in (0 new inversions, held-out-validated).
  - **The gate is also LATENT on the current picker.** `monotone_speed_gate` lives in
    `auto_tune`, which only reaches it after a successful pick — but the baked
    `rav1e_picker_v0_1_1.bin` LUT has narrow content-specific coverage: measured 2026-07-06,
    it returns `TargetOutOfRange` for the train26 s1024 renditions at z40-z90 (even z85, its
    own test target, is OUT on 5004). So the gate rarely fires in practice today; it becomes
    live once (a) `MONOTONE_GATE_LIVE` flips AND (b) the picker covers the content. Correct +
    release-gated meanwhile. (Not a regression — auto_tune already errored out-of-range there.)
- `gate-monotone` runs against zenavif's own encode API → `../ravif` path dep (consts OFF =
  registry now); the pre-flip fixture envelope also would have caught the catastrophic
  registry `fine_dir` recon-desync had the procedural fixtures triggered it (they don't).
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

## Deep-tier check — the s1-s3 exclusion is validated (`benchmarks/mono_fullladder_2026-07-06.tsv`)

gate-monotone excludes s1-s3 "to keep the gate fast, assumed monotone". Confirmed on the full
s1-s10 ladder (3 synthetic inverters + 1 photo control, armed @ q80): **the deep tiers are
monotone.** s1 is the slowest (6-30 s on 1024px, the deep mode) AND the best RD by a clear
margin (6096: s1 110491/91.30 beats every faster tier); s2==s3 (byte-identical aliasing, per
the directive OK); none of s1/s2/s3 is Pareto-dominated by a faster tier. The only meaningful
inversion across the whole ladder is the s5-valley (already fixed) — s10 is the fast-worst end
(monotone). So excluding s1-s3 from the gate loses nothing, and s1's 30 s cost is why.

## Armed validation — the gate has teeth (`benchmarks/gate_monotone_armed_2026-07-06.tsv`)

The arms are release-gated, but you don't need the release train to *test* them — point zenavif's
`ravif` dep at the armed dev-clone (`../ravif--statusmeasure/ravif`, all 6 `_LIVE` consts flipped
+ zenrav1e master) and build. Done 2026-07-06: **gate-monotone FAILS on the armed build (2
inversions) where it's empty on registry** — proving the guardrail bites once the arms are live.
Caught: `screen/q80 s6 dominated by faster s7 AND s8` (fine_directional_intra ON at s6, OFF at
s7+, doing 38% more work — 1362 vs 986 ms — for equal-or-worse RD on the procedural screen
fixture).

Two things this run taught:
- **The caught inversion is a PROCEDURAL-FIXTURE artifact, not a real-content bug.** Checked
  against the armed corpus label sweeps at q80: 7028 (real line plot) has s6 with *better* RD
  than s7 (fine_dir HELPS on real directional content); 8302 is a tradeoff; 6096's s7 edge is
  under the 20% time margin. And at q40 fine_dir helps everywhere. So fine_dir at s6 is content +
  q conditional; `gen_screen`'s ultra-regular pattern just over-represents the one useless regime.
- **gate-monotone's procedural fixtures are a COARSE guardrail.** They exercise the encoder and
  catch *some* inversions, but they don't cleanly exhibit the corpus s5-valley the fix targets
  (the fixtures' s5 keeps better quality than s9), and they can flag fixture-specific inversions.
  The **corpus label sweeps remain the real validation** of the s5-valley fix; the gate is the
  fast CI tripwire. Corpus-based fixtures (via `codec-corpus`) are the improvement that would let
  the gate guard the actual s5-valley — tracked, not yet done.
- **Post-flip envelope:** these 2 known fixture inversions are what to pin (`just gate-monotone-pin`)
  at the dep bump, replacing today's empty registry envelope.

## The cooptloop-branch flip (2026-07-10) — envelope pinned, magnitudes recorded

The flip ran for real on the `cooptloop` branch (ravif--cooptloop armed clone + zenrav1e
master by path; `MONOTONE_GATE_LIVE=true`). gate-monotone caught 4 inversions; row dump
(`GATE_MONOTONE_DEBUG=1`) magnitudes:

- **`screen/q80: s5 < s9`** — the corpus s5-valley showing up in the fixture at last
  (s9: −3.6% bytes, +0.12 ssim2, 3.1× faster). Real, meaningful, and policy-handled:
  `monotone_speed_gate` remaps s5→s9 on synthetic content in `auto_tune`. Pinned.
- **`photo/q40: s6/s7/s8 < s5`** — fixture-noise scale: the cells are ~1.2 KB files, the
  deltas are 13–28 BYTES and ≤0.064 ssim2 (s5 1166 B/66.696 vs s6 1194 B/66.632). They
  pass the raw Pareto test only because %-of-tiny-file inflates byte deltas (the sweep
  discipline's fixed-overhead lesson). Sub-margin under the program's meaningful-inversion
  rule on the score axis, marginal on bytes. Pinned with this note as provenance.
- **The real signal in photo/q40 is the TIME-ORDER flip:** armed s6-8 cost ~2.2× s5
  (272 vs 123 ms) on flat content at low-q where the bundle buys nothing. That is a
  ladder re-tiering input — COOPT_LOOP Phase 4 ("speed = budget on the new loop") owns
  it. Do NOT patch it per-fixture.
- Follow-up (needs user sign-off, it's a gate-semantics change): the gate's dominance
  test could adopt the documented meaningful-RD margin (≥1% bytes OR ≥0.2 ssim2) plus an
  absolute-bytes floor for tiny cells; today it is pure Pareto + the 25% time margin.

(This corrects an earlier session claim that the monotonicity residuals were "structurally blocked
pre-flip" — they're blocked for *registry users*, but fully testable on the local armed build.)

## Held-out validation (2026-07-06, `benchmarks/mono_val_labels_doccharts_2026-07-06.tsv`)

15 **doccharts** origins — distinct from the 24 train origins (5000/5030/6000/6600
vs train's 5004/5048/6018) — swept armed @ q80, run through the same fit sim:
- **10/15 invert; the s5-valley pattern generalizes** (nps/noaa reports, patents,
  scans-text all show s5<s9).
- **s5→s9 removes 9 valley inversions with 0 NEW inversions** — the critical safety
  property (remapping never *creates* an inversion) holds on unseen content. Mean
  remap RD delta 4% (mostly *improvement*: s9 smaller than the s5 valley).
- **Gate recall 9/10.** The one miss (6600, a smooth scanned illustration) sits at
  gfs 0.676 with photo-like pf 0.004 — just above the 0.64 threshold, which cannot
  rise (train clean photo 9100 sits at gfs 0.675). Precision is imperfect (3/5 clean
  fire), but s5→s9 is *safe* on misfires (bounded RD cost, never a new inversion), so
  the gate's justification is the safe remap, not a perfect content classifier.

## Pattern-2 residual (`s6/s7/s8<s4`) — measured NOT separable on this corpus

The second pattern (the s6+ bundle *hurts*, s4 dominates s6/7/8) affects 6/24
(7028, 7050, 7058, 8414, 9074, 9228). The razor pair (7050/7052) is already
covered by `tx_budget_gate` (pf>.85 & dcty>100 → Largest). The other 4 do NOT
separate from the bundle-*helps* content (6096/6018/8268/8302, where s6 is a
measured better-quality tier): `scan_pattern2_features.py` over the FULL ~100
zenanalyze features finds **no clean single-feature split** — the best
(aq_map_p75, laplacian_variance_p90, orientation_energy_ratio) still misclassify
3/24 and the class ranges overlap almost entirely. With 6 positives on 24
origins that is overfit noise, not signal.

**Confirmed on 39 origins** (train 24 + doccharts 15, 10 bundle-hurts): no clean
split at 1 OR 2 features. Best single = `orientation_energy_ratio` err=5/39 with
overlapping ranges (hurt 1.12-1.30 / rest 1.06-1.20); best **LOOCV** 2-feature
depth-2 tree = `cb_peak_sharpness + flat_color_block_ratio` **err=6/39** (vs 10/39
trivial base rate — catches ~4 of 10 net). Not usable: a safe pattern-2 gate needs
HIGH PRECISION (a false-fire on bundle-*helps* content remaps away a real quality
win), which 6/39 LOO error cannot give.

**Entanglement** with the existing RD fit makes it worse: 7028 (pf 0.901, dcty 12)
is bundle-*hurts*, but `tx_budget_gate`'s VAL fit puts pf-high/dcty-low content
(5343/8103 corner) in Size1 as its *measured-best* class — the RD gate and the
monotonicity concern DISAGREE on the same image.

**RESOLVED by a probe — not a feature gate (`benchmarks/probe_monotone_sim_2026-07-06.tsv`).**
Pattern-2 doesn't need to be *predicted* from features (which fails); it can be *measured*.
Simulating a probe over the 24 armed origins (`scripts/rd_gap/sim_probe_monotone.py`): for a
requested speed, also encode a small anchor set and keep the best RD within the requested speed's
time budget. **The 2-anchor probe {s4, s9} fixes ALL 24 requested-tier inversions (0 new),
pattern-2 included**, because on large content the bundle makes s6/7/8 *slower* than s4, so the
dominator is both better AND cheaper — the probe just looks. The overriding picks are strictly
good for the user: **+0.401 ssim2 and the winning encode 1120 ms faster** than the requested tier.
Cost: **2.16× total encode wall** (run requested + 2 anchors, keep the best). probe-s4 alone fixes
15/24 (misses the s5-valley); probe-full fixes all 24 at 5.78× — {s4,s9} is the sweet spot.

### The hunt for a non-2× solution (2026-07-06) — cheap options measured DEAD
User: "we have to find a solution that isn't 2x." Every cheap/free candidate was measured and
falsified:
- **Thumbnail probe (~1.1×) — DOESN'T TRANSFER** (`benchmarks/thumbnail_probe_xscale_2026-07-06.tsv`).
  Deciding s4-vs-s6 on a 384px downscale matches full-res only 4/8, and the failures are damaging:
  it MISSES marginal inversions (8414/7050: s4's small RD edge washes out at 384px → reads "tie")
  and FALSE-FIRES on ties (6096: full-res s6 is +0.6 ssim2 better, thumb says s4 → remap loses
  real quality). Only the strong inversions (7028 +1.12, 7058 +1.54) survive downscaling.
- **Encoder early-exit — WRONG PREMISE.** The bundle is NOT wasting time: on 7028, REG-s6 (bundle
  off) is 74850B/87.35 vs ARMED-s6 53398B/88.64 — the bundle earns its cost (29% smaller, +1.29
  ssim2). It just can't close the gap to s4's deeper base search. Early-exit would make s6 *worse*.
- **Global time-cap (keep s6 faster than s4) — sacrifices real wins.** Capping the bundle so s6
  stays under s4's time would gut its earned gains on bundle-*helps* content (6096, where s6 is a
  legit better slow-tier), just to fix line-plot content. No free lunch.
- **Feature gate — not separable** (the §above LOOCV 6/39). Bundle-helps scans (6096) and
  bundle-hurts line-plots (7028) overlap in feature space.

**Conclusion: pattern-2 is a genuine preset-Pareto fact** (s6's fast-base+bundle is dominated by
s4's slow-base on line/plot content, AND slower) with no cheap *predictor of helps-vs-hurts*.

### THE SOLUTION — a SELECTIVE probe (near-1×, `benchmarks/selective_probe_gate_2026-07-06.tsv`)
The breakthrough: features can't predict helps-vs-hurts, but they don't need to. **0 photos ever
invert** — the 4 inverters are plots (7028/7050/7058) + 1 screenshot (8414), and `patch_fraction`
separates them CLEANLY (photos pf ≤ 0.389, inverters pf ≥ 0.518). So gate on the *safe-to-skip*
property, not the unpredictable one:
- `patch_fraction ≤ τ` (τ≈0.45, photo-like) → **no probe, 1× cost** (inversion is impossible here).
- `patch_fraction > τ` (structured) → **probe s4** alongside the requested s6/7/8, keep the best RD
  in the time budget (~1.5× on this minority; the probe *measures* domination, so helps-vs-hurts
  non-separability is moot).
- s5-valley stays free via `monotone_speed_gate`.

Validated: **100% recall on inverters** (4/4 — a miss would ship an inversion; all pf ≥ 0.518 > τ),
12/24 correctly skipped, 8 harmless extra-probes (structured non-inverters — cost, not wrong
results). Cost model: photo 1×, structured ~1.5×, so **~1.1× on 80%-photo real traffic — not 2×**,
and the guarantee is complete (the probe is reliable on exactly the content that can invert). This
is the answer to "find a solution that isn't 2×."

**IMPLEMENTED — and it is now the DEFAULT `encode_rgb8` path (automatic, per the user directive
"make it the default path, automatic").** `encode_rgb8` is an auto-monotone *dispatcher*: with
`target-quality` + `auto-tune` present it runs the selective probe (encode requested → release-gate
+ tier-gate → pf-gate photo-skip → probe s4 → deterministic Pareto pick on (bytes, score)); without
those features, on non-bundle speeds, on photo-like content, or with the gate off, it is exactly one
`encode_rgb8_once` (the renamed single-encode primitive) — no decode, no score, no extra encode. The
codec trait and `encode_with` inherit the guarantee for free; the target-quality search and the
two-pass path route to `encode_rgb8_once` so their repeated encodes never nest the probe. Signature
unchanged (no public API break). **Armed end-to-end validated through the default path** (temp
gate-flip + armed dep, reverted): `encode_rgb8(7028 plot)` = direct-s4 bytes (30161B, swapped);
`encode_rgb8(5004 photo)` = direct-s6 bytes (130996B, probe skipped). Registry unit tests pin the
release-gate + ineligible-speed passthrough (RGB8 + RGBA8).

**RGBA8 covered too** (`encode_rgba8` is the same dispatcher; a shared generic `probe_monotone_core`
takes per-type encode/patch-fraction/score closures — RGBA8 drops alpha for `patch_fraction` and
scores via `score_rgba8`, ssim2 compositing alpha on mid-gray). **RGB16/RGBA16 deliberately keep
their single-encode paths:** pattern-2 was only ever measured on 8-bit *structured* content (line
plots), and HDR 16-bit content is photo-like (low `patch_fraction`) so the probe would never fire —
covering them would add code that can't activate. Revisit only if a 16-bit screen-content inversion
is ever measured. Remaining: τ validation on a larger corpus.

Remaining alternatives if even ~1.5×-on-structured is too much: parallel probe (1× wall / 2×
compute), or deep preset R&D to make s6 Pareto-competitive with s4 on line content (free at
runtime, real encoder work, not guaranteed).

### The 2× symmetric probe (reference)
The two mechanisms are complementary: `monotone_speed_gate` (feature gate) is the **free**
partial guarantee (s5-valley only, no extra encode); the probe is the **complete** guarantee at
~2× encode. The probe is the right answer for pattern-2, but it's a new public encode API
(`encode_rgb8_monotone` / an `AutoTuneOptions::monotone_probe` flag) — **awaiting user sign-off**
before landing (API-stability rule). The feature-gate recipe below stays as the cheaper-but-partial
alternative if the probe's 2× cost is unacceptable.

*Feature-gate recipe (the cheaper partial path, if the probe is rejected):* ≥50 origins/class,
sweep armed s4/6/7/8 @ q{50,80} + full features, label via `scan_pattern2_features.py`, target
precision >0.9 (the 1-2 feature LOOCV here fails at 6/39); remap true-fires s6/7/8→s4. RISKY
(touches s6/7/8 where the bundle wins on photos+scans). Tool + 39-origin baseline committed.

## Progress log
- 2026-07-06: program started. gate-monotone (chunk 1) committed ad85a8c4, verified.
- 2026-07-06: chunk-2 fit complete (findings above). chunk-3 `monotone_speed_gate`
  implemented in fast_heads + wired into auto_tune (armed-build-specific, no
  encoder passthrough), committed 7211deb2, verified on origin.
- 2026-07-06: pattern-2 residual measured NOT separable on the 24-origin corpus
  (scan_pattern2_features.py) → scoped as a dense-fit chunk, not shipped.
- 2026-07-06: release-gated `monotone_speed_gate` behind `MONOTONE_GATE_LIVE` after
  measuring registry-s5 is not a valley (would regress) — 8d2f1cf2.
- 2026-07-06: ARMED-BUILD validation (user: "flip without a train, it's a hard drive").
  Pointed zenavif at ../ravif--statusmeasure/ravif + built → gate-monotone has teeth
  (caught fine_dir s6<s7 on the procedural fixture; shown a fixture artifact vs real
  corpus). Committed 0342d0f1.
- 2026-07-06: PROBE simulation over the armed data — probe {s4,s9} is the complete
  monotonicity guarantee (fixes all 24, pattern-2 included, strictly-good picks, 2.16×
  wall). Resolves pattern-2 via measurement, not features. Awaiting user sign-off on the
  opt-in probe API. sim_probe_monotone.py + probe_monotone_sim_2026-07-06.tsv.
