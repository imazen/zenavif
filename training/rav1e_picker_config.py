"""
Codec config for the zenavif rav1e knob picker — v0.2 (post Phase 2 OAT).

Used by `zentrain/tools/train_hybrid.py`. Defines paths to the
predictor_sweep + extract_features TSVs, the feature subset the
picker consumes, the target_zq grid, the regex that parses the
sweep's config_name strings, and the explicit axis schema.

Run training from the zenavif checkout:

    cd ~/work/zen/zenavif
    PYTHONPATH=~/work/zen/zenavif/training:~/work/zen/zenanalyze/zentrain/tools \\
        python3 ~/work/zen/zenanalyze/zentrain/tools/train_hybrid.py \\
            --codec-config rav1e_picker_config

Cell taxonomy (CATEGORICAL_AXES):
  - speed ∈ {1..10}  — 10 levels
  - qm ∈ {0, 1}      — 2 levels (Phase 2 OAT: +4.2 % bytes when off)
  → 20 cells

Scalar prediction heads (SCALAR_AXES, post Phase 2 OAT):
  - vaq_strength (0.0..3.0)
  - seg_boost (1.0..2.0)
  - rdo_tx_decision_override (0/1; left as scalar so trainer learns
    a probability and the runtime threshold-rounds)
  - segmentation_complex_override
  - encode_bottomup_override
  - lrf_override
  - partition_range_override (-1=fine, 0=preset, +1=coarse)

CULLED knobs (Phase 2 OAT, median |Δ% bytes| < 0.5 % AND p90 < 1.5 %):
  cdef, complex_prediction_modes, fast_deblock, lru_on_skip, sgr_full,
  trellis, tune_still, vaq (at default strength 1.0; non-1.0 strengths
  surface via vaq_strength scalar).

Sweep config_name strings have the form:
  s{speed}_q{q}_qm{qm}_vaq{vaq}_strength{vaq_strength}_tune{tune_still}\
   _seg{seg_boost}_rdo{rdo_tx}_segc{seg_complex}_bu{bottomup}_lrf{lrf}_pr{pr_idx}

The legacy short form (Phase 1a) is parsed too — extra knobs default
to "preset" / 0 in the parsed dict so v0.1 + v0.2 rows mix cleanly.
"""

from __future__ import annotations

import os
import re
from pathlib import Path

# ---------- Paths ----------

# zenavif/examples/predictor_sweep.rs writes here. Bump the date when
# rerunning the sweep on a new corpus or with a new config grid.
# Env override RAV1E_PARETO_TSV / RAV1E_FEATURES_TSV lets the orchestrator
# point at a snapshot taken by training/clean_tsv.py while a live sweep
# is appending — avoids the trainer racing the writer.
PARETO = Path(os.environ.get("RAV1E_PARETO_TSV", "benchmarks/rav1e_phase1a_2026-04-30.tsv"))
FEATURES = Path(
    os.environ.get(
        "RAV1E_FEATURES_TSV", "benchmarks/rav1e_phase1a_features_2026-04-30.tsv"
    )
)

OUT_JSON = Path("benchmarks/rav1e_picker_v0_1.json")
OUT_LOG = Path("benchmarks/rav1e_picker_v0_1.log")


# ---------- Schema ----------

# Start broad: every feature in zenanalyze's SUPPORTED set that's
# defensible for AVIF/AV1 encode prediction. After v0.1 trains, run
# `tools/feature_ablation.py` + `feature_group_ablation.py` to prune.
#
# Excluded:
# - HDR features: this corpus is sRGB-only; HDR signals are ~constant
#   across train (not load-bearing). Re-add when an HDR corpus joins.
# - Alpha features: zenavif still encodes only RGB (no alpha) in
#   Phase 1a.
KEEP_FEATURES = [
    # Default tier — broad image-property signals
    "feat_variance",
    "feat_edge_density",
    "feat_chroma_complexity",
    "feat_cb_sharpness",
    "feat_cr_sharpness",
    "feat_uniformity",
    "feat_flat_color_block_ratio",
    "feat_distinct_color_bins",
    "feat_high_freq_energy_ratio",
    "feat_luma_histogram_entropy",
    # Composite / classifier signals
    "feat_text_likelihood",
    "feat_screen_content_likelihood",
    "feat_natural_likelihood",
    "feat_grayscale_score",
    # Per-axis chroma sharpness — important for screen content and
    # text where horizontal vs vertical edges differ
    "feat_cb_horiz_sharpness",
    "feat_cb_vert_sharpness",
    "feat_cb_peak_sharpness",
    "feat_cr_horiz_sharpness",
    "feat_cr_vert_sharpness",
    "feat_cr_peak_sharpness",
    # Experimental tier
    "feat_colourfulness",
    "feat_laplacian_variance",
    "feat_variance_spread",
    "feat_palette_density",
    "feat_dct_compressibility_y",
    "feat_dct_compressibility_uv",
    "feat_patch_fraction",
    "feat_patch_fraction_fast",
    "feat_aq_map_mean",
    "feat_aq_map_std",
    "feat_noise_floor_y",
    "feat_noise_floor_uv",
    "feat_gradient_fraction",
    "feat_skin_tone_fraction",
    "feat_edge_slope_stdev",
    "feat_line_art_score",
    "feat_quant_survival_y",
    "feat_quant_survival_uv",
    # Geometry / size — load-bearing for size-class invariance
    "feat_log_pixels",
    "feat_log_min_dim",
    "feat_log_max_dim",
    "feat_aspect_min_over_max",
    "feat_log_aspect_abs",
]

