"""
Codec config for the zenavif rav1e knob picker — v0.1 (Phase 1a baseline).

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
  - speed ∈ {1..10}  — 10 cells
  - (Phase 2+) chroma_subsampling, qm, vaq, tune_still get added once
    Phase 2 OAT confirms which macro knobs survive the cull threshold.

Scalar prediction heads (SCALAR_AXES):
  - (none in v0.1 — Phase 1a only varies speed/q; secondary knobs
    like vaq_strength enter as scalars in v0.2 once Phase 2 OAT
    confirms which survive the cull threshold.)

For time-budget constraints, encode_ms is exposed via a side
lookup-table baked alongside the picker (median encode_ms / pixel
per (speed, size_class) cell from the training data). The auto_tune
runtime applies this LUT independently of the MLP forward pass.

So 10 cells × (1 bytes_log + 0 scalars) = 10 output dimensions.

The Phase 1a sweep emits config_name strings of the form:
  `s{speed}_q{q}_qm{qm}_vaq{vaq}_strength{vaq_strength}_tune{tune_still}`
e.g. `s4_q60_qm1_vaq0_strength1.0_tune1`. The Phase 1a corpus holds
qm/vaq/vaq_strength/tune_still constant; downstream phases add
sweep coverage on those axes.
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

# Phase 1a: only `speed` varies as a categorical axis. Phase 2+ will
# extend with chroma_subsampling, qm, vaq, tune_still after the OAT
# sensitivity cull.
CATEGORICAL_AXES = ["speed"]

# Phase 1a has no secondary scalar knobs (vaq_strength held constant
# at 1.0). Phase 2+ adds vaq_strength here once the OAT cull confirms
# vaq is a load-bearing macro knob.
SCALAR_AXES: list = []
SCALAR_SENTINELS: dict = {}
SCALAR_DISPLAY_RANGES: dict = {}


# ---------- Config-name parser ----------

# Format: s{speed}_q{q}_qm{qm}_vaq{vaq}_strength{vaq_strength}_tune{tune_still}
#   speed: u8 (1..10)
#   q: u8 (target zensim approximation, 0..100)
#   qm/vaq/tune_still: 0|1 booleans
#   vaq_strength: f32 with one decimal
# Examples:
#   s4_q60_qm1_vaq0_strength1.0_tune1
#   s10_q95_qm0_vaq1_strength0.5_tune0
_CONFIG_RE = re.compile(
    r"^s(?P<speed>\d+)_q(?P<q>\d+)"
    r"_qm(?P<qm>\d+)_vaq(?P<vaq>\d+)"
    r"_strength(?P<strength>[\d.]+)"
    r"_tune(?P<tune>\d+)$"
)


def parse_config_name(name: str) -> dict:
    """Decompose a Phase 1a+ predictor_sweep config name into axes."""
    m = _CONFIG_RE.match(name)
    if not m:
        raise ValueError(f"unparseable rav1e config name: {name}")
    return {
        # Categorical (cells):
        "speed": int(m.group("speed")),
        # Held constant in Phase 1a; will become categorical in Phase 2+:
        "qm": int(m.group("qm")),
        "vaq": int(m.group("vaq")),
        "tune_still": int(m.group("tune")),
        "vaq_strength": float(m.group("strength")),
        # Scalars (q is the zensim-target axis, not a head):
        "q": int(m.group("q")),
    }
