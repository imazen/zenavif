# S10 PROGRAM — the ultra-fast tier vs JPEG (2026-07-05)

**Goal (user direction):** make zenavif's ultra-fast tier competitive **with JPEG as the
scoreboard anchor** — at this speed class the competitor is JPEG, not aom. The
SPEED_LADDER measured s10 as a cliff, not a shippable point: +32.6/+49.0 ssim2/ba3n vs
matched-time aom arms (train26/legacy), with the ravif table amputating at s9-s10:
partition floor (16,16) (rects structurally dead), CDEF+LRF fully off, reduced_tx_set on,
`tx_domain_rate` on (s10; "20% faster but 10% larger files"), angle deltas off (s7+).

**Scoreboard:** matched-ssim2 bytes ratios (zenavif/zenjpeg, <1 = we're smaller) AND
encode-ms ratios, per family, on train26 + the doc-chart supplement — both the registry
config (what ships today) and master-with-gated-arms (what the dep bump ships). Timing
uses the new `enc_int_ms` column (internal encoder-only ms on both sides — cavif's PNG
decode is a third of its wall at s10-class speeds and would poison the ratios; the JPEG
side's sweep_cell reports internal encode/decode ms natively).

**Evaluation policy** (docs/RD_GAP_VS_LIBAOM.md): per-family FIRST, aggregates
cluster-mass-weighted, photos-merit keepable. Byte-identity for untouched tiers;
PALCONF conformance on every armed zr cell.

## Infrastructure (landed)

- zenjpeg@d4f88211 `sweep_cell` example: encodes any sweep-grammar cell id
  (`config_from_cell_id`), roundtrips through zenjpeg's OWN decoder, reports internal
  enc/dec ms. Ships to the box as a prebuilt binary (the zenjpeg dev-dep graph
  hard-requires the 344 MB jpegli-cpp submodule; not worth a box source build).
- zenavif@ebb98c4d rd_gap harness: `jpeg_cell.sh` (JPEG anchor cells, row-cache
  integrated), `run_gap.sh` JPEG_CONFIGS/ZR=off arms + `enc_int_ms` 14th column,
  `chain_s10.sh`, `mine_canonical_jpeg.py`, remote ZEN_REPOS/-s10 tree wiring.
- Dev trees: `ravif--s10` (s4tier-era env-passthrough dev patch rebased onto ravif
  main d72304a1 + zenrav1e--s10 path dep + two NEW passthroughs: `ZENRAVIF_TXDR`,
  `ZENRAVIF_FDI`; + `ZR_ENC_MS` internal-timing stderr line in cavif) and
  `zenrav1e--s10` (master 57de2815, no source changes yet).

## JPEG anchor arms

- `jp3_t0_small_420` — zenjpeg's shipped default stratum (jpegli-heritage tables,
  trellis OFF, Smallest scan search, 4:2:0).
- `moz_tr14.75+dc_small_420` — the mozjpeg-class trellis arm (λ₁ 14.75 + delta-DC).
- `jp3_tr14.5_small_420` — the jpegli-class trellis midpoint.
- `jpeg_best` (mine only) — per-image frontier over ALL 54 canonical zenjpeg cells
  (tables {jp3, jp3[.5,.5], moz, pw4, gls} × trellis {off, 14.5, 14.75+dc} ×
  {420,422,444,xybBq}); the strongest possible JPEG opponent.

## Phase 1a — canonical-dataset breadth mine (measured 2026-07-05, local)

`mine_canonical_jpeg.py --split train` over the canonical picker dataset
(2026-06-27; n=2,307 variants scoreable, registry zenavif s2-s8 per-speed frontiers
over its format axes {qm/noqm, 420/444, bd8/bd10, rgb} vs the JPEG frontiers).
Caveats: class-blind (content_class unpopulated in the parquet — per-family truth
comes from the fresh train26 runs); encode_ms from fleet boxes (multi-config,
not solo) — ratio magnitudes are indicative, the fresh solo pass is the truth.

Bytes ratio (zenavif/jpeg) medians [p25..p75] at matched ssim2, vs **jpeg_best**:

| speed | ssim2 50 | ssim2 60 | ssim2 70 | ssim2 80 | ms ratio (frontier mean) |
|---|---|---|---|---|---|
| s2 | 0.822 [0.63..0.99] | 0.780 | 0.757 | 0.701 [0.57..0.84] | 193x |
| s4 | 0.850 [0.64..1.02] | 0.800 | 0.774 | 0.716 | 108x |
| s6 | 0.920 [0.72..1.08] | 0.874 | 0.893 | 0.814 | 45x |
| s8 | 0.943 [0.76..1.09] | 0.893 | 0.914 | 0.832 [0.72..0.93] | 32x |

vs **jpeg_moz** (the mozjpeg-class single arm): s8 = 0.885/0.844/0.839/0.722 at 26x.

**Reading:** the coordinator's mine reproduces (registry s8 beats jpeg-best by ~6-17%
bytes at matched ssim2 50-80, at ~30x the encode time). The JPEG margin THINS
monotonically with speed — s2 keeps 18-30%, s8 keeps 6-17%, and the p75 crosses 1.0
(worse than JPEG) at s6+ for a quarter of images. The canonical dataset has NO
s9/s10 cells; a naive extrapolation of the trend would put the s10 cliff (+37 BD
over s8) far above JPEG — measured fresh below, never extrapolated.

