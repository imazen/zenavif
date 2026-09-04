//! **The tune contract** — what a backend-tuner bake declares about
//! itself, validated at load.
//!
//! A ZNPR bake is a bag of floats until something says what its inputs
//! and outputs *mean*. For [`AvifTuner`](super::AvifTuner) that meaning
//! is four metadata keys, all written by the training lane, all checked
//! here against the model's real widths. A bake that does not declare a
//! well-formed contract is **refused** — it is never read positionally
//! and hoped for.
//!
//! # The keys
//!
//! | key | shape |
//! |---|---|
//! | [`CELLS_KEY`] | one cell label per cell, `\n`- or `,`-separated |
//! | [`HEADS_KEY`] | the per-cell output heads, in order; must contain `bytes_log` |
//! | [`INPUT_ORDER_KEY`] | the full input vector's names — every source feature exactly once, plus [`ZQ_NORM_INPUT`] exactly once |
//! | [`LAYOUT_KEY`] | *(optional)* `cell_major` (default) or `head_major` |
//!
//! # Cell labels
//!
//! A cell is one encoder configuration the model scores. Its label is
//! the backend, then zero or more `key=value` knobs, comma-separated:
//!
//! ```text
//! svt,chroma=420,speed=6,svttune=3,qm=1,qmmin=2,qmmax=10
//! rav1e,chroma=444,speed=4,tune=still,qm=0
//! aom,chroma=420,speed=6
//! ```
//!
//! Knobs are **backend-scoped** — see [`TuneCell`] for the full map and
//! for why a knob on the wrong backend is refused rather than ignored.
//!
//! The grammar is **closed**: an unknown backend or an unknown knob key
//! is a load error naming it, not a silently-ignored token. That is
//! deliberate — a typo'd knob that parsed as "absent" would train one
//! thing and encode another, and nothing downstream would notice.
//!
//! # Output layout
//!
//! `n_outputs == cells.len() * heads.len()`. With the default
//! `cell_major` layout, output index is `cell * n_heads + head`;
//! `head_major` is `head * n_cells + cell`. The bake declares which; a
//! bake whose output count does not equal the product is refused.


use crate::auto_tune::AutoTuneError;
use crate::{Av1Backend, EncodeBitDepth, EncodeChromaSubsampling};

/// Metadata key: one cell label per output cell.
pub const CELLS_KEY: &str = "zenavif.tune.cells";

/// Metadata key: the per-cell output heads, in order.
pub const HEADS_KEY: &str = "zenavif.tune.heads";

/// Metadata key: the FULL input-vector names in input order — every
/// source feature plus [`ZQ_NORM_INPUT`].
pub const INPUT_ORDER_KEY: &str = "zenavif.tune.input_order";

/// Metadata key (optional): `cell_major` (default) or `head_major`.
pub const LAYOUT_KEY: &str = "zenavif.tune.layout";

/// The one non-image input: the caller's requested quality target
/// divided by 100.
///
/// The encoder's own `q` is **not** an input — `q` is part of the
/// decision the tuner makes, so there is no q-leakage from training into
/// inference. Same convention `zenpicker`'s cell contract uses, so a
/// trainer that emits one can emit the other.
pub const ZQ_NORM_INPUT: &str = "zq_norm";

/// One per-cell output head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TuneHead {
    /// `ln(encoded_bytes)`. **Required** — it is the objective the pick
    /// argmins over.
    BytesLog,
    /// The encoder `quality` value to use for this cell at this target.
    /// Optional; without it the caller's target passes through on the
    /// encoder's generic quality scale.
    Quality,
    /// `ln(encode_wall_ms)`. Optional; without it the time budget masks
    /// on the measured table in [`super::stub`] instead.
    EncodeMsLog,
}

impl TuneHead {
    /// The label this head carries in [`HEADS_KEY`].
    pub const fn label(self) -> &'static str {
        match self {
            Self::BytesLog => "bytes_log",
            Self::Quality => "quality",
            Self::EncodeMsLog => "encode_ms_log",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "bytes_log" => Some(Self::BytesLog),
            "quality" => Some(Self::Quality),
            "encode_ms_log" => Some(Self::EncodeMsLog),
            _ => None,
        }
    }
}

