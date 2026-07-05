#!/usr/bin/env python3
"""Hyperparameter-expert label store builder (docs/FEATURE_HINTS_PLAN.md section E).

Aggregates the per-image x per-arm x per-q RD outcomes from every mechanism fit
sweep + the wedge dataset into ONE queryable parquet, so future threshold-rule /
MLP heads fit against a single store instead of one-shot TSV artifacts. New fit
sweeps append by adding a SOURCES entry and re-running.

Output:
  /mnt/v/output/zenavif/hyperparam-labels-2026-07-03/labels.parquet
  /mnt/v/output/zenavif/hyperparam-labels-2026-07-03/_MANIFEST.json

Honesty contract (read this before consuming the store):
  * encoder_rev: the sweeps span several zenrav1e master revisions. A per-arm
    delta is only valid WITHIN one sweep_source (all its arms ran the same
    session/binary chain); cross-sweep byte comparisons need the byte-identity
    notes in the manifest. The store records, the consumer filters.
  * q_kind: 'cavif_q' (cavif -Q quality 0-100), 'aom_cq' (aomenc --cq-level),
    'rav1e_quantizer' (rav1e --quantizer 0-255). NEVER pool q across kinds.
  * split: computed from the canonical LSD origin rule
    (zenmetrics/scripts/picker/origin_split.py). The features parquet's own
    'split' column is an OLDER convention that disagrees on 1148/2157 origins —
    do not use it.
  * feature_join: key '<origin_path>|<crop_label>|<size_class>' into
    imazen26_features_2026-06-23.parquet. feature_join_exact=True only for
    wedge rows (pixel-verified 123/123, rel-tol 1e-3). train26 rows get
    feature_join_exact=False: the sweep corpus was materialized with
    vipsthumbnail --linear while the features parquet renditions are
    Lanczos3-sRGB (image crate) — same origin, same nominal size class, exact
    same WxH (verified), different resampler. Content-level features are
    robust to this; pixel-exact features are not. Legacy-corpus rows: NULL.
  * enc_ms: reliability varies per source (contention / different hosts); see
    manifest 'enc_ms_notes'. Never make cross-source speed claims from it.
  * palette-ab rows were scored through a different pipeline (rav1e CLI on
    color.py 420 y4m, aomdec decode) — absolute ssim2/butteraugli are NOT
    comparable to cavif rows; within-source arm deltas are valid.
"""

import collections
import csv
import json
import os
import subprocess
import sys

import numpy as np
import pandas as pd
import pyarrow as pa
import pyarrow.parquet as pq

ZEN = os.path.expanduser("~/work/zen")
sys.path.insert(0, os.path.join(ZEN, "zenmetrics/scripts/picker"))
from origin_split import origin_id as lsd_origin_id  # noqa: E402
from origin_split import split_of  # noqa: E402

OUT_DIR = "/mnt/v/output/zenavif/hyperparam-labels-2026-07-03"
FEATURES_PARQUET = "/mnt/v/output/imazen-26-features/imazen26_features_2026-06-23.parquet"
TRAIN26_MANIFEST = "/mnt/v/output/rd-gap-train26-2026-07-02/_MANIFEST.json"
WEDGE_DIR = "/mnt/v/output/rd-gap-wedge-2026-07-03"
WEDGE_MAP = os.path.join(ZEN, "zenavif/scripts/rd_gap/wedge_corpus_map.tsv")

SRC = {
    "tune": "/mnt/v/output/zenavif/tune-ss2-2026-07-02",
    "deltaq": "/mnt/v/output/zenavif/deltaq-2026-07-02",
    "qmdist": "/mnt/v/output/zenavif/qmdist-2026-07-03",
    "lfsharp": "/mnt/v/output/zenavif/lfsharp-2026-07-03",
    "desyncfix": "/mnt/v/output/zenavif/desyncfix-2026-07-03",
    "palette": "/mnt/v/output/zenrav1e-palette/sweep-20260703-final2",
    "palmech": "/mnt/v/output/rd-gap-palette-ab-2026-07-03/results",
    "sizedecay": "/mnt/v/output/zenavif/sizedecay-2026-07-03",
    "speedladder": "/mnt/v/output/zenavif/speedladder-2026-07-04",
    "fastwins": "/mnt/v/output/zenavif/fastwins-20260704",
    "p1part": "/mnt/v/output/zenavif/p1part-20260704",
    "p2heads": "/mnt/v/output/zenavif/p2heads-20260704",
    "s4tier": "/mnt/v/output/zenavif/s4tier-20260704",
}
# palette-mech A/B val corpus (14 VAL-LSD origins materialized with the wedge
# conventions; join-verified 108/108) — see its _MANIFEST.json + picks_val14.json
PALVAL_DIR = "/mnt/v/output/rd-gap-palette-val-2026-07-03"

RD_COLS = ["image", "w", "h", "family", "encoder", "fmt", "q", "bytes", "bpp",
           "ssim2", "enc_ms", "butteraugli_3n", "butteraugli_max"]


def J(**kw):
    return json.dumps(kw, sort_keys=True)


