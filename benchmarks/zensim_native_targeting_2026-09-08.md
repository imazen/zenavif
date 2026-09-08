# Native AVIF targeting — September 8, 2026

**Completed development screen; no spatial improvement or shippable-model claim.**
Train-calibrated scalar targeting hits 28/28 jointly witnessed requests within
±1 by three complete encodes. Active zerosum spatial steering hits 24/28,
with worse error tails and mixed independent rate-distortion results. The
new map binding is exercised, but this strategy remains a failed shipping
candidate. The default codec/model settings are unchanged.

## Frozen experiment

Twelve canonical imazen-26 training families fit codec-specific seeds; eight
separate development families supply evaluation (two each photo, document,
graphic and screen). No terminal holdout is consumed. Inputs are opaque sRGB8,
long side 256. Configuration: Zenravif speed 6, 444, one encoder thread,
CQ 1..255 at 17 knots; D by-ID, formula revision 1; bin 8, zerosum gain 10,
factor clamp 1.15. Neutral gain is zero. All candidate scores/maps execute
via the complete Rust BakeScorer surface. Exact identities and owner output:
[result JSON](zensim_native_targeting_2026-09-08.json).

At each bound CQ, state resets and runs one scalar or three map-arm encodes.
Bounds finish before controllers execute; controllers never receive them.
Only FIRST-encode training scores fit seeds, because no previous map exists
on shot one. The three calibration curves are identical. Each targeting shot
is exactly one complete encode/decode; subsequent active shots can consume
the preceding decoded map. All selected bytes are decoded/scored again.

## Coverage and targeting

Only **28/80 requests** have a witness within ±1 in all arms, including two
negative requests. Scalar/neutral separately witness 52/80; active witnesses
32/80. The remaining sparse-grid gaps are unresolved, not proven impossible.
The 504 cases compare all arms, policies and budgets on the joint subset.
±1 is an operational score tolerance, not a universal perceptual error bar.

| Calibrated arm | Maximum encodes | Hits ±1 | Median absolute error | p95 | Undershoots >1 | Median total ms |
|---|---:|---:|---:|---:|---:|---:|
| scalar | 1 | 12/28 | 1.247 | 6.689 | 14 | 295.6 |
| scalar | 2 | 23/28 | 0.432 | 3.640 | 1 | 480.8 |
| scalar | 3 | 28/28 | 0.324 | 0.849 | 0 | 480.0 |
| neutral | 1 | 12/28 | 1.247 | 6.689 | 14 | 294.2 |
| neutral | 2 | 23/28 | 0.432 | 3.640 | 1 | 479.9 |
| neutral | 3 | 28/28 | 0.324 | 0.849 | 0 | 480.8 |
| active | 1 | 12/28 | 1.247 | 6.689 | 14 | 296.2 |
| active | 2 | 19/28 | 0.591 | 5.151 | 1 | 485.8 |
| active | 3 | 24/28 | 0.376 | 2.175 | 2 | 491.0 |

Midpoint three-shot controls hit 14/28 scalar/neutral and 13/28 active.
All 168 neutral target results match scalar bytes and scores exactly; all
56 first-shot active results also match scalar exactly. Consumed active maps
across the calibrated 1/2/3-shot cells are 0/16/25. Neutral maps are measured
but do not change encoder state.

## Independent emitted-pixel judging

All 912 validation images (408 bounds + 504 selected outputs) have finite
SSIMULACRA2 and Butteraugli pnorm3 results from the pinned CPU judge.
Positive savings below mean fewer bytes at matched judge quality, using
per-image log-byte interpolation without extrapolation against the scalar
bound ladder. These are descriptive point medians, not independent-family
confidence intervals or direct matched-quality confirmation.

| Active output | Content | SSIMULACRA2 byte savings | Butteraugli byte savings |
|---|---|---:|---:|
| Fixed-CQ bounds | photo | 0.052% | -1.824% |
| Fixed-CQ bounds | document | 0.052% | -2.051% |
| Fixed-CQ bounds | graphic | 0.133% | -2.673% |
| Fixed-CQ bounds | screen | 0.600% | -4.806% |
| Calibrated three-shot | photo | -0.092% | -0.275% |
| Calibrated three-shot | document | -1.447% | 0.212% |
| Calibrated three-shot | graphic | -1.623% | -0.663% |
| Calibrated three-shot | screen | -0.697% | -5.079% |

Fixed-CQ active maps slightly improve SSIMULACRA2 but worsen Butteraugli in
every content class. Three-shot active outputs worsen SSIMULACRA2 in every
class and Butteraugli in three of four. Sparse interpolation includes a
sampling effect even for identical scalar output at a new knob; first-shot
interpolation savings are not causal map benefit. Neutral fixed-CQ judge
comparisons are zero to floating-point rounding. All pair identities,
encoded hashes and decoded pixel hashes are verified before aggregation.

## Full cost and validation

Train bounds cost 1,428 full encodes over 612 records. Validation bounds cost
952 full encodes and 816 map comparisons over 408 records, separately from
815 full target encodes, 815 outer scalar comparisons, 545 map comparisons
and 504 terminal decode/score checks. Of those maps, 107 non-neutral fields
are actually consumed. There are zero extra internal reconstructions beyond
the complete AV1 encodes. Final unused maps still count as evaluations.
Process high-water RSS is 140,440 KiB; it is not a per-arm peak estimate.
Total time includes search plus terminal decode/score, excludes offline
bounds/calibration and PNG output. Fixed-order 256-pixel timings support no
general speedup claim. Another 204 train-only scalar outputs were judged
independently; validation outputs are not newly admitted training labels.

Checks: pinned Git dependency closure release build and example Clippy;
four shared Rust seed/search tests; six pre-output source/calibration
rejections; four original analyzer rejection controls; three new judge-pair
controls (wrong reference, duplicate, missing). Re-analysis leaves every
JSON field unchanged for this AVIF study and the prior 810-case JXL study.
Local script lint and diff checks pass. No CI was awaited.

## Reproduction and remaining gates

Use `examples/zensim_cq_rd.rs` at zenavif `d20915f46256`, with zensim
`8462c824042b`. Full commands, explicit environment, calibration and pinned
binaries are retained under `/mnt/v/output/zensim/avif-native-target-2026-09-08/`
in `PINNED_COMMANDS.json`, `JUDGE_COMMANDS.json` and `RESULT_COMPLETE.json`.
Use fresh output paths; never overwrite the historical study. The shared
analyzer is `zensim/scripts/v_next/rd_probe_analyze_2026-07-18.py --target-loop
<validation-output>`. A compact Windows-accessible copy is under
`~/work/zensim-validation-2026-09-08/avif-native-targeting/`.

Spatial utility, broad attainable coverage, perceptual qualification and
HDR/alpha/full-resolution targeting remain failed or incomplete. This record
does not freeze a replacement model. JPEG/WebP candidate bindings and useful
native allocation still need work. The later September 7 paired H+rav1e
floor study already failed all five floor gates; the September 6 proposal to
add that data is historical, not an unperformed next experiment.