/// Output index layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum TuneLayout {
    /// `out[cell * n_heads + head]` — the default.
    #[default]
    CellMajor,
    /// `out[head * n_cells + cell]`.
    HeadMajor,
}

impl TuneLayout {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "cell_major" => Some(Self::CellMajor),
            "head_major" => Some(Self::HeadMajor),
            _ => None,
        }
    }
}

/// One encoder configuration the model scores — a backend plus knobs.
///
/// Built by [`parse`](Self::parse) from a cell label.
///
/// # Knobs are backend-scoped, and never conflated
///
/// "tune" and "qm" name **different knobs on different backends**, so
/// this grammar spells them differently rather than papering over it:
///
/// | label knob | backend | maps to |
/// |---|---|---|
/// | `tune=still\|psycho` | zenravif | [`EncoderConfig::with_still_image_tuning`](crate::EncoderConfig::with_still_image_tuning) — rav1e's `Tune` enum |
/// | `qm=0\|1` | zenravif | [`EncoderConfig::with_qm`](crate::EncoderConfig::with_qm) — window not exposed |
/// | `svttune=<u8>` | zenav1-svt | `SvtParams::tune` — SVT's numeric tune |
/// | `qm=0\|1`, `qmmin=<u8>`, `qmmax=<u8>` | zenav1-svt | `SvtParams::{enable_qm, min_qm_level, max_qm_level}` |
/// | `scm=<u8>` | zenav1-svt | `SvtParams::force_screen_content_mode` |
/// | `sharp=<i8>` | zenav1-svt | `SvtParams::sharpness` |
///
/// Declaring a knob on a backend that has no such control is a load
/// error naming both — because the alternative (accept and ignore) is
/// exactly the defect the AVIF knob DOE found in the harness it was
/// measuring with: two arms carried distinct fingerprints while emitting
/// byte-identical bitstreams, and 8,972 cells were spent before anyone
/// noticed ([imazen/zenav1-svt#17](https://github.com/imazen/zenav1-svt/issues/17)).
///
/// Every knob is `Option`: absent means "the encoder's own default for
/// this backend and speed", which is a different statement from
/// "explicitly off", and [`config_for_cell`](super::config_for_cell)
/// keeps them distinct.
#[derive(Debug, Clone, PartialEq)]
pub struct TuneCell {
    backend: Av1Backend,
    speed: u8,
    chroma: EncodeChromaSubsampling,
    bit_depth: Option<EncodeBitDepth>,
    enable_qm: Option<bool>,
    tune_still_image: Option<bool>,
    svt_tune: Option<u8>,
    svt_qm_window: Option<(u8, u8)>,
    svt_screen_content_mode: Option<u8>,
    svt_sharpness: Option<i8>,
    label: String,
}

