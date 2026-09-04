//! **Backend + knob tuner** — the production one-shot that answers
//! "which AV1 encoder, and with which knobs, for this image at this
//! quality target inside this time budget?".
//!
//! This is the [`crate::auto_tune`] family's backend-choosing sibling.
//! Where [`EncoderConfig::auto_tune`](crate::EncoderConfig) picks a
//! zenrav1e *speed preset* from a bundled bake, this module picks a
//! [`Av1Backend`] **and** its knobs from a bake the **caller supplies**,
//! and reports what it expects that choice to cost in bytes and wall
//! time.
//!
//! # Two implementations, one trait
//!
//! [`AvifTuning`] has exactly two implementors, and which one you hold is
//! always visible on the result via [`AvifTune::source`]:
//!
//! | type | where the decision comes from |
//! |---|---|
//! | [`AvifTuner`] | a ZNPR v3 bake the caller loaded — [`AvifTuner::from_bytes`] |
//! | [`StubTuner`] | a fixed table of **measured** defaults, no model |
//!
//! [`StubTuner`] exists so a consumer can integrate the production path
//! *before* the trained bake lands, then swap in [`AvifTuner`] with one
//! line. It is not a guess: every constant in it cites the campaign
//! measurement it came from (see [`stub`]).
//!
//! # No bundled weights
//!
//! **Nothing in this module `include_bytes!`s a model.** The bake is
//! always the caller's — bytes in, tuner out. That is the standing rule
//! (a published codec crate does not ship picker weights by default);
//! bundling would be a separate, user-gated proposal.
//!
//! # Feature negotiation
//!
//! [`AvifTuner::tune`] takes the same optional
//! [`zenanalyze_api::Offer`] every other head in this crate takes, and
//! resolves its features the same way: reuse the caller's shared
//! analysis pass when the offer covers the bake's declared columns,
//! otherwise run zenanalyze itself over exactly those columns. One
//! upstream pass can therefore serve the tuner, the palette gate and
//! the fast-tier heads together.

use crate::auto_tune::{AutoTuneError, QualityTarget};
use crate::{Av1Backend, EncoderConfig};

pub mod contract;
pub mod stub;

pub use contract::{TuneCell, TuneContract, TuneHead};
pub use stub::StubTuner;

/// Which AV1 backends the caller is willing to receive.
///
/// A tuner never returns a backend outside this mask, and never returns
/// one the build cannot actually encode with: [`Self::built`] is the
/// cargo-feature-derived set, and every tuner intersects with it. An
/// empty intersection is [`AutoTuneError::NoCellAllowed`], not a silent
/// fallback to the default backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllowedBackends {
    zenravif: bool,
    zenav1_svt: bool,
    zenav1_aom: bool,
}

impl AllowedBackends {
    /// Every backend this **build** can encode with, derived from the
    /// cargo features that gate each seam.
    ///
    /// `Zenravif` is present whenever `encode` is on; `Zenav1Svt` needs
    /// `zenav1-svt`; `Zenav1Aom` needs `zenav1-aom-encode`. The retired
    /// [`Av1Backend::Svtav1`] is never included — no build can encode
    /// with it.
    pub const fn built() -> Self {
        Self {
            zenravif: cfg!(feature = "encode"),
            zenav1_svt: cfg!(feature = "zenav1-svt"),
            zenav1_aom: cfg!(feature = "zenav1-aom-encode"),
        }
    }

    /// No backend allowed — build up with [`Self::with`].
    pub const fn none() -> Self {
        Self {
            zenravif: false,
            zenav1_svt: false,
            zenav1_aom: false,
        }
    }

    /// Add one backend to the mask. An unrecognized (future) variant is
    /// ignored, so this stays additive as [`Av1Backend`] grows.
    #[must_use]
    pub fn with(mut self, backend: Av1Backend) -> Self {
        match backend {
            Av1Backend::Zenravif => self.zenravif = true,
            Av1Backend::Zenav1Svt => self.zenav1_svt = true,
            Av1Backend::Zenav1Aom => self.zenav1_aom = true,
            _ => {}
        }
        self
    }

