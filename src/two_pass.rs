//! Butteraugli-diffmap-guided two-pass encoding — the spatial closed loop.
//!
//! Pass 1 encodes normally, decodes with zenavif's own decoder (the signal a
//! user actually sees), and computes the butteraugli per-pixel difference map
//! against the source. The map is pooled per 64×64 superblock into AC
//! quantizer scale factors — superblocks whose butteraugli error is high
//! relative to their MSE get a finer quantizer (more bits), over-served
//! superblocks a coarser one — and pass 2 re-encodes with the map applied
//! through zenrav1e's per-SB `delta_q` machinery (real delta-q syntax + RDO
//! distortion follow).
//!
//! The pooling formula is a port of libaom's `tune=butteraugli`
//! (`av1/encoder/tune_butteraugli.c` at rev 632172a4, evaluated on our
//! corpora 2026-07-03: −2.4..−3.5% median butteraugli-3n BD-rate on photos
//! at cpu2/cpu6 with ssim2 neutral-to-better): per-block weight
//! `min(mse/butteraugli₁₂, 5) + K`, geometric-mean normalized, clamped —
//! then mapped from libaom's rdmult (λ) domain into a quantizer scale via
//! `q = λ^(1/2)` with a tunable exponent (`strength`). Differences from
//! libaom, deliberate: full-resolution diffmap from the *actual* pass-1
//! encode at the real quantizer (libaom uses a half-resolution preliminary
//! encode at fixed q96); RGB MSE instead of YUV; the map moves the coded
//! per-SB quantizer (with zenrav1e's distortion follow), not just λ.
//!
//! **Release gate:** requires zenravif's live `FrameHints` passthrough
//! (`zenravif::FRAME_HINTS_LIVE`). Until the zenrav1e dep bump the driver
//! fails honestly with [`crate::Error::Encode`] instead of silently paying
//! for a second pass that cannot steer anything.

use crate::DecoderConfig;
use crate::encoder::{EncodedImage, EncoderConfig, encode_rgb8_once};
use crate::error::{Error, Result};
use enough::Stop as _;
use imgref::ImgRef;
use rgb::Rgb;
use whereat::at;

/// Whether the underlying zenravif build's `FrameHints` passthrough is
/// live (re-exported so callers can check the release gate before paying
/// for a pass they know will be refused).
pub use ravif::FRAME_HINTS_LIVE;

/// Which metric's per-pixel error map drives the second pass.
///
/// The pooling contract every backend must satisfy: a full-resolution
/// `f32` map, one value per source pixel, **higher = perceptually worse**,
/// non-negative, on a scale where the backend's `map_eps` separates
/// "no reliable signal" from real error. The pooling/normalization layer
/// (geometric-mean normalize + clamp) is scale-invariant beyond that.
///
/// Pluggable by design (user directive 2026-07-03): butteraugli is the only
/// backend whose crate publicly exposes a per-pixel map today. SSIMULACRA2
/// (`fast-ssim2` — much cheaper per call, which matters at 2-pass cost) and
/// zensim profile-B slot in here the moment their crates expose maps;
/// verified absent at fast-ssim2 `585006c` / zensim HEAD 2026-07-03 (the
/// internal per-scale machinery exists in both, the public API does not).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum TwoPassMetric {
    /// Butteraugli perceptual distance map (`butteraugli` crate,
    /// `with_compute_diffmap` — the libaom `tune=butteraugli` analog).
    #[default]
    Butteraugli,
}

impl TwoPassMetric {
    /// Parses a metric name (harness / CLI convention).
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "butteraugli" | "butter" => Some(Self::Butteraugli),
            _ => None,
        }
    }

    /// Harness name for logging / cache keys.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Butteraugli => "butteraugli",
        }
    }
}

