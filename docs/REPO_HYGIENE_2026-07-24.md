# Branch + git-bloat inventory and filtering plan (2026-07-24)

Scope: imazen/zenavif, imazen/zenav1-aom, imazen/svtav1. Measured on the
dev-32gb clones after `git fetch --all --prune`; sizes cross-checked against
GitHub's reported repo size. Nothing here has been executed — branch
deletion and history rewriting are user-decision items (unmerged branches
are never deleted unilaterally; filter-repo invalidates clones and rev pins).

## 1. Unmerged-branch inventory

### imazen/zenav1-aom — CLEAN
`main` only. Zero unmerged branches, zero stale refs.

### imazen/svtav1 — 3 branches
| branch | vs master | verdict |
|---|---|---|
| `master` | base | — |
| `wave2/entropy-c-parity` | **fully merged** (strict ancestor) | safe to delete (`git push origin :wave2/entropy-c-parity`); zenavif no longer pins it (rev-pinned to master now) |
| `hdr-hybrid` | +26, active 2026-07-16 | KEEP — live HDR-fork work (chroma-recon fix tip); owned by the hdr program |

(The dozens of `aom/*`, `hdr/*`, `upstream/*` branches visible in the dev
checkout are LOCAL extra remotes — upstream SVT-AV1 and the SVT-AV1-HDR
fork — not refs on imazen/svtav1.)

### imazen/zenavif — 13 unmerged branches (the cleanup target)
Active:
| branch | vs main | note |
|---|---|---|
| `svtav1-rs-backend` | ahead, current | PR #31, CI green — the live branch |
| `cooptloop` | +42 (2026-07-12) | recent bench program (two-pass A/B verdict); results appear recorded in main's docs — confirm then archive |

Superseded by main (content landed via other commits or explicitly closed;
propose: tag `archive/<name>` at tip, then delete branch):
- `caterr-categorized-error` (+489) — PR #27, closed 2026-07-20 as superseded by main's newer taxonomy
- `docs/error-location-and-decode-token` (+464) — README content since rewritten on main
- `hdr-mdcv-st2086-fix` (+461) — #45 fix; verify the ST-2086 scale fix is on main before archiving
- `feat/gainmap-decode` (+234, 2026-03) — gain-map decode long since landed
- `rename-animation-frame` (+251, 2026-03) — rename landed
- `svtav1` (+289, 2026-03) — pre-svtav1-rs spike, superseded by this PR
- `abandoned/spike-av1-backends-2026-05-23` (+411) — self-labeled abandoned

Data/archive branches (hold bench/training artifacts; these carry most of
the 41.9 MB of blobs reachable ONLY from stale branches — decide
tag-and-delete vs keep):
- `work/recover-zenavif-bench-2026-05-08` (+400)
- `docs/recovery-register-2026-05-08` (+399)
- `feat/v0.5-picker-2026-05-04` (+397), `feat/v04-picker-2026-05-04` (+396)

## 2. Bloat measurements

| repo | GitHub pack | uncompressed (all refs) | verdict |
|---|---|---|---|
| zenavif | 15.4 MB | 107 MB | mild bloat, targeted fixes below |
| zenav1-aom | 6.9 MB | ~30 MB | CLEAN — no action |
| svtav1 | 84.9 MB | 396 MB (master-reachable) | the big one; C-history dominated |