# ---------------------------------------------------------------------------
# Source registry: one entry per raw file. Everything the store needs to stay
# honest lives here — arm identity, knobs, encoder rev, q semantics, corpus.
# To APPEND a future fit sweep: add entries + rerun. rows= is a hard assert.
# ---------------------------------------------------------------------------
def sources():
    s = []

    # --- Tune::Ssimulacra2 stage arms (2026-07-02) — legacy 22-image corpus ---
    tune_rev_base = "zenrav1e@2fac1af6 (master; byte-identical to splitcost baseline)"
    tune_rev_ws = "zenrav1e-ws@6257b65f (staged tune impl; landed as a37faea8)"
    for fn, arm, knob, rev in [
        ("tune_base.tsv", "ss2/tune-off_s2", J(speed=2, tune="off"), tune_rev_base),
        ("tune_s1chroma.tsv", "ss2/stage1-chromadq_s2", J(speed=2, tune="ss2-staged", stages="1(chroma-dq)"), tune_rev_ws),
        ("tune_s2lambda.tsv", "ss2/stage12-lambda_s2", J(speed=2, tune="ss2-staged", stages="1+2(frame-lambda)"), tune_rev_ws),
        ("tune_s3qm.tsv", "ss2/stage123-qm_s2", J(speed=2, tune="ss2-staged", stages="1+2+3(ss2-QM-curves)"), tune_rev_ws),
        ("tune_s4trellis.tsv", "ss2/stage1234-trellis025_s2", J(speed=2, tune="ss2-staged", stages="1..4(trellis-x0.25)"), tune_rev_ws),
        ("tune_s5varboost.tsv", "ss2/stage12345-varboost_s2", J(speed=2, tune="ss2-staged", stages="1..5(seg-varboost)"), tune_rev_ws),
        ("tune_composed13.tsv", "ss2/composed13_s2", J(speed=2, tune="ssimulacra2", mechanisms="1+3"), tune_rev_ws),
        ("tune_s4t100.tsv", "ss2/composed13-trellis100_s2", J(speed=2, tune="ssimulacra2", mechanisms="1+3+trellis-x1.0"), tune_rev_ws),
        ("tune_s1speed.tsv", "ss2/composed13_s1deep", J(speed=1, s1_deep=True, tune="ssimulacra2", mechanisms="1+3"), tune_rev_ws),
    ]:
        s.append(dict(path=f"{SRC['tune']}/{fn}", kind="rd_tsv", corpus="legacy22",
                      sweep_source="tune-ss2-2026-07-02", arm_id=arm, knob_json=knob,
                      encoder_rev=rev, q_kind="cavif_q",
                      speed=1 if "s1deep" in arm else 2, rows=264))

    # --- deltaq variance-boost strength arms (2026-07-02) — train26 + legacy + aom refs ---
    dq_rev = "zenrav1e-ws@66733720+env (deltaq chain d125713f..165e83b1; str1 byte-verified == baked 165e83b1)"
    for fn, arm, strength in [
        ("t26_s2_dq0.tsv", "deltaq/str0_s2", 0.0), ("t26_s2_dq_str1.tsv", "deltaq/str1_s2", 1.0),
        ("t26_s2_dq_str2.tsv", "deltaq/str2_s2", 2.0), ("t26_s2_dq_str3.tsv", "deltaq/str3_s2", 3.0),
        ("t26_s2_dq_str4p5.tsv", "deltaq/str4.5_s2", 4.5), ("t26_s2_dq_str6.tsv", "deltaq/str6_s2", 6.0),
    ]:
        s.append(dict(path=f"{SRC['deltaq']}/{fn}", kind="rd_tsv", corpus="train26",
                      sweep_source="deltaq-2026-07-02", arm_id=arm,
                      knob_json=J(speed=2, tune="ssimulacra2", deltaq_variance_boost_strength=strength),
                      encoder_rev=dq_rev, q_kind="cavif_q", speed=2, rows=288))
    s.append(dict(path=f"{SRC['deltaq']}/t26_s2_dq_str3_segB.tsv", kind="rd_tsv", corpus="train26",
                  sweep_source="deltaq-2026-07-02", arm_id="deltaq/str3-keepseg_s2",
                  knob_json=J(speed=2, tune="ssimulacra2", deltaq_variance_boost_strength=3.0, keep_segmentation=True),
                  encoder_rev=dq_rev, q_kind="cavif_q", speed=2, rows=288))
    for fn, arm, spd in [("legacy_s2_deltaq.tsv", "deltaq/str1_s2", 2), ("legacy_s1_deltaq.tsv", "deltaq/str1_s1deep", 1)]:
        s.append(dict(path=f"{SRC['deltaq']}/{fn}", kind="rd_tsv", corpus="legacy22",
                      sweep_source="deltaq-2026-07-02", arm_id=arm,
                      knob_json=J(speed=spd, s1_deep=(spd == 1), tune="ssimulacra2", deltaq_variance_boost_strength=1.0),
                      encoder_rev=dq_rev, q_kind="cavif_q", speed=spd, rows=264))
    aom_rev = "aomenc-3.14.1@632172a4"
    for fn, arm, cpu, tune in [
        ("aom_cpu2.tsv", "ref/aom-cpu2_420", 2, "default"),
        ("aom_cpu0_default.tsv", "ref/aom-cpu0-default_420", 0, "default"),
        ("aom_cpu0_ss2.tsv", "ref/aom-cpu0-ss2tune_420", 0, "ssimulacra2"),
    ]:
        s.append(dict(path=f"{SRC['deltaq']}/{fn}", kind="rd_tsv", corpus="legacy22",
                      sweep_source="deltaq-2026-07-02", arm_id=arm,
                      knob_json=J(encoder="aomenc", cpu_used=cpu, tune=tune, fmt="420"),
                      encoder_rev=aom_rev, q_kind="aom_cq", speed=cpu, rows=176))

    # --- QM-weighted RD distortion arms (2026-07-03) — train26 coarse/full + legacy ---
    qm_base_rev = "zenrav1e@165e83b1 (master, baked tune+deltaq1.0; coarse base 144/144 bpp-EXACT vs deltaq str1)"
    qm_ws_rev = "zenrav1e-ws@17a311ce/45204079 on 165e83b1 (landed 3710a573+4279a673)"
    qm = [
        ("t26c_s2_base.tsv", "qmdist/off_s2", J(speed=2, tune="ssimulacra2", qm_dist="off"), qm_base_rev, 2, 144),
        ("t26c_s2_tx1.tsv", "qmdist/tx1-txdomain-unweighted_s2", J(speed=2, tune="ssimulacra2", qm_dist="tx-domain-unweighted"), qm_ws_rev, 2, 144),
        ("t26c_s2_tx2.tsv", "qmdist/tx2-txdomain-qmweighted_s2", J(speed=2, tune="ssimulacra2", qm_dist="tx-domain-QM-weighted"), qm_ws_rev, 2, 144),
        ("t26c_s2_tr1.tsv", "qmdist/tr1-trellis-unweighted_s2", J(speed=2, tune="ssimulacra2", qm_dist="off", trellis="on-unweighted"), qm_ws_rev, 2, 144),
        ("t26c_s2_tr2.tsv", "qmdist/tr2-trellis-qmweighted_s2", J(speed=2, tune="ssimulacra2", qm_dist="off", trellis="on-QM-weighted"), qm_ws_rev, 2, 144),
        ("t26c_s2_tx2tr2.tsv", "qmdist/tx2tr2-both_s2", J(speed=2, tune="ssimulacra2", qm_dist="tx-domain-QM-weighted", trellis="on-QM-weighted"), qm_ws_rev, 2, 144),
        ("t26c_s2_tx3.tsv", "qmdist/tx3-ratio_s2", J(speed=2, tune="ssimulacra2", qm_dist="ratio"), qm_ws_rev, 2, 144),
        ("full_t26_s2_win.tsv", "qmdist/ratio-final_s2", J(speed=2, tune="ssimulacra2", qm_dist="ratio"), qm_ws_rev, 2, 288),
        ("full_t26_s1_win.tsv", "qmdist/ratio-final_s1deep", J(speed=1, s1_deep=True, tune="ssimulacra2", qm_dist="ratio"), qm_ws_rev, 1, 288),
        ("full_t26_s1_base.tsv", "qmdist/off_s1deep", J(speed=1, s1_deep=True, tune="ssimulacra2", qm_dist="off"), qm_base_rev, 1, 288),
    ]
    for fn, arm, knob, rev, spd, n in qm:
        s.append(dict(path=f"{SRC['qmdist']}/{fn}", kind="rd_tsv", corpus="train26",
                      sweep_source="qmdist-2026-07-03", arm_id=arm, knob_json=knob,
                      encoder_rev=rev, q_kind="cavif_q", speed=spd, rows=n))
    for fn, arm, spd in [("full_legacy_s2_win.tsv", "qmdist/ratio-final_s2", 2),
                         ("full_legacy_s1_win.tsv", "qmdist/ratio-final_s1deep", 1)]:
        s.append(dict(path=f"{SRC['qmdist']}/{fn}", kind="rd_tsv", corpus="legacy22",
                      sweep_source="qmdist-2026-07-03", arm_id=arm,
                      knob_json=J(speed=spd, s1_deep=(spd == 1), tune="ssimulacra2", qm_dist="ratio"),
                      encoder_rev=qm_ws_rev, q_kind="cavif_q", speed=spd, rows=264))

    # --- LF sharpness schedule arms (2026-07-03) — train26 coarse/full + legacy ---
    lf_base_rev = "zenrav1e@9b79b442 (master post palette/skip-recon; ws binary byte-identical 18/18 md5)"
    lf_ws_rev = "zenrav1e-ws@aba01be7+0f5e4c47 on 9b79b442"
    lf = [
        ("t26c_s2_lf0.tsv", "lfsharp/sharp0_s2", J(speed=2, tune="ssimulacra2", lf_sharpness="0"), lf_base_rev, 2, 144),
        ("t26c_s2_const7.tsv", "lfsharp/const7_s2", J(speed=2, tune="ssimulacra2", lf_sharpness="const7"), lf_ws_rev, 2, 144),
        ("t26c_s2_adaptive.tsv", "lfsharp/adaptive-iq_s2", J(speed=2, tune="ssimulacra2", lf_sharpness="adaptive{<=112:7,<=160:1,else:0}"), lf_ws_rev, 2, 144),
        ("t26c_s2_still.tsv", "lfsharp/still753_s2", J(speed=2, tune="ssimulacra2", lf_sharpness="still{7,5,3}@{80,160}"), lf_ws_rev, 2, 144),
        ("full_t26_s2_still.tsv", "lfsharp/still753-final_s2", J(speed=2, tune="ssimulacra2", lf_sharpness="still{7,5,3}@{80,160}"), lf_ws_rev, 2, 288),
        ("full_t26_s2_lf0.tsv", "lfsharp/sharp0-final_s2", J(speed=2, tune="ssimulacra2", lf_sharpness="0"), lf_base_rev, 2, 288),
        ("full_t26_s1_still.tsv", "lfsharp/still753-final_s1deep", J(speed=1, s1_deep=True, tune="ssimulacra2", lf_sharpness="still{7,5,3}@{80,160}"), lf_ws_rev, 1, 288),
        ("full_t26_s1_lf0.tsv", "lfsharp/sharp0-final_s1deep", J(speed=1, s1_deep=True, tune="ssimulacra2", lf_sharpness="0"), lf_base_rev, 1, 288),
    ]
    for fn, arm, knob, rev, spd, n in lf:
        s.append(dict(path=f"{SRC['lfsharp']}/{fn}", kind="rd_tsv", corpus="train26",
                      sweep_source="lfsharp-2026-07-03", arm_id=arm, knob_json=knob,
                      encoder_rev=rev, q_kind="cavif_q", speed=spd, rows=n))
    for fn, arm, knob, rev, spd in [
        ("full_legacy_s2_lf0.tsv", "lfsharp/sharp0-final_s2", J(speed=2, tune="ssimulacra2", lf_sharpness="0"), lf_base_rev, 2),
        ("full_legacy_s2_still.tsv", "lfsharp/still753-final_s2", J(speed=2, tune="ssimulacra2", lf_sharpness="still{7,5,3}@{80,160}"), lf_ws_rev, 2),
        ("full_legacy_s1_lf0.tsv", "lfsharp/sharp0-final_s1deep", J(speed=1, s1_deep=True, tune="ssimulacra2", lf_sharpness="0"), lf_base_rev, 1),
        ("full_legacy_s1_still.tsv", "lfsharp/still753-final_s1deep", J(speed=1, s1_deep=True, tune="ssimulacra2", lf_sharpness="still{7,5,3}@{80,160}"), lf_ws_rev, 1),
    ]:
        s.append(dict(path=f"{SRC['lfsharp']}/{fn}", kind="rd_tsv", corpus="legacy22",
                      sweep_source="lfsharp-2026-07-03", arm_id=arm, knob_json=knob,
                      encoder_rev=rev, q_kind="cavif_q", speed=spd, rows=264))

    # --- desync-fix close-out arms (2026-07-03) — train26, LOCAL host (7950X) ---
    dsf_rev = "zenrav1e@17cff82f-era master (post #32/#33 fixes; shipped config byte-neutral vs 9b79b442)"
    s.append(dict(path=f"{SRC['desyncfix']}/t26_s2_tune_shipped.tsv", kind="rd_tsv", corpus="train26",
                  sweep_source="desyncfix-2026-07-03", arm_id="desyncfix/shipped_s2",
                  knob_json=J(speed=2, tune="ssimulacra2", filter_intra=False),
                  encoder_rev=dsf_rev, q_kind="cavif_q", speed=2, rows=288))
    s.append(dict(path=f"{SRC['desyncfix']}/t26_s2_tune_fion.tsv", kind="rd_tsv", corpus="train26",
                  sweep_source="desyncfix-2026-07-03", arm_id="desyncfix/filterintra-on_s2",
                  knob_json=J(speed=2, tune="ssimulacra2", filter_intra=True),
                  encoder_rev=dsf_rev, q_kind="cavif_q", speed=2, rows=288))

    # --- wedge paletteoff arm TSV (parquet arms are ingested separately) ---
    s.append(dict(path=f"{WEDGE_DIR}/results/wedge_zr_paloff.tsv", kind="rd_tsv", corpus="wedge26",
                  sweep_source="wedge-2026-07-03", arm_id="wedge/zr-paletteoff_s2",
                  knob_json=J(speed=2, tune="ssimulacra2", palette="off", depth=8),
                  encoder_rev="zenrav1e@32477046 via ravif--wedge@9d2b97c", q_kind="cavif_q",
                  speed=2, rows=354))

    # --- palette A/B final2 (2026-07-03) — train26 via 420 y4m, rav1e CLI ---
    s.append(dict(path=f"{SRC['palette']}/results.tsv", kind="palette_tsv", corpus="train26",
                  sweep_source="palette-ab-final2-2026-07-03", arm_id=None, knob_json=None,
                  encoder_rev="zenrav1e@49982460 (rav1e CLI, isolated: still-picture threads=1 lrf=false filter-intra=false)",
                  q_kind="rav1e_quantizer", speed=None, rows=720))

    # --- palette-gate mechanism A/B (2026-07-03) — the rule-1 graduation data:
    # palette {off,always,auto} x sizes {256,512,1024|native,c50} x configs
    # {shipped cavif s2+s6, isolated rav1e CLI s2+s6} on wedge fired/quiet/photo
    # train files + the 14-origin VAL corpus (first val RD labels for the gate).
    # Shipped arms: ravif--wedge@9d2b97c -> zenrav1e--wedge@32477046 (byte-
    # continuous with the wedge zr arms, verified 7052.native q60 = 2646 B).
    # always/auto cells PALCONF-verified (aomdec + rav1d-safe raw md5 agree).
    palmech_rev = "zenrav1e@32477046 via ravif--wedge@9d2b97c (box zenavif-sweep-2)"
    for fn, arm, pal, spd, rows in [
        ("pal_shipped_always.tsv", "palette-mech/shipped-always_s2", "always", 2, 1464),
        ("pal_shipped_off.tsv", "palette-mech/shipped-off_s2", "off", 2, 792),
        ("pal_shipped_auto.tsv", "palette-mech/shipped-auto_s2", "auto", 2, 504),
        ("pal_shipped_s6_always.tsv", "palette-mech/shipped-always_s6", "always", 6, 252),
        ("pal_shipped_s6_off.tsv", "palette-mech/shipped-off_s6", "off", 6, 252),
        ("pal_shipped_s6_auto.tsv", "palette-mech/shipped-auto_s6", "auto", 6, 252),
    ]:
        s.append(dict(path=f"{SRC['palmech']}/{fn}", kind="rd_tsv", corpus="mech26",
                      sweep_source="palette-mech-ab-2026-07-03", arm_id=arm,
                      knob_json=J(speed=spd, tune="ssimulacra2", palette=pal, depth=8),
                      encoder_rev=palmech_rev, q_kind="cavif_q", speed=spd, rows=rows))
    # Isolated-config arms (same binary chain, rav1e CLI; conformance: every
    # palette-armed cell aomdec-decoded AND raw-md5-agreed with rav1d-safe —
    # 1800/1800). enc_ms contended (JOBS=26); the timing sidecar TSV is the
    # authoritative time source for this sweep.
    s.append(dict(path=f"{SRC['palmech']}/pal_iso_all.tsv", kind="palette_sizes_tsv",
                  corpus="mech26", sweep_source="palette-mech-iso-2026-07-03",
                  arm_id=None, knob_json=None,
                  encoder_rev="zenrav1e@32477046 (rav1e CLI, isolated: still-picture threads=1 lrf=false filter-intra=false)",
                  q_kind="rav1e_quantizer", speed=None, rows=2700))
    # s8 corroboration run for the SPEED-CONDITIONAL threshold A/B (fresh
    # encodes on the restored zenavif-sweep-1 box; binary byte-continuity
    # with the mech run sha-proven against the kept 7052 s2-q60-auto IVF).
    # Conformance: 1350/1350 aomdec-decoded, 900/900 palette-armed cells
    # raw-md5-agreed with rav1d-safe. enc_ms JOBS=22-contended (always/off
    # within-cell ratio median 2.13x is the honest fired-cost signal).
    s.append(dict(path=f"{SRC['palmech']}/pal_iso_s8.tsv", kind="palette_sizes_tsv",
                  corpus="mech26", sweep_source="palette-mech-iso-s8-2026-07-03",
                  arm_id=None, knob_json=None,
                  encoder_rev="zenrav1e@32477046 (rav1e CLI, isolated: still-picture threads=1 lrf=false filter-intra=false)",
                  q_kind="rav1e_quantizer", speed=None, rows=1350))

    # --- size-decay isolation A/B (2026-07-03) — wedge #3 / HYPERPARAM rule 2:
    # leave-one-out Tune::Ssimulacra2 mechanism arms x sizes {256,512,1024} on
    # the 12-origin photo-like TRAIN (wedge26 files) + 12-origin VAL
    # (palette-val files) ladders; 16-pt q grid (the 12-pt grid + 78/82/88/92
    # high-q densification — the decay lives in the high-q band). Encoder:
    # zenrav1e--sizedecay workspace commit 1428ecdd on master c9c2d5f7 via
    # ravif--wedge@9d2b97c (ZENRAV1E_SD_DISABLE leave-one-out gates +
    # ZENRAV1E_SD_RAMP long-edge ramp trials; env-unset byte-identical to the
    # master binary — md5-gated locally AND on-box). Verdict: qmdist convicted
    # for the size decay (pre-registered rule), everything else acquitted;
    # ramp trials + val in the same source. enc_ms cache-replayed on hits —
    # never use for speed claims.
    sd_rev = ("zenrav1e-ws@1428ecdd on c9c2d5f7 via ravif--wedge@9d2b97c "
              "(SD gates; env-unset == master md5)")
    sd_arms = [("sd_full.tsv", "sizedecay/full_s2",
                J(speed=2, tune="ssimulacra2", palette="auto")),
               ("sd_off.tsv", "sizedecay/off_s2",
                J(speed=2, tune="off", palette="auto"))]
    sd_arms += [(f"sd_no_{m}.tsv", f"sizedecay/no-{m}_s2",
                 J(speed=2, tune="ssimulacra2", palette="auto", sd_disable=m))
                for m in ("chromadq", "qmcurves", "boost", "qmdist", "lfsharp")]
    for fn, arm, knob in sd_arms:
        s.append(dict(path=f"{SRC['sizedecay']}/train_arms/{fn}", kind="rd_tsv",
                      corpus="mech26", sweep_source="sizedecay-2026-07-03",
                      arm_id=arm, knob_json=knob, encoder_rev=sd_rev,
                      q_kind="cavif_q", speed=2, rows=576))
    for fn, arm, knob in sd_arms[:2]:
        s.append(dict(path=f"{SRC['sizedecay']}/val_arms/{fn}", kind="rd_tsv",
                      corpus="mech26", sweep_source="sizedecay-2026-07-03",
                      arm_id=arm + "-val", knob_json=knob, encoder_rev=sd_rev,
                      q_kind="cavif_q", speed=2, rows=576))
    s.append(dict(path=f"{SRC['sizedecay']}/val_cpu2/val_cpu2.tsv", kind="rd_tsv",
                  corpus="mech26", sweep_source="sizedecay-2026-07-03",
                  arm_id="sizedecay/ref-cpu2-val",
                  knob_json=J(encoder="aomenc", cpu_used=2, fmt="420+444"),
                  encoder_rev="aomenc-3.14.1@632172a4", q_kind="aom_cq",
                  speed=2, rows=576))
    for m256 in ("0", "0.25", "0.5"):
        s.append(dict(path=f"{SRC['sizedecay']}/ramp_arms/sd_ramp_qmdist_{m256}.tsv",
                      kind="rd_tsv", corpus="mech26",
                      sweep_source="sizedecay-2026-07-03",
                      arm_id=f"sizedecay/ramp-qmdist-m{m256}_s2",
                      knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                                  sd_ramp=f"qmdist:{m256}",
                                  ramp="longedge clamp((log2(maxdim)-8)/2, m256, 1)"),
                      encoder_rev=sd_rev, q_kind="cavif_q", speed=2, rows=384))

    # --- size-decay NON-TUNE isolation A/B (2026-07-03) — the tune-off
    # baseline's own small-px decay. Arms ADD one coding tool each to the
    # tune-OFF (Psychovisual) + palette-auto baseline, unconditional at all q,
    # via ZENRAVIF_SD2_* dev passthroughs (ravif--wedge@9d2b97c+dev ->
    # zenrav1e--wedge @ master b0098eb1; base env-unset verified
    # byte-identical to the box sizedecay off arm 576/576 cells). Armed cells
    # ran the PALCONF aomdec+rav1d-safe conformance gate. prange464 was
    # DROPPED (zenrav1e#34, fixed 1dabba91); yuv420 produced NO rows — every
    # cell aomdec-rejected (zenavif#29, ravif 420 non-conformance). psnr +
    # combo64 arms ran against master 1dabba91 (#34 fix; base byte-identity
    # re-verified). Verdicts: NO arm met the pre-registered size-conviction;
    # segmentation value FADES toward small (see the RD_GAP section).
    ntd = "/mnt/v/output/zenavif/sizedecay-nontune-2026-07-03"
    nt_rev = ("ravif--wedge@9d2b97c+SD2 -> zenrav1e master b0098eb1 "
              "(psnr/combo64: 1dabba91; base byte-identical to off arm)")
    for fn, arm, knob in [
        ("sdn_base.tsv", "sdnontune/base_s2", J(speed=2, tune="off", palette="auto")),
        ("sdn_prange432.tsv", "sdnontune/prange432_s2",
         J(speed=2, tune="off", palette="auto", sd2_prange="4,32")),
        ("sdn_rdotx.tsv", "sdnontune/rdotx_s2",
         J(speed=2, tune="off", palette="auto", sd2_rdotx=1)),
        ("sdn_cdef.tsv", "sdnontune/cdef_s2",
         J(speed=2, tune="off", palette="auto", sd2_cdef=1)),
        ("sdn_lrf.tsv", "sdnontune/lrf_s2",
         J(speed=2, tune="off", palette="auto", sd2_lrf=1)),
        ("sdn_segoff.tsv", "sdnontune/segoff_s2",
         J(speed=2, tune="off", palette="auto", sd2_seg="off")),
        ("sdn_combo32.tsv", "sdnontune/combo32_s2",
         J(speed=2, tune="off", palette="auto", sd2_prange="4,32",
           sd2_rdotx=1, sd2_cdef=1, sd2_lrf=1)),
        ("sdn_psnr.tsv", "sdnontune/psnr_s2", J(speed=2, tune="psnr", palette="auto")),
        ("sdn_combo64.tsv", "sdnontune/combo64_s2",
         J(speed=2, tune="off", palette="auto", sd2_prange="4,64",
           sd2_rdotx=1, sd2_cdef=1, sd2_lrf=1)),
    ]:
        s.append(dict(path=f"{ntd}/train_arms/{fn}", kind="rd_tsv",
                      corpus="mech26", sweep_source="sizedecay-nontune-2026-07-03",
                      arm_id=arm, knob_json=knob, encoder_rev=nt_rev,
                      q_kind="cavif_q", speed=2, rows=576))
    # aom cpu0 reference at 256 (12 train origins x 8 cq x 420+444)
    s.append(dict(path=f"{ntd}/train_arms/aom_cpu0_256.tsv", kind="rd_tsv",
                  corpus="mech26", sweep_source="sizedecay-nontune-2026-07-03",
                  arm_id="sdnontune/ref-cpu0-256",
                  knob_json=J(encoder="aomenc", cpu_used=0, fmt="420+444"),
                  encoder_rev="aomenc-3.14.1@632172a4", q_kind="aom_cq",
                  speed=0, rows=192))
    # val ship-candidate confirm (rdotx on the 24-file <=512 val slice)
    s.append(dict(path=f"{ntd}/val_arms/sdn_rdotx.tsv", kind="rd_tsv",
                  corpus="mech26", sweep_source="sizedecay-nontune-2026-07-03",
                  arm_id="sdnontune/rdotx_s2-val",
                  knob_json=J(speed=2, tune="off", palette="auto", sd2_rdotx=1),
                  encoder_rev=nt_rev, q_kind="cavif_q", speed=2, rows=384))

    # --- SPEED-LADDER GAP MAP arms (2026-07-04) — the fast-tier labels the drift
    # verdict wanted re-encoded at current master (speed/qm heads + encode_ms LUTs):
    # zr s{2,4,6,8,10} x {tune-ss2+palette, off} + aom --allintra cpu{2,4,6,8,9} x
    # {default, iq} on train26 + legacy, BUTTER on, every zr cell PALCONF-clean.
    # RD-row enc_ms = JOBS=22/24 on 48 dedicated cores (corroboration grade);
    # timing_* rows = solo RD_CACHE=off JOBS=1 (honest wall). --threads 1 pinned.
    sl_rev = "zenrav1e@184a616f (master tip c4047cec) via ravif a284209+devpatch-b2180ec2"
    sl_aom_rev = "aomenc-3.14.1@632172a4 --allintra"
    sld = SRC["speedladder"]
    for corpus, tag, nzr, naom in (("train26", "t26", 288, 192), ("legacy22", "leg", 264, 176)):
        for spd in (2, 4, 6, 8, 10):
            for cfg, knob in (("tune", J(speed=spd, tune="ssimulacra2", palette="auto", threads=1)),
                              ("off", J(speed=spd, tune="off", threads=1))):
                s.append(dict(path=f"{sld}/zr_{tag}_s{spd}_{cfg}.tsv", kind="rd_tsv",
                              corpus=corpus, sweep_source="speedladder-2026-07-04",
                              arm_id=f"speedladder/zr-s{spd}-{cfg}", knob_json=knob,
                              encoder_rev=sl_rev, q_kind="cavif_q", speed=spd, rows=nzr))
        for cpu in (2, 4, 6, 8, 9):
            for t, knob in (("def", J(encoder="aomenc", cpu_used=cpu, usage="allintra", tune="default", fmt="420")),
                            ("iq", J(encoder="aomenc", cpu_used=cpu, usage="allintra", tune="iq", fmt="420"))):
                s.append(dict(path=f"{sld}/aom_{tag}_cpu{cpu}{t}.tsv", kind="rd_tsv",
                              corpus=corpus, sweep_source="speedladder-2026-07-04",
                              arm_id=f"speedladder/aom-cpu{cpu}{t}-ai", knob_json=knob,
                              encoder_rev=sl_aom_rev, q_kind="aom_cq", speed=cpu, rows=naom))
    # t26 GOOD-mode anchor replays (values == deltaq-2026-07-02-convention refs; the
    # store previously had these for legacy only)
    for g, cpu, tune in (("cpu2", 2, "default"), ("cpu0def", 0, "default"), ("cpu0ss2", 0, "ssimulacra2")):
        s.append(dict(path=f"{sld}/aomgood_t26_{g}.tsv", kind="rd_tsv",
                      corpus="train26", sweep_source="speedladder-2026-07-04",
                      arm_id=f"ref/aom-{g}_420", knob_json=J(encoder="aomenc", cpu_used=cpu, tune=tune, fmt="420"),
                      encoder_rev="aomenc-3.14.1@632172a4", q_kind="aom_cq", speed=cpu, rows=192))

    # --- FASTWINS P0 arms (2026-07-04) — the s4->s6 rdo_tx cliff decomposition
    # (tx-size vs tx-type vs depth vs reduced-set, the P1 tx-search seed labels)
    # + the tile-count RD curve (w1_* --threads N arms measure the OLD default
    # tile formula min(threads, px/min_tile^2); pool size is bitstream-inert).
    # All train26 tune-ss2+palette, PALCONF-clean; RD = coarse 6-q, confirm =
    # full 12-q. enc_ms contended (JOBS=24) — solo wall lives in the raw dir's
    # timing_*.tsv, not appended here.
    fw_rev = "zenrav1e@d82c16ba via ravif 55f8c935+7baad5f9+devpatch-86de6714"
    fwd = SRC["fastwins"]
    fw_knobs = {
        "base":   J(),
        "size1":  J(tx_size_rdo=1, tx_size_depth=1),
        "size2":  J(tx_size_rdo=1),
        "type":   J(tx_type_rdo=1),
        "typred": J(tx_type_rdo=1, reduced_tx=1),
        "min":    J(tx_size_rdo=1, tx_size_depth=1, tx_type_rdo=1, reduced_tx=1),
        "full":   J(tx_size_rdo=1, tx_type_rdo=1),
        "red":    J(reduced_tx=1),
    }
    for spd, arms in ((6, list(fw_knobs)), (8, ["base", "size1", "min", "red"])):
        for arm in arms:
            k = json.loads(fw_knobs[arm]); k.update(speed=spd, tune="ssimulacra2", palette="auto", threads=1)
            s.append(dict(path=f"{fwd}/w2_s{spd}_{arm}.tsv", kind="rd_tsv",
                          corpus="train26", sweep_source="fastwins-2026-07-04",
                          arm_id=f"fastwins/s{spd}-{arm}", knob_json=json.dumps(k, sort_keys=True),
                          encoder_rev=fw_rev, q_kind="cavif_q", speed=spd, rows=144))
    for spd, thrs in ((6, (2, 4, 8, 16, 48)), (4, (1, 4, 8, 48))):
        for t in thrs:
            s.append(dict(path=f"{fwd}/w1_s{spd}_thr{t}.tsv", kind="rd_tsv",
                          corpus="train26", sweep_source="fastwins-2026-07-04",
                          arm_id=f"fastwins/s{spd}-thr{t}-oldtilepolicy",
                          knob_json=J(speed=spd, tune="ssimulacra2", palette="auto", threads=t, tile_policy="old"),
                          encoder_rev=fw_rev, q_kind="cavif_q", speed=spd, rows=144))
    for spd in (6, 8):
        for arm in ("base", "size1"):
            k = json.loads(fw_knobs[arm]); k.update(speed=spd, tune="ssimulacra2", palette="auto", threads=1, grid="full12q")
            s.append(dict(path=f"{fwd}/confirm_s{spd}_{arm}.tsv", kind="rd_tsv",
                          corpus="train26", sweep_source="fastwins-2026-07-04",
                          arm_id=f"fastwins/confirm-s{spd}-{arm}", knob_json=json.dumps(k, sort_keys=True),
                          encoder_rev=fw_rev, q_kind="cavif_q", speed=spd, rows=288))

    # --- P1PART partition liveness+pruning arms (2026-07-04, P1 lever 1) ---
    # Rect/4-way partitions kept LIVE at fast tiers (rect_thr / part_max axes)
    # with the zenrav1e topdown_prune knob controlling cost (none_breakout /
    # rect+4way rel-gap margins / homogeneity gate). s6/s8 arms ride the P0
    # landed baseline (tx-size RDO depth-1); s4 rides the stock table. All
    # train26 tune-ss2+palette, PALCONF-clean; RD = coarse 6-q, confirm =
    # full 12-q; enc_ms contended (JOBS=24) — solo wall in timing_*.tsv.
    # Wave 1 (_pr1/_pr2/_bk/no4 + shape arms) ran zenrav1e 725f5f71 with the
    # ORIGINAL SYMMETRIC margin semantics; waves 2-4 ran 767c8ff5 (one-sided
    # NONE-dominance margins). base2 is the 144/144 byte-identity sentinel
    # across the change (knob-off arms identical on both revs).
    p1_rev_w1 = "zenrav1e@725f5f71 (symmetric margins) via ravif 4f2caa93+p1part-devpatch"
    p1_rev_w2 = "zenrav1e@767c8ff5 (one-sided margins) via ravif 4f2caa93+p1part-devpatch"
    p1d = SRC["p1part"]
    p1_w1 = {
        "base":       J(),
        "r16":        J(rect_thr=16),
        "r16no4":     J(rect_thr=16, prune_4wm=0.0, margin_semantics="sym"),
        "r16m32":     J(rect_thr=16, part_max=32),
        "r32m32":     J(rect_thr=32, part_max=32),
        "r16_bk":     J(rect_thr=16, prune_bk=1.0),
        "r16_pr1":    J(rect_thr=16, prune_bk=1.0, prune_rectm=0.25, prune_4wm=0.05, prune_varg=3.0, margin_semantics="sym"),
        "r16_pr2":    J(rect_thr=16, prune_bk=2.0, prune_rectm=0.15, prune_4wm=0.05, prune_varg=3.0, margin_semantics="sym"),
        "r16m32_pr1": J(rect_thr=16, part_max=32, prune_bk=1.0, prune_rectm=0.25, prune_4wm=0.05, prune_varg=3.0, margin_semantics="sym"),
        "r16m32_pr2": J(rect_thr=16, part_max=32, prune_bk=2.0, prune_rectm=0.15, prune_4wm=0.05, prune_varg=3.0, margin_semantics="sym"),
    }
    p1_w2 = {
        "base2":        J(),
        "r16_pr3":      J(rect_thr=16, prune_bk=1.0, prune_rectm=0.25, prune_4wm=0.05, prune_varg=3.0, margin_semantics="1side"),
        "r16_pr4":      J(rect_thr=16, prune_bk=2.0, prune_rectm=0.10, prune_4wm=0.02, prune_varg=3.0, margin_semantics="1side"),
        "r16_vg3":      J(rect_thr=16, prune_varg=3.0),
        "r16_vg2":      J(rect_thr=16, prune_varg=2.0),
        "r16no4_pr3":   J(rect_thr=16, prune_4wm=0.0, prune_bk=1.0, prune_rectm=0.25, prune_varg=3.0, margin_semantics="1side"),
        "r16m32_pr3":   J(rect_thr=16, part_max=32, prune_bk=1.0, prune_rectm=0.25, prune_4wm=0.05, prune_varg=3.0, margin_semantics="1side"),
        "r16_bkvg2":    J(rect_thr=16, prune_bk=1.0, prune_varg=2.0),
        "r16_bkvg3":    J(rect_thr=16, prune_bk=1.0, prune_varg=3.0),
        "r16_bk4vg2":   J(rect_thr=16, prune_bk=4.0, prune_varg=2.0),
        "r16no4_bkvg2": J(rect_thr=16, prune_4wm=0.0, prune_bk=1.0, prune_varg=2.0, margin_semantics="1side"),
        "r16m32_bkvg2": J(rect_thr=16, part_max=32, prune_bk=1.0, prune_varg=2.0),
        "r16no4_1side": J(rect_thr=16, prune_4wm=0.0, margin_semantics="1side"),
    }
    P1 = [
        (6, "p1_w1", ["base", "r16", "r16no4", "r16m32", "r32m32", "r16_bk",
                      "r16_pr1", "r16_pr2", "r16m32_pr1", "r16m32_pr2"]),
        (8, "p1_w1", ["base", "r16", "r16_pr1", "r16_pr2"]),
        (4, "p1_w1", ["base", "r16", "r16_pr1"]),
        (6, "p1_w2", ["base2", "r16_pr3", "r16_pr4", "r16_vg3", "r16_vg2",
                      "r16no4_pr3", "r16m32_pr3", "r16_bkvg2", "r16_bkvg3",
                      "r16_bk4vg2", "r16no4_bkvg2", "r16m32_bkvg2"]),
        (8, "p1_w2", ["r16_pr3", "r16_pr4", "r16_bkvg2", "r16no4_bkvg2"]),
        (4, "p1_w2", ["r16_pr3", "r16_pr4", "r16_bkvg2", "r16no4_bkvg2"]),
    ]
    for spd, wave, arms in P1:
        knobs = p1_w1 if wave == "p1_w1" else p1_w2
        rev = p1_rev_w1 if wave == "p1_w1" else p1_rev_w2
        for arm in arms:
            k = json.loads(knobs[arm])
            k.update(speed=spd, tune="ssimulacra2", palette="auto", threads=1)
            if spd in (6, 8):
                k.update(tx_size_rdo=1, tx_size_depth=1)  # P0 landed baseline
            s.append(dict(path=f"{p1d}/p1_s{spd}_{arm}.tsv", kind="rd_tsv",
                          corpus="train26", sweep_source="p1part-2026-07-04",
                          arm_id=f"p1part/s{spd}-{arm}", knob_json=json.dumps(k, sort_keys=True),
                          encoder_rev=rev, q_kind="cavif_q", speed=spd, rows=144))
    # Wave-4 supplement: plain no4 (one-sided rev) at s8/s4.
    for spd in (8, 4):
        k = json.loads(p1_w2["r16no4_1side"])
        k.update(speed=spd, tune="ssimulacra2", palette="auto", threads=1)
        if spd == 8:
            k.update(tx_size_rdo=1, tx_size_depth=1)
        s.append(dict(path=f"{p1d}/p1_s{spd}_r16no4.tsv", kind="rd_tsv",
                      corpus="train26", sweep_source="p1part-2026-07-04",
                      arm_id=f"p1part/s{spd}-r16no4", knob_json=json.dumps(k, sort_keys=True),
                      encoder_rev=p1_rev_w2, q_kind="cavif_q", speed=spd, rows=144))
    # Confirm grids: base + the 4wm-only config + the SHIPPED gate triple
    # (r16no4_bkvg2 = the landed s4-s8 configuration), full 12-q.
    for spd in (6, 8, 4):
        for arm in ("base", "r16no4", "r16no4_bkvg2"):
            k = json.loads(
                p1_w1["base"] if arm == "base"
                else p1_w2["r16no4_1side"] if arm == "r16no4"
                else p1_w2["r16no4_bkvg2"])
            k.update(speed=spd, tune="ssimulacra2", palette="auto", threads=1, grid="full12q")
            if spd in (6, 8):
                k.update(tx_size_rdo=1, tx_size_depth=1)
            s.append(dict(path=f"{p1d}/confirm_s{spd}_{arm}.tsv", kind="rd_tsv",
                          corpus="train26", sweep_source="p1part-2026-07-04",
                          arm_id=f"p1part/confirm-s{spd}-{arm}", knob_json=json.dumps(k, sort_keys=True),
                          encoder_rev=p1_rev_w2, q_kind="cavif_q", speed=spd, rows=288))

    # --- P2HEADS arms (2026-07-04, FAST_TIER_PARITY P2) ---
    # Head-3 intra-mode-budget axis (top-7 keyframe intra RDO with
    # filter_intra=Some(false), the zenrav1e#5-safe form; base = forced
    # top-3) + the composed per-image fast mode (heads 1+2 frozen threshold
    # rules; per-(tx,partition)-class sub-runs merged by class name) on
    # train26 AND the 14-origin VAL-LSD corpus (honest held-out). All
    # tune-ss2+palette, PALCONF-clean. Binary chain: zenrav1e master
    # 39f0ecdd (INCLUDES the one-sided margin fix 767c8ff5 — an earlier
    # stale-workspace pass at e944ea71 ran symmetric semantics and was
    # discarded; base cells byte-match p1part 144/144, ship cells re-verified
    # byte-identical after the fix). Composed classes ride SIZE1/MIN tx envs
    # x ship(r16no4_bkvg2)/m32(r16m32_bkvg2) prune envs; *i7* arms add
    # global top-7 intra. enc_ms contended (JOBS=24); solo wall in p2t_*.tsv
    # (raw dir only).
    p2_rev = "zenrav1e@39f0ecdd via ravif 0191489b+p2heads-devpatch(bd0b33d2)"
    p2d = SRC["p2heads"]
    p2_tx = {"none": {}, "size1": dict(tx_size_rdo=1, tx_size_depth=1),
             "min": dict(tx_size_rdo=1, tx_size_depth=1, tx_type_rdo=1, reduced_tx=1)}
    p2_part = {"ship": dict(rect_thr=16, prune_4wm=0.0, prune_bk=1.0, prune_varg=2.0),
               "m32": dict(rect_thr=16, part_max=32, prune_bk=1.0, prune_varg=2.0)}
    for name, spd, extra in [
        ("s6-base", 6, {}), ("s6-intra7", 6, dict(intra_modes=7)),
        ("s6-ship", 6, p2_part["ship"]),
        ("s6-intra7ship", 6, dict(intra_modes=7, **p2_part["ship"])),
        ("s8-base", 8, {}), ("s8-intra7", 8, dict(intra_modes=7)),
    ]:
        k = dict(p2_tx["size1"], speed=spd, tune="ssimulacra2", palette="auto",
                 threads=1, **extra)
        s.append(dict(path=f"{p2d}/p2_{name.replace('-', '_', 1)}.tsv", kind="rd_tsv",
                      corpus="train26", sweep_source="p2heads-2026-07-04",
                      arm_id=f"p2heads/{name}", knob_json=json.dumps(k, sort_keys=True),
                      encoder_rev=p2_rev, q_kind="cavif_q", speed=spd, rows=144))
    for nm, path, rows_, k in [
        ("confirm-s6-base", "p2_conf_s6_base.tsv", 288, dict(p2_tx["size1"], grid="full12q")),
        ("confirm-s6-ship", "p2_conf_s6_ship.tsv", 288,
         dict(p2_tx["size1"], grid="full12q", **p2_part["ship"])),
    ]:
        k.update(speed=6, tune="ssimulacra2", palette="auto", threads=1)
        s.append(dict(path=f"{p2d}/{path}", kind="rd_tsv",
                      corpus="train26", sweep_source="p2heads-2026-07-04",
                      arm_id=f"p2heads/{nm}", knob_json=json.dumps(k, sort_keys=True),
                      encoder_rev=p2_rev, q_kind="cavif_q", speed=6, rows=rows_))
    p2_cls_n = dict(none_ship=2, none_m32=1, size1_ship=13, size1_m32=5,
                    min_ship=1, min_m32=2)
    p2v_cls_n = dict(none_ship=1, none_m32=2, size1_ship=5, size1_m32=3,
                     min_ship=1, min_m32=2)
    for i7 in ("", "i7"):
        for cls, n in p2_cls_n.items():
            tx, part = cls.rsplit("_", 1)
            k = dict(p2_tx[tx], speed=6, tune="ssimulacra2", palette="auto",
                     threads=1, grid="full12q", **p2_part[part])
            if i7:
                k.update(intra_modes=7)
            s.append(dict(path=f"{p2d}/p2c{i7}_{cls}.tsv", kind="rd_tsv",
                          corpus="train26", sweep_source="p2heads-2026-07-04",
                          arm_id=f"p2heads/composed{i7}-{cls}",
                          knob_json=json.dumps(k, sort_keys=True),
                          encoder_rev=p2_rev, q_kind="cavif_q", speed=6, rows=n * 12))
    # VAL leg (mech26 corpus machinery — same val pngs as the palette-mech A/B)
    for nm, path, rows_, k in (
        [("val-base", "p2v_base.tsv", 168, dict(p2_tx["size1"])),
         ("val-ship", "p2v_ship.tsv", 168, dict(p2_tx["size1"], **p2_part["ship"]))]
        + [(f"val-composed-{cls}", f"p2vc_{cls}.tsv", n * 12,
            dict(p2_tx[cls.rsplit('_', 1)[0]], **p2_part[cls.rsplit('_', 1)[1]]))
           for cls, n in p2v_cls_n.items()]
        + [(f"val-composedi7-{cls}", f"p2vi7_{cls}.tsv", n * 12,
            dict(p2_tx[cls.rsplit('_', 1)[0]], intra_modes=7,
                 **p2_part[cls.rsplit('_', 1)[1]]))
           for cls, n in p2v_cls_n.items()]):
        k.update(speed=6, tune="ssimulacra2", palette="auto", threads=1, grid="full12q")
        s.append(dict(path=f"{p2d}/{path}", kind="rd_tsv",
                      corpus="mech26", sweep_source="p2heads-2026-07-04",
                      arm_id=f"p2heads/{nm}", knob_json=json.dumps(k, sort_keys=True),
                      encoder_rev=p2_rev, q_kind="cavif_q", speed=6, rows=rows_))
    # Attribution-factoring + rules-v2 reassignment cells: the val W-gate
    # false-fire conviction (8103 (none,ship) +18.1 vs (size1,m32) -1.9) and
    # the three v2-remapped images' measured cells.
    for nm, path, corpus_, rows_, k in [
        ("valx-size1-m32", "p2vx_size1_m32.tsv", "mech26", 24,
         dict(p2_tx["size1"], **p2_part["m32"])),
        ("valx-none-ship", "p2vx_none_ship.tsv", "mech26", 24,
         dict(**p2_part["ship"])),
        ("rx-7028-size1-m32", "p2rx_7028_size1_m32.tsv", "train26", 12,
         dict(p2_tx["size1"], **p2_part["m32"])),
        ("rx-7028-size1-m32-i7", "p2rx_7028_size1_m32_i7.tsv", "train26", 12,
         dict(p2_tx["size1"], intra_modes=7, **p2_part["m32"])),
        ("valx2-size1-m32-i7", "p2rx_valx2_size1_m32_i7.tsv", "mech26", 24,
         dict(p2_tx["size1"], intra_modes=7, **p2_part["m32"])),
    ]:
        k.update(speed=6, tune="ssimulacra2", palette="auto", threads=1, grid="full12q")
        s.append(dict(path=f"{p2d}/{path}", kind="rd_tsv",
                      corpus=corpus_, sweep_source="p2heads-2026-07-04",
                      arm_id=f"p2heads/{nm}", knob_json=json.dumps(k, sort_keys=True),
                      encoder_rev=p2_rev, q_kind="cavif_q", speed=6, rows=rows_))

    # --- S4TIER 2026-07-04 (chain_s4tier.sh; FAST_TIER_PARITY last column) ---
    # zenrav1e master 0d392334 (071e9844 num_modes_rdo_override knob + fmt)
    # via the ravif--s4tier devpatch (p2heads passthroughs + INTRA_MODES=5 +
    # CDEF/LRF force envs); box cavif sha256/16 26091145a8cdc388. The cont
    # phase re-encoded the p2heads s6+size1 base and i7 coarse arms under
    # this chain and both byte-matched 144/144 (knob-off identity) — those
    # duplicate cells are NOT re-registered here. i5 = ComplexKeyframes +
    # filter_intra=off + num_modes_rdo_override=5 (the new knob's top-5
    # midpoint); cdef/lrf arms force the filters on at every q (the stock
    # table gates both at Q<~50). v3 composed classes = v2 rules with the
    # tx D bound at 23.69 (fit_s4_tier.py refit); full_* = the 5 oracle-
    # extra images (no honest gate; upper-bound factoring cells).
    s4_rev = "zenrav1e@0d392334 via ravif d72304a1+s4tier-devpatch(26091145)"
    s4d = SRC["s4tier"]
    s4_tx = dict(p2_tx, full=dict(tx_size_rdo=1, tx_type_rdo=1))
    for nm, path, spd, rows_, k in [
        ("s6-i5", "s4_s6_i5.tsv", 6, 144, dict(p2_tx["size1"], intra_modes=5)),
        ("s6-i5ship", "s4_s6_i5ship.tsv", 6, 144,
         dict(p2_tx["size1"], intra_modes=5, **p2_part["ship"])),
        ("s8-i5", "s4_s8_i5.tsv", 8, 144, dict(p2_tx["size1"], intra_modes=5)),
        ("s6-cdef", "s4_s6_cdef.tsv", 6, 144,
         dict(p2_tx["size1"], cdef="force-on", **p2_part["ship"])),
        ("s6-lrf", "s4_s6_lrf.tsv", 6, 144,
         dict(p2_tx["size1"], lrf="force-on", **p2_part["ship"])),
    ]:
        k.update(speed=spd, tune="ssimulacra2", palette="auto", threads=1)
        s.append(dict(path=f"{s4d}/{path}", kind="rd_tsv",
                      corpus="train26", sweep_source="s4tier-2026-07-04",
                      arm_id=f"s4tier/{nm}", knob_json=json.dumps(k, sort_keys=True),
                      encoder_rev=s4_rev, q_kind="cavif_q", speed=spd, rows=rows_))
    s4_cls_n = dict(none_ship=2, size1_ship=8, size1_m32=3, min_ship=6, min_m32=5)
    s4x_cls_n = dict(full_ship=3, full_m32=2)
    s4v_cls_n = dict(none_ship=1, size1_ship=1, size1_m32=5, min_ship=5, min_m32=2)
    for im in ("i7", "i5"):
        for cls, n in s4_cls_n.items():
            tx, part = cls.rsplit("_", 1)
            k = dict(s4_tx[tx], speed=6, tune="ssimulacra2", palette="auto",
                     threads=1, grid="full12q", intra_modes=int(im[1]),
                     **p2_part[part])
            s.append(dict(path=f"{s4d}/s4c_{cls}_{im}.tsv", kind="rd_tsv",
                          corpus="train26", sweep_source="s4tier-2026-07-04",
                          arm_id=f"s4tier/v3{im}-{cls}",
                          knob_json=json.dumps(k, sort_keys=True),
                          encoder_rev=s4_rev, q_kind="cavif_q", speed=6, rows=n * 12))
        for cls, n in s4v_cls_n.items():
            tx, part = cls.rsplit("_", 1)
            k = dict(s4_tx[tx], speed=6, tune="ssimulacra2", palette="auto",
                     threads=1, grid="full12q", intra_modes=int(im[1]),
                     **p2_part[part])
            s.append(dict(path=f"{s4d}/s4v_{cls}_{im}.tsv", kind="rd_tsv",
                          corpus="mech26", sweep_source="s4tier-2026-07-04",
                          arm_id=f"s4tier/val-v3{im}-{cls}",
                          knob_json=json.dumps(k, sort_keys=True),
                          encoder_rev=s4_rev, q_kind="cavif_q", speed=6, rows=n * 12))
    for cls, n in s4x_cls_n.items():
        part = cls.rsplit("_", 1)[1]
        k = dict(s4_tx["full"], speed=6, tune="ssimulacra2", palette="auto",
                 threads=1, grid="full12q", intra_modes=7, **p2_part[part])
        s.append(dict(path=f"{s4d}/s4x_{cls}_i7.tsv", kind="rd_tsv",
                      corpus="train26", sweep_source="s4tier-2026-07-04",
                      arm_id=f"s4tier/oraclex-{cls}",
                      knob_json=json.dumps(k, sort_keys=True),
                      encoder_rev=s4_rev, q_kind="cavif_q", speed=6, rows=n * 12))
    k = dict(p2_tx["size1"], speed=6, tune="ssimulacra2", palette="auto",
             threads=1, grid="full12q")
    s.append(dict(path=f"{s4d}/s4v_base.tsv", kind="rd_tsv",
                  corpus="mech26", sweep_source="s4tier-2026-07-04",
                  arm_id="s4tier/val-base", knob_json=json.dumps(k, sort_keys=True),
                  encoder_rev=s4_rev, q_kind="cavif_q", speed=6, rows=168))

    # --- intraBC chunk B hash-search A/B (2026-07-04) — P3 item 1
    # (zenavif benchmarks/ibc_hash_ab_2026-07-04.tsv). Isolated rav1e-CLI
    # config; every armed cell aomdec-decoded + raw-md5-agreed with
    # rav1d-safe (640/640). hashoff is byte-identical to chunk A / master
    # 0d392334 (81/81 gate cells). Mixed corpus (uvpal sample: train26 +
    # wedge natives + legacy fam-7 trio) for the always-armed AB; pure
    # train26 for the sc10 residual-column pass (tune-ss2 + palette auto).
    ibc = "/mnt/v/output/p3bc-ab-2026-07-04"
    ibc_rev = ("zenrav1e@184eb713 (chunk B; rav1e CLI, isolated: "
               "still-picture threads=1 lrf=false filter-intra=false)")
    for fn, tag, pal, tune, rows in [
        ("ibcA_hashoff.tsv", "chunkA-always", "always", None, 200),
        ("ibcB_hashon.tsv", "chunkAB-always", "always", None, 200),
        ("sc10_hashoff.tsv", "chunkA-auto-ss2", "auto", "ssimulacra2", 120),
        ("sc10_hashon.tsv", "chunkAB-auto-ss2", "auto", "ssimulacra2", 120),
    ]:
        s.append(dict(path=f"{ibc}/{fn}", kind="intrabc_ab_tsv", corpus="mixed",
                      sweep_source="intrabc-hash-2026-07-04", arm_id=f"ibc/{tag}",
                      knob_json=J(cli="rav1e", still_picture=True, threads=1,
                                  lrf=False, filter_intra=False, palette=pal,
                                  tune=tune, intrabc=True,
                                  intrabc_hash=("chunkAB" in tag)),
                      encoder_rev=ibc_rev, q_kind="rav1e_quantizer", speed=None,
                      rows=rows))

    # --- TUNER2 P3-residual arms (2026-07-04; chain_tuner2.sh) ---
    # zenrav1e@6435e6f9 (variance_boost_strength/deep + quant_rounding_bias
    # knobs) via the ravif--tuner2 devpatch (box cavif sha256/16
    # 80ff3fe2f8ce1810). Byte-continuity: t2_cont8 96/96 byte-identical to
    # the store's speedladder/zr-s2-tune rows (env-off identity on the new
    # binary chain), so those rows are the same-binary base curves for the
    # deep/dz arms; the cont cells are NOT re-registered. valstr fills the
    # boost head's named data gap (strength labels on the 14 VAL-LSD
    # origins); drift re-measures {0,4.5} on 3 train origins to de-confound
    # binary drift (the 2026-07-02 deltaq labels predate qmdist+lfsharp).
    t2 = "/mnt/v/output/zenavif/tuner2-20260704"
    t2_rev = "zenrav1e@6435e6f9 via ravif--tuner2 devpatch (80ff3fe2f8ce1810)"
    for sval, tag in [("0.0", "str0"), ("1.0", "str1"), ("2.0", "str2"),
                      ("3.0", "str3"), ("4.5", "str4.5")]:
        s.append(dict(path=f"{t2}/t2_valstr_{sval}.tsv", kind="rd_tsv",
                      corpus="mech26", sweep_source="valstr-2026-07-04",
                      arm_id=f"valstr/{tag}_s2",
                      knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                                  threads=1, variance_boost_strength=float(sval)),
                      encoder_rev=t2_rev, q_kind="cavif_q", speed=2, rows=168))
    for d in ("3.0:4", "4.5:4"):
        s.append(dict(path=f"{t2}/t2_deep_{d.replace(':', '_')}.tsv", kind="rd_tsv",
                      corpus="train26", sweep_source="tuner2-2026-07-04",
                      arm_id=f"tuner2/deep{d.split(':')[0]}_s2",
                      knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                                  threads=1, variance_boost_deep=d),
                      encoder_rev=t2_rev, q_kind="cavif_q", speed=2, rows=144))
    for kk in ("118", "128"):
        s.append(dict(path=f"{t2}/t2_dz_{kk}.tsv", kind="rd_tsv",
                      corpus="train26", sweep_source="tuner2-2026-07-04",
                      arm_id=f"tuner2/qround{kk}_s2",
                      knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                                  threads=1, quant_rounding_bias=int(kk)),
                      encoder_rev=t2_rev, q_kind="cavif_q", speed=2, rows=144))
    for sval in ("0.0", "4.5"):
        s.append(dict(path=f"{t2}/t2_drift_{sval}.tsv", kind="rd_tsv",
                      corpus="train26", sweep_source="tuner2-2026-07-04",
                      arm_id=f"tuner2/drift-str{sval}_s2",
                      knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                                  threads=1, variance_boost_strength=float(sval),
                                  drift_check=True),
                      encoder_rev=t2_rev, q_kind="cavif_q", speed=2, rows=36))
    # Full-t26 strength-0 arm (current binary): with the store's
    # speedladder/zr-s2-tune rows (byte-continuity-proven same-binary
    # str-1.0) this gives CURRENT-binary per-image str1-vs-str0 labels on
    # all 24 train origins — the anti-boost-gate fit input (the valstr data
    # showed boost is disastrous on chart-class val content: 8103 +7.3 /
    # 5343 +5.8 at str1).
    s.append(dict(path=f"{t2}/t2_t26str0.tsv", kind="rd_tsv",
                  corpus="train26", sweep_source="tuner2-2026-07-04",
                  arm_id="tuner2/t26str0_s2",
                  knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                              threads=1, variance_boost_strength=0.0),
                  encoder_rev=t2_rev, q_kind="cavif_q", speed=2, rows=288))

    # --- SSIMRD per-16x16 ssim-rdmult curve arms (2026-07-05;
    # chain_ssimrd.sh) --- zenrav1e@57de2815 (ssim_rdmult_strength knob, the
    # aom av1_set_mb_ssim_rdmult_scaling port) via the ravif--ssimrd devpatch
    # (box cavif sha256/16 909857ad43f9c227). Byte gates: knob-off +
    # Some(0.0) 36/36 md5-identical to master cavif; env-off box rows
    # byte-equal to the store speedladder/zr-s2-tune train26 rows 288/288
    # (so those rows remain same-binary base curves; the fresh sr_base rows
    # are registered anyway as the 12q current-binary bases both corpora).
    # Verdict: monotone honest negative (docs/RD_GAP_VS_LIBAOM.md "SSIMRD");
    # 6018's train-side tri-metric win refuted by val sibling 6091.
    sr = "/mnt/v/output/zenavif/ssimrd-20260705"
    sr_rev = "zenrav1e@57de2815 via ravif--ssimrd devpatch (909857ad43f9c227)"
    s.append(dict(path=f"{sr}/sr_base_t26.tsv", kind="rd_tsv",
                  corpus="train26", sweep_source="ssimrd-2026-07-05",
                  arm_id="ssimrd/base_s2",
                  knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                              threads=1),
                  encoder_rev=sr_rev, q_kind="cavif_q", speed=2, rows=288))
    s.append(dict(path=f"{sr}/sr_base_val.tsv", kind="rd_tsv",
                  corpus="mech26", sweep_source="ssimrd-2026-07-05",
                  arm_id="ssimrd/base-val_s2",
                  knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                              threads=1),
                  encoder_rev=sr_rev, q_kind="cavif_q", speed=2, rows=168))
    for sval in ("0.25", "0.5", "1.0", "2.0"):
        s.append(dict(path=f"{sr}/sr_str_{sval}.tsv", kind="rd_tsv",
                      corpus="train26", sweep_source="ssimrd-2026-07-05",
                      arm_id=f"ssimrd/str{sval}_s2",
                      knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                                  threads=1, ssim_rdmult_strength=float(sval)),
                      encoder_rev=sr_rev, q_kind="cavif_q", speed=2, rows=144))
    s.append(dict(path=f"{sr}/sr_val_0.5.tsv", kind="rd_tsv",
                  corpus="mech26", sweep_source="ssimrd-2026-07-05",
                  arm_id="ssimrd/val-str0.5_s2",
                  knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                              threads=1, ssim_rdmult_strength=0.5),
                  encoder_rev=sr_rev, q_kind="cavif_q", speed=2, rows=84))

    # --- COEFF_RD_STACK composed-posture arms (2026-07-05;
    # chain_coeffrd.sh + docs/COEFF_RD_STACK.md) --- zenrav1e@3e5ff155 +
    # @9bc2b71a (EncoderConfig::coeff_rd_stack: flat rounding [0 =
    # fitted-Valin sentinel] + always-on descent at lambda-scale + aom
    # sharpness guards) via the ravif--coeffrd devpatch (box cavif
    # f4e17fbb7de6f0c4 for base/A-E arms; sentinel rebuild 92ef3ca3437d90b0
    # for the G arms + s1deep base — s2-inert, byte-gated). Byte gates:
    # default-None 36/36 sha vs master rav1e; env-off box rows byte-equal
    # to ssimrd/base_s2 288/288. Verdict: HONEST NEGATIVE at every posture
    # (RD_GAP "COEFF_RD_STACK"); knob stays landed default-off as infra.
    cr = "/mnt/v/output/zenavif/coeffrd-20260705"
    cr_rev = "zenrav1e@9bc2b71a via ravif--coeffrd devpatch (f4e17fbb/92ef3ca3)"
    s.append(dict(path=f"{cr}/cr_base_t26.tsv", kind="rd_tsv",
                  corpus="train26", sweep_source="coeffrd-2026-07-05",
                  arm_id="coeffrd/base_s2",
                  knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                              threads=1),
                  encoder_rev=cr_rev, q_kind="cavif_q", speed=2, rows=288))
    s.append(dict(path=f"{cr}/cr_base_dc.tsv", kind="rd_tsv",
                  corpus="doccharts15", sweep_source="coeffrd-2026-07-05",
                  arm_id="coeffrd/base-dc_s2",
                  knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                              threads=1),
                  encoder_rev=cr_rev, q_kind="cavif_q", speed=2, rows=180))
    for tag, spec in (("A", "128:0.1328:1:0"), ("B", "128:0.35:1:0"),
                      ("C", "128:1.0:1:0"), ("D", "128:4.25:1:0"),
                      ("E", "128:1.0:0:0")):
        s.append(dict(path=f"{cr}/cr_{tag}_t26.tsv", kind="rd_tsv",
                      corpus="train26", sweep_source="coeffrd-2026-07-05",
                      arm_id=f"coeffrd/{tag}_s2",
                      knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                                  threads=1, coeff_rd_stack=spec),
                      encoder_rev=cr_rev, q_kind="cavif_q", speed=2, rows=144))
        s.append(dict(path=f"{cr}/cr_{tag}_dc.tsv", kind="rd_tsv",
                      corpus="doccharts15", sweep_source="coeffrd-2026-07-05",
                      arm_id=f"coeffrd/{tag}-dc_s2",
                      knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                                  threads=1, coeff_rd_stack=spec),
                      encoder_rev=cr_rev, q_kind="cavif_q", speed=2, rows=90))
    for tag, spec in (("G0_1_0_0_0", "0:1.0:0:0"),
                      ("G0_0_35_0_0", "0:0.35:0:0")):
        s.append(dict(path=f"{cr}/cr_{tag}_t26.tsv", kind="rd_tsv",
                      corpus="train26", sweep_source="coeffrd-2026-07-05",
                      arm_id=f"coeffrd/{tag}_s2",
                      knob_json=J(speed=2, tune="ssimulacra2", palette="auto",
                                  threads=1, coeff_rd_stack=spec),
                      encoder_rev=cr_rev, q_kind="cavif_q", speed=2, rows=144))
    # Fresh current-binary s1 baselines on the legacy continuity corpus:
    # plain-s1 (ravif-main state, S1_DEEP_ARMS_LIVE=false binary) AND the
    # shipped deep mode (devpatch flip) — the drift rule's fresh-baseline
    # assets for any future s1 program.
    s.append(dict(path=f"{cr}/cr_s1_base_leg.tsv", kind="rd_tsv",
                  corpus="legacy22", sweep_source="coeffrd-2026-07-05",
                  arm_id="coeffrd/base-plain_s1",
                  knob_json=J(speed=1, tune="ssimulacra2", palette="auto",
                              threads=1, s1_deep_arms=False),
                  encoder_rev=cr_rev, q_kind="cavif_q", speed=1, rows=264))
    s.append(dict(path=f"{cr}/cr_s1deep_base_leg.tsv", kind="rd_tsv",
                  corpus="legacy22", sweep_source="coeffrd-2026-07-05",
                  arm_id="coeffrd/base_s1deep",
                  knob_json=J(speed=1, tune="ssimulacra2", palette="auto",
                              threads=1, s1_deep_arms=True),
                  encoder_rev=cr_rev, q_kind="cavif_q", speed=1, rows=264))
    # Cache-hot aom reference replays fetched with the program (t26 iq+def,
    # doccharts iq — the first aom rows on the doccharts corpus).
    aom_rev5 = "aomenc-3.14.1@632172a4 (build_slow)"
    s.append(dict(path=f"{cr}/aom_t26_cpu2iq.tsv", kind="rd_tsv",
                  corpus="train26", sweep_source="coeffrd-2026-07-05",
                  arm_id="coeffrd/ref-aom-cpu2iq-ai_420",
                  knob_json=J(encoder="aomenc", cpu_used=2, tune="iq",
                              allintra=True, fmt="420"),
                  encoder_rev=aom_rev5, q_kind="aom_cq", speed=2, rows=192))
    s.append(dict(path=f"{cr}/aom_t26_cpu2def.tsv", kind="rd_tsv",
                  corpus="train26", sweep_source="coeffrd-2026-07-05",
                  arm_id="coeffrd/ref-aom-cpu2def-ai_420",
                  knob_json=J(encoder="aomenc", cpu_used=2, tune="default",
                              allintra=True, fmt="420"),
                  encoder_rev=aom_rev5, q_kind="aom_cq", speed=2, rows=192))
    s.append(dict(path=f"{cr}/aom_dc_cpu2iq.tsv", kind="rd_tsv",
                  corpus="doccharts15", sweep_source="coeffrd-2026-07-05",
                  arm_id="coeffrd/ref-aom-cpu2iq-ai-dc_420",
                  knob_json=J(encoder="aomenc", cpu_used=2, tune="iq",
                              allintra=True, fmt="420"),
                  encoder_rev=aom_rev5, q_kind="aom_cq", speed=2, rows=120))
    return s


