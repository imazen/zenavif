//! SVT-AV1 still-image encoder knobs for [`crate::Av1Backend::SvtRs`].
//!
//! This module is **private and always compiled**. [`SvtParams`] is reachable
//! from outside the crate only as `zenavif::expert::SvtParams`, which
//! `src/expert.rs` re-exports behind the `__expert` cargo feature — the type
//! is unstable expert surface exactly like the rest of that module.
//!
//! It lives here rather than in `expert.rs` because the svt-rs encode seam
//! (`src/encoder_svt_rs.rs`) needs the type on the ordinary `encode-svt-rs`
//! path, where `__expert` is off. Defining it inside the `__expert`-gated
//! module broke every `--features encode` build: `EncoderConfig` names the
//! type in a field, so `cargo check --lib --features encode` failed with
//! `E0433: cannot find \`expert\` in \`crate\`` on 2026-09-02 (CI run
//! 33657668239, red on ubuntu/macos/windows). Keep the definition here and the
//! re-export there.

// ============================================================================
// SVT-AV1 still-image knobs (`Av1Backend::SvtRs`)
// ============================================================================

/// Still-image encoder knobs for the [`crate::Av1Backend::SvtRs`] backend.
///
/// **Unstable surface** — same contract as `expert::InternalParams`: that surface is
/// explicitly not part of the public API and exists so a sweep / picker /
/// calibration pipeline can drive parameter combinations. Apply via
/// [`crate::EncoderConfig::with_svt_params`].
///
/// Every field's [`Default`] is **what the seam configures today**, so a
/// default `SvtParams` is byte-identical to not setting one at all. The
/// defaults are SVT-AV1 v4.2.0 *mainline* defaults (tune 1 = PSNR, QM off,
/// variance boost off, sharpness 0) — deliberately **not** the upstream
/// still-image recipe, which is `--tune 3` + variance boost + QM. Measuring
/// the distance between those two is the point of the knob sweep.
///
/// Fields map one-to-one onto `svtav1_encoder::pipeline::EncodePipeline`'s
/// `hdr` (a `HdrForkConfig`) and its tile fields. Knobs that the port
/// refuses, ignores or has no consumer for are deliberately absent — see
/// `zenmetrics/benchmarks/avif_knob_dossier_2026-09-01.md` §4.2 for the
/// refused/inert inventory and §8.1 for why these nine are the ones worth
/// sweeping.
///
/// # Tune is a super-factor
///
/// `tune` 3 (IQ) and 4 (MS-SSIM) **rewrite other fields of this struct** at
/// encode time via the port's own `HdrForkConfig::apply_tune_overrides`
/// (`enable_qm`, the QM levels, `sharpness`, the variance-boost trio, and for
/// IQ also `max_tx_size` — by qp — and `screen_content_mode`). Setting those
/// fields alongside `tune = 3` does not do what it looks like: the tune wins.
/// [`Self::resolved`] applies exactly that rewrite, which is what the sweep
/// planner fingerprints, so aliased spellings collapse to one cell instead of
/// being encoded repeatedly under different names.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvtParams {
    /// `--tune`: 0 = VQ, 1 = PSNR (default), 2 = SSIM, 3 = IQ (the only mode
    /// upstream marks still-image-only), 4 = MS-SSIM. Slot 5 is
    /// `TUNE_FILM_GRAIN` in this port's fork enum — **not** mainline's VMAF —
    /// and is not swept.
    pub tune: u8,
    /// `--enable-variance-boost`: per-64×64 delta-q that lowers qindex on
    /// low-variance superblocks. The main perceptual lever that is not the
    /// quantizer. Off in SVT mainline; forced on by tune 3/4.
    pub enable_variance_boost: bool,
    /// `--variance-boost-strength`, 1..=4. Upstream: "strength 3 is best for
    /// still images". **3 and 4 saturate to the same plan**, so there are
    /// three distinct levels. Out-of-range values panic in a release build
    /// (the port guards this with `debug_assert` only), so
    /// [`Self::clamped`] pins it — apply it before handing a value to the
    /// encoder.
    pub variance_boost_strength: u8,
    /// `--variance-octile`, 1..=8 (upstream recommends 4–7): how much of a
    /// superblock must be low-contrast to be boosted. Same release-mode
    /// out-of-range panic as `variance_boost_strength`; same clamp.
    pub variance_octile: u8,
    /// `--enable-qm`: quantization matrices. **Off by default in SVT**, in
    /// contrast to libaom, which turns them on for images.
    pub enable_qm: bool,
    /// `--qm-min`, 0..=15. Applied to luma and chroma alike, matching what
    /// the port's tune-IQ override does.
    pub min_qm_level: u8,
    /// `--qm-max`, 0..=15 (15 = identity).
    pub max_qm_level: u8,
    /// `--sharpness`. **Categorical, not a linear dial** — both backends'
    /// image tunes force 7, and the underlying behaviour is a set of discrete
    /// switches rather than a smooth ramp. The port clamps to 0..=7 at use,
    /// so negatives are indistinguishable from 0.
    pub sharpness: i8,
    /// `--scm`: `None` derives the screen-content mode from the preset (the
    /// default); `Some(3)` forces the anti-alias-aware detector on at any
    /// preset, enabling palette + IntraBC. Decisive on text/UI content.
    pub force_screen_content_mode: Option<u8>,
    /// `--ac-bias`, 0.0..=8.0: RD bias toward high-frequency error (texture
    /// and grain retention). Live in mainline; default 0.0.
    pub ac_bias: f64,
    /// `--max-tx-size`, **32 or 64 only** (the port hard-refuses anything
    /// else). 32 forbids 64×64 square transforms. Tune IQ selects it *by qp*
    /// (32 at qp ≤ 45), i.e. upstream's own optimum is quality-dependent.
    pub max_tx_size: u8,
    /// `--tile-columns` as log2. Encode + decode parallelism at an efficiency
    /// cost; unlike `threads` it moves bytes, so it is a modelled axis.
    pub tile_cols_log2: u8,
    /// `--tile-rows` as log2.
    pub tile_rows_log2: u8,
}

