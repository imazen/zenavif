//! End-to-end gates for the AVIF backend + knob tuner: **tune → encode →
//! decode read-back → score**, with real encodes.
//!
//! Requires `--features auto-tune,encode,encode-imazen`.
//!
//! ⚠ **Add `zenav1-svt` to exercise the routing gates for real.** With
//! only one backend built there is nothing to route *between*, so the
//! budget gate degenerates to "the tuner refuses an unsatisfiable
//! budget" — a true statement, but not the routing claim. Verified by
//! negative control: with `zenav1-svt` on and the stub's routing
//! bypassed, `time_budget_routes_to_the_faster_backend` and
//! `budget_outranks_the_reach_preference` both FAIL (quoting the
//! measured 2971.29 ms vs 66.16 ms); with `zenav1-svt` off, the same
//! sabotage passes. The gates are real; the feature is what makes them
//! bite.
//!
//! # What these gates are, and are not
//!
//! Two of them have teeth from **committed measurement**:
//!
//! - [`time_budget_routes_to_the_faster_backend`] — the
//!   `alpha + beta * MP` table transcribed from
//!   `/mnt/v/output/avif-speed-instrument-2026-09-03/speed_alpha_beta.tsv`
//!   says zenravif needs ~2,971 ms for 1 MP at speed 6 and svt ~66 ms.
//!   A 100 ms budget must therefore flip the pick. This is criterion 4's
//!   "routes to the optimal AV1 encoder per the time + resource budget,
//!   measured".
//! - [`high_quality_target_routes_to_the_backend_that_can_reach_it`] —
//!   svt-as-configured cannot reach ssim2 90 on 16 of 32 campaign
//!   references at any q or speed; zenravif misses on 1 of 32.
//!
//! The rest gate the *path*: that the config the tuner returns actually
//! encodes, that the file decodes, and that what comes back is what was
//! asked for.
//!
//! # Fixtures are synthetic, and labelled as such
//!
//! They are generated in-process (deterministic, no corpus dependency,
//! nothing committed). They are **proxies with the right first-order
//! character** — flat regions and hard edges for the screen-content
//! proxy, smooth gradients plus noise for the photo proxy — and nothing
//! here claims a campaign BD-rate number from them. The campaign's own
//! record is emphatic that content class is not guessable: AI-generated
//! images behave like photos, not like screen content, on the very knob
//! most people would assume covers "synthetic".

#![cfg(all(feature = "auto-tune", feature = "encode"))]

use almost_enough::{StopToken, Unstoppable};
use imgref::{Img, ImgVec};
use rgb::Rgb;
use zenavif::backend_tuner::{
    AllowedBackends, AvifTuning, StubTuner, TuneRequest, TuneSource, stub,
};
use zenavif::{Av1Backend, EncodeChromaSubsampling, EncoderConfig, QualityTarget};

const W: u32 = 192;
const H: u32 = 192;

/// Whether this build can encode with the zenav1-svt seam.
const fn svt_is_built() -> bool {
    cfg!(feature = "zenav1-svt")
}

// ── Fixtures ────────────────────────────────────────────────────────

/// Photo proxy: smooth two-axis gradient plus deterministic fine noise.
fn fixture_photo() -> Vec<u8> {
    let mut px = Vec::with_capacity((W * H * 3) as usize);
    for y in 0..H {
        for x in 0..W {
            // A cheap deterministic hash for the noise term — no rand dep,
            // and identical on every host so byte counts are comparable.
            let n = ((x.wrapping_mul(2654435761) ^ y.wrapping_mul(2246822519)) >> 24) as u8;
            px.push((x * 255 / W) as u8);
            px.push((y * 255 / H) as u8);
            px.push(((x + y) * 255 / (W + H)) as u8 ^ (n >> 5));
        }
    }
    px
}

/// Screen-content proxy: flat white ground, hard black gridlines, solid
/// colour blocks, and 1-px rules — the flat-region + hard-edge structure
/// a plot or a screenshot has.
fn fixture_screen() -> Vec<u8> {
    let mut px = vec![0xF5u8; (W * H * 3) as usize];
    let put = |px: &mut Vec<u8>, x: u32, y: u32, rgb: [u8; 3]| {
        let i = ((y * W + x) * 3) as usize;
        px[i..i + 3].copy_from_slice(&rgb);
    };
    for y in 0..H {
        for x in 0..W {
            // Grid every 24 px, 1 px wide: maximal high-frequency chroma
            // and luma edges.
            if x % 24 == 0 || y % 24 == 0 {
                put(&mut px, x, y, [0x11, 0x11, 0x11]);
            }
            // Two saturated bars — chroma detail that 4:2:0 must carry.
            if (40..72).contains(&y) && (16..160).contains(&x) {
                put(&mut px, x, y, [0xD0, 0x20, 0x30]);
            }
            if (104..120).contains(&y) && (16..(16 + (x % 130).min(120))).contains(&x) {
                put(&mut px, x, y, [0x20, 0x60, 0xD0]);
            }
        }
    }
    px
}

