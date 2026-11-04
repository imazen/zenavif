//! Every public codec entry path must complete inside a 512 KiB caller stack.
//!
//! Hosts that run codec work on bounded stacks (rayon workers sized below the
//! 2 MiB default, FFI/embedder threads, wasm) need a hard guarantee: a decode
//! or encode call must not overflow ~512 KiB of stack. Historically this was
//! not true — rav1d-safe's `rav1d_open` peaked near 0.5 MiB on context
//! construction (its issue #15), and the fix pattern is initial heap
//! allocation (`vec![..].into_boxed_slice().try_into()` for `Box<[T; N]>`,
//! zerocopy `new_box_zeroed` for big structs) instead of `Box::new([..; N])`,
//! which materializes the whole array on the stack first.
//!
//! Each `#[test]` runs the codec call on a thread spawned with
//! `stack_size(ZENAVIF_STACK_KB)` (default 512). A stack overflow aborts the
//! process, so under `cargo test` one failing case kills the test binary —
//! run per-case (`cargo test --test stack_headroom <name>`) or under nextest
//! (process per test). `ZENAVIF_STACK_KB` exists for bisecting the true peak
//! when a case fails; the committed gate stays at 512.
//!
//! Measured minima (x86_64, this workspace's optimized+debuginfo test
//! profile, 16 KiB bisect granularity, 2026-11):
//!
//! | path                                          | min KiB |
//! |-----------------------------------------------|--------:|
//! | zenav1-svt encode_rgb8                        |    ~341 |
//! | rav1d-safe decode (every case below)          |    ~310 |
//! | zenav1-aom container decode                   |    ~310 |
//! | zenav1-svt encode_gray8                       |    ~264 |
//! | zenav1-aom encode (rgb8/rgba8/gray8)          |    ~248 |
//! | zenravif encode (all four cases)              |    ~248 |
//! | zenav1-aom raw-OBU decode                     |    ~171 |
//!
//! The AOM encode path peaked ~419 KiB before zenav1-aom `10be4beb` boxed
//! `CoeffCostSet::by_txs_ctx` (RealCosts 44 -> ~5 KiB, twice live in
//! `encode_key_frame_with`, rebuilt per SB in the pack loop). Remaining
//! stack-heavy sites identified but not yet heap-first: rav1d-safe's
//! `Rav1dFrameContext::default` (~86 KiB by value per frame context in
//! `rav1d_open`), zenrav1e's `FrameData` (~29 KiB through
//! `BTreeMap::entry`), zenavif's `encoder_svt_rs::encode_rgb8_frame`
//! (~49 KiB frame), zenav1-svt's `pipeline/ra.rs` 16 KiB table box.
//!
//! Input generation and file reads run on the parent thread: the contract is
//! about the codec call, not our fixtures.

use std::thread;

#[cfg(feature = "encode")]
use almost_enough::StopToken;
#[cfg(feature = "encode")]
use imgref::{Img, ImgVec};
#[cfg(feature = "encode")]
use rgb::{Rgb, Rgba};
use zenavif::{
    DecoderConfig, Unstoppable, decode_animation_with, decode_av1_obu_yuv, decode_with,
};

fn stack_size() -> usize {
    std::env::var("ZENAVIF_STACK_KB")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(512)
        * 1024
}

/// Run `f` on a thread whose stack is limited to `stack_size()` bytes.
///
/// A panic inside `f` propagates as a test failure; a stack overflow aborts
/// the process (the failure this harness exists to surface).
fn on_limited_stack<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    thread::Builder::new()
        .stack_size(stack_size())
        .spawn(f)
        .expect("spawn limited-stack thread")
        .join()
        .unwrap_or_else(|e| {
            let msg = e
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| e.downcast_ref::<&'static str>().copied())
                .unwrap_or("<non-string panic>");
            panic!("codec call panicked on a limited stack: {msg}");
        })
}

#[cfg(feature = "encode")]
fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