    /// Whether `backend` is in the mask.
    pub fn contains(&self, backend: Av1Backend) -> bool {
        match backend {
            Av1Backend::Zenravif => self.zenravif,
            Av1Backend::Zenav1Svt => self.zenav1_svt,
            Av1Backend::Zenav1Aom => self.zenav1_aom,
            _ => false,
        }
    }

    /// Intersection — the backends allowed by **both** masks.
    #[must_use]
    pub fn intersect(self, other: Self) -> Self {
        Self {
            zenravif: self.zenravif && other.zenravif,
            zenav1_svt: self.zenav1_svt && other.zenav1_svt,
            zenav1_aom: self.zenav1_aom && other.zenav1_aom,
        }
    }

    /// Whether the mask permits nothing.
    pub fn is_empty(&self) -> bool {
        !(self.zenravif || self.zenav1_svt || self.zenav1_aom)
    }
}

impl Default for AllowedBackends {
    /// [`Self::built`] — every backend this build can actually encode with.
    fn default() -> Self {
        Self::built()
    }
}

/// What the caller wants, and what it will accept.
///
/// `width`/`height` are load-bearing, not decoration: the wall-time
/// estimate is `alpha + beta * megapixels` and the campaign measured
/// that dropping `alpha` (quoting a bare ms/MP) misprices small images
/// by up to 20x. There is no size-free form of this request.
#[derive(Debug, Clone)]
pub struct TuneRequest {
    /// The quality to hit, in the bake's metric space.
    pub target: QualityTarget,
    /// Reject any cell whose predicted wall time exceeds this many
    /// milliseconds. `None` = no time constraint.
    ///
    /// The estimate this is compared against is a **median** over the
    /// campaign's content classes; per-source wall time is
    /// feature-conditioned and varies severalfold within one class, so
    /// treat a budget as a soft preference, not a deadline guarantee.
    pub time_budget_ms: Option<f32>,
    /// Backends the caller will accept. Intersected with
    /// [`AllowedBackends::built`] before any pick.
    pub allowed_backends: AllowedBackends,
    /// Source width in pixels.
    pub width: u32,
    /// Source height in pixels.
    pub height: u32,
    /// Whether the source carries an alpha channel. Backends that cannot
    /// encode alpha are masked out when this is `true`.
    pub has_alpha: bool,
}

impl TuneRequest {
    /// A request for `target` at `width`x`height`, no time budget, every
    /// built backend allowed, no alpha.
    pub fn new(target: QualityTarget, width: u32, height: u32) -> Self {
        Self {
            target,
            time_budget_ms: None,
            allowed_backends: AllowedBackends::built(),
            width,
            height,
            has_alpha: false,
        }
    }

    /// Set the wall-time budget in milliseconds.
    #[must_use]
    pub fn with_time_budget_ms(mut self, ms: f32) -> Self {
        self.time_budget_ms = Some(ms);
        self
    }

    /// Restrict which backends may be returned.
    #[must_use]
    pub fn with_allowed_backends(mut self, allowed: AllowedBackends) -> Self {
        self.allowed_backends = allowed;
        self
    }

    /// Declare that the source has alpha.
    #[must_use]
    pub fn with_alpha(mut self, has_alpha: bool) -> Self {
        self.has_alpha = has_alpha;
        self
    }

    /// Megapixels, for the wall-time estimate.
    pub(crate) fn megapixels(&self) -> f32 {
        (u64::from(self.width) * u64::from(self.height)) as f32 / 1_000_000.0
    }

    /// The quality target as a plain number.
    pub(crate) fn target_value(&self) -> f32 {
        let QualityTarget::Zensim(v) = self.target;
        v
    }
}