## Phase 1b — fresh train26 + doccharts scoreboard (MEASURED 2026-07-05, round 1)

Chain round 1: 30 TSVs, **0 CELLFAIL / 0 CONFFAIL** (every zr cell PALCONF:
aomdec-clean + rav1d-safe byte-agree). Record `benchmarks/rd_gap_s10_2026-07-05.tsv`.

### Solo internal encode ms/MP (median, sample_timing4, single-threaded, box)

| arm | ms/MP | vs jpeg-moz | vs jpeg-default |
|---|---|---|---|
| jpeg jp3_t0 (shipped default) | 26.0 | 0.35x | 1x |
| jpeg jp3_tr14.5 (jpegli-class trellis) | 64.6 | 0.87x | 2.5x |
| jpeg moz_tr14.75+dc (mozjpeg-class) | 73.9 | 1x | 2.8x |
| **mas_s10 (tune)** | **337** | **4.6x** | **13.0x** |
| mas_s9 (tune) | 411 | 5.6x | 15.8x |
| s10+txdr0 | 416 | 5.6x | 16.0x |
| s10+p816 | 619 | 8.4x | 23.8x |
| s10+rects | 752 | 10.2x | 29.0x |
| mas_s8c (composed crossed config) | 2394 | 32.4x | 92.2x |

(Registry cavif emits no internal-ms line; its wall ms/MP carries the ~108 ms/MP
PNG decode — reg_s10 wall ≈ 345, i.e. internal ≈ mas_s10-class minus the tune's
~10-20%.)

### Bytes ratio zr/jpeg-moz at matched ssim2 (train26 medians; <1 = we're smaller)

| arm | ss50 | ss60 | ss70 | ss80 |
|---|---|---|---|---|
| reg_s8 | 0.745 | 0.737 | 0.692 | 0.720 |
| reg_s9 | 0.833 | 0.843 | 0.858 | 0.788 |
| **reg_s10** | **1.059** | **1.047** | 0.970 | 0.869 |
| mas_s8c | 0.575 | 0.558 | 0.496 | 0.560 |
| mas_s9 | 0.668 | 0.683 | 0.725 | 0.775 |
| **mas_s10** | **0.841** | **0.790** | 0.826 | 0.841 |

**Headline: registry s10 LOSES to mozjpeg-class JPEG outright** (>1.0 at ssim2
≤60; ≥1.0 in 7 of 12 train26 families at ss50 — interiors 1.16, nps 1.31, scans
1.15, 6600 1.32, screenshots 1.20, 9094 1.16; doccharts ALL 1.09-1.22, 6800
charts 1.36). **The tune alone rescues s10 to 0.79-0.84** (only 5000-nps still
loses, 1.02-1.13; doccharts ALL 0.78-0.93) at 1.2x time — the tune is again
mandatory. mas_s9-tune loses only 5000-nps (0.999-1.11). The already-crossed
s8-composed config is 38-50% smaller than mozjpeg-class JPEG at 32x its time.

