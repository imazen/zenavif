# SPEED-LADDER GAP MAP — zenrav1e vs libaom across the full speed range (2026-07-04)

**Question:** everything to date (docs/RD_GAP_VS_LIBAOM.md) measured s1/s2 vs cpu0/cpu2 —
the slow end. The real web workload lives at the FAST tiers (libavif defaults to speed 6).
Who wins at matched wall-time across the whole ladder, and where is the crossover?

**Grid** (all cells BUTTER on, 420, 1024-long-edge renditions):
- zenrav1e: cavif `-s{2,4,6,8,10} --threads 1 --depth 8`, 12-q grid Q{30..95}, each ×
  {tune = `ZENRAVIF_TUNE=ssimulacra2 ZENRAVIF_PALETTE=auto` (the shipped-best config,
  final-sweep recipe), tune-off (plain default)} — 10 arms. cavif via the ravif dev-patch
  (path dep → zenrav1e master@origin `184a616f`, tune/palette env passthroughs,
  `S1_DEEP_ARMS_LIVE=true` [inert at s≥2]; patch reverted after, never committed).
- libaom (pinned 3.14.1 `632172a4`, build_slow): `aomenc --allintra --cpu-used={2,4,6,8,9}`
  × {default, `--tune=iq`} — the libavif-1.4 still-image operating mode (`--tune=iq`
  verified present in the pinned build) — plus the cached GOOD-mode continuity anchors
  (cpu2-default, cpu0-default, cpu0+tune=ssimulacra2). cq grid {8..63}, standard
  `--end-usage=q --passes=1 --lag-in-frames=0` cell flags.
- Corpora: train26 (24 TRAIN origins, 12 families — primary, per-family slices) +
  legacy 22 (continuity).
- Conformance: **every zenrav1e RD cell ran PALCONF** (extract AV1 → aomdec must decode
  cleanly AND byte-agree with rav1d-safe) — the fast tiers exercise TX_MODE_LARGEST and
  reduced-tx paths the slow tiers never hit.
- Timing: separate solo pass, `RD_CACHE=off`, JOBS=1, 4 images × 3 q × every arm
  (`sample_timing4.tsv`), single-threaded encoders, wall ms/MP.
- Box: zenavif-sweep-1 (ccx63, 48c), restored from snapshot; harness
  `scripts/rd_gap/chain_speed_ladder.sh` + `analyze_speed_ladder.py`.

Results TSV: `benchmarks/rd_gap_speed_ladder_2026-07-04.tsv`; raw per-arm TSVs:
`/mnt/v/output/zenavif/speedladder-2026-07-04/` (+ Tower mirror, pointer file).

---

## Mechanism liveness at the fast tiers (source audit)

Verified in source at ravif `a284209` (`ravif/src/av1encoder.rs` SpeedTweaks) + zenrav1e
`184a616f`. cavif builds zenravif **without the `imazen` feature** → `enable_qm=false,
enable_vaq=false, enable_trellis=false, seg_boost=1.0` in every arm; QM/boost/etc. arrive
only via the tune. `quantizer` thresholds: `low_quality` = qindex>150 (~Q≤50),
`high_quality` = qindex<80 (~Q≥80).

### Speed-table axis (what `-s` actually moves; still images, 1024 px, `--threads 1`)