/// Where a [`AvifTune`] came from — always reported, never inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TuneSource {
    /// A ZNPR bake the caller supplied.
    Model,
    /// [`StubTuner`]'s measured default table — no model was consulted.
    Stub,
}

/// The tuner's answer.
///
/// [`Self::config`] is ready to encode with: the backend, quality,
/// speed, chroma and the knobs the chosen cell declares are already
/// applied to it. The caller may of course keep adjusting it (metadata,
/// ICC, threads) — those axes are not the tuner's business.
#[derive(Debug, Clone)]
pub struct AvifTune {
    backend: Av1Backend,
    config: EncoderConfig,
    cell_label: String,
    expected_bytes: Option<f32>,
    expected_wall_ms: Option<f32>,
    margin: Option<f32>,
    source: TuneSource,
}

impl AvifTune {
    /// The chosen AV1 backend. Also set on [`Self::config`]; exposed
    /// separately so a caller can log or route on it without
    /// destructuring the config.
    pub fn backend(&self) -> Av1Backend {
        self.backend
    }

    /// The encoder config to use, knobs applied.
    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }

    /// Take the config by value.
    pub fn into_config(self) -> EncoderConfig {
        self.config
    }

    /// The cell label the decision resolved to, verbatim as the bake (or
    /// the stub table) declared it — e.g. `svt,chroma=420,speed=6,tune=still,qm=1`.
    pub fn cell_label(&self) -> &str {
        &self.cell_label
    }

    /// Predicted encoded size in bytes, when the bake carries a
    /// [`TuneHead::BytesLog`] head. `None` from [`StubTuner`], which
    /// predicts nothing.
    pub fn expected_bytes(&self) -> Option<f32> {
        self.expected_bytes
    }

    /// Predicted wall time in milliseconds for this cell at this size.
    ///
    /// From the bake's [`TuneHead::EncodeMsLog`] head when it has one,
    /// otherwise from the measured `alpha + beta * MP` table in
    /// [`stub`]. `None` only when neither is available.
    pub fn expected_wall_ms(&self) -> Option<f32> {
        self.expected_wall_ms
    }

    /// How much better the winning cell scored than the runner-up, in
    /// the model's own score units (larger = a more confident pick).
    /// `None` when only one cell survived the masks, or from
    /// [`StubTuner`].
    pub fn margin(&self) -> Option<f32> {
        self.margin
    }

    /// Whether this came from a model or the stub table.
    pub fn source(&self) -> TuneSource {
        self.source
    }
}

/// The one-shot tuning entry point, implemented by [`AvifTuner`] (model)
/// and [`StubTuner`] (measured defaults).
///
/// Taking this as `&dyn AvifTuning` is the intended integration shape: a
/// consumer wires the trait once and swaps the implementor when the bake
/// lands.
pub trait AvifTuning {
    /// Choose a backend + knobs for this image and request.
    ///
    /// `rgb` is packed RGB8 (`width * height * 3` bytes) — the same
    /// input [`crate::encode_rgb8`] takes. `offer`, when supplied, is a
    /// shared zenanalyze pass the tuner reuses instead of running its
    /// own; `None` means "analyze it yourself".
    ///
    /// # Errors
    ///
    /// [`AutoTuneError::NoCellAllowed`] when the masks (backend,
    /// alpha, time budget) eliminate every cell;
    /// [`AutoTuneError::FeatureExtraction`] /
    /// [`AutoTuneError::Inference`] when analysis or the forward pass
    /// fails.
    fn tune(
        &self,
        rgb: &[u8],
        offer: Option<&zenanalyze_api::Offer<'_>>,
        request: &TuneRequest,
    ) -> Result<AvifTune, AutoTuneError>;
}