impl Default for SvtParams {
    fn default() -> Self {
        Self {
            tune: 1,
            enable_variance_boost: false,
            variance_boost_strength: 2,
            variance_octile: 5,
            enable_qm: false,
            min_qm_level: 8,
            max_qm_level: 15,
            sharpness: 0,
            force_screen_content_mode: None,
            ac_bias: 0.0,
            max_tx_size: 64,
            tile_cols_log2: 0,
            tile_rows_log2: 0,
        }
    }
}

// `is_default` / `deviations` / `resolved` are consumed only by the
// `__expert`-gated sweep planner (`src/sweep.rs`); `clamped` is on the
// ordinary encode path. Without `__expert` the first three are genuinely
// unreachable, so silence dead_code there rather than crate-wide.
#[cfg_attr(not(feature = "__expert"), allow(dead_code))]
impl SvtParams {
    /// `true` when every field is at its [`Default`] — i.e. this config asks
    /// for exactly what the seam does with no `SvtParams` at all.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// How many fields deviate from the default, counting the
    /// variance-boost trio and the QM triple as **one** deviation each
    /// (they are one knob with a compound value, not three independent
    /// ones — a design that counted them separately would spend its
    /// interaction budget crossing a knob with itself).
    // `ac_bias` is an f64, but its values are literal grid points copied from
    // the axis definition, never computed, so exact comparison is asking the
    // right question: "is this field still the default spelling?"
    #[allow(clippy::float_cmp)]
    #[must_use]
    pub fn deviations(&self) -> u8 {
        let d = Self::default();
        u8::from(self.tune != d.tune)
            + u8::from(
                self.enable_variance_boost != d.enable_variance_boost
                    || self.variance_boost_strength != d.variance_boost_strength
                    || self.variance_octile != d.variance_octile,
            )
            + u8::from(
                self.enable_qm != d.enable_qm
                    || self.min_qm_level != d.min_qm_level
                    || self.max_qm_level != d.max_qm_level,
            )
            + u8::from(self.sharpness != d.sharpness)
            + u8::from(self.force_screen_content_mode != d.force_screen_content_mode)
            + u8::from(self.ac_bias != d.ac_bias)
            + u8::from(self.max_tx_size != d.max_tx_size)
            + u8::from(
                self.tile_cols_log2 != d.tile_cols_log2 || self.tile_rows_log2 != d.tile_rows_log2,
            )
    }

    /// Clamp the two fields the port guards with `debug_assert` only.
    ///
    /// `variance_boost_strength` indexes a `[f64; 5]` and `variance_octile`
    /// feeds `octile * SUBBLOCKS_IN_OCTILE - 1`, both behind assertions that
    /// **vanish in a release build** — and every fleet worker is a release
    /// build. The seam clamps rather than refuses so a sweep cell can never
    /// take down a worker.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.variance_boost_strength = self.variance_boost_strength.clamp(1, 4);
        self.variance_octile = self.variance_octile.clamp(1, 8);
        self.min_qm_level = self.min_qm_level.min(15);
        self.max_qm_level = self.max_qm_level.min(15);
        self.max_tx_size = if self.max_tx_size == 32 { 32 } else { 64 };
        self.sharpness = self.sharpness.clamp(0, 7);
        self
    }

    /// The configuration the encoder will actually run: [`Self::clamped`]
    /// with the port's own tune overrides applied for the given CLI-domain
    /// `qp`.
    ///
    /// This is a **transcription** of `HdrForkConfig::apply_tune_overrides`
    /// (zenav1-svt `hdr_mode.rs`), restricted to the fields this struct
    /// carries. It is kept here rather than called through the port so the
    /// sweep planner can resolve a cell without an `encode-svt-rs` build; the
    /// test `resolved_matches_the_port_tune_overrides` (behind that feature)
    /// pins the two together.
    #[must_use]
    pub fn resolved(self, qp: u8) -> Self {
        let mut r = self.clamped();
        // TUNE_IQ (3) and TUNE_MS_SSIM (4) share this block.
        if r.tune == 3 || r.tune == 4 {
            r.enable_qm = true;
            r.min_qm_level = 4;
            r.max_qm_level = 10;
            r.sharpness = 7;
            r.enable_variance_boost = true;
            r.variance_boost_strength = 3;
        }
        // IQ only, on top of the shared block.
        if r.tune == 3 {
            r.max_tx_size = if qp <= 45 { 32 } else { 64 };
            r.force_screen_content_mode = Some(3);
        }
        r
    }
}