/// Scan proxy: near-bilevel ink on paper with a low-amplitude texture.
fn fixture_scan() -> Vec<u8> {
    let mut px = Vec::with_capacity((W * H * 3) as usize);
    for y in 0..H {
        for x in 0..W {
            let paper = 0xEEu8.saturating_sub(((x ^ y) & 0x07) as u8);
            // Text-like runs: short dark strokes on a baseline grid.
            let ink = (y % 16) < 9 && (x % 7) < 4 && (x % 31) != 0;
            let v = if ink { 0x1A } else { paper };
            px.extend_from_slice(&[v, v, v.saturating_sub(4)]);
        }
    }
    px
}

fn fixtures() -> Vec<(&'static str, Vec<u8>, ImgVec<Rgb<u8>>)> {
    [
        ("photo", fixture_photo()),
        ("screen", fixture_screen()),
        ("scan", fixture_scan()),
    ]
    .into_iter()
    .map(|(n, raw)| {
        let i = img(&raw);
        (n, raw, i)
    })
    .collect()
}

// ── Helpers ─────────────────────────────────────────────────────────

fn encode(cfg: &EncoderConfig, rgb: &ImgVec<Rgb<u8>>) -> Vec<u8> {
    zenavif::encode_rgb8(rgb.as_ref(), cfg, StopToken::new(Unstoppable))
        .expect("encode must succeed")
        .avif_file
}

/// Pack a flat RGB8 buffer into the `imgref` view the encoder takes.
fn img(rgb: &[u8]) -> ImgVec<Rgb<u8>> {
    let px: Vec<Rgb<u8>> = rgb
        .chunks_exact(3)
        .map(|c| Rgb::new(c[0], c[1], c[2]))
        .collect();
    Img::new(px, W as usize, H as usize)
}

/// Decode read-back through zenavif-parse: assert the file is a real
/// AVIF and report the coded chroma the `av1C` box declares.
///
/// This is the gate that would have caught the campaign's own confound.
/// Every "backend difference" it measured was **totally** confounded with
/// chroma — verified by reading `av1C` out of 1,114 bitstreams, zero
/// exceptions — because one arm pinned 4:2:0 and the other kept 4:4:4.
/// Asserting the *requested* chroma is what actually landed is how a
/// caller keeps that from happening silently.
fn readback_chroma(avif: &[u8]) -> EncodeChromaSubsampling {
    let cfg = zenavif_parse::DecodeConfig::default();
    let parser =
        zenavif_parse::AvifParser::from_owned_with_config(avif.to_vec(), &cfg, &Unstoppable)
            .expect("the tuner's output must parse as AVIF");
    // The `av1C` box — the same property the campaign read out of 1,114
    // bitstreams to prove its backend table was a chroma confound.
    let av1c = parser.av1_config().expect("av1C property");
    assert!(
        !av1c.monochrome,
        "these fixtures are colour; a monochrome av1C means the config did not land"
    );
    if av1c.chroma_subsampling_x == 1 && av1c.chroma_subsampling_y == 1 {
        EncodeChromaSubsampling::Yuv420
    } else {
        EncodeChromaSubsampling::Yuv444
    }
}

// ── Measured routing gates ──────────────────────────────────────────