/// Options for the diffmap-guided second pass.
///
/// The defaults are the libaom `tune=butteraugli` constants (rev 632172a4,
/// recode-loop path) translated to the quantizer domain; expect refitting —
/// aom constants have transferred as *mechanisms*, not values, throughout
/// this program.
#[derive(Debug, Clone)]
pub struct TwoPassOptions {
    /// The per-pixel error-map source for the closed loop.
    pub metric: TwoPassMetric,
    /// Exponent applied to the normalized rdmult weight when converting to
    /// a quantizer scale: `q_scale = weight^(strength/2)`. `1.0` is
    /// λ-parity with libaom (λ ∝ q²); `0.0` disables the correction
    /// (pass 2 == pass 1 up to encoder determinism); larger values steer
    /// harder.
    pub strength: f64,
    /// Additive stabilizer on the raw `mse/error` ratio before
    /// normalization (libaom's `K`; 0.4 on their recode path, 0.3 on the
    /// preliminary-pass path).
    pub k: f64,
    /// Post-normalization clamp on the rdmult-domain weight
    /// (libaom: `[0.4, 2.5]`).
    pub weight_clamp: (f64, f64),
    /// Exponent of the per-superblock error-map p-norm pool (libaom: 12 —
    /// close to max-pooling).
    pub pool_exponent: f64,
    /// Butteraugli HF asymmetry for the *loop's* metric. `1.0` matches the
    /// crate default and this program's evaluation scoring; libaom's shim
    /// uses `0.8`. A refit knob, not a quality dial.
    pub hf_asymmetry: f32,
    /// Butteraugli intensity target in nits (SDR convention: 80).
    pub intensity_target: f32,
    /// When `Some(q)`, pass 1 encodes at this FIXED quality instead of the
    /// caller's — libaom's preliminary-pass shape (their loop probes at
    /// fixed `q_index 96` regardless of the target rate), which makes the
    /// error map a quasi-content-intrinsic "where does this content
    /// degrade" signal instead of a self-referential residual of the very
    /// allocation being corrected. A mid-low probe (e.g. `40.0`) also
    /// makes pass 1 cheaper than the real encode. `None` = probe at the
    /// caller's quality (the v1 behavior).
    pub probe_quality: Option<f32>,
}

impl Default for TwoPassOptions {
    fn default() -> Self {
        Self {
            metric: TwoPassMetric::Butteraugli,
            strength: 1.0,
            k: 0.4,
            weight_clamp: (0.4, 2.5),
            pool_exponent: 12.0,
            hf_asymmetry: 1.0,
            intensity_target: 80.0,
            probe_quality: None,
        }
    }
}

/// A computed per-pixel error map plus the backend's whole-image scores
/// (for diagnostics). The map obeys the [`TwoPassMetric`] contract.
struct ErrorMap {
    map: imgref::ImgVec<f32>,
    /// Backend-native "worst" aggregate (butteraugli: max norm).
    score_worst: f64,
    /// Backend-native "bulk" aggregate (butteraugli: 3-norm).
    score_bulk: f64,
    /// Below this map/MSE magnitude a superblock has no reliable signal
    /// and stays neutral (libaom's eps guard, backend-scaled).
    map_eps: f64,
}

/// Computes the per-pixel error map for `options.metric` between the
/// source and the pass-1 decode. THE seam where new map backends land.
fn compute_error_map(
    src: ImgRef<'_, Rgb<u8>>,
    dec: ImgRef<'_, Rgb<u8>>,
    options: &TwoPassOptions,
) -> Result<ErrorMap> {
    match options.metric {
        TwoPassMetric::Butteraugli => {
            let params = butteraugli::ButteraugliParams::new()
                .with_hf_asymmetry(options.hf_asymmetry)
                .with_intensity_target(options.intensity_target)
                .with_compute_diffmap(true);
            let ba = butteraugli::butteraugli(src, dec, &params).map_err(|e| {
                at!(Error::Encode(format!(
                    "two-pass: butteraugli diffmap computation failed: {e}"
                )))
            })?;
            let map = ba.diffmap.ok_or_else(|| {
                at!(Error::Encode(
                    "two-pass: butteraugli returned no diffmap".to_string()
                ))
            })?;
            Ok(ErrorMap {
                map,
                score_worst: ba.score,
                score_bulk: ba.pnorm_3,
                map_eps: 0.01,
            })
        }
    }
}

/// Outcome of a two-pass encode: the pass-2 result plus the closed-loop
/// diagnostics (pass-1 size and scores, the applied map).
#[derive(Debug)]
pub struct TwoPassEncode {
    /// The pass-2 (final) encode.
    pub encode: EncodedImage,
    /// Total size of the pass-1 AVIF that fed the diffmap, for cost/benefit
    /// reporting.
    pub pass1_bytes: usize,
    /// Pass-1 butteraugli max-norm score vs the source.
    pub pass1_butteraugli_max: f64,
    /// Pass-1 butteraugli 3-norm score vs the source.
    pub pass1_butteraugli_3n: f64,
    /// The per-superblock AC quantizer scale map pass 2 was encoded with
    /// (frame superblock raster order; `1.0` = neutral).
    pub sb_q_scale: Box<[f32]>,
}

