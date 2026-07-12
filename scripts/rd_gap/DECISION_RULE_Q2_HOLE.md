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