/// Apply a resolved cell to a fresh [`EncoderConfig`].
///
/// One place builds the config, so the model path and the stub path can
/// never drift in what a cell *means*.
///
/// # It refuses rather than silently dropping a knob
///
/// The zenav1-svt knobs (`svttune`, the QM window, `scm`, `sharp`) are
/// reachable only through
/// [`EncoderConfig::with_svt_params`](crate::EncoderConfig), which is
/// gated on the unstable `__expert` feature. On a build without it, a
/// cell that declares those knobs is an
/// [`AutoTuneError::LutMalformed`] naming the cell — **not** an encode
/// with the knobs quietly dropped.
///
/// That is a direct lesson from the wave this tuner is built on: two
/// AVIF DOE arms carried distinct configuration fingerprints while
/// emitting byte-identical bitstreams, because the knob reached the
/// harness but not the encoder. It cost 8,972 cells before anyone
/// noticed. A tuner that dropped knobs the same way would report a
/// backend+knob decision it did not actually make.
pub(crate) fn config_for_cell(
    cell: &TuneCell,
    quality: f32,
) -> Result<EncoderConfig, AutoTuneError> {
    let mut config = EncoderConfig::new()
        .backend(cell.backend())
        .quality(quality)
        .speed(cell.speed())
        .chroma_subsampling(cell.chroma());
    if let Some(depth) = cell.bit_depth() {
        config = config.bit_depth(depth);
    }

    // zenravif's own knobs (rav1e's `Tune`, its window-less QM switch).
    #[cfg(feature = "encode-imazen")]
    if cell.backend() == Av1Backend::Zenravif {
        if let Some(qm) = cell.enable_qm() {
            config = config.with_qm(qm);
        }
        if let Some(still) = cell.tune_still_image() {
            config = config.with_still_image_tuning(still);
        }
    }

    if cell.declares_svt_knobs() {
        #[cfg(feature = "__expert")]
        {
            let mut svt = crate::expert::SvtParams::default();
            if let Some(t) = cell.svt_tune() {
                svt.tune = t;
            }
            if let Some(on) = cell.enable_qm() {
                svt.enable_qm = on;
            }
            if let Some((lo, hi)) = cell.svt_qm_window() {
                svt.min_qm_level = lo;
                svt.max_qm_level = hi;
            }
            if let Some(scm) = cell.svt_screen_content_mode() {
                svt.force_screen_content_mode = Some(scm);
            }
            if let Some(s) = cell.svt_sharpness() {
                svt.sharpness = s;
            }
            config = config.with_svt_params(svt);
        }
        #[cfg(not(feature = "__expert"))]
        return Err(AutoTuneError::LutMalformed(format!(
            "cell {:?} declares zenav1-svt knobs, which this build cannot apply \
             (they need the `__expert` feature). Refusing rather than encoding \
             with the knobs dropped.",
            cell.label()
        )));
    }
    Ok(config)
}

/// Whether a cell can serve this request at all, before any scoring.
///
/// Masks, in order: the caller's backend mask (already intersected with
/// the build's), then alpha support. A cell that fails either is not a
/// candidate — it is never merely penalized.
pub(crate) fn cell_is_viable(cell: &TuneCell, allowed: AllowedBackends, has_alpha: bool) -> bool {
    if !allowed.contains(cell.backend()) {
        return false;
    }
    // zenav1-aom's still seam refuses alpha by name (src/encoder_aom.rs);
    // asking for it would be a runtime error, so mask it here instead.
    if has_alpha && cell.backend() == Av1Backend::Zenav1Aom {
        return false;
    }
    true
}

/// A bake-driven backend + knob tuner.
///
/// Construct with [`from_bytes`](Self::from_bytes) over ZNPR v3 bytes the
/// **caller** owns. Construction validates the bake's tune contract
/// (see [`TuneContract`]) and refuses anything that does not declare one
/// — a family-score bake, a speed-preset bake, or a bake whose declared
/// cells disagree with its real output width can never be read as
/// backend cells.
pub struct AvifTuner {
    model: zenpredict::Model,
    contract: TuneContract,
}

