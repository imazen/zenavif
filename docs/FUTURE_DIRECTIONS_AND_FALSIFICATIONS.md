# Future directions & the falsification ledger (2026-07-05)

Two halves: where to go next, and the catalog of things we tried and FALSIFIED —
almost all for the same root cause: **they were isolated pieces of co-designed loops**.
Read this before proposing any "port X from aom" or "add a Y multiplier" — the odds
are it's below, with a measurement. Record pointers are the TSVs/docs named per line;
the full narrative ledger is docs/RD_GAP_VS_LIBAOM.md.

## Part 1 — the falsification catalog

### Class A: foreign constants on our λ/D/R currencies (aom-calibrated values, Daala-lineage loop)
| Tried | Verdict | Record |
|---|---|---|
| Frame-level ssim-rdmult weight | +4.41% — rejected | TUNE_SSIMULACRA2_PLAN (a2) |
| Per-16×16 ssim-rdmult curve (faithful port, strength-swept) | monotone negative, photos 0/6 at every strength; composition overshoot with our masking+boost | rd_gap_ssimrd_2026-07-05.tsv |
| Literal QM-PSNR tx-domain distortion | +4.5% med — cdef_dist worth more than tx-SSE; the RATIO composition shipped instead | qmdist memory + RD_GAP |
| VAQ | hurts; psy tune already carries it | early benchmarks (ravif 7265eea) |
| Trellis λ / aom coefficient postures (the full stack, 7 arms) | ALL lose (+0.97..+14.02), vetoes everywhere — the definitive coherence refutation | COEFF_RD_STACK.md + rd_gap_coeffrd TSV |
| Dead-zone/rounding constants (QROUND 118/128) | +0.94/+2.67, 20/23 vetoed — aom's no-skip is the whole valuation stack, not a constant | TUNER2 record |
| Deeper AQ spread ({36,64}-style curve) | never fires on the target class; their spread was strength-driven and our composed tune subsumed it | TUNER2 record |
| Boost strength >1.0 (global, then refit head, twice) | photos-vetoed; marginal COLLAPSED 2-4 BD as qmdist+lfsharp landed (label drift) | tune-marginal-drift memory |

### Class B: half-loops (one side of a coupled pair)
| Tried | Verdict | Record |
|---|---|---|
| FP-quant posture without aom's descent / descent over Valin input un-gated | both directions lose — `skip_trellis ? B : FP` is a COUPLING, not two features | COEFF_RD_STACK.md |
| tx TYPE-RDO standalone (no size half) | bamax veto at 2.4× | rd_gap_fastwins TSV |
| reduced_tx_set standalone | null at s6/s8 | rd_gap_fastwins TSV |
| CDEF forced-on / LRF forced-on at hi-q | null / adverse — aom's edge is strength ADAPTATION, not enablement | rd_gap_s4tier TSV |
| Blanket-Always intraBC (no content gate) | photos +3..8% (spec filters-off trade) | intraBC chunk A record |

