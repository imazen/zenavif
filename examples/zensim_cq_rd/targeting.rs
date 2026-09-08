//! AVIF-owned adapter for the shared Rust native targeting experiment.
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, path::Path, time::Instant};
use zenpredict_serving::Model;
use zensim::{BakeScorer, Fused944Session, PrecomputedReference, RgbSlice};
use zensim_target::{
    CodecKind,
    codec::CodecBackend,
    native_probe::{NativeProbeBackend, NativeProbeCodec, NativeProbeWork},
};

pub(super) struct Avif;
const KNOTS: [f32; 17] = [
    1., 16., 32., 48., 64., 80., 96., 112., 128., 144., 160., 176., 192., 208., 224., 240., 255.,
];
impl NativeProbeCodec for Avif {
    fn codec(&self) -> CodecKind {
        CodecKind::Avif
    }
    fn configuration(&self) -> &str {
        "avif:CQ1-255,Zenravif,s6,444,8bit,opaque-sRGB8,threads1;scalar:no-maps;neutral:zerosum-gain0;active:zerosum-gain10,factor1.15;previous-decoded-map;bin8;formula1;first-probe-calibration"
    }
    fn quality_knots(&self) -> &[f32] {
        &KNOTS
    }
    fn bound_encodes(&self, arm: &str) -> usize {
        if arm == "scalar" { 1 } else { 3 }
    }
    fn backend<'a>(
        &'a self,
        arm: &str,
        model: &'a Model,
        _scratch: &Path,
        _bake: &Path,
    ) -> Result<Box<dyn NativeProbeBackend + 'a>> {
        ensure!(
            matches!(arm, "scalar" | "neutral" | "active"),
            "unknown native AVIF arm"
        );
        ensure!(
            std::env::var("ZENSIM_FORMULA_REV").as_deref() == Ok("1"),
            "native AVIF requires explicit formula revision 1"
        );
        Ok(Box::new(Native {
            map_enabled: arm != "scalar",
            gain: if arm == "active" { 10. } else { 0. },
            state: RefCell::new(State {
                scorer: BakeScorer::new(model)?,
                pre: None,
                session: Fused944Session::new(),
                scales: Vec::new(),
                work: Vec::new(),
            }),
        }))
    }
    fn decode(&self, encoded: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
        Ok(super::decode_rgb8(encoded, width as usize, height as usize).into_flattened())
    }
}
struct State<'a> {
    scorer: BakeScorer<'a>,
    pre: Option<PrecomputedReference>,
    session: Fused944Session,
    scales: Vec<f32>,
    work: Vec<NativeProbeWork>,
}
struct Native<'a> {
    map_enabled: bool,
    gain: f32,
    state: RefCell<State<'a>>,
}
fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}
impl NativeProbeBackend for Native<'_> {
    fn take_work(&self) -> Vec<NativeProbeWork> {
        std::mem::take(&mut self.state.borrow_mut().work)
    }
}
impl CodecBackend for Native<'_> {
    fn quality_range(&self) -> (f32, f32) {
        (1., 255.)
    }
    fn lower_quality_means_higher_score(&self) -> bool {
        true
    }
    fn encode_decode(
        &self,
        rgb: &[u8],
        width: u32,
        height: u32,
        knob: f32,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let (w, h) = (width as usize, height as usize);
        ensure!(
            w > 0
                && h > 0
                && rgb.len() == w.checked_mul(h).and_then(|n| n.checked_mul(3)).unwrap_or(0),
            "packed RGB8 shape"
        );
        let px = rgb.as_chunks::<3>().0;
        let mut state = self.state.borrow_mut();
        let consumed = usize::from(self.map_enabled && state.scales.iter().any(|&x| x != 1.));
        let hint = if consumed == 1 {
            Some(state.scales.clone().into_boxed_slice())
        } else {
            None
        };
        let settings = super::EncodeSettings {
            speed: 6,
            threads: 1,
        };
        let start = Instant::now();
        let encoded = super::encode_at(px, w, h, f64::from(knob), hint, &settings);
        let encode_seconds = start.elapsed().as_secs_f64();
        let start = Instant::now();
        let decoded = super::decode_rgb8(&encoded, w, h);
        let decode_seconds = start.elapsed().as_secs_f64();
        let start = Instant::now();
        if self.map_enabled {
            let source = RgbSlice::new(px, w, h);
            if state.pre.is_none() {
                state.pre = Some(state.scorer.precompute_reference(&source)?);
            }
            let State {
                scorer,
                pre,
                session,
                scales,
                ..
            } = &mut *state;
            let spatial = scorer.compute_with_ref_and_attribution(
                &source,
                pre.as_ref().unwrap(),
                &RgbSlice::new(&decoded, w, h),
                Some("avif"),
                session,
                8,
            )?;
            ensure!(
                spatial.unsupported_feature_ids().is_empty() && !spatial.has_corruption_gate(),
                "unsupported spatial terms/discontinuous gate"
            );
            let (cols, rows) = (w.div_ceil(super::SB), h.div_ceil(super::SB));
            let mut q = Vec::with_capacity(cols * rows);
            for y in 0..rows {
                for x in 0..cols {
                    q.push(spatial.attribution().query_rect(
                        x * super::SB,
                        y * super::SB,
                        (x + 1) * super::SB,
                        (y + 1) * super::SB,
                    ));
                }
            }
            ensure!(q.iter().all(|x| x.is_finite()), "nonfinite block map");
            scales.resize(q.len(), 1.);
            super::redistribute_zerosum(scales, &q, self.gain, 1.15);
            ensure!(
                scales.iter().all(|x| x.is_finite() && *x > 0.),
                "invalid block scale"
            );
        }
        let native_loop_ms = if self.map_enabled {
            start.elapsed().as_secs_f64() * 1e3
        } else {
            0.
        };
        let decoded = decoded.into_flattened();
        state.work.push(NativeProbeWork {
            knob,
            encode_seconds,
            decode_seconds,
            internal_reconstructions: 0,
            native_pixel_comparisons: usize::from(self.map_enabled),
            map_evaluations: usize::from(self.map_enabled),
            consumed_maps: consumed,
            native_loop_ms,
            encoded_sha256: sha(&encoded),
            decoded_sha256: sha(&decoded),
        });
        Ok((encoded, decoded))
    }
}
