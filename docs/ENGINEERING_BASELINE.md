# Engineering baseline: locked expectations for the cleanliness/refactor pass

Locked 2026-07-05, before any refactoring, per user directive. This is the contract the
pass is held to — what the codebase must GUARANTEE afterward, and what structural
qualities we are refactoring TOWARD. Scope: zenavif + ravif + zenrav1e (the encode stack)
and the measurement infrastructure around them.

Current debt inventory (measured): ravif 7 `*_LIVE` gated consts + 16 uncomment-at-bump
sites; zenrav1e ~50 public knob fields; zenavif 30 src modules, 44 rd_gap scripts, 17
docs, 136 benchmark files, 27 status sections in CLAUDE.md.

## A. Invariants — locked as EXECUTABLE gates, not conventions

These are the properties that made the program's velocity possible. Each must exist as a
runnable check (justfile + CI where marked) before the refactor starts, so the pass can
be verified change-by-change. A refactor commit that can't run the relevant gate doesn't
land.

1. **Byte-exactness of the off-state.** Every encoder knob/feature whose default is
   "off" produces byte-identical bitstreams to the knob's absence. Gate: the md5
   identity harness (the 27/27, 36/36-class checks used all program) promoted from
   ad-hoc script runs to `just gate-identity` with a pinned cell grid. CI on zenrav1e.
2. **Cross-decoder conformance.** Every armed configuration decodes aomdec-clean AND
   rav1d-safe raw-md5-agrees (the PALCONF protocol). Gate: `just gate-conformance`
   running the pinned grid on a small fixed corpus; the full-sweep version stays in
   scripts. The 6-class silent-corruption taxonomy (project memory) is the review
   checklist for any change touching coding paths.