### Phase-2 single-axis probes at s10 (BD vs s10-tune base, train26 6q; ssim2/ba3n/bamax med, RD-pass time)

| axis | ssim2 | ba3n | bamax | time | verdict |
|---|---|---|---|---|---|
| tx_domain_rate OFF | **−7.45** | −5.86 | −6.15 | 1.14x | 22/22 better — the cliff's #1 owner |
| tx-size RDO d1 (size1) | −7.82 | −13.33 | −13.03 | 1.49x | 21/21 |
| rects (8,16)+prune triple | −4.27 | −5.78 | −3.84 | 2.31x | 21-22/23 |
| partition floor (8,16) | −1.58 | −1.21 | −1.00 | 1.84x | txdr-masked (see s9) |
| CDEF forced on | −1.70 | −2.45 | −1.89 | **1.04x** | 22-23/23, near-free |
| partition (8,32) | +10.2 | +18.2 | +17.5 | 2.03x | RULED OUT (32-blocks + TX LARGEST misprice) |
| fine-directional-intra on | ~0 | ~0 | +0.15 | 1.10x | null |
| reduced_tx off | 0.00 | 0.00 | 0.00 | 0.99x | null (CDF-adapted, matches FASTWINS) |

At s9 (no txdr in the base): **floor (8,16) alone = −13.45/−17.15/−20.74, 23/23
better at 1.89x** — the (16,16) floor is the s9 cliff's dominant owner, and s10's
txdr was masking most of the partition win in the s10 single-axis row.

## Phase 2 — composed arms (MEASURED, rounds 2-3)

BD vs the old s10-tune rung (train26 6q, ssim2/ba3n/bamax medians, RD-pass time):

| arm | ssim2 | ba3n | bamax | time | note |
|---|---|---|---|---|---|
| c1 txdr0+cdef | −8.78 | −9.40 | −8.94 | 1.24x | 22/22 |
| c3 c1+size1 | −19.23 | −24.37 | −22.85 | 1.62x | |
| c2 c1+p816 | −23.21 | −26.67 | −25.45 | 2.29x | txdr0 unmasks the partition win |
| c4 c1+p816+size1 | −32.39 | −37.24 | −36.58 | 3.15x | |
| c5 c4+rect-triple | −34.43 | −38.02 | −35.98 | 6.33x | poor marginal at this tier |
| c6 satd1 alone | −0.89 | −0.61 | −2.35 | **0.81x** | SATD-decides: faster and free |
| **c7 c4+satd1** | −28.94 | −33.80 | −31.16 | 2.09x | keeps 89% of c4 at 66% time |
| c8 c4+satd2 | −31.58 | −36.73 | −33.72 | 2.55x | |
| c9 s9+p816+size1 (vs s9) | −17.85 | −20.55 | −27.50 | 2.52x | converges to the c4 point |

