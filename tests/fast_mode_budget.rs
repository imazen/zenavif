#![cfg(feature = "encode-imazen")]
use imgref::{Img, ImgVec};
use rgb::Rgb;
use zenavif::{Av1Backend, EncodeBitDepth, EncodeChromaSubsampling, EncoderConfig};

// Same deterministic photo witness as the original engineering-baseline gate.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }

    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }
}

/// Photo-like RGB: smooth gradients + quadratic bumps + dense noise.
fn gen_photo_rgb(w: usize, h: usize, seed: u64) -> ImgVec<Rgb<u8>> {
    let mut rng = Lcg::new(seed);
    let mut buf = Vec::with_capacity(w * h);
    for j in 0..h {
        for i in 0..w {
            let base = 40 + (i * 160) / w + (j * 90) / h;
            let bump = (i * i) / (w * 5) + (j * j) / (h * 5) + (i * j) / (w * 8);
            let n1 = (rng.next_u32() % 17) as usize;
            let n2 = (rng.next_u32() % 17) as usize;
            let n3 = (rng.next_u32() % 17) as usize;
            let r = (base + bump + n1).min(245) as u8;
            let g = (base + bump / 2 + 12 + n2).min(245) as u8;
            let b = (30 + (i * 110) / w + (j * 140) / h + n3).min(245) as u8;
            buf.push(Rgb { r, g, b });
        }
    }
    Img::new(buf, w, h)
}

#[test]
fn fastest_photo_preserves_original_quality_and_size_budget() {
    let src = gen_photo_rgb(512, 384, 0x0001);
    let config = EncoderConfig::new()
        .backend(Av1Backend::Zenravif)
        .quality(35.)
        .speed(10)
        .bit_depth(EncodeBitDepth::Eight)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
        .threads(Some(8));
    let encoded = zenavif::encode_rgb8(
        src.as_ref(),
        &config,
        almost_enough::StopToken::new(almost_enough::Unstoppable),
    )
    .unwrap();
    let decoded = zenavif::decode_with(
        &encoded.avif_file,
        &zenavif::DecoderConfig::new().prefer_8bit(true).threads(1),
        &zenavif::Unstoppable,
    )
    .unwrap();
    let rgb = decoded.try_as_imgref::<Rgb<u8>>().unwrap();
    let original = Img::new(
        src.pixels().map(|p| [p.r, p.g, p.b]).collect::<Vec<_>>(),
        512,
        384,
    );
    let output = Img::new(
        rgb.pixels().map(|p| [p.r, p.g, p.b]).collect::<Vec<_>>(),
        512,
        384,
    );
    let score = fast_ssim2::compute_ssimulacra2(original.as_ref(), output.as_ref()).unwrap();
    // The original July baseline, with its unchanged tolerances. Improvements
    // are allowed; a faster search must not make the file larger AND worse.
    assert!(score >= 57.477 - 0.5, "photo quality regressed: {score}");
    assert!(
        encoded.avif_file.len() * 100 <= 1064 * 102,
        "photo size regressed: {}",
        encoded.avif_file.len()
    );
}