# ---------------------------------------------------------------------------
def load_train26_map():
    man = json.load(open(TRAIN26_MANIFEST))
    m = {}
    for p in man["picks"]:
        op = p["image_path"]
        base = op.rsplit("/", 1)[-1]
        sweep_name = base[:-4] + ".s1024.png"  # strip .png, append rendition suffix
        sc = "1024" if p["native_longedge"] > 1024 else "native"
        m[sweep_name] = dict(origin_path=op, origin_id=str(p["origin_id"]),
                             content_class=p["content_class"], size_class=sc)
    return m


def load_wedge_map():
    man = json.load(open(f"{WEDGE_DIR}/_MANIFEST.json"))
    opath = {str(p["origin_id"]): p["image_path"] for p in man["picks"]}
    m = {}
    with open(WEDGE_MAP) as f:
        for row in csv.DictReader(f, delimiter="\t"):
            row["origin_path"] = opath.get(row["origin_id"])
            m[row["file"]] = row
    return m


def load_mech26_map():
    """wedge26 ∪ palette-val corpora (both materialized with the wedge
    conventions and join-verified against the features parquet: 123/123 +
    108/108)."""
    m = dict(load_wedge_map())
    with open(f"{PALVAL_DIR}/corpus_map.tsv") as f:
        for row in csv.DictReader(f, delimiter="\t"):
            m[row["file"]] = row
    return m