impl AvifTuner {
    /// Parse a ZNPR v3 bake and validate its tune contract.
    ///
    /// # Errors
    ///
    /// [`AutoTuneError::Inference`] when the bytes are not a parseable
    /// ZNPR model; [`AutoTuneError::LutMalformed`] naming exactly what
    /// disagreed when the contract is absent or ill-formed.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AutoTuneError> {
        let model = zenpredict::Model::from_bytes(bytes)
            .map_err(|e| AutoTuneError::Inference(format!("Model::from_bytes: {e}")))?;
        let contract = TuneContract::from_model(&model)?;
        Ok(Self { model, contract })
    }

    /// Parse a bake, checking its `schema_hash` against `expected`
    /// **before** any section parsing. Use this when the consumer
    /// compiles in the hash of the bake it was built against.
    ///
    /// # Errors
    ///
    /// As [`from_bytes`](Self::from_bytes), plus a schema mismatch.
    pub fn from_bytes_with_schema(bytes: &[u8], expected: u64) -> Result<Self, AutoTuneError> {
        let model = zenpredict::Model::from_bytes_with_schema(bytes, expected)
            .map_err(|e| AutoTuneError::Inference(format!("Model::from_bytes_with_schema: {e}")))?;
        let contract = TuneContract::from_model(&model)?;
        Ok(Self { model, contract })
    }

    /// The validated contract — cells, heads, and the input order the
    /// bake declares.
    pub fn contract(&self) -> &TuneContract {
        &self.contract
    }

    /// The parsed model, for metadata reads and diagnostics.
    pub fn model(&self) -> &zenpredict::Model {
        &self.model
    }

    /// The full input width a caller must supply — the bake's
    /// [`caller_input_width`](zenpredict::Model::caller_input_width),
    /// which is **not** `n_inputs` on a dead-column-pruned bake.
    pub fn caller_input_width(&self) -> usize {
        self.model.caller_input_width()
    }
}

impl AvifTuning for AvifTuner {
    fn tune(
        &self,
        rgb: &[u8],
        offer: Option<&zenanalyze_api::Offer<'_>>,
        request: &TuneRequest,
    ) -> Result<AvifTune, AutoTuneError> {
        let allowed = request.allowed_backends.intersect(AllowedBackends::built());
        if allowed.is_empty() {
            return Err(AutoTuneError::NoCellAllowed);
        }

        let zq_norm = request.target_value() / 100.0;
        let resolver = features::Resolver::new(rgb, request.width, request.height, offer);
        let values = resolver.resolve(self.contract.image_features())?;
        let input = self.contract.build_input(zq_norm, &values)?;

        let mut predictor = zenpredict::Predictor::new(&self.model);
        let scores: Vec<f32> = if self.model.has_nontrivial_feature_transforms() {
            predictor.predict_transformed(&input)
        } else {
            predictor.predict(&input)
        }
        .map_err(|e| AutoTuneError::Inference(format!("{e}")))?
        .to_vec();

        let n_cells = self.contract.cells().len();
        let mpx = request.megapixels();

        // Score each viable cell. bytes_log is the objective (smaller is
        // better); the time budget is a hard mask, never a penalty term.
        let mut best: Option<(usize, f32)> = None;
        let mut runner_up: Option<f32> = None;
        let mut per_cell_ms = vec![None; n_cells];

        for (idx, cell) in self.contract.cells().iter().enumerate() {
            if !cell_is_viable(cell, allowed, request.has_alpha) {
                continue;
            }
            // Wall time: the bake's own head when it has one, else the
            // measured table. Either way the budget masks on it.
            let ms = self
                .contract
                .head_value(&scores, idx, TuneHead::EncodeMsLog)
                .map(|log_ms| log_ms.exp())
                .or_else(|| stub::estimated_wall_ms(cell, mpx));
            per_cell_ms[idx] = ms;
            if let (Some(budget), Some(est)) = (request.time_budget_ms, ms)
                && est > budget
            {
                continue;
            }
            let Some(bytes_log) = self.contract.head_value(&scores, idx, TuneHead::BytesLog) else {
                return Err(AutoTuneError::LutMalformed(format!(
                    "cell {idx} has no {} output",
                    TuneHead::BytesLog.label()
                )));
            };
            match best {
                Some((_, best_score)) if bytes_log >= best_score => {
                    if runner_up.is_none_or(|r| bytes_log < r) {
                        runner_up = Some(bytes_log);
                    }
                }
                Some((_, best_score)) => {
                    runner_up = Some(best_score);
                    best = Some((idx, bytes_log));
                }
                None => best = Some((idx, bytes_log)),
            }
        }

        let (idx, best_score) = best.ok_or(AutoTuneError::NoCellAllowed)?;
        let cell = &self.contract.cells()[idx];

        // Quality: the bake's own head when declared, else the caller's
        // target passed through on the encoder's generic scale. Which one
        // happened is visible in the contract, not guessed at here.
        let quality = self
            .contract
            .head_value(&scores, idx, TuneHead::Quality)
            .unwrap_or_else(|| request.target_value());

        Ok(AvifTune {
            backend: cell.backend(),
            config: config_for_cell(cell, quality)?,
            cell_label: cell.label().to_owned(),
            expected_bytes: self
                .contract
                .head_value(&scores, idx, TuneHead::BytesLog)
                .map(f32::exp),
            expected_wall_ms: per_cell_ms[idx],
            margin: runner_up.map(|r| r - best_score),
            source: TuneSource::Model,
        })
    }
}