# Zq target grid: cover the q5..q60 region densely per CLAUDE.md
# web-focused-aggressive-compression rule (selector training data
# must weight low-q proportionally; we encode at every q so the
# picker can argmin over a dense range without quantization).
#
# q step 5 from 30..70, q step 2 from 70..100 = production-relevant
# perceptibility band. Sub-30 cells are typically too lossy to ship
# but we keep step-5 down to 30 for coverage.
ZQ_TARGETS = list(range(30, 70, 5)) + list(range(70, 96, 2))


# ---------- Axis schema ----------

# v0.2 schema (post Phase 2 OAT 2026-04-30).
# qm survives the cull (median +4.2 % bytes when off) so promotes to
# a categorical axis. vaq @ default strength is culled but
# vaq_strength as a scalar is kept (non-default values save 2-3 %).
CATEGORICAL_AXES = ["speed", "qm"]

# Scalar heads — survivors with continuous-ish ranges.
SCALAR_AXES: list = [
    "vaq_strength",
    "seg_boost",
    "rdo_tx_off",  # 0 = preset (on at speed=4), 1 = forced off
    "seg_complex_on",  # 0 = preset (off at speed=4), 1 = forced on
    "bottomup_on",  # 0 = preset (off at speed=4), 1 = forced on
    "lrf_on",  # 0 = preset, 1 = forced on
    "partition_range_idx",  # -1=fine_4_16, 0=preset, +1=coarse_16_64
]
SCALAR_SENTINELS: dict = {}
SCALAR_DISPLAY_RANGES: dict = {
    "vaq_strength": (0.0, 3.0),
    "seg_boost": (1.0, 2.0),
    "rdo_tx_off": (0, 1),
    "seg_complex_on": (0, 1),
    "bottomup_on": (0, 1),
    "lrf_on": (0, 1),
    "partition_range_idx": (-1, 1),
}


# ---------- Config-name parser ----------

# Phase 1a (v0.1) short form:
#   s{speed}_q{q}_qm{qm}_vaq{vaq}_strength{vaq_strength}_tune{tune_still}
# Phase 3+ (v0.2) full form appends the surviving knob axes:
#   ..._seg{seg_boost}_rdo{rdo_tx}_segc{seg_complex}_bu{bottomup}_lrf{lrf}_pr{pr_idx}
_CONFIG_RE_BASE = re.compile(
    r"^s(?P<speed>\d+)_q(?P<q>\d+)"
    r"_qm(?P<qm>\d+)_vaq(?P<vaq>\d+)"
    r"_strength(?P<strength>[\d.]+)"
    r"_tune(?P<tune>\d+)"
    r"(?P<rest>.*)$"
)
_CONFIG_RE_REST = re.compile(
    r"_seg(?P<seg>[\d.]+)"
    r"_rdo(?P<rdo>\d+)"
    r"_segc(?P<segc>\d+)"
    r"_bu(?P<bu>\d+)"
    r"_lrf(?P<lrf>\d+)"
    r"_pr(?P<pr>-?\d+)$"
)


def parse_config_name(name: str) -> dict:
    """Decompose a predictor_sweep config name into axes."""
    m = _CONFIG_RE_BASE.match(name)
    if not m:
        raise ValueError(f"unparseable rav1e config name: {name}")
    out = {
        # Categorical (cells):
        "speed": int(m.group("speed")),
        "qm": int(m.group("qm")),
        # Scalars:
        "vaq_strength": float(m.group("strength")),
        # Defaults for missing v0.2 knobs — Phase 1a rows fall here:
        "seg_boost": 1.0,
        "rdo_tx_off": 0,
        "seg_complex_on": 0,
        "bottomup_on": 0,
        "lrf_on": 0,
        "partition_range_idx": 0,
        # Pre-cull legacy keys we still parse but don't use as axes:
        "vaq": int(m.group("vaq")),
        "tune_still": int(m.group("tune")),
        # q is the target-zensim axis, not a head:
        "q": int(m.group("q")),
    }
    rest = m.group("rest")
    if rest:
        rm = _CONFIG_RE_REST.match(rest)
        if not rm:
            raise ValueError(f"unparseable rav1e v0.2 suffix in: {name}")
        out["seg_boost"] = float(rm.group("seg"))
        out["rdo_tx_off"] = int(rm.group("rdo"))
        out["seg_complex_on"] = int(rm.group("segc"))
        out["bottomup_on"] = int(rm.group("bu"))
        out["lrf_on"] = int(rm.group("lrf"))
        out["partition_range_idx"] = int(rm.group("pr"))
    return out
