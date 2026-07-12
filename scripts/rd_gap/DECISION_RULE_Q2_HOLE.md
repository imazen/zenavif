# Pre-registered decision rule — Q2: the ladder-hole tiers (2026-07-12)

Registered BEFORE any graft arm was swept.

**Question:** does grafting s6-bundle members onto armed s9 (S10_RETIER row) produce a
tier in the 700–2600 ms solo band that WINS the svt p1t4 (1916 ms) and/or p2t4
(1323 ms) matched-time cells — the two G2 failures caused by the ladder hole?

**Arms (ZENRAVIF_Q2_GRAFT env, ravif--cooptloop@ae5925a):** base s9 (no graft), +i7
(ComplexKeyframes/filter-intra-off), +prune (rects@16 under the P1 gate triple),
+txd2 (size-RDO depth 2), +txmin (size1 + type-RDO reduced set), +i7prune. All at
speed 9, 4:2:0, train26.

**Procedure:** coarse-select on the 6q grid {30,50,60,75,85,95} (candidate search, not
verdict); the winner(s) re-run on the Q1 dense grid for the confirm. Walls: median
enc_ms under the standard 12-job dispatch for selection; the confirm's time claim uses
a JOBS=1 timing pass.

**Decision rule per SVT cell (p1t4, p2t4):** a graft config WINS its cell iff its solo
median wall ≤ 1.2× the reference's AND banded ssim2 BD ≤ 0% in every band with
overlap AND no butteraugli veto (objective.py). Any config winning either cell → Q2
positive (commit config + verdict). No config winning either → Q2 HONEST NEGATIVE:
commit the sweep TSVs + the gap sizes; the hole then belongs to Phase-4 proper
(budget-tier derivation on the new loop) rather than graft shortcuts.

## Incident (2026-07-12, recorded before re-sweep verdicts): first sweep partially void

The first 6-arm sweep produced byte-identical RD for {base, i7, txd2} and for
{prune, i7prune}. Root cause: the graft match ran BEFORE `speed_settings()`'s
expert-override block, so the armed s9 tier's own knobs (size1 depth, SATD
num_modes, prune quartet) clobbered graft fields — txd2's depth-2 was directly
overwritten by the retier's depth-1; i7prune's prune half survived but its i7 half
composed with the same clobber question as i7 alone. Fixed at ravif--cooptloop@
679feb3 (graft match moved to run LAST; unset env still byte-identical). All six
arms re-swept post-fix. prune/txmin numbers from sweep 1 were real (those fields
were not expert-clobbered) and their selection outcome is expected to replicate.
NOTE: if i7 remains byte-identical to base post-fix, that is a MEASURED
genuinely-inert verdict at armed s9 (plausible mechanism: num_modes_rdo_override=1
collapses mode-set expansion), not a harness defect.

## VERDICT (2026-07-12, post-orderfix re-sweep): HONEST NEGATIVE

No graft config wins either SVT cell under the pre-registered rule. Re-swept arms
(ravif--cooptloop@679feb3, grafts run LAST), coarse 6q, walls 12-job selection medians
vs SOLO refs (p1t4 1958 ms / p2t4 1355 ms):
  base  539 ms  +12.62/+9.81 mass (vetoed)      i7     == base byte-identical —
  txd2  621 ms  +11.28/+8.50 (vetoed)                  MEASURED genuinely inert at
  prune 883 ms   +9.05/+6.41 (vetoed)                  armed s9 (num_modes_rdo=1
  txmin 1020 ms  +8.24/+5.61 (vetoed; high-band        collapses mode-set expansion);
                 -0.29 vs p2t4 only neg cell)          i7prune == prune accordingly.
prune/txmin replicate sweep-1 exactly (their fields were never clobbered — predicted).
Grafts move walls INTO the 700-2600 ms hole but leave +5.6-9.1% mass with butteraugli
vetoes everywhere: s6-bundle members grafted on s9 do not manufacture a competitive
tier. The hole passes to Phase-4 proper (budget-tier derivation on the coopt loop).
No dense confirm run (rule: winners only). TSVs: benchmarks/q2_hole_2026-07-12/
(sweep-1 void-for-cause raws kept at /mnt/v/.../q2-hole-2026-07-12/pre-orderfix/).
