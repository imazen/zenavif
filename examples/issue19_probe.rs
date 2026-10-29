//! Quality-step probe for the zenav1-svt backend (revalidation instrument
//! for zenav1-svt issue #19: the q15 -> q16 quality-ladder inversion on
//! `090d19695a8b43c2_512sq.png` from the canonical dial corpus).
//!
//! Encodes one source at a floor-dense quality grid through the exact
//! consumer path the ladder instrument used — `zenavif::encode_rgb8` +
//! `Av1Backend::Zenav1Svt`, Yuv420, default speed 4 — decodes via
//! `zenavif::decode` (rav1d-safe), scores ssimulacra2 + butteraugli, and
//! persists every `.avif` beside a TSV of `q, qp_eff, bytes, sha256,
//! ssim2, butter_max, butter_3n`.
//!
//! By default this measures the zenav1-svt rev pinned in the manifest.
//! To measure in-flight zenav1-svt work, add the documented dev-only
//! patch to this manifest and revert before landing:
//!   [patch."https://github.com/imazen/zenav1-svt"]
//!   zenav1-svt = { path = "../zenav1-svt/rust/svtav1" }
//!
//! ```sh
//! cargo run --release --features encode-svt-rs --example issue19_probe -- \
//!   <source.png> <out_dir> [q_start q_end]
//! ```

use std::io::BufWriter;
use std::path::Path;

use almost_enough::{StopToken, Unstoppable};
use imgref::{Img, ImgRef, ImgVec};
use rgb::Rgb;
use sha2::{Digest, Sha256};
use zenavif::{Av1Backend, EncodeChromaSubsampling, EncoderConfig};

fn write_png(path: &str, img: ImgRef<'_, Rgb<u8>>) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
    let mut enc = png::Encoder::new(BufWriter::new(f), img.width() as u32, img.height() as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    let mut buf = Vec::with_capacity(img.width() * img.height() * 3);
    for row in img.rows() {
        for p in row {
            buf.extend_from_slice(&[p.r, p.g, p.b]);
        }
    }
    w.write_image_data(&buf).map_err(|e| e.to_string())
}

fn load_rgb8(path: &Path) -> Result<ImgVec<Rgb<u8>>, String> {
    let img = image::open(path).map_err(|e| format!("open {path:?}: {e}"))?;
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let buf: Vec<Rgb<u8>> = rgb
        .pixels()
        .map(|p| Rgb {
            r: p[0],
            g: p[1],
            b: p[2],
        })
        .collect();
    Ok(Img::new(buf, w, h))
}

fn tri(src: ImgRef<'_, Rgb<u8>>) -> ImgVec<[u8; 3]> {
    let mut out = Vec::with_capacity(src.width() * src.height());
    for row in src.rows() {
        out.extend(row.iter().map(|p| [p.r, p.g, p.b]));
    }
    Img::new(out, src.width(), src.height())
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let src_path = args
        .next()
        .ok_or("usage: issue19_probe <png> <out_dir> [q0 q1]")?;
    let out_dir = args
        .next()
        .ok_or("usage: issue19_probe <png> <out_dir> [q0 q1]")?;
    let q0: f32 = args.next().map_or(13.0, |s| s.parse().expect("q0"));
    let q1: f32 = args.next().map_or(18.0, |s| s.parse().expect("q1"));

    let reference = load_rgb8(Path::new(&src_path))?;
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("mkdir {out_dir}: {e}"))?;

    let src_tri = tri(reference.as_ref());
    let ba_params = butteraugli::ButteraugliParams::new();

    println!(
        "image={src_path} {}x{}",
        reference.width(),
        reference.height()
    );
    println!("q\tqp_eff\tbytes\tsha256_16\tssim2\tbutter_max\tbutter_3n\tdistinct");
    let mut prev_hash = String::new();
    let mut rows = String::from("q\tqp_eff\tbytes\tsha256\tssim2\tbutter_max\tbutter_3n\n");
    let mut q = q0;
    while q <= q1 + 0.01 {
        let cfg = EncoderConfig::new()
            .quality(q)
            .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
            .backend(Av1Backend::Zenav1Svt);
        let enc = zenavif::encode_rgb8(reference.as_ref(), &cfg, StopToken::new(Unstoppable))
            .map_err(|e| format!("q{q}: encode: {e}"))?;
        let qp_eff = svtav1::avif::AvifEncoder::quality_to_qp_static(q).max(1);
        let hash: String = Sha256::digest(&enc.avif_file)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect();
        std::fs::write(format!("{out_dir}/q{q:02.0}.avif"), &enc.avif_file)
            .map_err(|e| format!("q{q}: write: {e}"))?;

        let decoded = zenavif::decode(&enc.avif_file).map_err(|e| format!("q{q}: decode: {e}"))?;
        let dec_img: ImgRef<'_, Rgb<u8>> = decoded
            .try_as_imgref::<Rgb<u8>>()
            .ok_or("decoded image not RGB8-viewable")?;
        write_png(&format!("{out_dir}/q{q:02.0}.dec.png"), dec_img)?;
        let ssim2 = fast_ssim2::compute_ssimulacra2(src_tri.as_ref(), tri(dec_img).as_ref())
            .map_err(|e| format!("q{q}: ssim2: {e}"))?;
        let ba = butteraugli::butteraugli(reference.as_ref(), dec_img, &ba_params)
            .map_err(|e| format!("q{q}: butteraugli: {e}"))?;

        let distinct = if hash != prev_hash { "yes" } else { "=prev" };
        println!(
            "{q:5.1}\t{qp_eff}\t{}\t{}\t{ssim2:.2}\t{:.4}\t{:.4}\t{distinct}",
            enc.avif_file.len(),
            &hash[..16],
            ba.score,
            ba.pnorm_3,
        );
        rows.push_str(&format!(
            "{q:.1}\t{qp_eff}\t{}\t{}\t{ssim2:.4}\t{:.6}\t{:.6}\n",
            enc.avif_file.len(),
            hash,
            ba.score,
            ba.pnorm_3,
        ));
        prev_hash = hash;
        q += 1.0;
    }
    std::fs::write(format!("{out_dir}/probe.tsv"), rows).map_err(|e| format!("tsv: {e}"))?;
    Ok(())
}