/// Deterministic photo-ish content: gradients plus LCG noise so quantizers and
/// predictors do real work at every quality tier.
#[cfg(feature = "encode")]
fn rgb_image(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let mut state = 0x2545_F491u32;
    let mut lcg = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 24) as u8
    };
    let mut buf = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let g = ((x * 255) / w.max(1)) as u8;
            let b = ((y * 255) / h.max(1)) as u8;
            let n = lcg() / 8;
            buf.push(Rgb {
                r: g.wrapping_add(n),
                g: b.wrapping_add(n / 2),
                b: ((u16::from(g) + u16::from(b)) / 2) as u8,
            });
        }
    }
    Img::new(buf, w, h)
}

#[cfg(feature = "encode")]
fn rgba_image(w: usize, h: usize) -> ImgVec<Rgba<u8>> {
    let rgb = rgb_image(w, h);
    Img::new(
        rgb.pixels()
            .enumerate()
            .map(|(i, p)| Rgba {
                r: p.r,
                g: p.g,
                b: p.b,
                a: (i % 251) as u8,
            })
            .collect::<Vec<_>>(),
        w,
        h,
    )
}

#[cfg(feature = "encode-mono")]
fn gray_image(w: usize, h: usize) -> ImgVec<u8> {
    let rgb = rgb_image(w, h);
    Img::new(rgb.pixels().map(|p| p.g).collect::<Vec<_>>(), w, h)
}

/// Extract the primary item's AV1 payload from an AVIF container for the
/// raw-OBU decode seam (same shape as `cross_backend_decode::primary_payload`).
fn primary_payload(avif: &[u8]) -> Vec<u8> {
    let cfg = zenavif_parse::DecodeConfig::default();
    let parser =
        zenavif_parse::AvifParser::from_owned_with_config(avif.to_vec(), &cfg, &Unstoppable)
            .expect("container parse");
    parser
        .primary_data()
        .expect("primary item")
        .as_ref()
        .to_vec()
}

