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

**Verdict / turnkey recipe for the deferred dense fit:**
1. Corpus: ≥50 origins/class across plots·screenshots·clipart·products·scans·photos
   (this dir has 1/origin — pull from a larger source; document TRAIN/VAL/TEST split
   by origin, no rendition leakage).
2. Sweep armed s4,s6,s7,s8 @ q{50,80} + FULL features; label bundle-hurts via
   `scan_pattern2_features.py` (s6/7/8 dominated by s4, meaningful margin).
3. The features here (single + 2-feature LOOCV) FAIL — try (a) a probe-encode signal
   (a cheap s9 vs s4 byte/quality ratio predicts the pattern directly), or (b) new
   descriptors; validate held-out, target precision >0.9 before landing.
4. Remap on true-fires = s6/7/8→s4 (s4 dominates them by definition, like s5→s9);
   the risk is precision, so gate conservatively.
It is RISKY (touches s6/7/8 where the bundle wins on photos+scans). Tool + 39-origin
labeled baseline committed so the dense fit starts here.

## Progress log
- 2026-07-06: program started. gate-monotone (chunk 1) committed ad85a8c4, verified.
- 2026-07-06: chunk-2 fit complete (findings above). chunk-3 `monotone_speed_gate`
  implemented in fast_heads + wired into auto_tune (armed-build-specific, no
  encoder passthrough), committed 7211deb2, verified on origin.
- 2026-07-06: pattern-2 residual measured NOT separable on the 24-origin corpus
  (scan_pattern2_features.py) → scoped as a dense-fit chunk, not shipped.