impl TuneCell {
    /// Parse one cell label — `backend[,key=value]*`.
    ///
    /// # Errors
    ///
    /// [`AutoTuneError::LutMalformed`] naming the offending token for an
    /// unknown backend, an unknown knob key, a knob that does not exist
    /// on the declared backend, or an out-of-range value. Nothing is
    /// silently skipped.
    pub fn parse(label: &str) -> Result<Self, AutoTuneError> {
        let mut parts = label.split(',').map(str::trim).filter(|s| !s.is_empty());
        let backend_tok = parts
            .next()
            .ok_or_else(|| AutoTuneError::LutMalformed(format!("{CELLS_KEY}: empty cell label")))?;
        let backend = parse_backend(backend_tok).ok_or_else(|| {
            AutoTuneError::LutMalformed(format!(
                "{CELLS_KEY}: cell {label:?} names unknown backend {backend_tok:?} \
                 (expected one of rav1e|zenravif, svt|zenav1svt, aom|zenav1aom)"
            ))
        })?;

        let mut cell = Self {
            backend,
            // The AV1 still-image default across every seam in this crate.
            speed: 6,
            chroma: EncodeChromaSubsampling::default(),
            bit_depth: None,
            enable_qm: None,
            tune_still_image: None,
            svt_tune: None,
            svt_qm_window: None,
            svt_screen_content_mode: None,
            svt_sharpness: None,
            label: label.trim().to_string(),
        };
        let is_svt = backend == Av1Backend::Zenav1Svt;
        let is_rav1e = backend == Av1Backend::Zenravif;

        for tok in parts {
            let (key, value) = tok.split_once('=').ok_or_else(|| {
                AutoTuneError::LutMalformed(format!(
                    "{CELLS_KEY}: cell {label:?} knob {tok:?} is not key=value"
                ))
            })?;
            let bad = |what: &str| {
                AutoTuneError::LutMalformed(format!(
                    "{CELLS_KEY}: cell {label:?} knob {key}={value:?} — {what}"
                ))
            };
            let wrong_backend = |needs: &str| {
                AutoTuneError::LutMalformed(format!(
                    "{CELLS_KEY}: cell {label:?} sets {key:?}, which only exists on {needs} \
                     (declared backend: {backend:?})"
                ))
            };
            match key {
                "speed" => {
                    cell.speed = value
                        .parse::<u8>()
                        .ok()
                        .filter(|s| *s <= 10)
                        .ok_or_else(|| bad("expected 0..=10"))?;
                }
                "chroma" => {
                    cell.chroma = match value {
                        "420" => EncodeChromaSubsampling::Yuv420,
                        "444" => EncodeChromaSubsampling::Yuv444,
                        _ => return Err(bad("expected 420 or 444")),
                    };
                }
                "depth" => {
                    cell.bit_depth = Some(match value {
                        "8" => EncodeBitDepth::Eight,
                        "10" => EncodeBitDepth::Ten,
                        "12" => EncodeBitDepth::Twelve,
                        _ => return Err(bad("expected 8, 10 or 12")),
                    });
                }
                "qm" => cell.enable_qm = Some(parse_bool(value).ok_or_else(|| bad("expected 0/1"))?),
                "tune" => {
                    if !is_rav1e {
                        return Err(wrong_backend("zenravif (SVT's is `svttune`)"));
                    }
                    cell.tune_still_image = Some(match value {
                        "still" => true,
                        "psycho" => false,
                        _ => return Err(bad("expected still or psycho")),
                    });
                }
                "svttune" => {
                    if !is_svt {
                        return Err(wrong_backend("zenav1-svt (rav1e's is `tune`)"));
                    }
                    cell.svt_tune = Some(value.parse::<u8>().map_err(|_| bad("expected a u8"))?);
                }
                "qmmin" | "qmmax" => {
                    if !is_svt {
                        return Err(wrong_backend(
                            "zenav1-svt (zenravif's `with_qm` exposes no window)",
                        ));
                    }
                    let v = value
                        .parse::<u8>()
                        .ok()
                        .filter(|v| *v <= 15)
                        .ok_or_else(|| bad("expected 0..=15"))?;
                    let (mut lo, mut hi) = cell.svt_qm_window.unwrap_or((8, 15));
                    if key == "qmmin" {
                        lo = v;
                    } else {
                        hi = v;
                    }
                    if lo > hi {
                        return Err(bad("qmmin must be <= qmmax"));
                    }
                    cell.svt_qm_window = Some((lo, hi));
                }
                "scm" => {
                    if !is_svt {
                        return Err(wrong_backend("zenav1-svt"));
                    }
                    cell.svt_screen_content_mode =
                        Some(value.parse::<u8>().map_err(|_| bad("expected a u8"))?);
                }
                "sharp" => {
                    if !is_svt {
                        return Err(wrong_backend("zenav1-svt"));
                    }
                    cell.svt_sharpness =
                        Some(value.parse::<i8>().map_err(|_| bad("expected an i8"))?);
                }
                _ => {
                    return Err(AutoTuneError::LutMalformed(format!(
                        "{CELLS_KEY}: cell {label:?} has unknown knob key {key:?} (known: \
                         speed, chroma, depth, qm, tune, svttune, qmmin, qmmax, scm, sharp)"
                    )));
                }
            }
        }
        Ok(cell)
    }