/// Encode an RGB8 image to AVIF with a butteraugli-diffmap-guided second
/// pass (see the [module docs](self) for the mechanism).
///
/// Costs one extra encode plus one decode and one butteraugli comparison
/// (~2.1× a single [`encode_rgb8`]). The alpha-less RGB8 path is the v1
/// surface; RGBA8/RGB16 follow the same pattern once the mechanism's win is
/// re-measured through this driver.
///
/// # Errors
///
/// - [`Error::Encode`] when the zenravif build's `FrameHints` passthrough
///   is release-gated off (`zenravif::FRAME_HINTS_LIVE == false`) — the
///   second pass could not steer the encoder, so the driver refuses to
///   silently double-encode.
/// - Any pass-1/pass-2 encode, decode, or scoring error.
pub fn encode_rgb8_two_pass(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    options: &TwoPassOptions,
    stop: almost_enough::StopToken,
) -> Result<TwoPassEncode> {
    if !ravif::FRAME_HINTS_LIVE {
        return Err(at!(Error::Encode(
            "two-pass-butteraugli: zenravif's FrameHints passthrough is release-gated off \
             (FRAME_HINTS_LIVE == false until the zenrav1e dep bump past 0.1.4); a second \
             pass could not steer the encoder, refusing to silently double-encode"
                .to_string()
        )));
    }

    // Pass 1: the probe encode — the caller's config, optionally at a
    // fixed probe quality (libaom's preliminary-pass shape).
    let pass1 = match options.probe_quality {
        Some(pq) => {
            let probe_cfg = config.clone().quality(pq);
            encode_rgb8_once(img, &probe_cfg, stop.clone())?
        }
        None => encode_rgb8_once(img, config, stop.clone())?,
    };

    // Decode with our own decoder — the pixels a user gets.
    let dec_config = DecoderConfig::new().prefer_8bit(true);
    let decoded = crate::decode_with(&pass1.avif_file, &dec_config, &stop)?;
    let dec_img: ImgRef<'_, Rgb<u8>> = decoded.try_as_imgref::<Rgb<u8>>().ok_or_else(|| {
        at!(Error::Encode(
            "two-pass-butteraugli: pass-1 decode not RGB8-viewable".to_string()
        ))
    })?;

    stop.check().map_err(|e| at!(Error::from(e)))?;

    // Per-pixel error map (the pluggable metric backend), source vs
    // pass-1 decode.
    let em = compute_error_map(img, dec_img, options)?;

    stop.check().map_err(|e| at!(Error::from(e)))?;

    // Pool per 64×64 superblock into quantizer scale factors.
    let sb_q_scale = pool_sb_q_scale(img, dec_img, em.map.as_ref(), em.map_eps, options);

    // Pass 2: re-encode with the map applied through the per-SB delta_q
    // machinery.
    let mut cfg2 = config.clone();
    cfg2.sb_q_scale = Some(sb_q_scale.clone());
    let pass2 = encode_rgb8_once(img, &cfg2, stop)?;

    Ok(TwoPassEncode {
        encode: pass2,
        pass1_bytes: pass1.avif_file.len(),
        pass1_butteraugli_max: em.score_worst,
        pass1_butteraugli_3n: em.score_bulk,
        sb_q_scale,
    })
}