**The old s9/s10 rungs were a mirage**: s9-tune ≈ s10-tune+txdr0 (c1 ratio 0.662 ≈
s9's 0.668 — the rungs differed by little besides txdr), and pushing the s10 arms
re-creates the s8-composed point (c5 ≈ mas_s8c cost and ratio). The rebuilt ladder
is a clean monotone pareto: satd1(277 ms/MP) → **s10'** → c1(412) → **s9'** →
c4/c9(1140) → s8c(2394).

### The re-tiered rows (round 3, LANDED release-gated)

- **s10' = c11 = txdr OFF + CDEF on + SATD-decides(1)**: **−5.73/−6.93/−7.80 BD vs
  the old s10 rung at 0.95x its time — strictly better AND faster** (21-23/23).
  Solo 315 ms/MP = **4.3x jpeg-moz / 12.1x jpeg-default**; bytes vs jpeg-moz
  0.689/0.695/0.718/0.778 (ss50-80; was 0.84/0.79/0.83/0.84), vs best-of-3
  0.684-0.835; doccharts ALL 0.71-0.85. Only 5000-nps stays marginally above
  parity (1.01-1.12); 6000 rescans ~parity.
- **s9' = c13 = s10' + partition floor (8,16) + depth-1 tx-size RDO**:
  −15.13/−18.23/−23.64 BD vs the old s9 rung at 1.62x its time (22/23). Solo
  663 ms/MP = 9.0x jpeg-moz; bytes vs moz 0.614/0.598/0.537/0.603, vs best-of-3
  0.598-0.660; doccharts 0.54-0.68.
- **Preset-expression equivalence PROVEN**: c13 (s9-preset) is byte-identical to
  c7 (s10-preset expression), 0/24 images differ — `reduced_tx_set` at s9 is
  confirmed inert under these rows.
- Landing: ravif `S10_RETIER_LIVE=false` (av1encoder.rs) — partition_range s9
  (8,16), `tx_domain_rate: Some(speed >= 10 && !RETIER)`, cdef always-on at
  s9/s10, size1 at s9, new `num_modes_rdo_override` SpeedTweaks field
  (`Some(1)` at s9/s10) + commented apply line. **Byte-gate 6/6 md5**: the
  committed const=false binary is byte-identical to pre-change at s9/s10 ×
  q30/60/90. Flip at the zenrav1e dep bump together with the tune default
  (the measured rows include tune-ss2 + palette Auto).

## Phase 3 — verdict: NOT NEEDED as a separate mechanism program

The first named candidate (SATD-decides/RD-codes-winner) was measured inside
phase 2 via the existing `num_modes_rdo_override` knob (zenrav1e@071e9844) and
SHIPPED as an s10'/s9' row ingredient. The heavier candidates (variance-only
top-level partitions, hash-region skips) are not required to meet the exit
criterion: the rebuilt rungs already sit at 0.53-0.78x JPEG bytes at 4.3-9.0x
mozjpeg-class encode time. **Sub-5x of zenjpeg-moz with a 22-31% byte win at
matched ssim2 is reached (s10').** Measured residual: the 5000-nps family
(textured landscape brochures) at s10' (1.01-1.12) — the same full-tx-headroom
class the s4-tier program documented; owner remains the tune/coefficient
programs, not the tier rows.

## Exit scoreboard (the honest multiples)

| point | solo ms/MP | vs jpeg-moz time | bytes vs jpeg-moz (ss50-80) | bytes vs best3 |
|---|---|---|---|---|
| jpeg default (jp3_t0) | 26 | 0.35x | — | — |
| jpeg moz-trellis | 74 | 1x | — | — |
| **s10' (re-tiered)** | **315** | **4.3x** | **0.69-0.78** | 0.68-0.84 |
| **s9' (re-tiered)** | **663** | **9.0x** | **0.54-0.60** | 0.60-0.66 |
| s8-composed (P1/P2) | 2394 | 32.4x | 0.50-0.62 | — |

The 26 ms/MP trellis-off JPEG default remains 12x faster than s10' — a
zenavif rung at that wall does not exist (registry s10 at 345 wall LOSES on
bytes; nothing in the AV1 toolset measured here reaches 26 ms/MP). The
defensible claim: at 4.3x mozjpeg-class time, 22-31% fewer bytes.

## Box / execution record

- Box: zenavif-sweep-1 (ccx63 48c, EUR 1.61/h), restored FROM_SNAPSHOT=auto
  2026-07-05 ~06:30Z; torn down --snapshot same session. 3 chain rounds,
  ~50 min box time, ~4,000 zr cells, 0 CELLFAIL / 0 CONFFAIL (every zr cell
  PALCONF: aomdec-clean + rav1d-safe raw-md5 agree).
- Raw TSVs: `/mnt/v/output/zenavif/s10-2026-07-05/` + Tower mirror
  (`benchmarks/rd_gap_s10_2026-07-05.pointer.md`); distilled record
  `benchmarks/rd_gap_s10_2026-07-05.tsv`.
- Follow-ups: (1) label-store append of the s10 arm TSVs (source entries in
  `scripts/hyperparam/build_label_store.py`) — queued, raws archived; (2) a
  registry-deps A/B of the registry-knob subset (txdr/cdef/partition floor,
  tune-less) if a pre-dep-bump LIVE flip is ever wanted — the gated flip at
  the bump needs nothing extra; (3) 5000-nps residual → tune/coeff programs;
  (4) zenwebp anchor arm (skipped: JPEG was the directed anchor; the webp
  frontier exists in the canonical mine data if needed).