def read_rd_tsv(path):
    """rd_gap TSV; tolerates the aom files whose header lacks the 2 butteraugli cols."""
    rows = []
    with open(path) as f:
        rd = csv.reader(f, delimiter="\t")
        hdr = next(rd)
        for raw in rd:
            if not raw or raw[0].startswith("#"):
                continue
            if len(raw) == len(hdr) + 2 and "butteraugli_3n" not in hdr:
                r = dict(zip(hdr + ["butteraugli_3n", "butteraugli_max"], raw))
            elif len(raw) == len(hdr):
                r = dict(zip(hdr, raw))
            else:
                raise ValueError(f"{path}: row width {len(raw)} vs header {len(hdr)}")
            rows.append(r)
    return rows


def size_slot(crop_label, size_class):
    if crop_label != "full":
        return "crop"
    return "top" if size_class in ("2048", "native") else size_class


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    t26 = load_train26_map()
    wmap = load_wedge_map()
    mmap = load_mech26_map()

    # feature-join universe (existence + dims verification)
    ft = pq.read_table(FEATURES_PARQUET, columns=["image_path", "crop_label", "size_class", "width", "height"])
    fd = ft.to_pydict()
    fidx = {}
    for i in range(ft.num_rows):
        fidx[(fd["image_path"][i], fd["crop_label"][i], fd["size_class"][i])] = (fd["width"][i], fd["height"][i])

    def feature_join_for(origin_path, crop_label, size_class, w, h):
        """Return (join_key or None). Verifies row existence AND exact dims."""
        if not origin_path:
            return None
        key = (origin_path, crop_label, size_class)
        dims = fidx.get(key)
        if dims is None or dims != (w, h):
            return None
        return f"{origin_path}|{crop_label}|{size_class}"

    out = []
    counts = collections.Counter()

    def emit(**kw):
        out.append(kw)
        counts[kw["sweep_source"], kw["source_file"]] += 1

    # ---- registry-driven TSV sources ----
    for src in sources():
        path, kind = src["path"], src["kind"]
        base = os.path.basename(path)
        if kind == "rd_tsv":
            rows = read_rd_tsv(path)
            assert len(rows) == src["rows"], f"{path}: {len(rows)} rows != expected {src['rows']}"
            for r in rows:
                img = r["image"]
                w, h = int(r["w"]), int(r["h"])
                if src["corpus"] == "train26":
                    m = t26.get(img)
                    assert m, f"{path}: train26 image not in manifest map: {img}"
                    oid, opath, cclass = m["origin_id"], m["origin_path"], m["content_class"]
                    crop, sc = "full", m["size_class"]
                    fj = feature_join_for(opath, crop, sc, w, h)
                    fj_exact = False if fj else None
                elif src["corpus"] == "wedge26":
                    m = wmap.get(img)
                    assert m, f"{path}: wedge image not in corpus map: {img}"
                    oid, opath, cclass = m["origin_id"], m["origin_path"], m["content_class"]
                    crop, sc = m["crop_label"], m["size_class"]
                    fj = feature_join_for(opath, crop, sc, w, h)
                    fj_exact = True if fj else None  # wedge corpus pixel-verified 123/123
                elif src["corpus"] == "mech26":
                    m = mmap.get(img)
                    assert m, f"{path}: mech26 image not in corpus maps: {img}"
                    oid, opath, cclass = m["origin_id"], m["origin_path"], m["content_class"]
                    crop, sc = m["crop_label"], m["size_class"]
                    fj = feature_join_for(opath, crop, sc, w, h)
                    fj_exact = True if fj else None  # both corpora pixel-verified
                else:  # legacy22
                    oid = lsd_origin_id(img)
                    opath, cclass, crop, sc = None, None, "full", "1024"
                    fj, fj_exact = None, None
                emit(image_id=img, corpus=src["corpus"], origin_id=oid, origin_path=opath,
                     split=split_of(img), content_class=cclass, family=r["family"],
                     crop_label=crop, size_class=sc, size_slot=size_slot(crop, sc),
                     w=w, h=h, px=w * h,
                     encoder=("libaom" if r["encoder"] == "libaom" else "zenrav1e"),
                     fmt=r["fmt"], speed=src["speed"], arm_id=src["arm_id"],
                     knob_json=src["knob_json"], q=float(r["q"]), q_kind=src["q_kind"],
                     bytes=int(r["bytes"]), bpp=float(r["bpp"]), ssim2=float(r["ssim2"]),
                     butteraugli_3n=float(r["butteraugli_3n"]) if r.get("butteraugli_3n") else None,
                     butteraugli_max=float(r["butteraugli_max"]) if r.get("butteraugli_max") else None,
                     enc_ms=float(r["enc_ms"]) if r.get("enc_ms") else None,
                     sweep_source=src["sweep_source"], source_file=base,
                     encoder_rev=src["encoder_rev"],
                     feature_join=fj, feature_join_exact=fj_exact,
                     file_bd_cpu2=None, file_bd_cpu0=None, file_reach_cpu2_bppx=None)
        elif kind == "palette_tsv":
            n = 0
            with open(path) as f:
                lines = [ln for ln in f if not ln.startswith("#")]
            for r in csv.DictReader(lines, delimiter="\t"):
                img = r["image"] + ".png"  # results.tsv strips the extension
                m = t26.get(img)
                assert m, f"{path}: palette image not in train26 map: {img}"
                # dims from the train26 feature row (verified exact vs sweep TSVs)
                key = (m["origin_path"], "full", m["size_class"])
                w, h = fidx[key]
                spd = int(r["speed"])
                arm = r["arm"]
                fj = feature_join_for(m["origin_path"], "full", m["size_class"], w, h)
                emit(image_id=img, corpus="train26", origin_id=m["origin_id"],
                     origin_path=m["origin_path"], split=split_of(img),
                     content_class=m["content_class"], family=None,
                     crop_label="full", size_class=m["size_class"],
                     size_slot=size_slot("full", m["size_class"]), w=w, h=h, px=w * h,
                     encoder="zenrav1e", fmt="420(y4m)", speed=spd,
                     arm_id=f"palette/{arm}_s{spd}",
                     knob_json=J(cli="rav1e", speed=spd, still_picture=True, threads=1,
                                 lrf=False, filter_intra=False, palette=arm),
                     q=float(r["q"]), q_kind=src["q_kind"], bytes=int(r["bytes"]),
                     bpp=int(r["bytes"]) * 8.0 / (w * h), ssim2=float(r["ssim2"]),
                     butteraugli_3n=float(r["butter_p3"]), butteraugli_max=float(r["butter_max"]),
                     enc_ms=float(r["enc_ms"]) if r.get("enc_ms") else None,
                     sweep_source=src["sweep_source"], source_file=base,
                     encoder_rev=src["encoder_rev"],
                     feature_join=fj, feature_join_exact=False if fj else None,
                     file_bd_cpu2=None, file_bd_cpu0=None, file_reach_cpu2_bppx=None)
                n += 1
            assert n == src["rows"], f"{path}: {n} rows != expected {src['rows']}"
        elif kind == "palette_sizes_tsv":
            # run_palette_iso.sh output over the mech26 corpus (wedge + val
            # renditions at their materialized sizes/crops).
            n = 0
            with open(path) as f:
                lines = [ln for ln in f if not ln.startswith("#")]
            for r in csv.DictReader(lines, delimiter="\t"):
                img = r["image"] + ".png"
                m = mmap.get(img)
                assert m, f"{path}: image not in mech26 corpus maps: {img}"
                w, h = int(m["width"]), int(m["height"])
                spd = int(r["speed"])
                arm = r["arm"]
                crop, sc = m["crop_label"], m["size_class"]
                fj = feature_join_for(m["origin_path"], crop, sc, w, h)
                emit(image_id=img, corpus="mech26", origin_id=m["origin_id"],
                     origin_path=m["origin_path"], split=split_of(img),
                     content_class=m["content_class"], family=m.get("family"),
                     crop_label=crop, size_class=sc,
                     size_slot=size_slot(crop, sc), w=w, h=h, px=w * h,
                     encoder="zenrav1e", fmt="420(y4m)", speed=spd,
                     arm_id=f"palette-mech-iso/{arm}_s{spd}",
                     knob_json=J(cli="rav1e", speed=spd, still_picture=True, threads=1,
                                 lrf=False, filter_intra=False, palette=arm),
                     q=float(r["q"]), q_kind=src["q_kind"], bytes=int(r["bytes"]),
                     bpp=int(r["bytes"]) * 8.0 / (w * h), ssim2=float(r["ssim2"]),
                     butteraugli_3n=float(r["butter_p3"]), butteraugli_max=float(r["butter_max"]),
                     enc_ms=float(r["enc_ms"]) if r.get("enc_ms") else None,
                     sweep_source=src["sweep_source"], source_file=base,
                     encoder_rev=src["encoder_rev"],
                     feature_join=fj, feature_join_exact=True if fj else None,
                     file_bd_cpu2=None, file_bd_cpu0=None, file_reach_cpu2_bppx=None)
                n += 1
            assert n == src["rows"], f"{path}: {n} rows != expected {src['rows']}"
        elif kind == "intrabc_ab_tsv":
            # run_palette_iso.sh output over the uvpal mixed corpus (train26 +
            # wedge natives + legacy fam-7 trio) or pure train26 (sc10).
            # Per-image corpus resolution: t26 -> wedge -> mech26 -> legacy.
            n = 0
            with open(path) as f:
                lines = [ln for ln in f if not ln.startswith("#")]
            for r in csv.DictReader(lines, delimiter="\t"):
                img = r["image"] + ".png"
                spd = int(r["speed"])
                m = t26.get(img)
                corpus = "train26"
                if m is None:
                    m = wmap.get(img)
                    corpus = "wedge26"
                if m is None:
                    m = mmap.get(img)
                    corpus = "mech26"
                if m is None:
                    assert img.startswith("o_"), \
                        f"{path}: image in no corpus map and not legacy: {img}"
                    corpus = "legacy22"
                    oid, opath, cclass = lsd_origin_id(img), None, None
                    crop, sc, fam = "full", "1024", "7000L"
                    w = h = 1024
                    fj, fj_exact = None, None
                else:
                    oid, opath = m["origin_id"], m["origin_path"]
                    cclass = m["content_class"]
                    crop = m.get("crop_label", "full")
                    sc = m["size_class"]
                    fam = m.get("family")
                    if corpus == "train26":
                        w, h = fidx[(opath, "full", sc)]
                    else:
                        w, h = int(m["width"]), int(m["height"])
                    fj = feature_join_for(opath, crop, sc, w, h)
                    fj_exact = (corpus != "train26") if fj else None
                emit(image_id=img, corpus=corpus, origin_id=oid,
                     origin_path=opath, split=split_of(img),
                     content_class=cclass, family=fam, crop_label=crop,
                     size_class=sc, size_slot=size_slot(crop, sc),
                     w=w, h=h, px=w * h,
                     encoder="zenrav1e", fmt="420(y4m)", speed=spd,
                     arm_id=f"{src['arm_id']}_s{spd}",
                     knob_json=src["knob_json"], q=float(r["q"]),
                     q_kind=src["q_kind"], bytes=int(r["bytes"]),
                     bpp=int(r["bytes"]) * 8.0 / (w * h),
                     ssim2=float(r["ssim2"]),
                     butteraugli_3n=float(r["butter_p3"]) if r.get("butter_p3") not in (None, "", "NA") else None,
                     butteraugli_max=float(r["butter_max"]) if r.get("butter_max") not in (None, "", "NA") else None,
                     enc_ms=float(r["enc_ms"]) if r.get("enc_ms") else None,
                     sweep_source=src["sweep_source"], source_file=base,
                     encoder_rev=src["encoder_rev"],
                     feature_join=fj, feature_join_exact=fj_exact,
                     file_bd_cpu2=None, file_bd_cpu0=None, file_reach_cpu2_bppx=None)
                n += 1
            assert n == src["rows"], f"{path}: {n} rows != expected {src['rows']}"

    # ---- wedge dataset parquet (zr / cpu2 / cpu0 arms, feature_join precomputed) ----
    wp = pq.read_table(f"{WEDGE_DIR}/wedge_dataset.parquet").to_pandas()
    warm = {
        "zr": ("wedge/zr-best_s2", J(speed=2, tune="ssimulacra2", palette="auto", depth=8),
               "zenrav1e@32477046 via ravif--wedge@9d2b97c", "cavif_q", 2, "zenrav1e"),
        "cpu2": ("wedge/aom-cpu2", J(encoder="aomenc", cpu_used=2, tune="default"),
                 "aomenc-3.14.1@632172a4", "aom_cq", 2, "libaom"),
        "cpu0": ("wedge/aom-cpu0-default", J(encoder="aomenc", cpu_used=0, tune="default"),
                 "aomenc-3.14.1@632172a4", "aom_cq", 0, "libaom"),
    }
    for _, r in wp.iterrows():
        arm_id, knob, rev, qk, spd, enc = warm[r["arm"]]
        fj = r["feature_join"] if r["feature_join"] else None
        emit(image_id=r["file"], corpus="wedge26", origin_id=str(r["origin_id"]),
             origin_path=r["origin_path"], split=split_of(r["file"]),
             content_class=r["content_class"], family=str(r["family"]),
             crop_label=r["crop_label"], size_class=r["size_class"],
             size_slot=r["size_slot"] if r["size_slot"] != "crop" else "crop",
             w=int(r["width"]), h=int(r["height"]), px=int(r["width"]) * int(r["height"]),
             encoder=enc, fmt=r["fmt"], speed=spd, arm_id=arm_id, knob_json=knob,
             q=float(r["q"]), q_kind=qk, bytes=int(r["bytes"]), bpp=float(r["bpp"]),
             ssim2=float(r["ssim2"]),
             butteraugli_3n=float(r["butteraugli_3n"]) if pd.notna(r["butteraugli_3n"]) else None,
             butteraugli_max=float(r["butteraugli_max"]) if pd.notna(r["butteraugli_max"]) else None,
             enc_ms=float(r["enc_ms"]) if pd.notna(r["enc_ms"]) else None,
             sweep_source="wedge-2026-07-03", source_file="wedge_dataset.parquet",
             encoder_rev=rev, feature_join=fj,
             feature_join_exact=True if fj else None,
             file_bd_cpu2=float(r["file_bd_cpu2"]) if pd.notna(r["file_bd_cpu2"]) else None,
             file_bd_cpu0=float(r["file_bd_cpu0"]) if pd.notna(r["file_bd_cpu0"]) else None,
             file_reach_cpu2_bppx=float(r["file_reach_cpu2_bppx"]) if pd.notna(r["file_reach_cpu2_bppx"]) else None)

    df = pd.DataFrame(out)

    # ---- verification gates ----
    assert df["feature_join"].notna().equals(df["feature_join_exact"].notna())
    # every non-null join must exist in the features parquet
    fkeys = {f"{k[0]}|{k[1]}|{k[2]}" for k in fidx}
    bad = set(df.loc[df["feature_join"].notna(), "feature_join"]) - fkeys
    assert not bad, f"feature_join keys missing from features parquet: {sorted(bad)[:5]}"
    assert df["split"].isin(["train", "val", "test"]).all(), "unsplittable image ids present"
    for c in ["bytes", "bpp", "ssim2", "q"]:
        assert df[c].notna().all(), f"NULLs in required column {c}"

    df = df.sort_values(["sweep_source", "arm_id", "image_id", "fmt", "q"]).reset_index(drop=True)
    table = pa.Table.from_pandas(df, preserve_index=False)
    pq.write_table(table, f"{OUT_DIR}/labels.parquet", compression="zstd")

    # ---- manifest ----
    def git(*args):
        return subprocess.run(["git", "-C", os.path.join(ZEN, "zenavif")] + list(args),
                              capture_output=True, text=True).stdout.strip()

    per_source = {k: int(v) for k, v in df.groupby("sweep_source").size().items()}
    per_file = {f"{k[0]}/{k[1]}": int(v) for k, v in counts.items()}
    join_cov = {
        k: dict(rows=int(g.shape[0]), joined=int(g["feature_join"].notna().sum()),
                exact=int((g["feature_join_exact"] == True).sum()))  # noqa: E712
        for k, g in df.groupby("corpus")
    }
    manifest = {
        "built": pd.Timestamp.utcnow().isoformat(),
        "build_commit": git("rev-parse", "HEAD"),
        "build_commit_subject": git("log", "-1", "--format=%s"),
        "builder": "scripts/hyperparam/build_label_store.py",
        "total_rows": int(df.shape[0]),
        "rows_per_source": per_source,
        "rows_per_file": per_file,
        "feature_join_coverage_per_corpus": join_cov,
        "split_rule": "canonical LSD origin rule via zenmetrics/scripts/picker/origin_split.py "
                      "(even->train, 1/3/5->val, 7/9->test). The features parquet's own 'split' "
                      "column is an OLDER convention (disagrees on 1148/2157 origins) — do not use it.",
        "q_kind_semantics": {
            "cavif_q": "cavif -Q quality 0-100 (higher=better)",
            "aom_cq": "aomenc --cq-level 0-63 (lower=better)",
            "rav1e_quantizer": "rav1e --quantizer 0-255 (lower=better)",
        },
        "feature_join": {
            "target": FEATURES_PARQUET,
            "key": "origin_path|crop_label|size_class (== image_path|crop_label|size_class in the parquet)",
            "exact_true": "wedge26 corpus: encode pixels == feature pixels (verified 123/123, rel-tol 1e-3)",
            "exact_false": "train26 corpus: same origin+size_class+WxH (dims verified exact) but the sweep "
                           "rendition is vipsthumbnail --linear while feature renditions are Lanczos3-sRGB; "
                           "content-level features are robust to this, pixel-exact features are not "
                           "(wedge anchors measured the rendition drift at ±5% bytes / ±2 ssim2)",
            "null": "legacy22 corpus (o_* dense-corpus renditions, not in imazen-26 features) and any "
                    "row whose derived key failed existence/dims verification (builder asserts none do)",
        },
        "encoder_rev_validity": "arm deltas are valid WITHIN one sweep_source; encoder_rev strings record "
                                "the measured binary chain per file. Cross-sweep byte-continuity that WAS "
                                "verified: qmdist t26c_s2_base == deltaq t26_s2_dq_str1 (144/144 bpp-EXACT); "
                                "lfsharp ws binary == 9b79b442 master (18/18 md5); desyncfix shipped config "
                                "byte-neutral vs 9b79b442.",
        "enc_ms_notes": {
            "deltaq-2026-07-02": "fit arms ran solo (clean); dq0 baseline + legacy confirms + aom refs ran "
                                 "under box contention — unusable for speed claims",
            "tune-ss2-2026-07-02": "JOBS=22 concurrent cells on ccx63",
            "qmdist-2026-07-03": "JOBS=24; enc_time_vs_base ratios in the committed summary are the "
                                 "session's own within-run comparisons",
            "lfsharp-2026-07-03": "JOBS=12x4 concurrent runs — contended",
            "desyncfix-2026-07-03": "LOCAL workstation (7950X), JOBS=6 — different host than every other source",
            "wedge-2026-07-03": "arm-parallel boxes (each arm solo per box)",
            "palette-ab-final2-2026-07-03": "local workstation, rav1e CLI single-threaded",
            "palette-mech-ab-2026-07-03": "JOBS=12-28 concurrent (box-2) — contended; the RD_CACHE=off "
                                          "timing sidecar benchmarks/hyperparam_palette_mech_timing_"
                                          "2026-07-03.tsv is the authoritative time source",
            "palette-mech-iso-2026-07-03": "JOBS=26 concurrent single-threaded rav1e cells — contended",
            "palette-mech-iso-s8-2026-07-03": "JOBS=22 concurrent single-threaded rav1e cells — contended; "
                                              "within-cell always/off ratio (median 2.13x, p90 3.27x) is the "
                                              "honest fired-cost signal at s8",
        },
        "palette_pipeline_caveat": "palette-ab rows: rav1e CLI on color.py-converted 420 y4m, aomdec decode, "
                                   "isolated config (still-picture, threads=1, lrf=false, filter-intra=false). "
                                   "Absolute ssim2/butteraugli NOT comparable to cavif rows; within-source arm "
                                   "deltas valid. All cells passed aomdec; palette-armed cells additionally "
                                   "passed aomdec-vs-rav1d-safe md5 agreement. Same pipeline + conformance "
                                   "bar for palette-mech-iso-2026-07-03 (1800/1800 armed cells md5-agree, "
                                   "zenrav1e@32477046, scripts/rd_gap/palette_iso_cell.sh) and for "
                                   "palette-mech-iso-s8-2026-07-03 (900/900 armed cells md5-agree, same "
                                   "binary sha-continuity-proven, IVFs kept in ivf_s8/); the shipped "
                                   "palette-mech-ab cavif arms were PALCONF-verified per cell instead "
                                   "(extract_av1 -> aomdec + rav1d-safe raw-md5, scripts/rd_gap/"
                                   "zenrav1e_cell.sh PALCONF=1). Grid caveat: shipped s2 arms are 12-pt "
                                   "while the reused wedge off/auto rows are 6-pt — restrict BD pairs to "
                                   "the common grid (analyze_palette_mech_ab.py does; mixing densities "
                                   "biases small-size trapezoids by +1..+4%).",
        "excluded": [
            "all *conformance*.tsv (pass/fail artifacts, different schema, not RD labels)",
            "zenrav1e-palette superseded runs (sweep-20260703, -auto, -colorpy) and fam7-continuity "
            "(legacy plot origins, no feature rows; committed summary exists in benchmarks/)",
            "drift-2026-07-02 (canonical-vs-master re-encode study, not a hyperparameter arm sweep)",
        ],
        "append_protocol": "add SOURCES entries (path, arm_id, knob_json, encoder_rev, q_kind, rows) and "
                           "re-run; the builder is deterministic and asserts row counts + join integrity",
    }
    with open(f"{OUT_DIR}/_MANIFEST.json", "w") as f:
        json.dump(manifest, f, indent=2)

    print(f"wrote {OUT_DIR}/labels.parquet: {df.shape[0]} rows x {df.shape[1]} cols")
    print(json.dumps(per_source, indent=2))
    print("join coverage:", json.dumps(join_cov, indent=2))
    print("arms:", df["arm_id"].nunique())


if __name__ == "__main__":
    main()