fn vector(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn decode_container(data: &[u8], config: &DecoderConfig) {
    let img = decode_with(data, config, &Unstoppable).expect("decode");
    assert!(img.width() > 0 && img.height() > 0);
}

// ---------------------------------------------------------------------------
// Decode — rav1d-safe (default backend, always built)
// ---------------------------------------------------------------------------

#[test]
fn decode_still_8bpc_rav1dsafe() {
    let files = [
        vector("tests/vectors/libavif/kodim03_yuv420_8bpc.avif"),
        vector("tests/vectors/libavif/kodim23_yuv420_8bpc.avif"),
        vector("tests/vectors/libavif/paris_icc_exif_xmp.avif"),
    ];
    on_limited_stack(move || {
        for data in &files {
            decode_container(data, &DecoderConfig::default());
        }
    });
}

#[test]
fn decode_still_hbd_rav1dsafe() {
    let files = [
        vector("tests/vectors/libavif/cosmos1650_yuv444_10bpc_p3pq.avif"),
        vector("tests/vectors/libavif/weld_sato_12B_8B_q0.avif"),
        vector("tests/vectors/libavif/colors-animated-12bpc-keyframes-0-2-3.avif"),
    ];
    on_limited_stack(move || {
        for data in &files {
            decode_container(data, &DecoderConfig::default());
        }
    });
}

#[test]
fn decode_still_alpha_grid_mono_rav1dsafe() {
    let files = [
        vector("tests/vectors/libavif/abc_color_irot_alpha_irot.avif"),
        vector("tests/vectors/libavif/alpha_noispe.avif"),
        vector("tests/vectors/libavif/sofa_grid1x5_420.avif"),
        vector("tests/vectors/libavif/seine_sdr_gainmap_srgb.avif"),
        vector("tests/vectors/zenavif/mono_gradient_10b_full.avif"),
        vector("tests/vectors/zenavif/mono_5x3_8b_full.avif"),
    ];
    on_limited_stack(move || {
        for data in &files {
            decode_container(data, &DecoderConfig::default());
        }
    });
}

#[test]
fn decode_still_multithreaded_rav1dsafe() {
    let data = vector("tests/vectors/libavif/kodim03_yuv420_8bpc.avif");
    on_limited_stack(move || {
        decode_container(&data, &DecoderConfig::new().threads(4));
    });
}

#[test]
fn decode_animation_rav1dsafe() {
    let files = [
        vector("tests/vectors/libavif/colors-animated-8bpc.avif"),
        vector("tests/vectors/libavif/colors-animated-8bpc-alpha-exif-xmp.avif"),
    ];
    on_limited_stack(move || {
        for data in &files {
            let anim = decode_animation_with(data, &DecoderConfig::default(), &Unstoppable)
                .expect("animation decode");
            assert!(!anim.frames.is_empty());
        }
    });
}

#[test]
fn decode_obu_rav1dsafe() {
    let payloads: Vec<Vec<u8>> = [
        "tests/vectors/libavif/kodim03_yuv420_8bpc.avif",
        "tests/vectors/libavif/cosmos1650_yuv444_10bpc_p3pq.avif",
    ]
    .iter()
    .map(|p| primary_payload(&vector(p)))
    .collect();
    on_limited_stack(move || {
        for payload in &payloads {
            let yuv = decode_av1_obu_yuv(payload, zenavif::DecodeBackend::Rav1dSafe)
                .expect("rav1d-safe OBU decode");
            assert!(yuv.width > 0);
        }
    });
}

// ---------------------------------------------------------------------------
// Decode — zenav1-aom (`zenav1-aom` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "zenav1-aom")]
#[test]
fn decode_obu_zenav1aom() {
    let payloads: Vec<Vec<u8>> = [
        "tests/vectors/libavif/kodim03_yuv420_8bpc.avif",
        "tests/vectors/libavif/cosmos1650_yuv444_10bpc_p3pq.avif",
    ]
    .iter()
    .map(|p| primary_payload(&vector(p)))
    .collect();
    on_limited_stack(move || {
        for payload in &payloads {
            // AVIF item payloads may start at the sequence header without a
            // temporal delimiter; prepend one on the refusal like
            // cross_backend_decode does.
            let yuv = decode_av1_obu_yuv(payload, zenavif::DecodeBackend::Zenav1Aom)
                .unwrap_or_else(|_| {
                    let mut with_td = vec![0x12, 0x00];
                    with_td.extend_from_slice(payload);
                    decode_av1_obu_yuv(&with_td, zenavif::DecodeBackend::Zenav1Aom)
                        .expect("zenav1-aom OBU decode")
                });
            assert!(yuv.width > 0);
        }
    });
}

#[cfg(feature = "zenav1-aom")]
#[test]
fn decode_still_zenav1aom() {
    let data = vector("tests/vectors/libavif/kodim03_yuv420_8bpc.avif");
    on_limited_stack(move || {
        decode_container(
            &data,
            &DecoderConfig::new().decode_backend(zenavif::DecodeBackend::Zenav1Aom),
        );
    });
}

// ---------------------------------------------------------------------------
// Encode — shared helpers
// ---------------------------------------------------------------------------

#[cfg(feature = "encode")]
mod encode_cases {
    use super::*;

    pub(super) use zenavif::{Av1Backend, EncoderConfig};

    pub(super) fn run_encode(
        backend: Av1Backend,
        cfg: impl FnOnce(EncoderConfig) -> EncoderConfig,
        w: usize,
        h: usize,
    ) {
        let img = rgb_image(w, h);
        let config = cfg(EncoderConfig::new().backend(backend));
        on_limited_stack(move || {
            let out = zenavif::encode_rgb8(img.as_ref(), &config, stop()).expect("encode_rgb8");
            assert!(!out.avif_file.is_empty());
        });
    }
}

#[cfg(feature = "encode")]
use encode_cases::{Av1Backend, EncoderConfig, run_encode};

// zenravif (default `encode` backend)
// ---------------------------------------------------------------------------

#[cfg(feature = "encode")]
#[test]
fn encode_rgb8_zenravif() {
    run_encode(Av1Backend::Zenravif, |c| c, 256, 256);
    run_encode(Av1Backend::Zenravif, |c| c, 1023, 767);
}

#[cfg(feature = "encode")]
#[test]
fn encode_rgba8_zenravif() {
    let img = rgba_image(257, 255);
    let config = EncoderConfig::new();
    on_limited_stack(move || {
        let out = zenavif::encode_rgba8(img.as_ref(), &config, stop()).expect("encode_rgba8");
        assert!(!out.avif_file.is_empty());
    });
}

#[cfg(feature = "encode-mono")]
#[test]
fn encode_gray8_zenravif() {
    let img = gray_image(257, 255);
    let config = EncoderConfig::new();
    on_limited_stack(move || {
        let out = zenavif::encode_gray8(img.as_ref(), &config, stop()).expect("encode_gray8");
        assert!(!out.avif_file.is_empty());
    });
}

#[cfg(feature = "encode")]
#[test]
fn encode_animation_zenravif() {
    let frames: Vec<zenavif::AnimationFrame> = (0..4)
        .map(|_| zenavif::AnimationFrame {
            pixels: rgb_image(129, 127),
            duration_ms: 100,
        })
        .collect();
    let config = EncoderConfig::new();
    on_limited_stack(move || {
        let out =
            zenavif::encode_animation_rgb8(&frames, &config, stop()).expect("animation encode");
        assert!(!out.avif_file.is_empty());
    });
}

// zenav1-svt (`zenav1-svt` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "zenav1-svt")]
#[test]
fn encode_rgb8_zenav1svt() {
    run_encode(
        Av1Backend::Zenav1Svt,
        |c| c.chroma_subsampling(zenavif::EncodeChromaSubsampling::Yuv420),
        256,
        256,
    );
    run_encode(
        Av1Backend::Zenav1Svt,
        |c| c.chroma_subsampling(zenavif::EncodeChromaSubsampling::Yuv420),
        257,
        255,
    );
}

#[cfg(all(feature = "zenav1-svt", feature = "encode-mono"))]
#[test]
fn encode_gray8_zenav1svt() {
    let img = gray_image(257, 255);
    let config = EncoderConfig::new()
        .backend(Av1Backend::Zenav1Svt)
        .chroma_subsampling(zenavif::EncodeChromaSubsampling::Yuv420);
    on_limited_stack(move || {
        let out = zenavif::encode_gray8(img.as_ref(), &config, stop()).expect("encode_gray8");
        assert!(!out.avif_file.is_empty());
    });
}

// zenav1-aom (`zenav1-aom-encode` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "zenav1-aom-encode")]
#[test]
fn encode_rgb8_zenav1aom() {
    run_encode(Av1Backend::Zenav1Aom, |c| c, 256, 256);
    run_encode(
        Av1Backend::Zenav1Aom,
        |c| c.chroma_subsampling(zenavif::EncodeChromaSubsampling::Yuv420),
        257,
        255,
    );
}

#[cfg(feature = "zenav1-aom-encode")]
#[test]
fn encode_rgba8_zenav1aom() {
    let img = rgba_image(257, 255);
    let config = EncoderConfig::new().backend(Av1Backend::Zenav1Aom);
    on_limited_stack(move || {
        let out = zenavif::encode_rgba8(img.as_ref(), &config, stop()).expect("encode_rgba8");
        assert!(!out.avif_file.is_empty());
    });
}

#[cfg(all(feature = "zenav1-aom-encode", feature = "encode-mono"))]
#[test]
fn encode_gray8_zenav1aom() {
    let img = gray_image(257, 255);
    let config = EncoderConfig::new().backend(Av1Backend::Zenav1Aom);
    on_limited_stack(move || {
        let out = zenavif::encode_gray8(img.as_ref(), &config, stop()).expect("encode_gray8");
        assert!(!out.avif_file.is_empty());
    });
}
