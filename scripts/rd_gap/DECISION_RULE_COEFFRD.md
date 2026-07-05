# Pre-registered decision rule — COEFF_RD_STACK posture arms (2026-07-05)

Registered BEFORE any arm TSV was read (base phase done + byte-continuity
gate 288/288 vs `ssimrd/base_s2`; coarse arms still running on
zenavif-sweep-2 at registration time). Binary: ravif--coeffrd cavif
sha256/16 `f4e17fbb7de6f0c4` (ravif main d72304a + tune/palette/coeffrd env
passthroughs → zenrav1e--coeffrd @ master 3e5ff155). Same evaluation-policy
shape as `DECISION_RULE_SSIMRD.md` (the 93b83401 policy): per-family FIRST,
cluster-mass weights, photos-merit KEEPABLE.

## Families + masses (train26, `_MANIFEST.json`)

photos {1236:56, 1438:43, 1614:100, 2000:21, 5004:14, 5048:45},
products/gen {9958:117, 9868:109, 9678:84, 9228:50, 9074:50},
screenshots {8268:115, 8196:78, 8302:50, 8414:8},
illustrations-9094 {9100:38, 9118:17},
scans {6096:32, 6018:12, 6606:2},
plots {7028:25, 7058:14, 7052:1, 7050:1}. Total mass 1082.

Doccharts supplement (15 origins, `sample_doccharts.tsv`): reported as its
own slice (the 6096-class near-lossless content is in-distribution there);
doccharts movement CANNOT rescue a train26 rejection, but a doccharts
regression > +0.50 median at the chosen arm is a veto.

## Winner selection (coarse, t26 6q, arms A-E)

Arms: A `128:0.1328:1:0` (aom ss2 posture), B `128:0.35:1:0`,
C `128:1.0:1:0`, D `128:4.25:1:0` (aom default-tune posture),
E `128:1.0:0:0` (unguarded control).

The arm with the best cluster-mass-weighted train26 median ssim2 BD vs the
same-binary env-off base, restricted to arms where NO family's butteraugli
medians (3n AND max) exceed +0.50 and the mass-weighted butteraugli medians
are ≤ +0.30. Tie within 0.1 BD → the arm closer to the aom-verbatim posture
(A > B > C > D > E). If no arm clears the mass-weighted bar but some arm
wins the photos family cleanly (photos median ssim2 ≤ −0.30, photos
butteraugli ≤ +0.30, other families ≤ +0.30 ssim2), that arm advances as a
PHOTOS-MERIT candidate. Arm F (winner + tu_zero_out) is measured only if
the winner's per-family slices show a plots/flat over-keep regression
(fam-7000 or doccharts median > +0.50); F must then beat the winner on the
regressing family without costing the winning families > 0.1.

## Ship bar (ALL must hold at WINNER, full 12q)

1. TRAIN (t26) mass-weighted median ssim2 BD ≤ −0.30% — OR the photos-merit
   form: photos-family median ≤ −0.30 with every other family ≤ +0.30.
2. TRAIN butteraugli: mass-weighted median ba3n ≤ +0.30 AND bamax ≤ +0.30;
   no family's ba3n/bamax median > +0.50. Per-cell fire-conservative veto:
   an image with ssim2 BD < 0 but ba3n BD > +1.0 is vetoed — banked as 0 in
   win counts.
3. Doccharts (12q at winner): median ssim2 BD ≤ +0.30 (supplement veto).
4. s6 spot check (6q): the armed posture must not regress the composed
   fast mode by > +0.50 median ssim2 — if it does, the knob ships as an
   s1/s2-only (slow-tier) arm, which is an acceptable verdict, not a kill.
5. Encode time: report solo walls; no hard bar (RD-first program, the
   trellis-in-every-trial cost is expected ≥ 1.66×) — but the verdict table
   must carry the measured ratio so the tier programs can budget it.
6. Conformance: full battery at the winner (both samplings), aomdec +
   rav1d-safe byte-agree, 0 CONFFAIL (quantization/eob semantics changed —
   the full-battery rule from CLAUDE.md applies).

## Termination

If no arm advances under rule 1 (mass-weighted AND photos-merit both
missed), the program terminates as an HONEST NEGATIVE: the composed stack
does not transplant either, completing the bracket around the two
half-stack rejections — and the doc verdict records that the coefficient
wall is REFUTED as a transplantable mechanism at s2 (the residual is then
aom's search-depth/valuation interplay not reachable by posture ports).
s1probe evidence (legacy val/test origins) is REPORT-ONLY in every branch
and cannot flip a verdict.