Details:
- **zenavif** offenders: `debug_simd` — a 3.7 MB committed BINARY (violates
  the >30 KB rule; filter target #1); `zenavif-parse/mp4parse/tests/avif/
  Microsoft/Summer_in_Tomsk_720p_5x4_grid.avif` 1.9 MB (imported with the
  parse history); `tests/references/*.png` ~1.4 MB (load-bearing for pixel
  tests — keep); `benchmarks/*.tsv` 7.6 MB cumulative (policy-compliant
  records — keep); 41.9 MB uncompressed reachable only from the stale
  branches above (freed by branch deletion + GitHub GC, no rewrite needed).
- **zenav1-aom**: own history is lean (top blobs are the 0.4 MB qm/sys-ref
  tables, legitimately). The 376 MB local `.git/modules/upstream` is the
  aomedia/aom SUBMODULE clone — not our repo. Local-clone relief only:
  `git config -f .gitmodules submodule.upstream.shallow true`.
- **svtav1**: history rooted at the 2026-02-09 graft (already truncated at
  fork time — good), but 1,071 commits of full upstream C churn: giant
  C files revised 100–600× each (`EbProductCodingLoop.c` 158 MB cumulative,
  `enc_handle.c` 127 MB, `EbEncHandle.c` 116 MB, ...). `Source/` = 3.2 GB
  of the 3.4 GB all-refs uncompressed volume; the Rust port (`rust/` +
  `svtav1-rs/`) is only ~90 MB.

## 3. Filtering plan (proposed, NOT executed)

**Phase 0 — prerequisites for ANY rewrite** (applies to phases 2–3):
1. Freeze: coordinate every active session (multiple Claudes push to these
   repos); `.workongoing` claim + announce in the repos' CLAUDE.md.
2. Backup: `git clone --mirror` each repo to `/root/repo-mirrors-<date>/`
   AND push a `backup/pre-filter-<date>` tag set.
3. **Rev-pin audit — the sharp edge**: zenavif's Cargo.toml + fuzz manifest
   pin zenav1-aom and zenav1-svt BY COMMIT SHA. A rewrite of either
   invalidates those pins (and any historical lockfiles). Every rewrite
   must be followed in the same session by re-pinning zenavif (and any
   other consumer) to the rewritten shas, and old shas must be kept
   fetchable via the backup mirror until all consumers re-pin.

**Phase 1 — zenavif branch cleanup (cheap, NO rewrite; needs user OK per
branch):** tag each approved stale branch `archive/<name>` (or delete
outright for `abandoned/*`), delete the branch, let GitHub GC reclaim.
Recovers the 42 MB stale-only volume from future clone costs and removes
11 branches of navigation noise. Reversible while the archive tags exist.

**Phase 2 — zenavif filter-repo (small, do AFTER PR #31 merges):**
`git filter-repo --invert-paths --path debug_simd --path
zenavif-parse/mp4parse/tests/avif/Microsoft/Summer_in_Tomsk_720p_5x4_grid.avif`
Est. 15.4 → ~9 MB pack. Costs: all clones re-clone; open PRs must be
closed first; crate-prefixed release tags are preserved by filter-repo but
must be force-pushed. Only worth doing bundled with Phase 1's GC.

**Phase 3 — svtav1 C-history flattening (the big win, optional):**
Replace the C history with its tip: filter `Source/ Docs/ test/
third_party/` to HEAD-only content (filter-repo path-rename/prune of
pre-tip blobs), keeping full history for `rust/` + `svtav1-rs/` + build
glue. Est. 85 → ~15 MB pack. Preconditions beyond Phase 0: the byte-parity
program only needs the C reference AT the pinned tip (history archaeology
uses the upstream remotes, which remain available); confirm with the
active svt sessions before scheduling. Alternative if rewrite is deemed
too disruptive: accept 85 MB (it is not growing fast — C churn stopped at
the graft) and only do Phase 0.3's pin-audit discipline going forward.

**Recommended order:** Phase 1 now (after per-branch user sign-off) →
Phase 2 at the PR #31 merge point → Phase 3 only if svt clone times
actually hurt (it is a one-time 85 MB; the upstream-submodule shallowing
in zenav1-aom saves more local disk than any origin rewrite).
## Merged-branch deletions 2026-07-24 (recovery shas)
Re-create any with: git push origin <sha>:refs/heads/<name>
zenavif origin/docs/error-location-and-decode-token 60c38cd1bbe310e691818be8543ed68b28b0e145
zenavif origin/hdr-mdcv-st2086-fix b0c753d62ae11eae8ea177b3a323d93c68ed40b8
svtav1 origin/wave2/entropy-c-parity 853c9cf48c0791a0e1a907e09d0143c6ce7cfc2c