    /// The AV1 backend this cell encodes with.
    pub fn backend(&self) -> Av1Backend {
        self.backend
    }

    /// The speed preset (0 = slowest / best, 10 = fastest).
    pub fn speed(&self) -> u8 {
        self.speed
    }

    /// Chroma subsampling.
    pub fn chroma(&self) -> EncodeChromaSubsampling {
        self.chroma
    }

    /// Explicit bit depth, or `None` for the encoder's default.
    pub fn bit_depth(&self) -> Option<EncodeBitDepth> {
        self.bit_depth
    }

    /// Quantization matrices on/off, or `None` for the encoder's default.
    pub fn enable_qm(&self) -> Option<bool> {
        self.enable_qm
    }

    /// rav1e `Tune::StillImage` (`true`) vs `Tune::Psychovisual`
    /// (`false`), or `None`. zenravif only.
    pub fn tune_still_image(&self) -> Option<bool> {
        self.tune_still_image
    }

    /// SVT's numeric `tune`, or `None`. zenav1-svt only.
    pub fn svt_tune(&self) -> Option<u8> {
        self.svt_tune
    }

    /// SVT's `(min_qm_level, max_qm_level)` window, or `None`.
    ///
    /// The window is a real part of the measurement: the AVIF knob DOE
    /// found `min=2,max=10` worth **-2.86%** BD-rate at native speed 6
    /// (24 of 30 images) while `min=8,max=15` read **-0.66%** and was
    /// byte-identical to the control on 11.1% of its cells. The axis is
    /// **categorical, not ordinal** — do not interpolate between windows.
    pub fn svt_qm_window(&self) -> Option<(u8, u8)> {
        self.svt_qm_window
    }

    /// SVT's `force_screen_content_mode`, or `None`. zenav1-svt only.
    pub fn svt_screen_content_mode(&self) -> Option<u8> {
        self.svt_screen_content_mode
    }

    /// SVT's `sharpness`, or `None`. zenav1-svt only.
    pub fn svt_sharpness(&self) -> Option<i8> {
        self.svt_sharpness
    }

    /// Whether this cell declares any zenav1-svt-only knob.
    ///
    /// Applying those needs the `__expert` surface
    /// ([`EncoderConfig::with_svt_params`](crate::EncoderConfig)); a
    /// build without it must **refuse** such a cell rather than encode
    /// it with the knobs dropped.
    pub fn declares_svt_knobs(&self) -> bool {
        self.svt_tune.is_some()
            || self.svt_qm_window.is_some()
            || self.svt_screen_content_mode.is_some()
            || self.svt_sharpness.is_some()
            || (self.backend == Av1Backend::Zenav1Svt && self.enable_qm.is_some())
    }

    /// The label verbatim as it was declared.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// True when both cells ask for the same configuration.
    pub(crate) fn same_config(&self, other: &Self) -> bool {
        self.backend == other.backend
            && self.speed == other.speed
            && self.chroma == other.chroma
            && self.bit_depth == other.bit_depth
            && self.enable_qm == other.enable_qm
            && self.tune_still_image == other.tune_still_image
            && self.svt_tune == other.svt_tune
            && self.svt_qm_window == other.svt_qm_window
            && self.svt_screen_content_mode == other.svt_screen_content_mode
            && self.svt_sharpness == other.svt_sharpness
    }
}

fn parse_backend(s: &str) -> Option<Av1Backend> {
    match s {
        "rav1e" | "zenravif" | "zenrav1e" => Some(Av1Backend::Zenravif),
        "svt" | "zenav1svt" | "zenav1-svt" => Some(Av1Backend::Zenav1Svt),
        "aom" | "zenav1aom" | "zenav1-aom" => Some(Av1Backend::Zenav1Aom),
        _ => None,
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "true" | "on" => Some(true),
        "0" | "false" | "off" => Some(false),
        _ => None,
    }
}

