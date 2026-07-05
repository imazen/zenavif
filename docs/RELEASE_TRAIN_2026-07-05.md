# Release train — staged 2026-07-05, each car awaits explicit user "go"

Every car follows the full ceremony: local `cargo test --all-targets` + doctests →
README verified by user → CI green ALL platforms → tag → GitHub release → `cargo publish`.
Nothing below is published; this is the staged state and exact order. The train is the
critical path to production: ~9 measured wins are release-gated behind car 4.

## Car 1 — rav1d-safe 0.6.0  ← LEAD (registry ships silent ARM pixel corruption today)
- Ships: the tile-threading race + worker-panic wedge fix (49df1fc0), the aarch64
  looprestoration set incl. the 16bpc truncate-vs-round silent corruption (0a1f961 +
  da53bfa3/710537f8), segment-id clamp underflow, 7 NEON MC-blend overshoots, committed
  LR regression vectors + threaded md5 gates.
- Why 0.6.0 not 0.5.8: main already carries `managed::Result` → `At<Error>`, new
  `Error::Cancelled`, `simd_test` feature rename (semver-checks-confirmed; prior
  sessions' work). Staged at `0579614` + follow-ups; CI 18/18 green (run 28717747678);
  114/114 nextest + 8/8 doctests.
- After publish: zenrav1e dev-dep bump `"0.5.7"`→`"0.6.0"` (unblocks its ARM CI leg,
  zenrav1e#37), zenavif dep `"0.5.3"`→`"0.6.0"` (the hang_stress + tile-race fixes
  reach users; re-run hang_stress per the #30 note).

## Car 2 — zenavif-parse 0.6.3 — MUST publish from `c36b822`, NOT main
- Ships: the size=0 extends-to-EOF mdat fix (f3c9f043) → zenavif's 12 failing HDR
  corpus vectors decode; closes zenavif#16 (57/57). CI+Fuzz green at that commit since
  06-11. zenavif needs zero code change (`"0.6.0"` req resolves to 0.6.3).
- main is now 0.7.0-pre (At<Error> migration) and gated on the zencodec release
  (car 5); its Fuzz red was the missing patch mirror — fixed 79551cf.

## Car 3 — zenavif-serialize 0.2.0
- Ships: gain-map mux correctness (alt-colr nclx+ICC carriers, av1C honesty +
  seq_profile, tmap brand + altr group + ispe + pixi — the libavif-interop set),
  try_to_vec/fallible API, u32 size guard (kills the >4 GiB silent-truncation class),
  output cap, forbid(unsafe_code). 71 tests + CI green (incl. clippy fix 0a48b468).
- 0.2.0 because the At<SerializeError> write-API break was already on main.

## Car 4 — the encoder chain: zenrav1e → zenravif/ravif → zenavif dep-bump + FLIP
- zenrav1e release past 0.1.4 (0.2.0 window; QUEUED BREAKING CHANGES in its changelog):
  ships Tune::Ssimulacra2 + per-SB delta_q/variance boost + QM-dist ratio + LF schedule
  + palette (luma+UV+Auto) + intraBC A/B + topdown_prune + tx/intra knobs + FrameHints
  + all conformance/desync fixes (6 corruption classes, #29 QM, #32/#33, #34/#35).
- zenravif (cavif-rs fork) 0.2.0 publish. NOT a first publish and encode is NOT off on
  registry: zenravif 0.1.3 IS published, so `zenavif 0.1.6` + `encode-imazen` already
  encodes today — but at zenrav1e 0.1.4 with EVERY gated win OFF (no tune, no palette, no
  s1/s6/s10 arms) AND carrying the 0.1.4 recon-desync bugs (measured: registry s5/s6/s9
  decode-quality crashes at Q85 — benchmarks/vs_cratesio_per_speed_2026-07-05.tsv). This
  car publishes 0.2.0 (gray/expert/tune-forwarding) so the flip below turns the WINS on.
- zenavif: bump deps + execute the CLAUDE.md dep-bump checklist — the flip of the 7
  gated consts (S1_DEEP, SMALL_PX_RDO_TX, S6_TX_SIZE_RDO, S6_PART_PRUNE, S6_INTRA7
  [re-weigh vs top-5 per S4TIER], S10_RETIER, FRAME_HINTS) + 16 uncomment sites +
  encode_plan mirror refresh + palette-gate forward + tune default decision +
  alpha-tune=Psnr guard + identity-test tightening (zenavif#8 closes) + QM re-benchmark.
  The §A gates (gate-identity/conformance/determinism/ladder/recon) verify the flip.
- After: re-run the ladder confirm on registry deps; close #8, #16, #28-notes, #37.

## Car 5 — zencodec 0.1.26 (PR #103 taxonomy) — needed for the LATER items
- Unblocks: zenavif-parse 0.7.0, the parked gain-map zencodec-trait impl
  (workspace `wrnqptsz`), dropping the two git-branch patches (parse root + fuzz).
- Owned by the caterr/zencodec program (another session's stack) — coordinate, don't
  unilaterally release.

## Post-train (production-readiness closers)
- The refactor pass per docs/ENGINEERING_BASELINE.md (SpeedPolicy table collapse —
  sequenced AFTER the flip deletes the gated scaffolding).
- docs/STATUS.md regeneration; CLAUDE.md Known-Bugs shrink (most sections resolve).
- rav1d-safe #423 (flush semantics) + #414 (NEON conformance) as the decoder follow-ups.
