# libavif v1.4.0 encoder-technology study — mechanisms, where they live, and our program state

**Date:** 2026-07-03. **Author:** research session (read-only; untracked until reviewed).
**Question answered:** what did libavif v1.4.0 ("Boasts Major Updates to Encoder Technology")
actually change, where does each mechanism live (libavif wrapper vs libaom), and what — if
anything — is new relative to the zenrav1e/zenavif program as of 2026-07-02 (all 10 partition
types, s1 deep mode, `Tune::Ssimulacra2` = chroma-deltaq + ss2 QM curves, per-SB delta_q
variance boost @ strength 1.0, QM-weighted RD distortion in flight, palette mode just started).

## Sources (verified, not from memory)

- Blog: [Libavif v1.4.0 Boasts Major Updates to Encoder Technology](https://aomedia.org/blog%20posts/Libavif_v1_4_0-Boasts-Major-Updates-to-Encoder-Technology/) (aomedia.org, fetched 2026-07-03).
- [libavif v1.4.0 release](https://github.com/AOMediaCodec/libavif/releases/tag/v1.4.0) (2026-03-04) + `CHANGELOG.md` at tag; [v1.3.0...v1.4.0 compare](https://github.com/AOMediaCodec/libavif/compare/v1.3.0...v1.4.0) (222 commits); the full `src/codec_aom.c` diff was read hunk-by-hunk.
- Follow-ups: v1.4.1 (2026-03-20, aom pin → 3.13.2; progressive/layered de-experimentalized), v1.4.2 (2026-05-26, aom pin → **3.14.1**, `AOM_TUNE_IQ` for layered inter frames too).
- libaom source at our pinned rev `632172a468f5e91c5b40daaa0a91f4a291c63af4` = **v3.14.1** (`~/work/aom`; same rev the RD-gap baselines and the TUNE_SSIMULACRA2 study used) — all file:line cites below are at this rev unless marked otherwise. libaom `CHANGELOG` entries for 3.12.0 / 3.13.0 / 3.14.0 read directly.
- Our state: `docs/RD_GAP_VS_LIBAOM.md`, `docs/TUNE_SSIMULACRA2_PLAN.md`, ravif `ravif/src/av1encoder.rs`, zenrav1e source, `src/encode_plan.rs`.

## TL;DR

**libavif v1.4.0's "major encoder update" is one decision + one table: make libaom's
`tune=iq` the default for still color images, and recalibrate the quality→QP mapping so
that default doesn't change file-size/speed expectations.** Every underlying RD mechanism
lives in **libaom** (tune IQ, debut 3.12.0, matured 3.13.0; SVT-AV1-PSY lineage), and our
`Tune::Ssimulacra2` program has already ported, measured, and in several cases *rejected
with data* most of it — tune IQ ≈ tune SSIMULACRA2 minus three small deltas. The genuinely
new-to-us items are: the **anti-aliasing-aware screen-content detection gate** (directly
feeds our just-started palette program), **intraBC encoder search** (unimplemented in
zenrav1e, screen-content-only), **tune=psnr for alpha** (libavif found perceptual tunes
ring on alpha — we currently encode alpha with `Tune::Psychovisual`), and the **loop-filter
sharpness schedule** (the one tune-IQ ingredient our tune never A/B'd; plumbing exists).
Defaults drift is real: "typical libavif output" is now `ALL_INTRA + tune=iq + IQ q→qp map`
(+ avifenc speed 6 / quality 60) — different from our raw-aomenc GOOD-mode baselines on
three axes, so an ecosystem-representative arm is worth adding for product claims, though
our tier-2 target (cpu0 + `--tune=ssimulacra2`) remains the *harder* RD bar.

---

## 1. What libavif v1.4.0 itself changed (wrapper-side)

All in `src/codec_aom.c` at tag v1.4.0 (diff hunk `@@ -541,86 +596,192 @@` and neighbors);
changelog lines quoted from `CHANGELOG.md` §[1.4.0].

### 1a. Default tuning selection (the headline)

New logic (codec_aom.c ~lines 707-760 at the tag):

- **Alpha planes → `AOM_TUNE_PSNR`** (was `AOM_TUNE_SSIM` for everything since v0.x).
  Changelog: *"Use AOM_TUNE_PSNR by default when encoding alpha with libaom because
  AOM_TUNE_SSIM causes ringing for alpha."*
- **Still color, `matrixCoefficients != IDENTITY` (i.e. not RGB), usage == ALL_INTRA,
  libaom ≥ 3.13.0 → `AOM_TUNE_IQ`.** Their comment: IQ "has been tuned for the YCbCr
  family… partially generalizes to other YUV-like spaces (YCgCo, ICtCp) including
  monochrome"; gated ≥3.13.0 because IQ's *bit allocation changed significantly between
  3.12.0 and 3.13.0*.
- Everything else (RGB/identity, old aom, non-all-intra) → `AOM_TUNE_SSIM` (old default).
- Lossless → no tune set (irrelevant).
- User-specified `tune=` codec option always wins, and the default tune is now applied
  **before** user codec-specific options (ordering fix) so user overrides of tune-IQ
  side-effects (`enable-qm`, `sharpness`, …) stick.

### 1b. New quality→quantizer mapping, designed for tune=iq

`tuneIqQualityToQuantizer[101]` LUT + `aomQualityToQuantizer(quality, isTuneIq)`
(codec_aom.c ~599-650). Piecewise linear; only used when tune=iq is in effect (alpha and
non-IQ paths keep the old linear `((100 - quality) * 63 + 50) / 100`). Their stated
rationale: **at the same QP, tune=iq produces larger files than tune=ssim** (QM near-flat
at high quality + variance boost + chroma boost all add bits), so quality values are
remapped to *higher* QPs to keep encode size/time at a given `--quality` predictable.
Properties (their comment): `qp(ssim) <= qp(iq)` for all qualities; quality 60 (avifenc
default) = qp 30 (was 25); QP step per quality point decreases 3→1 as quality rises.

Selected values (qp, ×4 ≈ qindex via `quantizer_to_qindex`, av1_quantize.c:1040):

| libavif quality | old linear qp (qindex) | tune-iq qp (qindex) | ravif/zenavif qindex (same 0-100 quality) |
|---|---|---|---|
| 90 | 6 (24) | 8 (32) | 50 |
| 80 | 13 (52) | 15 (60) | 71 |
| 70 | 19 (76) | 23 (92) | 107 |
| 60 | 25 (100) | 30 (120) | 129 |
| 50 | 32 (128) | 37 (148) | 150 |
| 40 | 38 (152) | 43 (172) | 172 |
| 30 | 44 (176) | 49 (196) | 194 |

(ravif column: `quality_to_quantizer`, `ravif/src/av1encoder.rs:1498-1508`, mirrored
verbatim in zenavif `src/encode_plan.rs:245`.) Notable: **our curve already sits nearly on
libavif's new IQ mapping for q30-50** (194/172/150 vs 196/172/148) and is coarser at q60+
(129 vs 120, 107 vs 92, 71 vs 60). So zenavif's quality semantics are closer to
libavif-v1.4-with-tune-iq than to old libavif — divergence concentrated at q≥70.

### 1c. Two-layer all-intra heuristic (progressive AVIF)

`TWO_LAYER_ALL_INTRA_QUALITY_THRESHOLD 10`: a layered image with `extraLayerCount == 1`
whose first layer has quality ≤ 10 is encoded ALL_INTRA rather than inter (codec_aom.c
~660-676). Benefits per their comment: first layer smaller than second, predictable sizes,
layered overhead 2-8%, and unlocks tune IQ (which at 3.13.x required all-intra). v1.4.2 +
aom 3.14 extends tune IQ to layered *inter* encoding too. **N/A for us** (zenavif does no
layered stills), but it's the ecosystem's progressive-AVIF recipe if we ever do.

### 1d. Non-RD wrapper changes worth knowing

- **CICP (color primaries / transfer / matrix) now always forwarded to the AV1 encoder**
  so the Sequence Header OBU carries it — for decoders that wrongly ignore the `colr` box
  (their #2850). **We already do this**: ravif builds `ColorDescription` at
  `ravif/src/av1encoder.rs:1201-1207` and zenrav1e writes it in the sequence header. No gap.
- Monochrome/alpha: drops the ancient libaom-2.0.0 chroma_check workaround; requires
  libaom > 2.0.0; sets `aomImage.monochrome = 1` directly.
- Sample Transforms (spec 1.2, 16-bit+ AVIF), Apple-style gain map conversion, PNG cICP
  chunk, CleanAperture/rotation applied when decoding to PNG/JPEG — container/apps side,
  out of encoder scope.
- avifenc defaults unchanged: quality 60, speed 6 (`apps/avifenc.c:782,1412` at the tag;
  the line-782 comment "Maps to a quantizer (QP) of 25" is stale — under tune=iq it's 30).

**Verdict on the blog's framing:** "libavif v1.4.0 encoder technology" = defaults + the
QP table. The technology itself is libaom's tune IQ.

---

## 2. What `tune=iq` actually is in libaom (and what it shares with `tune=ssimulacra2`)

`handle_tuning()` (`av1/av1_cx_iface.c:1954-1996`) sets identically for **both** IQ and
SSIMULACRA2: `enable_qm=1, qm_min=2, qm_max=10, sharpness=7,
dist_metric=AOM_DIST_METRIC_QM_PSNR, enable_cdef=CDEF_ADAPTIVE(3),
enable_chroma_deltaq=1, deltaq_mode=DELTA_Q_VARIANCE_BOOST(6),
screen_detection_mode=ANTIALIASING_AWARE(2)`. IQ-only extra: `enable_adaptive_sharpness=1`
(av1_cx_iface.c:1987-1992). Documented in `aom/aomcx.h:1756-1812`.

**The only three IQ-vs-SS2 differences** (verified in source):
1. **Adaptive sharpness on** (IQ) vs off (SS2 — "takes a small SSIMULACRA2 hit on the
   lower quality end", cx_iface comment).
2. **4:2:0 chroma delta-q offset 16** (IQ) vs 20 (SS2) — `av1_quantize.c:916` (`int offset
   = (tuning == AOM_TUNE_SSIMULACRA2) ? 20 : 16;`). 4:2:2 (+6) and 4:4:4 (+24) identical.
3. **Luma QM curve** = `aom_get_qmlevel_allintra` (IQ) vs `aom_get_qmlevel_luma_ssimulacra2`
   (SS2) — `av1_quantize.c:996-1006`. Chroma curves identical (444-specific or allintra).

Everything else — per-SB variance-boost delta-q (`allintra_vis.c:1075-1127`), per-block
SSIM rdmult scaling shared with tune=ssim (`encodeframe_utils.c:35-70`,
`partition_search.c:626-631`), frame rdmult weight (`rd.c:406-434`), QM-weighted in-block
distortion (`tx_search.c:1078`, `txb_rdopt_utils.h:48-64`), trellis rshift 5→7 = rdmult÷32
(`txb_rdopt.c:443-461`), LF sharpness syntax (`picklpf.c:220-231`), CDEF_ADAPTIVE
(`pickcdef.c:837-1096`), speed-feature overrides (`speed_features.c:1526-1541`:
`skip_intra_in_interframe=0, inter_mode_rd_model_estimation=0, use_intrabc=1,
intra_pruning_with_hog ≤3`; `speed_features.c:2898-2903`: `zero_low_cdef_strengths` at
qindex≤140), and the 1.125× inter-candidate bias (`rdopt.c:796-846`, **inter-only, no-op
for single-frame stills** — verified) — is shared, and is exactly what
`docs/TUNE_SSIMULACRA2_PLAN.md` studied at this same rev.

### libaom version timeline (from `~/work/aom/CHANGELOG`)

| aom | date | tune-IQ-relevant content |
|---|---|---|
| 3.12.0 | 2025-02-10 | **TUNE_IQ debut** (credited to SVT-AV1-PSY: Ogaard, Rosato, Barba, Djebrouni); deltaq-mode 6 (Variance Boost); enable-cdef=3 (adaptive); all-intra sharpness→syntax element; all-intra QM defaults 4/10. Their claim: up to **12% (ss2) / 14% (DSSIM) / 17% (Butteraugli)** on CLIC vs untuned. |
| 3.13.0 | 2025-09-02 | **TUNE_SSIMULACRA2 debut**; `--screen-detection-mode` (mode 2 = AA-aware); `--enable-adaptive-sharpness` debut; variance boost extended to speeds 8-9; intraBC search speedups; palette-overuse fix (b:421196988). IQ's bit allocation changed "significantly" (per libavif comment) → libavif only defaults to IQ at ≥3.13.0. |
| 3.13.1 | — | bug fixes. = libavif v1.4.0's pin. |
| 3.14.0 | 2026-05-12 | IQ/SS2 extended to **inter** modes (layered images; up to 15%/30% vs TUNE_SSIM); **screen-detection mode 2 becomes the all-intra-usage default**; adaptive-sharpness threshold tweak (QPs 29/30); minor SS2 QM/chroma-dq tweaks (≤0.2%); "re-tune encoder features… 20-30% encoder time reduction, 1-5% vmaf" general retune; adaptive-CDEF made more decoder-friendly. |
| 3.14.1 | 2026-05-22 | 2 bug fixes. = **our pin** = libavif v1.4.2's pin. |

Also relevant: at our rev, **ALL_INTRA usage alone** (no tune) now defaults to
`enable_cdef=0`, `screen_detection_mode=2`, `qm_min=4/qm_max=10`
(`av1_cx_iface.c:3080-3103`) — with tune IQ, `enable_cdef` is then re-raised to
CDEF_ADAPTIVE by `handle_tuning`. And `DELTA_Q_VARIANCE_BOOST` has **no hard all-intra
gate at 3.14.x** (3.14.0 extended it to good/realtime; confirmed no usage check at
`encoder_utils.c:979` / `encodeframe.c:260,357`), so our GOOD-mode
`aomenc --tune=ssimulacra2` tier-2 baseline does get variance boost. (libavif's "deltaq-mode=6
can only be used in all intra mode" comment was true of 3.13.x.)

---

## 3. Mechanism-by-mechanism map against our program

### (a) Already ported / measured by us (with citations)

| libaom mechanism | our state |
|---|---|
| Chroma delta-q (420 −16/−20, 422 +6, 444 +24, ramped) | **Ships** in `Tune::Ssimulacra2` (`zenrav1e@a37faea8`): −2.79% ssim2 med, 20/22 (`TUNE_SSIMULACRA2_PLAN.md` step 1). We ship the SS2 values (444 +24; 420 −20-class curve). |
| QM enable + IQ/SS2 level curves | **Ships** (step 3): −7.79% med, biggest single lever; required fixing zenrav1e#29 (qm_v gating + transposed rect QM). We use the SS2-specific luma curve; IQ's `allintra` luma curve is the documented fallback for any butteraugli-vetoed knob (none needed it — all keep/drop verdicts were metric-consistent). |
| deltaq-mode 6 Variance Boost | **Ships, re-fit**: real per-SB delta_q syntax (`zenrav1e@d125713f` + `66733720`), strength **1.0** refit on train26 (`165e83b1`) — aom's default 3.0 measured worse on zenrav1e (activity masking already covers part of it; keep-segmentation arm re-confirmed double-boost). Tier-2 gap +10.10%→+5.63% (s2). |
| Frame rdmult weight (`rd.c:406`) | **Measured, REJECTED** (step 2): +4.41% ssim2, 0/22 — aom-calibrated rdmult doesn't transfer to zenrav1e's Daala λ. |
| Trellis coefficient retention (sharpness=7 → trellis rdmult ÷32) | **Measured, REJECTED** (step 4): ~0 at ×0.25 and ×1.0 λ; zenrav1e's trellis differs. |
| Per-block SSIM rdmult scaling (shared w/ tune=ssim) | **Equivalent already active**: zenrav1e `Tune::Psychovisual` `apply_ssim_boost` activity masking — verified real and worth ~9.5% median BD vs SSE-RDO (`RD_GAP_VS_LIBAOM.md` §"Perceptual-tune parity"). Same idea, distortion-side. |
| 1.125× inter-candidate RD bias | **N/A for stills** (inter-only; verified `rdopt.c:797-817`). |
| Speed-feature overrides (skip_intra_in_interframe, inter rd model, HOG pruning clamps) | **N/A / no analog**: inter-frame or aom-specific pruning heuristics zenrav1e doesn't have; our s1/s2 search is already unpruned in the relevant dimensions. |
| CICP into sequence header (libavif 1.4.0 change) | **Already done** (`ravif/src/av1encoder.rs:1201`). |

### (b) Known + planned / in flight

| mechanism | plan item |
|---|---|
| `dist_metric=QM_PSNR` (QM-weighted in-block RD distortion) | `TUNE_SSIMULACRA2_PLAN.md` item 6 — **in flight right now** (the qmdist session; coarse-grid arms running per `.workongoing`). |
| CDEF_ADAPTIVE qindex thresholds (off ≤32 / halved ≤220 / full ≥221; zero-low-strengths ≤140) | Plan item 7 ("Filters"), not yet attempted. Expected small: our libaom `--enable-cdef=0` ablation on photos was ±1% noise (`RD_GAP_VS_LIBAOM.md` confirmed findings #3). zenrav1e's StillImage tune has its own CDEF scaling (`encoder.rs:1436-1444`) but the shipped ss2 tune doesn't touch CDEF. |
| Tune default wiring into zenavif/zenravif at the dep bump | CLAUDE.md "Tune::Ssimulacra2 — SHIPPED… decide the default for still images". libavif shipping tune-by-default (March 2026) is ecosystem confirmation of the product call. |

### (c) NEW to us — ranked by expected still-image impact

**c1. Anti-aliasing-aware screen-content detection → per-image palette/intraBC gates.**
`estimate_screen_content_antialiasing_aware` (`av1/encoder/encoder.c:2209-2440`, debut aom
3.13.0, all-intra default since 3.14.0). Algorithm, precisely:
- Walk the **luma** plane in 16×16 blocks (HBD down-converted to 8-bit by `>> (bd-8)`).
  A `fast_detection` speed feature (good speed ≥3, `speed_features.c:442`) checks a
  checkerboard half of blocks and doubles the counts.
- Per block: count distinct 8-bit values with early-exit threshold
  (`av1_count_colors_with_threshold`). ≤4 colors (`kSimpleColorThresh`) → **palette
  candidate**; if per-pixel variance (`av1_get_perpixel_variance`, BLOCK_16X16, on the
  *original* bit depth) > 5 (`kVarThresh`) → also **intraBC candidate**.
- 5-40 colors (`kComplexInitialColorThresh`): **dilate the dominant value** into its
  8-neighborhood (`av1_find_dominant_value` = most frequent 8-bit value;
  `av1_dilate_block`, encoder.c:2154-2207) to absorb anti-aliased edge pixels, then
  re-count: ≤6 colors (`kComplexFinalColorThresh`) **and** var > 5 → palette + intraBC
  candidate. (This dilation step is the "anti-aliasing aware" innovation — classic
  detection missed AA text/graphics.)
- \>40 colors → **photo block**.
- Frame decision (encoder.c:2404-2422): `allow_screen_content_tools =
  (count_palette − count_photo/16) · 256 · 10 > w·h` (i.e. ≳10% palettizable area with
  photo blocks penalizing at 1/16 weight); `allow_intrabc` additionally requires
  `(count_intrabc − count_photo/16) · 256 · 12 > w·h`; `is_screen_content_type` from a
  third pair of thresholds (15/4, 30).

**Why it matters to us:** `allow_screen_content_tools` is the frame-level gate for
`av1_allow_palette` — the palette program (just started) needs exactly this policy, and
zenrav1e has nothing (palette was 100% unimplemented; the plots family costs +51.8%
median bytes in the libaom `--enable-palette=0` ablation). Classification: **per-image
computed gate** (cheap single pass) — and a natural zenanalyze hyperparameter-expert
candidate instead: our feature set (content class, palette features) can subsume the
16×16-block statistic and feed `allow_screen_content_tools`/palette-search-effort as
picker knobs, which fits our architecture better than a hardcoded in-encoder pass.
Expected impact: 0 on photos; the *activation policy* half of the +51.8%-median
screen-content win (the search itself being the other half). Also note aom's cautionary
fix b:421196988 (3.13.0): palette *overuse* inflated sizes at speed 8 — RD-gate it.

**c2. IntraBC (intra block copy) encoder search.** Tune IQ/SS2 force `use_intrabc=1`
(`speed_features.c:1528`); aom 3.13.0 optimized its hash-based search. zenrav1e:
**unimplemented** — `allow_intrabc: false` hardcoded (`src/encoder.rs:1436`), header
plumbing only. Per-block mechanism, large implementation (frame-internal MV search +
hash matching + IBC-specific constraints: LF disabled, wavefront-legal search area).
Expected: significant on screen content only (complements palette; libaom gates it
separately because it forces loop filters off — their detection requires high-variance
palettizable blocks specifically). Photos: ~0. Priority: after palette lands and only if
screen-content traffic matters (same scoping note as palette in `RD_GAP_VS_LIBAOM.md`).

**c3. Alpha-plane tune = PSNR.** libavif moved alpha from TUNE_SSIM → TUNE_PSNR because
"AOM_TUNE_SSIM causes ringing for alpha" (changelog; codec_aom.c ~717). **We encode alpha
with `Tune::Psychovisual`** (ravif alpha config `tune_still_image: false` →
`enc_config_for` default, `ravif/src/av1encoder.rs:1313-1345,1795`; `enable_qm=false` for
alpha is already right). Psychovisual's SSIM-boost activity masking is the same *family*
of perceptual reweighting that libavif found rings on alpha masks. Cheap, novel A/B:
`Tune::Psnr` for the alpha encode, scored on alpha-plane fidelity + composite ssim2 +
visual edges. Also a **wiring guard**: when `Tune::Ssimulacra2` becomes the zenavif color
default at the dep bump, do NOT apply it to alpha (its QM curves on a Cs400 plane would
smear mask edges; chroma deltaq is inert). Classification: global per-plane constant.
Expected: correctness/subjective win on RGBA content; bytes change small.

**c4. Loop-filter sharpness schedule (constant 7 + IQ's adaptive clamp).** Two layers:
(i) `sharpness=7` written to the frame header for all-intra/IQ/SS2 (`picklpf.c:220-231`)
— reduces how much deblocking can alter block-edge samples ("favor perceived sharpness";
part of the original 3.12.0 IQ package); (ii) IQ-only `enable_adaptive_sharpness`
(`picklpf.c:232-249`, debut 3.13.0): clamp sharpness by base qindex — **≤112 → 7, ≤160 →
1, else 0** ("sharpness levels are highly nonlinear… in practice pick 0, 1, 7") — to
avoid low-quality blocking; costs a little ss2 at low q, which is why SS2 tune omits it.
**Our state:** the shipped `Tune::Ssimulacra2` leaves deblock sharpness at 0. zenrav1e has
the syntax plumbed (`fs.deblock.sharpness`, header write at `src/header.rs:1087`) and a
*dormant* StillImage-only schedule (<80 → 7, <160 → 5, else 3, `src/encoder.rs:4392-4400`;
StillImage tune is off by default and measured no-effect long ago); the public
`EncoderConfig.sharpness` field (`encoder.rs:778`) is currently dead code. This is the one
remaining tune-IQ ingredient (besides QM-dist, in flight) never A/B'd on zenrav1e. Cheap
experiment: under Tune::Ssimulacra2, sweep {0 (today), constant 7, aom-adaptive {7,1,0} at
{112,160}, zenrav1e-StillImage {7,5,3} at {80,160}} with the mandatory butteraugli veto —
sharpness is exactly the kind of knob that can game ssim2 while hurting butteraugli.
Classification: global constants (qindex-thresholded); per-image strength is a plausible
picker knob later. Expected: small ssim2 (±0-1%), mostly subjective sharpness; aom shipped
it on subjective grounds.

**c5. libavif's quality→QP recalibration as precedent (wrapper-level).** Not an RD
mechanism, but a product lesson measured by libavif: **switching the default tune shifts
the bitrate-at-QP curve enough that the quality parameter must be re-fit** (they built a
dedicated 101-entry LUT so tune-iq's q60 costs like tune-ssim's q60). At our dep bump,
when Tune::Ssimulacra2 (+deltaq boost) becomes the zenavif default, re-validate
`quality_to_quantizer` (`src/encode_plan.rs:245`, mirror of ravif) against the tuned
encoder — the curve was fit under Psychovisual. Our `encode_rgb8_with_target`
(target-quality convergence) reduces the stakes, but the static quality API and the
picker LUTs consume the curve. Classification: global LUT re-fit (size/quality-swept per
the sweep discipline; the table above shows we're already near libavif-IQ semantics at
q30-50 and diverge at q70+).

**Explicitly not new to us / not applicable:** two-layer all-intra trick (no layered
stills), tuning-before-user-options ordering (we control tune wiring directly), Sample
Transforms/gain-map/cICP work (container/decode side), aom 3.14's general encoder-speed
retune (baseline drift we already absorbed by pinning 3.14.1), monochrome workaround
removal (never had it).

---

## 4. Defaults drift — do our baselines need a libavif arm?

**What changed in the ecosystem:** libavif ≥1.4.0 (March 2026; Chrome/Firefox/etc. will
follow their vendored pins) produces, for a default still encode: **ALL_INTRA usage +
tune=iq + the IQ q→qp map + avifenc speed 6 / quality 60** — i.e. QM 2-10 with allintra
curves, qm-psnr RD distortion, variance-boost per-SB delta-q, chroma delta-q, LF sharpness
7 + adaptive clamp, adaptive CDEF, AA screen detection, intraBC allowed. That is a
materially different (and on ssim2, better) encoder than "aomenc defaults".

**Our baselines** (`scripts/rd_gap/aom_cell.sh:36`): raw `aomenc --cpu-used={0,2}
--end-usage=q --passes=1 --lag-in-frames=0`, GOOD usage (no `--allintra`), default
tune=psnr → none of the tune-IQ machinery. Tier-2 adds `--tune=ssimulacra2` (GOOD mode) —
which activates the full shared mechanism set at our 3.14.1 pin (variance boost included,
verified un-gated) and is **strictly harder than libavif's default** on the ssim2 axis
(cpu0 vs speed 6; ss2-specific curves vs IQ's).

**Verdict:**
- **RD-parity program: no change needed.** cpu0-default and cpu0-ss2tune bracket
  libavif-defaults from below and above; the tier-2 target remains the right stretch goal.
- **Ecosystem-representative claims (picker training, "vs typical AVIF" product
  numbers): add one arm.** Cheapest faithful approximation with our existing binary:
  `aomenc --allintra --cpu-used=6 --tune=iq --end-usage=q --cq-level=<tuneIqQualityToQuantizer[q]>`
  (aom 3.14.1 = libavif v1.4.2's exact pin; `--allintra` matters — it selects the
  all-intra speed-feature schedule and, per aom 3.14, different usage defaults). Exact
  fidelity would build avifenc v1.4.2. Two caveats our GOOD-mode ss2 arm carries vs
  libavif: usage-mode speed-feature tables differ, and libavif users sit at speed 6, not
  cpu0 — both make "typical AVIF in the wild" *weaker* than tier-2, so our published gap
  numbers vs tier-2 are conservative.
- One legacy-comparison footnote: any historical "libavif" numbers in the literature
  (pre-2026) are tune=ssim + linear QP map; post-v1.4.0 numbers are tune=iq + new map.
  Cross-version libavif comparisons are not apples-to-apples on either axis.

---

## 5. Actionable follow-ups (in priority order)

1. **Palette program (running):** adopt the AA-aware detection *policy* — either port
   `estimate_screen_content_antialiasing_aware` (16×16 color-count + dominant-value
   dilation + var>5, thresholds §c1) as zenrav1e's `allow_screen_content_tools` gate, or
   express it as zenanalyze features → picker knob. Keep aom's photo-block 1/16 penalty
   and the palette-overuse lesson (b:421196988).
2. **Alpha tune A/B (cheap):** `Tune::Psnr` vs `Tune::Psychovisual` for the alpha plane in
   ravif/zenavif; and pin "alpha never gets Tune::Ssimulacra2" in the dep-bump wiring.
3. **LF sharpness sweep (cheap):** {0, 7, adaptive-{7,1,0}@{112,160}, {7,5,3}@{80,160}}
   under Tune::Ssimulacra2, butteraugli-vetoed, on train26; wire via the currently-dead
   `EncoderConfig.sharpness` or the tune. The last unmeasured tune-IQ ingredient once
   QM-dist lands.
4. **At the zenrav1e dep bump** (adds to the existing CLAUDE.md checklist): re-validate
   `quality_to_quantizer` against the tuned default (libavif precedent §c5); add the
   libavif-defaults baseline arm to `scripts/rd_gap/` if product-facing comparisons are
   regenerated.
5. **IntraBC:** file as the screen-content follow-on after palette; do not start for the
   photo gap.
6. **CDEF_ADAPTIVE thresholds:** fold into the same sweep as (3) if convenient; expect
   noise on photos.

## 6. Honesty / verification notes

- All libaom cites are at **3.14.1** (`632172a4`, our local checkout — shallow clone, 1
  commit, so per-mechanism landing dates come from aom's CHANGELOG, not `git log`).
  libavif v1.4.0 users run 3.13.1: they get everything except the 3.14.0 items (IQ-inter,
  screen-detection-2-as-allintra-default, adaptive-sharpness QP29/30 tweak, minor ss2 QM
  tweaks). I did not diff 3.13.1 vs 3.14.1 mechanism internals beyond the CHANGELOG.
- libavif file:line refs are approximate (derived from diff hunk offsets at the tag);
  aom refs are exact. BD numbers quoted from aom/libavif are theirs, unre-measured.
- The blog's "sped up bottlenecks in encoder and decoder" maps to aom 3.13/3.14 perf work
  (intraBC search, AArch64 SIMD, speed-feature retune) — perf-only, not itemized here.
- I did not audit `codec_rav1e.c`/`codec_svt.c` diffs in depth (no changelog-visible
  encoder-default changes for those codecs; rav1e pin bumped to 0.8.1).
- Claim "GOOD-mode --tune=ssimulacra2 gets variance boost at 3.14.1" is source-verified
  (no usage gate found; 3.14.0 changelog corroborates) but not empirically re-confirmed
  by bitstream inspection in this session.