3. **Determinism vs parallelism.** Bytes are independent of thread count and core count
   (P0's tile fix made this true; keep it true). Gate: encode at threads {1, 8, max}
   → identical md5. CI.
4. **Decode robustness.** No panics, no unbounded allocation on untrusted input
   (bug-sweep's #18/#21 work). Gate: the fuzz regression harness + `try_*` fallible
   paths stay; new decode-path allocations use the established alloc_util helpers.
5. **Recon honesty.** Encoder reconstruction byte-agrees with conforming decoders at
   every shipped speed/config (the #32/#33 class). Gate: the recon-probe example kept
   runnable; any new filter/prediction work re-runs it.
6. **Perf floors.** The ladder positions are regression-gated coarsely: s2-tune,
   s6-composed, s10' each keep a pinned (bytes, ssim2, enc_int_ms) envelope on a
   3-image fixed cell set — catches accidental de-tuning during refactor. `just
   gate-ladder` (local, not CI — timing).
7. **Provenance of constants.** Every fitted constant in source carries its
   corpus+date+TSV pointer comment (already the norm — the pass must not strip them,
   and should ADD them where missing).

## B. The structural target per repo

### ravif (the policy layer)
- **Kill the `*_LIVE` const + commented-apply-line pattern at the dep bump.** It was
  correct for release-gating against a registry dep; it does not survive as
  architecture. Target: ONE `SpeedPolicy` table (per-tier struct of knob values,
  data not code) that `speed_settings()` reads; the 7 flips + 16 uncomment sites
  become one table edit. The dep-bump flip becomes a single reviewable diff.
- Tier policy (which knobs at which speed) must be READABLE AS A TABLE — today it is
  scattered through conditionals. This is the single highest-value readability fix.

### zenrav1e (the mechanism layer)
- **Knob taxonomy with verdicts.** Every public knob gets classified in one doc table:
  `default-on` / `expert` (kept, measured-positive somewhere) / `probe` (kept
  default-off, measured-negative, retained for refit — with its verdict TSV pointer) /
  `dead` (measured-dead everywhere → DELETE: the margin-gate semantics that never won,
  any knob with no surviving caller). Deleting dead knobs is in-scope for the pass;
  deleting `probe` knobs is not (they are cheap and carry provenance).
- Mechanism modules stay separable: palette, intrabc(+hash), topdown_prune, the tune's
  five mechanisms — each already lives in its own module/commit lineage; the pass must
  not merge them into monoliths. New rule: one mechanism = one module + one knob + one
  test file exercising its liveness AND its off-identity.
- The `rdo.rs` hot paths keep their fixed-array/bounds-check discipline; refactor for
  clarity must show `cargo asm`-level parity or better on the two measured hot kernels.

### zenavif (the product layer)
- **Public API review before anything else grows**: the config surface accumulated
  `with_*` methods across gain maps, HDR, palette, target-quality, two-pass, heads.
  Expectations: everything not needed by imageflow/zencodecs integration goes
  `pub(crate)`; the `EncoderConfig` builder is the one public entry; feature flags
  (`encode`, `auto-tune`, `target-quality`, `two-pass-butteraugli`, `__expert`) each
  carry a doc sentence stating their support tier. Additive-only from here (semver).
- **Heads/gates are recommend-only until the dep bump** (fast_heads, palette gate,
  auto_tune) — the pass must keep the recommend/apply seam explicit, because that seam
  is what let every head ship safely gated.
- The dropped-but-kept infra (two-pass driver, sweep planner) moves behind `__expert`/
  dev features with a one-line status header pointing at its verdict doc — kept
  BECAUSE the FrameHints consumers are coming, but clearly marked not-a-product-path.

### Measurement infrastructure (scripts/rd_gap + label store)
- **Promote the load-bearing four, archive the rest.** Load-bearing: `cell_cache.sh`,
  the PALCONF cell protocol (`zenrav1e_cell.sh`/`aom_cell.sh`), `run_gap.sh`, the
  label-store builder. These get header docs + a smoke test. The ~30 per-program
  `chain_*.sh`/analyzers are HISTORY — move to `scripts/rd_gap/archive/` with their
  program docs; they re-run only against their pinned TSVs.
- The label store is a first-class artifact: its builder, schema, and the
  tune-marginal-drift rule (baselines must be current-master encodes) documented in
  one place (`docs/LABEL_STORE.md`), because every future head/refit consumes it.

### Docs + CLAUDE.md
- **Split current-state from history.** RD_GAP_VS_LIBAOM.md and the 27 CLAUDE.md
  sections are append-only logs — correct as records, wrong as onboarding. Target:
  one `docs/STATUS.md` (the current ladder, the current defaults, the open residuals,
  the release-gate list — regenerated at each milestone) + the existing logs kept
  as-is for provenance. CLAUDE.md's Known Bugs shrinks to OPEN items + one pointer;
  resolved sections move to `docs/HISTORY_KNOWN_BUGS.md` at the dep bump.

## C. Sequencing — what the pass must wait for

1. **COEFFRD finishes first** (in flight): it may touch rdo.rs/quantize; refactoring
   under it guarantees conflicts.
2. **The dep bump lands before the ravif policy-table refactor** — the flip DELETES the
   gated scaffolding; refactoring the scaffolding first is wasted motion. Order:
   release train → flip per checklist → THEN collapse to the SpeedPolicy table.
3. Everything in section A (gates) can and should land BEFORE the bump — they verify
   the flip itself.
4. The measurement-infra + docs consolidation can run anytime (no code risk).

## D. What the pass must NOT do

- No behavior changes disguised as cleanup: every refactor commit is byte-identity-
  verified (gate A1/A2) or explicitly declared behavioral with a measurement.
- No test relaxations, no `#[ignore]`, no graceful skips (standing rules).
- No deleting `probe` knobs, provenance comments, benchmark TSVs, or program docs —
  history is load-bearing here (it is how verdicts avoid being re-litigated).
- No public API breaks in zenavif/ravif without the semver ceremony; zenrav1e's next
  release already carries its queued breaks (0.2.0 window) — additions ride it, but
  each addition still gets the QUEUED BREAKING CHANGES entry.
- No merging mechanism modules or collapsing the recommend/apply seam.

## E. Definition of done for the pass

- `just gate-identity && just gate-conformance && just gate-ladder` green before and
  after every refactor commit; full test suites green per crate; clippy -D warnings.
- The dep-bump flip is one diff against one table, reviewed against the CLAUDE.md
  checklist, with gates green on both sides.
- A newcomer can answer "what does speed N do, and why?" from SpeedPolicy + STATUS.md
  + one knob-taxonomy table without reading history docs.
