//! Writes one AVIF per coded depth so an EXTERNAL decoder (macOS `sips`,
//! `file(1)`, anything not in this workspace) can be pointed at them.
//!
//!   CARGO_TARGET_DIR=../../target cargo run --release --bin emit -- <out-dir>
use imgref::{Img, ImgVec};
use rgb::Rgb;
use zenavif::{Av1Backend, EncodeBitDepth, EncodeChromaSubsampling, EncoderConfig};

fn stop() -> almost_enough::StopToken {
    almost_enough::StopToken::new(almost_enough::Unstoppable)
}

fn grad16(w: usize, h: usize) -> ImgVec<Rgb<u16>> {
    let mut b = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            b.push(Rgb {
                r: ((x * 65535) / w.max(1)) as u16,
                g: ((y * 65535) / h.max(1)) as u16,
                b: (((x + y) * 32767) / (w + h).max(1)) as u16,
            });
        }
    }
    Img::new(b, w, h)
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: emit <out-dir>");
    let src = grad16(192, 128);
    for (depth, tag) in [
        (EncodeBitDepth::Eight, "bd8"),
        (EncodeBitDepth::Ten, "bd10"),
        (EncodeBitDepth::Twelve, "bd12"),
    ] {
        let cfg = EncoderConfig::new()
            .backend(Av1Backend::Zenav1Aom)
            .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
            .bit_depth(depth)
            .quality(88.0)
            .speed(5);
        let out = zenavif::encode_rgb16(src.as_ref(), &cfg, stop()).expect("encode");
        let p = format!("{dir}/{tag}.avif");
        std::fs::write(&p, &out.avif_file).expect("write");
        println!("{p}\t{} bytes", out.avif_file.len());
    }
    let mut ppm = format!("P6\n{} {}\n255\n", src.width(), src.height()).into_bytes();
    for p in src.buf() {
        ppm.extend_from_slice(&[(p.r >> 8) as u8, (p.g >> 8) as u8, (p.b >> 8) as u8]);
    }
    std::fs::write(format!("{dir}/source.ppm"), ppm).expect("write ppm");
}