fn split_list(raw: &str) -> Vec<&str> {
    raw.split(['\n', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split a list that may itself contain commas inside each item — cell
/// labels are comma-separated internally, so they are newline-separated
/// externally.
fn split_lines(raw: &str) -> Vec<&str> {
    raw.split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

fn read_key<'m>(model: &'m zenpredict::Model, key: &str) -> Result<&'m str, AutoTuneError> {
    model
        .metadata()
        .get_utf8(key)
        .map_err(|e| AutoTuneError::LutMalformed(format!("{key}: {e:?}")))
}

/// The validated contract a backend-tuner bake declares.
#[derive(Debug, Clone)]
pub struct TuneContract {
    cells: Vec<TuneCell>,
    heads: Vec<TuneHead>,
    layout: TuneLayout,
    image_features: Vec<String>,
    input_order: Vec<String>,
    zq_index: usize,
}

impl TuneContract {
    /// Read and validate the contract from a parsed bake.
    ///
    /// Checks, in the order they run — each failure names what disagreed:
    ///
    /// 1. [`CELLS_KEY`] is present, non-empty, every label parses, and no
    ///    two cells describe the same configuration;
    /// 2. [`HEADS_KEY`] is present, every head parses, no head repeats,
    ///    and [`TuneHead::BytesLog`] is among them;
    /// 3. `n_outputs == cells * heads`;
    /// 4. [`LAYOUT_KEY`], when present, parses;
    /// 5. [`INPUT_ORDER_KEY`]'s length equals the model's
    ///    [`caller_input_width`](zenpredict::Model::caller_input_width)
    ///    — which is **not** `n_inputs` on a dead-column-pruned bake;
    /// 6. the input order holds exactly one [`ZQ_NORM_INPUT`] and every
    ///    other name exactly once.
    ///
    /// # Errors
    ///
    /// [`AutoTuneError::LutMalformed`] for any of the above.
    pub fn from_model(model: &zenpredict::Model) -> Result<Self, AutoTuneError> {
        let mut cells = Vec::new();
        for label in split_lines(read_key(model, CELLS_KEY)?) {
            let cell = TuneCell::parse(label)?;
            if cells.iter().any(|c: &TuneCell| c.same_config(&cell)) {
                return Err(AutoTuneError::LutMalformed(format!(
                    "{CELLS_KEY}: cell {label:?} repeats an earlier configuration"
                )));
            }
            cells.push(cell);
        }
        if cells.is_empty() {
            return Err(AutoTuneError::LutMalformed(format!("{CELLS_KEY}: empty")));
        }

        let mut heads = Vec::new();
        for label in split_list(read_key(model, HEADS_KEY)?) {
            let head = TuneHead::parse(label).ok_or_else(|| {
                AutoTuneError::LutMalformed(format!(
                    "{HEADS_KEY}: unknown head {label:?} \
                     (known: bytes_log, quality, encode_ms_log)"
                ))
            })?;
            if heads.contains(&head) {
                return Err(AutoTuneError::LutMalformed(format!(
                    "{HEADS_KEY}: head {label:?} appears more than once"
                )));
            }
            heads.push(head);
        }
        if !heads.contains(&TuneHead::BytesLog) {
            return Err(AutoTuneError::LutMalformed(format!(
                "{HEADS_KEY}: no {} head — it is the objective the pick argmins over",
                TuneHead::BytesLog.label()
            )));
        }

        let expected_outputs = cells.len() * heads.len();
        if model.n_outputs() != expected_outputs {
            return Err(AutoTuneError::LutMalformed(format!(
                "{} cells x {} heads = {expected_outputs} outputs, but the model scores {}",
                cells.len(),
                heads.len(),
                model.n_outputs()
            )));
        }

        let layout = match model.metadata().get_utf8(LAYOUT_KEY) {
            Ok(raw) => TuneLayout::parse(raw.trim()).ok_or_else(|| {
                AutoTuneError::LutMalformed(format!(
                    "{LAYOUT_KEY}: unknown layout {:?} (known: cell_major, head_major)",
                    raw.trim()
                ))
            })?,
            Err(_) => TuneLayout::default(),
        };

        let input_order: Vec<String> = split_list(read_key(model, INPUT_ORDER_KEY)?)
            .into_iter()
            .map(str::to_string)
            .collect();
        let width = model.caller_input_width();
        if input_order.len() != width {
            return Err(AutoTuneError::LutMalformed(format!(
                "{INPUT_ORDER_KEY}: {} names but the model takes {width} inputs",
                input_order.len()
            )));
        }

        let mut zq_index = None;
        let mut image_features = Vec::with_capacity(input_order.len().saturating_sub(1));
        for (i, name) in input_order.iter().enumerate() {
            if name == ZQ_NORM_INPUT {
                if zq_index.replace(i).is_some() {
                    return Err(AutoTuneError::LutMalformed(format!(
                        "{INPUT_ORDER_KEY}: {ZQ_NORM_INPUT} appears more than once"
                    )));
                }
                continue;
            }
            if image_features.contains(name) {
                return Err(AutoTuneError::LutMalformed(format!(
                    "{INPUT_ORDER_KEY}: {name:?} appears more than once"
                )));
            }
            image_features.push(name.clone());
        }
        let zq_index = zq_index.ok_or_else(|| {
            AutoTuneError::LutMalformed(format!(
                "{INPUT_ORDER_KEY}: no {ZQ_NORM_INPUT} input"
            ))
        })?;

        Ok(Self {
            cells,
            heads,
            layout,
            image_features,
            input_order,
            zq_index,
        })
    }

    /// The cells, in declared order.
    pub fn cells(&self) -> &[TuneCell] {
        &self.cells
    }

    /// The per-cell heads, in declared order.
    pub fn heads(&self) -> &[TuneHead] {
        &self.heads
    }

    /// The output index layout.
    pub fn layout(&self) -> TuneLayout {
        self.layout
    }

    /// The source-feature names the bake consumes, in input order and
    /// **without** [`ZQ_NORM_INPUT`]. This is the feature contract: a
    /// caller must be able to supply exactly these.
    pub fn image_features(&self) -> &[String] {
        &self.image_features
    }

    /// Every input's name in input order, including [`ZQ_NORM_INPUT`].
    pub fn input_order(&self) -> &[String] {
        &self.input_order
    }

    /// Whether the bake declares `head`.
    pub fn has_head(&self, head: TuneHead) -> bool {
        self.heads.contains(&head)
    }

    /// Read one head's output for one cell, or `None` when the bake does
    /// not declare that head.
    pub fn head_value(&self, outputs: &[f32], cell: usize, head: TuneHead) -> Option<f32> {
        let h = self.heads.iter().position(|x| *x == head)?;
        let idx = match self.layout {
            TuneLayout::CellMajor => cell * self.heads.len() + h,
            TuneLayout::HeadMajor => h * self.cells.len() + cell,
        };
        outputs.get(idx).copied()
    }

    /// **The contract mapping.** Materialize the model's input vector
    /// from resolved source-feature values plus a normalized target.
    ///
    /// `feature_values` must be one value per
    /// [`image_features`](Self::image_features) entry, in that order —
    /// which is exactly what a resolver hands back. A length mismatch is
    /// an error, never a silent prefix read.
    ///
    /// # Errors
    ///
    /// [`AutoTuneError::LutMalformed`] on a length mismatch.
    pub fn build_input(
        &self,
        zq_norm: f32,
        feature_values: &[f32],
    ) -> Result<Vec<f32>, AutoTuneError> {
        if feature_values.len() != self.image_features.len() {
            return Err(AutoTuneError::LutMalformed(format!(
                "build_input: {} values for {} source features",
                feature_values.len(),
                self.image_features.len()
            )));
        }
        let mut out = vec![0.0_f32; self.input_order.len()];
        let mut next = 0usize;
        for (i, slot) in out.iter_mut().enumerate() {
            if i == self.zq_index {
                *slot = zq_norm;
            } else {
                *slot = feature_values[next];
                next += 1;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_label_parses_backend_and_svt_knobs() {
        let c = TuneCell::parse("svt,chroma=420,speed=6,svttune=3,qm=1,qmmin=2,qmmax=10")
            .expect("parses");
        assert_eq!(c.backend(), Av1Backend::Zenav1Svt);
        assert_eq!(c.chroma(), EncodeChromaSubsampling::Yuv420);
        assert_eq!(c.speed(), 6);
        assert_eq!(c.svt_tune(), Some(3));
        assert_eq!(c.enable_qm(), Some(true));
        assert_eq!(c.svt_qm_window(), Some((2, 10)));
        assert!(c.declares_svt_knobs());
    }

    #[test]
    fn rav1e_cell_parses_its_own_tune_spelling() {
        let c = TuneCell::parse("rav1e,chroma=444,speed=4,tune=still,qm=0").expect("parses");
        assert_eq!(c.backend(), Av1Backend::Zenravif);
        assert_eq!(c.tune_still_image(), Some(true));
        assert_eq!(c.enable_qm(), Some(false));
        assert!(
            !c.declares_svt_knobs(),
            "a zenravif cell must not need the __expert svt surface"
        );
    }

    #[test]
    fn a_knob_on_the_wrong_backend_is_refused_not_ignored() {
        // The zenav1-svt#17 defect class: a knob that reaches the
        // fingerprint but not the bitstream. Refusing is the whole point.
        let err = TuneCell::parse("rav1e,svttune=3").expect_err("svttune is svt-only");
        assert!(format!("{err}").contains("svttune"));
        let err = TuneCell::parse("svt,tune=still").expect_err("tune= is rav1e-only");
        let msg = format!("{err}");
        assert!(msg.contains("svttune"), "the error must point at the right spelling: {msg}");
        assert!(TuneCell::parse("rav1e,qmmin=2").is_err(), "no window on rav1e");
        assert!(TuneCell::parse("aom,scm=3").is_err(), "scm is svt-only");
    }

    #[test]
    fn absent_knob_is_not_the_same_as_off() {
        let bare = TuneCell::parse("rav1e").expect("parses");
        assert_eq!(bare.enable_qm(), None, "absent = encoder default");
        let off = TuneCell::parse("rav1e,qm=0").expect("parses");
        assert_eq!(off.enable_qm(), Some(false), "explicit off");
    }

    #[test]
    fn unknown_knob_key_is_refused_not_ignored() {
        let err = TuneCell::parse("svt,chroma=420,tuen=still").expect_err("typo must fail");
        let msg = format!("{err}");
        assert!(
            msg.contains("tuen"),
            "the error must name the offending key, got: {msg}"
        );
    }

    #[test]
    fn unknown_backend_is_refused() {
        let err = TuneCell::parse("x265,speed=6").expect_err("unknown backend must fail");
        assert!(format!("{err}").contains("x265"));
    }

    #[test]
    fn out_of_range_values_are_refused() {
        assert!(TuneCell::parse("svt,speed=11").is_err());
        assert!(TuneCell::parse("svt,chroma=422").is_err(), "no 4:2:2 seam");
        assert!(TuneCell::parse("svt,depth=16").is_err());
        assert!(TuneCell::parse("svt,qmmin=16").is_err());
        assert!(
            TuneCell::parse("svt,qmmin=10,qmmax=2").is_err(),
            "an inverted window must not parse"
        );
    }

    #[test]
    fn head_labels_round_trip() {
        for h in [TuneHead::BytesLog, TuneHead::Quality, TuneHead::EncodeMsLog] {
            assert_eq!(TuneHead::parse(h.label()), Some(h));
        }
    }

    #[test]
    fn same_config_ignores_the_label_spelling() {
        // `rav1e` and `zenravif` are the same backend; the dedup check
        // must catch a cell respelled rather than re-specified.
        let a = TuneCell::parse("rav1e,speed=6").expect("parses");
        let b = TuneCell::parse("zenravif,speed=6").expect("parses");
        assert_ne!(a.label(), b.label());
        assert!(a.same_config(&b));
    }
}