### Class C: operating-point non-transfer (mechanism pays on THEIR base, not ours)
| Tried | Verdict | Record |
|---|---|---|
| Butteraugli two-pass, aom formula | +2.20% — their closed loop pays because their base is unmasked; our open-loop psy base already spends that headroom | DIFFMAP_TWO_PASS.md |
| Two-pass boost-only strength sweep | inverted-U with optimum AT break-even (~2× time, no norm crosses) — dropped as default | DIFFMAP_TWO_PASS.md |
| aom partition_search_breakout as-is | inter-only in aom; had to re-derive for allintra (the vargate is what pays) | rd_gap_p1part TSV |
| Partition margin gates (both semantics) | dead — the contested-band premise is false on our cost model | rd_gap_p1part TSV |
| Skip-gated breakout | null at every τ (lives in the vargate's shadow) | rd_gap_p1part TSV |

### Class D: loop-internal inconsistency (OUR OWN pieces disagreeing — each fix here WON)
The exception that proves the rule: these "failures" flipped to wins once the loop was
made self-consistent — evidence for the co-opt thesis, not against porting per se.
| Case | Arc | Record |
|---|---|---|
| Mixed 3-way partitions (Phase 2) | +0.60% REGRESSION on the biased SPLIT-trial estimate → −0.58% WIN after b073182c fixed the estimate | rd_gap_phase2v2 TSV |
| Fork trellis "off by default" | it was HARD-DEAD below Q80 on a private λ — every historical forced-on A/B was a silent no-op | COEFF_RD_STACK.md |
| Filter-intra "12 dB regression" (zenrav1e#5, years old) | was an encoder-recon DESYNC, not the tool; post-fix arming still rejected on honest RD (+1.82, veto) | desyncfix TSV |
| tx-domain rate at fast tiers | so miscalibrated that amputating the whole tx search beat keeping it — fixed by re-tiering (s10′) | S10_PROGRAM.md |

### Class E: evaluation isolation (the measurement itself was the error)
| Lesson | Consequence |
|---|---|
| Diverse ≠ representative (k-means one-per-cluster) | ALL-medians over-weight rare classes and dilute photo wins → per-family first, mass weights, photos-only merit KEEPABLE (RD_GAP "EVALUATION POLICY") |
| Single-origin winners | 6018's train win refuted by class-sibling 6091 on val — TWICE (SSIMRD, TUNER2). Never ship a one-origin rule |
| Label drift | stored arm labels go stale as the composed config moves (2-4 BD) — regenerate baselines per encoder rev |
| Corpus gaps read as mechanism gaps | the "no document-chart class" block was a SUBSET artifact (imazen-26 has 145 train candidates) |
| MLP-vs-thresholds at n=24 | capacity is never the binder at small n — origin transfer is (q₀ MLP: train p90 4.08, val 7.70, REJECTED; threshold/formula heads shipped everywhere) |
| Speed-table non-transfer | a lever measured at one tier doesn't transfer down (i7 dominated by i5 at s4; 32-rects +0.16 for +1.0×; s4-native +4.22 at ~10×) |

### Class F: ruled out on clean re-measurement
prange (4,64)+(8,32) widenings (+0.48/+10..18); fire-always palette (value sits inside
photo mass at 1.8-2.1× fired cost); fdi/reduced-tx at s10 (null); s9 old-tier expression
(byte-identical to s10-preset form — a redundant rung); 5000-class "stable gate" for
full-tx (no honest gate exists at n=24 — oracle-only headroom).

## Part 2 — future directions

1. **The co-optimized-loop program** — the flagship successor; the full month plan with
   phases and acceptance criteria is docs/COOPT_LOOP_PLAN.md. Attacks everything Class
   A-D above the RIGHT way: joint λ–D–R calibration against our objective.
2. **zensim profile-B diffmaps + analysis-driven allocation**: per-SB hints at zero
   extra encodes through FrameHints (the surviving path from two-pass); external
   priorities (zensally saliency/faces, text masks) through the same channel.
3. **The unified (target, effort) entry point**: one call → q₀ seed + rung + heads +
   gates; effort as ms/MP envelopes (enc_int_ms plumbing exists). The product face of
   everything measured. Needs the dep bump to arm.
4. **Prediction round 3 on representative corpora**: the doc-chart anti-boost gate
   (sample_doccharts.tsv ready), per-image boost/AQ heads with the class evidence from
   TUNER2, the 9000-family q₀ gap (p90 15.6), RGBA/RGB16/zensim q₀ seeding.
5. **intraBC completion**: 4:1 slivers, sub-8×8, odd-DV chroma subpel, SB128 (its
   biggest wins pend 128-SB support); 128×128 SB generally (large-image fast tiers).
6. **zenrav1e#28**: validate the real TX_64X16/64 path → lift the sliver caps.
7. **Learned components inside the loop** (post-COOPT phase 1): a tiny trace-fitted
   distortion predictor (D that predicts the metric directly), fitted rate tables per
   context — OUR versions, calibrated on OUR objective, replacing hand curves.
8. **Ecosystem**: zenpipe/imageflow-v3 integration, zencodecs registry entry, the WASM
   decode story (wasm128 SIMD exists), animation encode + animation/RGBA16 target-quality.
9. **HDR completion**: 10-bit-base gain-map reconstruction (ultrahdr kernels),
   EncodeBitDepth::Twelve public API (breaking-change window), gain-map zencodec trait
   (after the zencodec release).
10. **Decoder hardening line**: rav1d-safe #423 (flush semantics), #414 (NEON
    conformance completion), #422 (a strictness mode so OUR decoder catches
    non-conformance without aomdec).
11. **The release cadence itself**: the train (RELEASE_TRAIN_2026-07-05.md), then the
    refactor pass (ENGINEERING_BASELINE.md), then SHORT gated-flip cycles — never again
    ~20 mechanisms deep behind one registry wall.