/// Pools a per-pixel error map + pixel MSE per 64×64 superblock into AC
/// quantizer scale factors (libaom `set_mb_butteraugli_rdmult_scaling`
/// translated to the quantizer domain; see the module docs).
///
/// Returns `ceil(w/64) × ceil(h/64)` factors in frame superblock raster
/// order — the grid zenrav1e's `FrameHints::sb_q_scale` expects. Superblocks
/// with negligible error on either signal (below `map_eps`) stay
/// neutral (`1.0`).
fn pool_sb_q_scale(
    src: ImgRef<'_, Rgb<u8>>,
    dec: ImgRef<'_, Rgb<u8>>,
    diffmap: ImgRef<'_, f32>,
    map_eps: f64,
    options: &TwoPassOptions,
) -> Box<[f32]> {
    const SB: usize = 64;
    let w = src.width();
    let h = src.height();
    debug_assert_eq!((diffmap.width(), diffmap.height()), (w, h));
    debug_assert_eq!((dec.width(), dec.height()), (w, h));
    let sb_cols = w.div_ceil(SB);
    let sb_rows = h.div_ceil(SB);
    let (lo, hi) = options.weight_clamp;
    let exp = options.pool_exponent;

    // Raw rdmult-domain weights; f64::NAN marks "no reliable signal".
    let mut weights = vec![f64::NAN; sb_cols * sb_rows];
    let mut log_sum = 0.0f64;
    let mut n_valid = 0u32;

    for sby in 0..sb_rows {
        for sbx in 0..sb_cols {
            let x0 = sbx * SB;
            let y0 = sby * SB;
            let x1 = (x0 + SB).min(w);
            let y1 = (y0 + SB).min(h);

            let mut pool = 0.0f64;
            let mut sse = 0.0f64;
            for y in y0..y1 {
                let dm_row = &diffmap[y];
                let src_row = &src[y];
                let dec_row = &dec[y];
                for x in x0..x1 {
                    pool += f64::from(dm_row[x]).max(0.0).powf(exp);
                    let s = src_row[x];
                    let d = dec_row[x];
                    let dr = f64::from(s.r) - f64::from(d.r);
                    let dg = f64::from(s.g) - f64::from(d.g);
                    let db = f64::from(s.b) - f64::from(d.b);
                    sse += dr * dr + dg * dg + db * db;
                }
            }
            let px = ((x1 - x0) * (y1 - y0)) as f64;
            let derror = pool.powf(1.0 / exp);
            let dmse = sse / (px * 3.0);

            // libaom's eps guard: no reliable signal -> neutral. The map
            // side uses the backend-scaled eps; the MSE side keeps
            // libaom's 0.01 (8-bit pixel-difference domain).
            if derror >= map_eps && dmse >= 0.01 {
                let w_raw = (dmse / derror).min(5.0) + options.k;
                weights[sby * sb_cols + sbx] = w_raw;
                log_sum += w_raw.ln();
                n_valid += 1;
            }
        }
    }

    if n_valid == 0 {
        return vec![1.0f32; sb_cols * sb_rows].into_boxed_slice();
    }
    let geo_mean = (log_sum / f64::from(n_valid)).exp();

    weights
        .into_iter()
        .map(|w_raw| {
            if w_raw.is_nan() {
                1.0
            } else {
                let w_norm = (w_raw / geo_mean).clamp(lo, hi);
                // rdmult (λ) domain -> quantizer domain: λ ∝ q².
                w_norm.powf(options.strength / 2.0) as f32
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use imgref::ImgVec;

    fn flat_imgs(w: usize, h: usize) -> (ImgVec<Rgb<u8>>, ImgVec<Rgb<u8>>) {
        let src = ImgVec::new(
            vec![
                Rgb {
                    r: 120u8,
                    g: 130,
                    b: 140
                };
                w * h
            ],
            w,
            h,
        );
        // Uniform decode error of 4/255 per channel -> nonzero MSE everywhere.
        let dec = ImgVec::new(
            vec![
                Rgb {
                    r: 124u8,
                    g: 126,
                    b: 144
                };
                w * h
            ],
            w,
            h,
        );
        (src, dec)
    }

    #[test]
    fn uniform_signal_pools_to_neutral() {
        let (src, dec) = flat_imgs(130, 70); // 3×2 SBs, ragged edges
        let dm = ImgVec::new(vec![0.5f32; 130 * 70], 130, 70);
        let map = pool_sb_q_scale(
            src.as_ref(),
            dec.as_ref(),
            dm.as_ref(),
            0.01,
            &TwoPassOptions::default(),
        );
        assert_eq!(map.len(), 6);
        for &s in map.iter() {
            // Every block has the same weight, so geomean normalization sends
            // every scale to exactly clamp-free 1.0.
            assert!((s - 1.0).abs() < 1e-6, "scale {s} not neutral");
        }
    }

    #[test]
    fn hot_superblock_gets_finer_quantizer() {
        let (src, dec) = flat_imgs(192, 128); // 3×2 SBs
        let mut dm = vec![0.3f32; 192 * 128];
        // SB (1,0): butteraugli says the error is much more visible than
        // the (identical) MSE suggests.
        for y in 0..64 {
            for x in 64..128 {
                dm[y * 192 + x] = 6.0;
            }
        }
        let dm = ImgVec::new(dm, 192, 128);
        let map = pool_sb_q_scale(
            src.as_ref(),
            dec.as_ref(),
            dm.as_ref(),
            0.01,
            &TwoPassOptions::default(),
        );
        assert_eq!(map.len(), 6);
        let hot = map[1];
        for (i, &s) in map.iter().enumerate() {
            if i == 1 {
                assert!(s < 1.0, "hot SB must get a finer quantizer, got {s}");
            } else {
                assert!(s > 1.0, "cool SBs must give bits back, got {s}");
                assert!(hot < s);
            }
        }
        // strength=0 disables the correction entirely.
        let map0 = pool_sb_q_scale(
            src.as_ref(),
            dec.as_ref(),
            dm.as_ref(),
            0.01,
            &TwoPassOptions {
                strength: 0.0,
                ..Default::default()
            },
        );
        assert!(map0.iter().all(|&s| (s - 1.0).abs() < 1e-6));
    }

    #[test]
    fn degenerate_signals_stay_neutral() {
        // Identical images: zero MSE and zero diffmap -> eps guard -> all 1.0.
        let (src, _) = flat_imgs(64, 64);
        let dm = ImgVec::new(vec![0.0f32; 64 * 64], 64, 64);
        let map = pool_sb_q_scale(
            src.as_ref(),
            src.as_ref(),
            dm.as_ref(),
            0.01,
            &TwoPassOptions::default(),
        );
        assert_eq!(map.len(), 1);
        assert_eq!(map[0], 1.0);
    }
}
