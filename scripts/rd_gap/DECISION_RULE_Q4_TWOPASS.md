# Pre-registered decision rule — Q4: the two-pass-as-kernel A/B (2026-07-12)

Registered BEFORE the A/B ran. Choice rationale vs DFIT8: the seven-diagnostic arc
proved the D kernel's missing information is not source-computable offline; the two
remaining carriers are transform-domain in-encoder features (DFIT8 — encoder surgery)
and DECODE-SIDE information. The two-pass butteraugli loop (encode_rgb8_two_pass,
feature two-pass-butteraugli, FRAME_HINTS_LIVE=true on this branch) already carries
decode-side information into per-SB q scaling — it is the cheapest decisive test of
the decode-side route, so it runs first.

**Question:** does the two-pass diffmap loop, on the ARMED branch at matched TOTAL
time, buy banded RD over single-pass — i.e. is decode-side sensitivity worth its 2x
cost as a kernel mechanism TODAY?

**Arms (train26, 420, 6q coarse select):**
  BASE: armed single-pass s6.
  ARM A: two-pass at s6 (pass1+pass2 both s6) — ~2x wall.
  Time-fair comparison: ARM A vs the SLOWER single-pass reference nearest its wall
  (armed s6's 2x-wall neighbor has no tier — the hole — so the fair frame is
  "ARM A vs BASE at +100% time" reported alongside "ARM A vs svt p0t4/aom cpu2-ss2ai
  at ARM A's matched band").

**Decision rule:** the two-pass mechanism is KERNEL-WORTHY iff ARM A vs BASE shows
banded ssim2 BD <= -2.0% in at least two bands with butteraugli veto clean (the
mechanism must pay clearly, not marginally, to justify 2x). It is a G2 ladder
candidate additionally iff ARM A beats svt p0t4/aom-cpu2-ss2ai at its matched band.
Negative on both = decode-side-at-2x refuted for the still ladder -> DFIT8
(transform-domain) becomes the sole kernel route; record and proceed.

## VERDICT (2026-07-12): DOUBLE NEGATIVE — decode-side-at-2x refuted

KERNEL-WORTHY leg FAILS: twopass_s6 vs single_s6 mass ssim2 BD = +1.45% (a regression,
not the required <=-2.0%), positive in ALL three bands (+1.06/+1.21/+2.84),
butteraugli VETOED (+1.72 ba3n / +3.28 bamax). Ladder leg FAILS: vs svt p0t4 +2.45
vetoed; vs aom cpu2-ss2ai +30.9. Wall ratio 1.91x (medians 8817/4621 ms; absolute
walls load-inflated ~8x by a foreign 280-thread sync — the ratio is same-window).
Per the pre-registered rule: DFIT8 (transform-domain in-encoder features) becomes the
SOLE kernel route; the two-pass loop is refuted as a ladder mechanism TODAY.
Honest scope notes: (1) one config tested (shipped driver: butteraugli diffmap,
aom-formula 12-norm pool, strength 1.0, clamp [0.4,2.5]) — this is the registered
arm, not the mechanism's tuned ceiling; (2) the armed stack already carries per-SB
spatial adaptation (Variance Boost delta_q + QM-dist ratio + LF schedule) — the
diffmap's marginal was plausibly consumed by composition (the tune-marginal-drift
pattern), consistent with the 2026-07-03 evaluate-first result (-2.4..-3.5% ba3n on
PLAIN aom tunes) no longer transferring to the composed tune.
TSVs + verdict: benchmarks/q4_twopass_2026-07-12/.