| mechanism | s2 | s4 | s6 | s8 | s10 | source |
|---|---|---|---|---|---|---|
| partition_range (min,max px) | (4,16); (4,32) at Q≲50 | (4,16) | (8,16) | (8,16) | (16,16) | ravif:1615 |
| rect partitions (HORZ/VERT) offered at bsize ≤ | 64×64 | 8×8 | 8×8 | 8×8 | **dead** (min block 16) | ravif:1676; zenrav1e encoder.rs:4470 |
| HORZ_4/VERT_4 (extended 4-way) | live (16/32/64 parents) | **dead** (16×16 ≰ 8×8) | dead | dead | dead | zenrav1e encoder.rs:4477 |
| mixed 3-way (HORZ_A/B, VERT_A/B) | off (s1-only) | off | off | off | off | ravif:1681 |
| split_trial_depth | 1 | 1 | 1 | 1 | 1 | ravif:1685 |
| tx-size/type RDO → tx_mode | SELECT at Q≲80, LARGEST at hi-q | same | **LARGEST always** | LARGEST | LARGEST | ravif:1646; zenrav1e:1700 |
| reduced_tx_set | no | **yes** | no | no | **yes** | ravif:1654 |
| tx_domain_rate | no | no | no | no | **yes** | ravif:1666 |
| tx_domain_distortion | preset true, **ignored** — Psy/SS2 tunes use pixel-domain cdef_dist | ← | ← | ← | ← | zenrav1e rdo.rs:296 |
| intra mode RDO depth | top-3 (cavif forces `Simple` at EVERY speed; zenrav1e preset would be top-7 ≤s6) | top-3 | top-3 | top-3 | top-3 | ravif:1629; zenrav1e rdo.rs:1613 |
| fine_directional_intra (angle deltas) | yes | yes | yes | **no** | no | ravif:1658 |
| filter_intra | off everywhere (derived from `Simple`; zenrav1e#5 history) | ← | ← | ← | ← | speedsettings.rs:329 |
| palette (tune cfg = Auto) | per-frame AA detection, **speed-independent** | ← | ← | ← | ← | zenrav1e api/internal.rs:646 |
| intraBC | off everywhere (chunk-A default off) | ← | ← | ← | ← | speedsettings.rs:109 |
| segmentation | Complex | Simple | Simple | Simple | Simple | ravif:1670 |
| CDEF | Q≲50 only | Q≲50 | Q≲50 | Q≲50 | **off** | ravif:1663 |
| LRF | Q≲50 only | Q≲50 | Q≲50 | Q≲50 | **off** | ravif:1662 |
| SGR complexity | Full | Reduced | Reduced | Reduced | (lrf off) | ravif:1630 |
| fast_deblock (LF level estimate vs search) | search | search | search | **estimate** (Q≲80) | estimate | ravif:1659 |
| SMALL_PX_RDO_TX (long_edge<1024 keeps tx RDO) | live but **inert at 1024**; fires only s≤4 | ← | **not extended to s6+** | — | — | ravif:1595,1648 |

### Tune axis (`Tune::Ssimulacra2` + palette Auto) — every mechanism is speed-INdependent

| tune mechanism | gate | fast-tier status |
|---|---|---|
| chroma delta-q (420 −20-class curve) | tune only | live s2..s10 (rate.rs:591) |
| ss2 QM curves (`using_qmatrix = tune`) | tune only | live s2..s10 (encoder.rs:1658, 2140) |
| per-SB variance boost, strength 1.0 (+ disables segmentation when it fires) | tune + KEY frame | live s2..s10 (encoder.rs:2087) |
| QM-dist ratio (size ramp m=1.0 at 1024) | tune only | live s2..s10 (encoder.rs:1576; rdo.rs:330; b0098eb1) |
| LF sharpness {7,5,3}@{80,160} | tune only | live s2..s10; at s8/s10 the *level search* degrades to fast_deblock's estimate but the sharpness syntax still applies (encoder.rs:2042; c1fab5b3) |

Not in the cavif path at any speed: trellis, VAQ, seg_boost, FrameHints/sb_q_scale
(release-gated expert API), the zenavif-side speed-conditional palette gate
(`src/palette_gate.rs` τ=0.197 s≤5 / 0.05 s≥6 — release-gated `auto_tune` wiring,
NOT exercised by cavif's in-encoder AA detection).

### Wrapper-level threading/tiling hazard (measured)

cavif with default `--threads` (= host cores) computes `tiles =
threads.min(px / min_tile_size²)`; `min_tile_size` falls to 128–256 at s≥4, so on a
48-core box a 1 MP s6 encode splits into every tile it can — **+3.6% bytes measured**
(84,383 → 87,388 B, 5004_nps @Q60 s6, threads 1 vs 48) and the bitstream becomes
core-count-dependent. All ladder arms pin `--threads 1`. Product note: zenavif callers
on many-core hosts hit this by default at fast speeds; a tiles-vs-speed policy (or
decoupling tile count from thread count) is a wrapper-level candidate.

### Cheap-win flags (off at a fast tier, plausibly trivial cost there)

1. **Rect-partition threshold cliff s2→s4 (64×64 → 8×8).** The single biggest
   structural drop in the table; topdown HORZ/VERT trials are shallow. A 16×16 or
   32×32 threshold at s4–s6 is the first candidate.
2. **reduced_tx_set inconsistency**: ON at s4, OFF at s6/s8, ON again at s9+. With
   `rdo_tx_decision` off (s6+), BOTH tx-size and tx-type RDO are disabled
   (rdo.rs:909-931) — every block codes the largest legal TX with DCT_DCT, but the
   full tx set keeps the *signaling alphabet* wide, so DCT_DCT costs more bits to
   write than it would under the reduced set. reduced_tx_set=true at s6/s8 is a
   pure signaling-cost win (unmeasured, expected small) plus ladder consistency.
3. **SMALL_PX_RDO_TX stops at s≤4**: small renditions at s6 (a common libavif-speed-6
   thumbnail shape) don't keep tx RDO. Extending the gate to s6 mirrors its philosophy
   (small frames = affordable deep search).
4. **Intra RDO depth top-3 everywhere**: with `filter_intra: Option<bool>` now
   overridable (zenrav1e@49982460), `ComplexKeyframes` + `filter_intra=Some(false)`
   would give top-7 intra RDO without the zenrav1e#5 filter_intra cost bug — a slow-tier
   (s1/s2) candidate rather than a fast-tier one.
5. **CDEF off at Q≳50 at every speed** while aom's tune=iq ships adaptive CDEF; photos
   measured ±1% (RD_GAP doc) but the fast-tier/low-bpp regime may differ — check the
   per-tier tables before acting.

---

## RESULTS (measured 2026-07-04; full tables in `benchmarks/rd_gap_speed_ladder_2026-07-04.tsv`)

**Execution record:** 66 runs, 1h38m box time, **0 CELLFAIL / 0 CONFFAIL anywhere** —
all 5,520 zenrav1e RD cells aomdec-clean AND rav1d-safe byte-identical (the fast tiers'
TX_MODE_LARGEST / reduced-tx / small-partition paths are conformance-clean; no new bugs).
Continuity PASS: fresh leg s2-tune vs GOOD refs reproduces the final-2026-07-03 ladder
(−12.52/−11.55/+0.05 vs −12.29/−11.58/+0.05, same win counts); fresh-vs-final same-arm
BD median 0.00 (5/23 images improved by the 4 newer zenrav1e master commits, none worse).

### The solo timing ladder (median wall ms/MP; zr includes ~108 ms/MP PNG decode)

```
aom-cpu9def 41    aom-cpu9iq 49    aom-cpu8def 125   aom-cpu8iq 165
zr-s10-off  231   zr-s10-tune 281  aom-cpu6def 361   aom-cpu6iq 470
zr-s8-off   667   zr-s8-tune  723  zr-s6-off   905   zr-s6-tune 1026
aom-cpu4def 1978  aom-cpu4iq 2779  zr-s4-off  4175   aom-cpu2def 4707
zr-s4-tune  5898  aom-cpu2iq 6639  zr-s2-tune 17647  zr-s2-off  22640
```

### THE VERDICT — time-normalized pareto (photos)

**The libaom `--allintra` ladder pareto-dominates every zenrav1e arm at matched
wall-time on photos — both corpora, ssim2 AND butteraugli. There is no fast-tier
crossover; the matched-time gap WIDENS with speed:**

| time class | pairing (nearest log-time) | t26 ssim2/ba3n med | leg ssim2/ba3n med |
|---|---|---|---|
| ~6 s/MP | zr-s4-tune vs aom-cpu2iq-ai (0.89×) | +6.4 / +6.5 | +7.9 / +3.9 |
| ~18 s/MP (no aom arm this slow) | zr-s2-tune vs aom-cpu2iq-ai (2.66×) | +2.4 / +3.2 | +0.6 / +0.0 |
| ~1-2 s/MP | zr-s6-tune vs aom-cpu4def-ai (0.52×) | +5.4 / +2.2 | +13.6 / **−1.8** |
| ~0.5-0.7 s/MP | zr-s8-tune vs aom-cpu6iq-ai (1.54×) | +5.9 / +4.1 | +7.7 / +3.7 |
| ~0.3 s/MP | zr-s10-tune vs aom-cpu6def-ai (0.78×) | +32.6 / +27.9 | +49.0 / +36.5 |

The one frontier touch is the extreme-quality tip: on legacy, zr-s2-tune (−12.41 vs
the cpu6iq common ref) beats the best measured aom-ai arm (cpu2iq −10.98) by −1.4% at
2.66× its wall time; on train26 the same pair is dead even. Everywhere else an aom-ai
arm exists that is BOTH faster and better. s10 is a cliff (partition (16,16), CDEF/LRF
off, tx_domain_rate) — not a shippable point.

**Reframe of the historical tables:** every prior "matched-speed vs cpu2 / slowest-best
vs cpu0" win stands — but those are GOOD-mode references, and GOOD mode is off the
pareto: aomenc GOOD cpu2 is ~4× slower than cpu2-allintra at LOWER quality than
cpu2iq-ai (leg: cpu2-good +2.26 vs cpu6iq-ai; cpu2iq-ai −10.98). The allintra schedule
is what the ecosystem actually runs (libavif). Against it, the zenrav1e speed schedule
is behind at every operating point — an aom speed-*schedule* gap (their fast tiers keep
tx-size SELECT, large partitions, adaptive CDEF and prune the *search*, while ravif's
table amputates the *toolset*: TX LARGEST at s6+, rect gate 8×8 at s4+, max block 16).

### The tune is unconditionally required at fast tiers (and nearly free)

tune-off is not shippable anywhere: +12 to +70 vs matched-time refs. The composed
`Tune::Ssimulacra2` + palette=Auto is worth 10-40 BD points at the fast tiers (e.g. t26
vs cpu6iq: s6 +25.4→+3.7, s8 +30.9→+5.9, s10 +69.9→+45.1) at 1.1-1.5× arm time — and at
s2 the tune is *faster* than tune-off (12.4 vs 17.5 s/im t26 RD means: palette rescues
plots, variance boost replaces Complex segmentation). RD-sweep mean enc_ms per arm (t26):
s2 12.4/17.5 s (tune/off), s4 4.3/3.9, s6 1.02/0.74, s8 0.83/0.56, s10 0.35/0.23.

### zr internal ladder (what each speed step costs, t26 photos, tune)

s4 vs s2: +3.31% (0/20 better, 0.35× time) · s6 vs s2: +14.7-ish (via cpu2-good deltas
−13.3→−3.4) · s8 ≈ s6 +2.5 · s10: +37 over s8. The s4→s6 boundary is the big RD cliff
(rdo_tx→LARGEST + partition min 4→8), matching the liveness table.

### Per-family at matched time (tune arms, t26) — where zr still wins, and the wedges

Wins at matched time: **6600 scans-illustrations (−18.6/−28.0/−13.1 at s4/s6/s8)**,
**8100 screenshots vs def-refs (s6 −10.4, s8 −3.8)**, people/food ~even at s6.
Everything else loses; plots/clipart lose big even with palette (aom-allintra ships
AA screen detection + intraBC BY DEFAULT at 3.14).

### WEDGE LIST — fast-tier program seeds (tune arms, matched-time refs, t26)

1. **s6 × fam-7000 plots +84.1 med (7052 +124.7); s8 +25.6** — even with palette Auto.
   Owners: **intraBC absence** (aom-ai has it on by default; the known screen-floor gap)
   compounded at fast tiers by the rect-partition 8×8 gate + TX LARGEST on razor edges.
2. **s6 × 1200 interiors +102.7 (1236 ornate-interior; fine at s4 +19.4)** — the
   cleanest single demonstration of the **s4→s6 rdo_tx cliff** (TX LARGEST + DCT-only
   on high-frequency architectural texture). Owner: `rdo_tx_decision` off at s6.
   Candidate: content-gated tx RDO at s6 (SMALL_PX-style hint: keep SELECT on
   high-edge-density frames), or a depth-1-only cheap SELECT mode.
3. **s6 × 9094 gen-illustrations +59.3 (bath-candles +129.8); s8 +20.4** —
   smooth-gradient art; aom fast tiers keep variance-boosted deltaq + adaptive CDEF +
   64×64 blocks; we cap blocks at 16 and drop CDEF at Q≳50. Owners: partition max 16
   (banding on gradients) + CDEF hi-q gate.
4. **s8 × 9000 clipart +58.8 (teapot)** — palette fires but edge blocks code as
   LARGEST/DCT with no rects. Owners: rdo_tx off + rect gate (same pair as #1).
5. **s6 × 5000 nps +32.5 / 6000 scans +20.2 (s8 +15.9)** — textured landscapes and
   1-bit rescans, the same s6-cliff owners; 6000 also wants the rect partitions the
   8×8 gate removes.
   *Traffic-weighted special mention:* **9226 AI-products +7.0/+10.1/+9.5 at s4/s6/s8**
   — moderate per-cell but ~32% of imazen-26 traffic → the largest expected-bytes wedge.

Mechanism hypotheses map 1:1 onto the liveness table's fast-tier amputations
(rdo_tx→LARGEST, rect gate 64→8, block cap 16, CDEF hi-q off) plus the two known
screen-content gaps (intraBC, near-lossless floor). This is exactly the
FEATURE_HINTS "fast mode that needs less brute force" program surface: per-image
{tx-RDO on/off, partition range, rect gate, CDEF} hints at s4-s6 cost budgets.

### aom-side observations (for reference tables)

- `--allintra` is dramatically faster than GOOD at the same cpu-used (cpu2: 4.7 vs
  ~20 s/MP class) and *better* with tune=iq; our GOOD-mode anchor frame under-sold aom's
  practical fast tiers.
- **tune=iq inverts on ssim2 at cpu-used 8** (t26: +11.9 ssim2 median vs cpu8-default,
  while butteraugli says −11.2): iq's fast-tier value is metric-dependent; at cpu6 it
  wins both metrics. cpu9iq similar direction. Use per-metric care when quoting
  "libavif-class" fast references.
- Timing asymmetry (zr wall includes PNG decode ~108 ms/MP; aom pre-converted y4m):
  adjusting zr down by the full constant changes no verdict (largest effect at s10:
  281→~173 ms/MP, still +27..+35 vs the arms bracketing that time).

### What this changes for the program

1. **The fast-mode program is now the headline gap** — parity was reached at the slow
   end (RD_GAP doc) but the web-workload tiers (libavif speed 6 ≈ cpu6-ai class) are
   +5-14% behind at matched time even with the full tune, and the ravif speed table's
   toolset amputations (not zenrav1e's search) look like the first-order cause.
2. Cheap-win queue (from the liveness audit + wedges): rect-gate 16×16/32×32 at s4-s6,
   content-gated tx RDO at s6, reduced_tx_set at s6/s8 (signaling-only win), s10
   partition floor (8,16)-or-(4,16) + CDEF re-enable, SMALL_PX_RDO_TX extension to s6.
3. The tune should be DEFAULT at every speed for still images (it is never worse at
   fast tiers and is cheaper at s2) — reinforces the dep-bump decision in CLAUDE.md.
4. Label store: the 40 RD arms + 6 anchors appended as `speedladder-2026-07-04`
   sources (the fast-tier speed/qm-head labels the drift verdict called for).
