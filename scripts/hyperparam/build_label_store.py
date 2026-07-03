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
        },
        "palette_pipeline_caveat": "palette-ab rows: rav1e CLI on color.py-converted 420 y4m, aomdec decode, "
                                   "isolated config (still-picture, threads=1, lrf=false, filter-intra=false). "
                                   "Absolute ssim2/butteraugli NOT comparable to cavif rows; within-source arm "
                                   "deltas valid. All cells passed aomdec; palette-armed cells additionally "
                                   "passed aomdec-vs-rav1d-safe md5 agreement. Same pipeline + conformance "
                                   "bar for palette-mech-iso-2026-07-03 (1800/1800 armed cells md5-agree, "
                                   "zenrav1e@32477046, scripts/rd_gap/palette_iso_cell.sh); the shipped "
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