/// Feature resolution shared by the model path — reuse a caller's
/// [`zenanalyze_api::Offer`] when it covers the bake's declared columns,
/// otherwise run zenanalyze over exactly those columns.
///
/// Same negotiation the palette gate and the fast-tier heads use, so one
/// upstream analysis pass serves all of them.
pub(crate) mod features {
    use super::*;

    /// Lazily resolves named feature columns for one image.
    pub(crate) struct Resolver<'a> {
        rgb: &'a [u8],
        width: u32,
        height: u32,
        offer: Option<&'a zenanalyze_api::Offer<'a>>,
    }

    impl<'a> Resolver<'a> {
        pub(crate) fn new(
            rgb: &'a [u8],
            width: u32,
            height: u32,
            offer: Option<&'a zenanalyze_api::Offer<'a>>,
        ) -> Self {
            Self {
                rgb,
                width,
                height,
                offer,
            }
        }

        /// Resolve every column at once, preferring the shared offer.
        ///
        /// Returns one value per requested column, in order. A column
        /// this build has no feature for is an error naming it — never a
        /// silent zero, which would score a wrong-shaped vector.
        pub(crate) fn resolve(&self, columns: &[String]) -> Result<Vec<f32>, AutoTuneError> {
            if let Some(values) = self.reuse_from_offer(columns) {
                return Ok(values);
            }
            self.own_pass(columns)
        }

        /// Try the caller's shared pass. `None` means "offer absent or
        /// does not cover these columns" — the caller then runs its own.
        fn reuse_from_offer(&self, columns: &[String]) -> Option<Vec<f32>> {
            let offer = self.offer?;
            let qualified: Vec<Option<String>> = columns
                .iter()
                .map(|c| {
                    let name = c.strip_prefix("feat_").unwrap_or(c);
                    let full = zenanalyze::versioning::feature_version_hash_by_name(name)?;
                    Some(zenanalyze_api::NamedFeature::qualified_for(
                        name,
                        zenanalyze_api::NamedFeature::fold_hash(full),
                    ))
                })
                .collect();
            // Any column this build cannot qualify means the offer cannot
            // be trusted to cover the request — fall back to our own pass
            // rather than substituting zeros for it.
            if qualified.iter().any(Option::is_none) {
                return None;
            }
            let wants: Vec<zenanalyze_api::NamedFeature<'_>> = qualified
                .iter()
                .flatten()
                .map(|q| zenanalyze_api::NamedFeature::from_qualified(q))
                .collect();
            let offered = offer.reuse_for(&zenanalyze_api::Request::new(
                zenanalyze_api::Select::Features(&wants),
            ))?;
            if offered.len() != columns.len() {
                return None;
            }
            Some(offered)
        }

        /// Run zenanalyze over exactly the requested columns.
        fn own_pass(&self, columns: &[String]) -> Result<Vec<f32>, AutoTuneError> {
            let supported = zenanalyze::feature::FeatureSet::SUPPORTED;
            let lookup = |col: &str| -> Option<zenanalyze::feature::AnalysisFeature> {
                let target = col.strip_prefix("feat_").unwrap_or(col);
                supported.iter().find(|f| f.name() == target)
            };
            let mut feature_set = zenanalyze::feature::FeatureSet::new();
            for c in columns {
                let f = lookup(c).ok_or_else(|| {
                    AutoTuneError::FeatureExtraction(format!(
                        "this zenanalyze build has no feature {c:?}"
                    ))
                })?;
                feature_set = feature_set.with(f);
            }
            let query = zenanalyze::feature::AnalysisQuery::new(feature_set);
            let analysis =
                zenanalyze::analyze_features_rgb8(self.rgb, self.width, self.height, &query);
            columns
                .iter()
                .map(|c| {
                    lookup(c).and_then(|f| analysis.get_f32(f)).ok_or_else(|| {
                        AutoTuneError::FeatureExtraction(format!("feature {c:?} not produced"))
                    })
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_backends_built_matches_cargo_features() {
        let built = AllowedBackends::built();
        assert_eq!(
            built.contains(Av1Backend::Zenravif),
            cfg!(feature = "encode")
        );
        assert_eq!(
            built.contains(Av1Backend::Zenav1Svt),
            cfg!(feature = "zenav1-svt")
        );
        assert_eq!(
            built.contains(Av1Backend::Zenav1Aom),
            cfg!(feature = "zenav1-aom-encode")
        );
    }

    #[test]
    fn allowed_backends_never_admits_the_retired_svtav1_variant() {
        // `Av1Backend::Svtav1` is deprecated and no build can encode with
        // it; `with` must not be able to add it to a mask.
        #[allow(deprecated)]
        let mask = AllowedBackends::none().with(Av1Backend::Svtav1);
        assert!(mask.is_empty());
        #[allow(deprecated)]
        let contained = AllowedBackends::built().contains(Av1Backend::Svtav1);
        assert!(!contained);
    }

    #[test]
    fn intersect_is_the_and_of_both_masks() {
        let a = AllowedBackends::none()
            .with(Av1Backend::Zenravif)
            .with(Av1Backend::Zenav1Svt);
        let b = AllowedBackends::none()
            .with(Av1Backend::Zenav1Svt)
            .with(Av1Backend::Zenav1Aom);
        let i = a.intersect(b);
        assert!(!i.contains(Av1Backend::Zenravif));
        assert!(i.contains(Av1Backend::Zenav1Svt));
        assert!(!i.contains(Av1Backend::Zenav1Aom));
    }

    #[test]
    fn alpha_masks_out_the_aom_still_seam() {
        let cell = TuneCell::parse("aom,chroma=420,speed=6").expect("parses");
        let all = AllowedBackends::none().with(Av1Backend::Zenav1Aom);
        assert!(cell_is_viable(&cell, all, false));
        assert!(
            !cell_is_viable(&cell, all, true),
            "zenav1-aom's still seam refuses alpha by name; the tuner must \
             mask it rather than hand back a config that errors at encode"
        );
    }

    #[test]
    fn megapixels_and_target_are_read_off_the_request() {
        let r = TuneRequest::new(QualityTarget::Zensim(82.0), 2000, 1000);
        assert!((r.megapixels() - 2.0).abs() < 1e-6);
        assert!((r.target_value() - 82.0).abs() < 1e-6);
    }
}
