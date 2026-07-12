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