/// **Measured gate.** A budget the slow backend provably cannot meet
/// must flip the pick to the fast one.
///
/// At 1 MP the transcribed table reads zenravif speed 6 = 2,971 ms and
/// zenav1-svt speed 6 = 66 ms. The backend campaign's iso-time read is
/// that at a 100 ms budget zenravif is over budget on 31 of 32
/// references. So a 100 ms request at 1 MP has exactly one correct
/// answer while both backends are allowed.
#[test]
fn time_budget_routes_to_the_faster_backend() {
    // First: the table itself must still say what this gate rests on.
    let rav = stub::wall_time_model(Av1Backend::Zenravif, 6).expect("zenravif s6 row");
    assert!(
        rav.estimate_ms(1.0) > 100.0,
        "the measured table must still put zenravif s6 over a 100 ms budget at 1 MP \
         (got {} ms) — if this fires, the table changed and the gate below is void",
        rav.estimate_ms(1.0)
    );

    let req = TuneRequest::new(QualityTarget::Zensim(82.0), 1000, 1000).with_time_budget_ms(100.0);
    let tuned = StubTuner::new().tune(&[], None, &req);

    if svt_is_built() {
        let svt = stub::wall_time_model(Av1Backend::Zenav1Svt, 6).expect("svt s6 row");
        assert!(
            svt.estimate_ms(1.0) <= 100.0,
            "svt s6 must fit the budget for this gate to mean anything"
        );
        let tuned = tuned.expect("svt fits, so a pick exists");
        assert_eq!(
            tuned.backend(),
            Av1Backend::Zenav1Svt,
            "a 100 ms budget at 1 MP must route away from zenravif ({} ms) \
             to zenav1-svt ({} ms)",
            rav.estimate_ms(1.0),
            svt.estimate_ms(1.0)
        );
        assert!(
            tuned.expected_wall_ms().expect("a measured estimate") <= 100.0,
            "the reported estimate must itself honour the budget"
        );
    } else {
        // Without the svt seam there is no cell that fits, and the honest
        // answer is a refusal — NOT zenravif silently blowing the budget.
        assert!(
            tuned.is_err(),
            "with only zenravif built, a 100 ms/1 MP budget is unsatisfiable; \
             the tuner must refuse rather than return a ~3 s encode"
        );
    }
}

/// **Measured gate.** With no budget, a high quality target must route to
/// the backend measured to reach it.
#[test]
fn high_quality_target_routes_to_the_backend_that_can_reach_it() {
    let req = TuneRequest::new(
        QualityTarget::Zensim(stub::SVT_REACH_CEILING_TARGET + 4.0),
        800,
        600,
    );
    let tuned = StubTuner::new().tune(&[], None, &req).expect("a pick");
    assert_eq!(
        tuned.backend(),
        Av1Backend::Zenravif,
        "svt-as-configured misses ssim2 90 on 16 of 32 campaign references \
         (6/6 plots, 5/5 screenshots); zenravif misses on 1 of 32"
    );
}

/// A budget and the reach preference can conflict. The budget wins —
/// a caller who asked for 100 ms must not be handed a multi-second
/// encode because the tuner preferred a different backend.
#[test]
fn budget_outranks_the_reach_preference() {
    if !svt_is_built() {
        return; // covered by the refusal arm of the budget gate above
    }
    let req = TuneRequest::new(QualityTarget::Zensim(95.0), 1000, 1000).with_time_budget_ms(100.0);
    let tuned = StubTuner::new().tune(&[], None, &req).expect("svt fits");
    assert_eq!(tuned.backend(), Av1Backend::Zenav1Svt);
}

// ── Real encode → decode read-back ──────────────────────────────────

/// **The path gate.** For every fixture: tune, encode for real, parse the
/// result, and assert the coded chroma is the chroma the tuner asked for.
#[test]
fn tuned_config_encodes_and_reads_back_what_it_asked_for() {
    let tuner = StubTuner::new();
    for (name, rgb, image) in fixtures() {
        let req = TuneRequest::new(QualityTarget::Zensim(80.0), W, H);
        let tuned = tuner.tune(&rgb, None, &req).expect("a pick");
        assert_eq!(tuned.source(), TuneSource::Stub);

        let avif = encode(tuned.config(), &image);
        assert!(!avif.is_empty(), "{name}: encode produced no bytes");

        let coded = readback_chroma(&avif);
        let requested = requested_chroma(&tuned);
        assert_eq!(
            coded,
            requested,
            "{name}: cell {:?} asked for {requested:?} but the bitstream codes {coded:?} — \
             a config that does not land is exactly the confound the campaign found",
            tuned.cell_label()
        );
    }
}

/// The chroma the tuned cell declared, re-derived from its label so the
/// assertion above compares the *request* against the *bitstream* rather
/// than a config field against itself.
fn requested_chroma(tuned: &zenavif::AvifTune) -> EncodeChromaSubsampling {
    if tuned.cell_label().contains("chroma=420") {
        EncodeChromaSubsampling::Yuv420
    } else if tuned.cell_label().contains("chroma=444") {
        EncodeChromaSubsampling::Yuv444
    } else {
        EncodeChromaSubsampling::default()
    }
}

/// Every fixture must encode to a file that decodes back to the right
/// dimensions — the minimum bar for "the tuner returned a usable config".
#[test]
fn tuned_encodes_round_trip_to_the_source_dimensions() {
    let tuner = StubTuner::new();
    for (name, rgb, image) in fixtures() {
        let req = TuneRequest::new(QualityTarget::Zensim(85.0), W, H);
        let tuned = tuner.tune(&rgb, None, &req).expect("a pick");
        let avif = encode(tuned.config(), &image);
        let decoded = zenavif::decode(&avif).expect("decode");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (W, H),
            "{name}: round-trip changed dimensions"
        );
    }
}

// ── Matched-quality byte comparison ─────────────────────────────────

/// Compare the tuner's pick against zenavif's plain default at the same
/// requested quality, on every fixture, and record the deltas.
///
/// The stub deliberately sets no knobs on zenravif — the campaign
/// certified none as a blind default — so on a zenravif build this is a
/// **non-regression** gate, not a win gate, and it is written to say so.
/// It exists to catch the failure it can actually catch: a tuner that
/// returns a config which encodes *worse* than doing nothing.
///
/// MEASURED at the time of writing: photo 1611 B, screen 916 B, scan
/// 5538 B — **+0.00% on all three, byte-identical to the plain default**.
/// That is the right answer for the stub, and it doubles as proof that
/// no knob leaks into the config by accident. A real bake is what turns
/// this into a win gate; when one lands, tighten the bound to the effect
/// it was selected for.
#[test]
fn tuned_pick_does_not_regress_bytes_against_the_plain_default() {
    let tuner = StubTuner::new();
    for (name, rgb, image) in fixtures() {
        let req = TuneRequest::new(QualityTarget::Zensim(80.0), W, H)
            .with_allowed_backends(AllowedBackends::none().with(Av1Backend::Zenravif));
        let tuned = tuner.tune(&rgb, None, &req).expect("a pick");

        let tuned_bytes = encode(tuned.config(), &image).len();
        let default_bytes = encode(&EncoderConfig::new().quality(80.0).speed(6), &image).len();

        let delta_pct =
            (tuned_bytes as f64 - default_bytes as f64) / (default_bytes as f64) * 100.0;
        println!(
            "{name}: tuned {tuned_bytes} B vs default {default_bytes} B ({delta_pct:+.2}%) \
             cell={:?}",
            tuned.cell_label()
        );
        assert!(
            delta_pct <= 0.5,
            "{name}: the tuner's pick costs {delta_pct:+.2}% more than the plain \
             default — a tuner must never be worse than doing nothing"
        );
    }
}

// ── The swap-in path, exercised before the real bake exists ─────────

/// The bake-driven path is reachable today: this proves
/// [`zenavif::AvifTuner`] loads a contract-carrying ZNPR, picks a cell,
/// and produces a config that encodes — so swapping the training lane's
/// bake in is a one-line change and not a discovery.
///
/// Uses a hand-baked two-cell model rather than the real bake, which
/// does not exist yet. What it gates is the *plumbing*: contract
/// validation, the forward pass, the argmin, and config construction.
#[test]
fn a_contract_carrying_bake_drives_the_model_path_end_to_end() {
    let Some(bytes) = tiny_bake::two_cell_bake() else {
        // The bake helper is only compiled when zenpredict-bake is
        // available as a dev-dependency; its absence is a build
        // configuration, reported loudly rather than passed silently.
        panic!(
            "tiny_bake::two_cell_bake() returned None — the ZNPR bake helper is \
             unavailable, so the model path is UNTESTED in this build"
        );
    };
    let tuner = zenavif::AvifTuner::from_bytes(&bytes).expect("the bake must carry a contract");
    assert_eq!(tuner.contract().cells().len(), 2);
    assert!(
        tuner.caller_input_width() >= 2,
        "the contract's input width includes zq_norm"
    );

    let rgb = fixture_screen();
    let image = img(&rgb);
    let req = TuneRequest::new(QualityTarget::Zensim(80.0), W, H)
        .with_allowed_backends(AllowedBackends::none().with(Av1Backend::Zenravif));
    let tuned = tuner.tune(&rgb, None, &req).expect("a model pick");
    assert_eq!(tuned.source(), TuneSource::Model);
    assert!(
        tuned.expected_bytes().is_some(),
        "a bytes_log head must yield an expected size"
    );

    let avif = encode(tuned.config(), &image);
    assert!(!avif.is_empty());
    let _ = readback_chroma(&avif);
}

mod tiny_bake;
