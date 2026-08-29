//! Decode-backend speed: rav1d-safe vs aom-rs on zenavif-encoded bitstreams.
//!
//! Unlike `decode_4way_bench` (which reads the aomenc conformance corpus
//! from disk), this bench is self-contained: it encodes its own cells with
//! the zenravif backend from corpus PNGs (mosaics at 1 and 4 Mpx, three
//! quality tiers, 4:2:0 + 4:4:4 + a 10-bit cell), extracts the AV1 OBU
//! payloads, verifies both backends decode them byte-identically, then
//! times `decode_av1_obu_yuv` per backend with interleaved round-robin
//! repetitions (min-of-reps reported).
//!
//! Usage:
//! ```text
//! cargo run --release --example decode_backend_bench \
//!   --features encode,aom-backend -- \
//!   [corpus_dir] [out_csv] [reps]
//! ```

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use almost_enough::{StopToken, Unstoppable};
use imgref::{Img, ImgRef, ImgVec};
use rgb::Rgb;

use zenavif::{
    Av1Backend, DecodeBackend, EncodeChromaSubsampling, EncoderConfig, decode_av1_obu_yuv,
};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

fn load_rgb8(path: &PathBuf) -> ImgVec<Rgb<u8>> {
    let img = image::open(path).expect("png").to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let buf: Vec<Rgb<u8>> = img
        .pixels()
        .map(|p| Rgb {
            r: p.0[0],
            g: p.0[1],
            b: p.0[2],
        })
        .collect();
    Img::new(buf, w, h)
}

fn mosaic(tiles: &[ImgVec<Rgb<u8>>], nx: usize, ny: usize) -> ImgVec<Rgb<u8>> {
    let t = tiles[0].width();
    let (w, h) = (t * nx, t * ny);
    let mut buf = vec![Rgb { r: 0, g: 0, b: 0 }; w * h];
    for i in 0..nx * ny {
        let tile = &tiles[i % tiles.len()];
        let (tx, ty) = ((i % nx) * t, (i / nx) * t);
        for (y, row) in tile.rows().enumerate() {
            let dst = (ty + y) * w + tx;
            buf[dst..dst + t].copy_from_slice(row);
        }
    }
    Img::new(buf, w, h)
}

fn primary_payload(avif: &[u8]) -> Vec<u8> {
    // Lenient on purpose: a corpus file with a container quirk should still
    // yield a measurement cell rather than dropping out of the sweep. Production
    // decode is strict -- see tests/parser_leniency_scope.rs.
    let cfg = zenavif_parse::DecodeConfig::default().lenient(true);
    let parser =
        zenavif_parse::AvifParser::from_owned_with_config(avif.to_vec(), &cfg, &Unstoppable)
            .expect("container parse");
    let mut payload = parser
        .primary_data()
        .expect("primary item")
        .as_ref()
        .to_vec();
    // aom-rs wants a full temporal unit; prepend a TD OBU if absent.
    if payload.first().map(|b| b >> 3 & 0xf) != Some(2) {
        let mut with_td = vec![0x12, 0x00];
        with_td.append(&mut payload);
        payload = with_td;
    }
    payload
}

struct BenchCell {
    name: String,
    width: usize,
    height: usize,
    payload: Vec<u8>,
}

fn encode_cell(
    name: &str,
    img: ImgRef<'_, Rgb<u8>>,
    quality: f32,
    subsampling: EncodeChromaSubsampling,
) -> BenchCell {
    let config = EncoderConfig::new()
        .quality(quality)
        .speed(6)
        .chroma_subsampling(subsampling)
        .backend(Av1Backend::Zenravif);
    let enc = zenavif::encode_rgb8(img, &config, stop()).expect("encode");
    BenchCell {
        name: name.to_string(),
        width: img.width(),
        height: img.height(),
        payload: primary_payload(&enc.avif_file),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let corpus = PathBuf::from(
        args.get(1)
            .map(String::as_str)
            .unwrap_or("/root/codec-corpus/CID22/CID22-512/validation"),
    );
    let out_csv = PathBuf::from(
        args.get(2)
            .map(String::as_str)
            .unwrap_or("/tmp/decode_backend_bench.csv"),
    );
    let reps: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(7);

    let mut paths: Vec<PathBuf> = fs::read_dir(&corpus)
        .expect("corpus dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
        .collect();
    paths.sort();
    paths.truncate(16);
    let tiles: Vec<ImgVec<Rgb<u8>>> = paths.iter().map(load_rgb8).collect();
    assert!(tiles.len() >= 4, "need at least 4 corpus images");

    let m1 = mosaic(&tiles, 2, 2); // 1024x1024, ~1 Mpx
    let m4 = mosaic(&tiles, 4, 4); // 2048x2048, ~4.2 Mpx

    eprintln!("encoding bench cells (zenravif s6)...");
    let mut cells = vec![
        encode_cell(
            "1Mpx-420-q30",
            m1.as_ref(),
            30.0,
            EncodeChromaSubsampling::Yuv420,
        ),
        encode_cell(
            "1Mpx-420-q60",
            m1.as_ref(),
            60.0,
            EncodeChromaSubsampling::Yuv420,
        ),
        encode_cell(
            "1Mpx-420-q85",
            m1.as_ref(),
            85.0,
            EncodeChromaSubsampling::Yuv420,
        ),
        encode_cell(
            "1Mpx-444-q85",
            m1.as_ref(),
            85.0,
            EncodeChromaSubsampling::Yuv444,
        ),
        encode_cell(
            "4Mpx-420-q30",
            m4.as_ref(),
            30.0,
            EncodeChromaSubsampling::Yuv420,
        ),
        encode_cell(
            "4Mpx-420-q60",
            m4.as_ref(),
            60.0,
            EncodeChromaSubsampling::Yuv420,
        ),
        encode_cell(
            "4Mpx-420-q85",
            m4.as_ref(),
            85.0,
            EncodeChromaSubsampling::Yuv420,
        ),
        encode_cell(
            "4Mpx-444-q85",
            m4.as_ref(),
            85.0,
            EncodeChromaSubsampling::Yuv444,
        ),
    ];

    // One 10-bit cell (zenravif encode_rgb16 at EncodeBitDepth::Ten).
    {
        let buf16: Vec<rgb::Rgb<u16>> = m1
            .buf()
            .iter()
            .map(|p| rgb::Rgb {
                r: (p.r as u16) << 8 | p.r as u16,
                g: (p.g as u16) << 8 | p.g as u16,
                b: (p.b as u16) << 8 | p.b as u16,
            })
            .collect();
        let img16 = Img::new(buf16, m1.width(), m1.height());
        let config = EncoderConfig::new()
            .quality(60.0)
            .speed(6)
            .bit_depth(zenavif::EncodeBitDepth::Ten);
        let enc = zenavif::encode_rgb16(img16.as_ref(), &config, stop()).expect("10-bit encode");
        cells.push(BenchCell {
            name: "1Mpx-b10-444-q60".to_string(),
            width: m1.width(),
            height: m1.height(),
            payload: primary_payload(&enc.avif_file),
        });
    }

    let backends: &[(&str, DecodeBackend)] = &[
        ("rav1d-safe", DecodeBackend::Rav1dSafe),
        ("aom-rs", DecodeBackend::AomRs),
    ];

    // Correctness gate before timing.
    for cell in &cells {
        let rav = decode_av1_obu_yuv(&cell.payload, DecodeBackend::Rav1dSafe).expect("rav1d");
        let aom = decode_av1_obu_yuv(&cell.payload, DecodeBackend::AomRs).expect("aom");
        assert_eq!(rav.y, aom.y, "{}: luma diverges", cell.name);
        assert_eq!(
            (rav.u == aom.u, rav.v == aom.v),
            (true, true),
            "{}: chroma diverges",
            cell.name
        );
        eprintln!(
            "gate {}: byte-identical ({} bytes payload)",
            cell.name,
            cell.payload.len()
        );
    }

    // Interleaved round-robin timing: rep-major, backend x cell inner.
    let mut times: Vec<Vec<Vec<f64>>> = vec![vec![Vec::new(); cells.len()]; backends.len()];
    for rep in 0..reps {
        eprintln!("rep {}/{reps}", rep + 1);
        for (bi, (_, backend)) in backends.iter().enumerate() {
            for (ci, cell) in cells.iter().enumerate() {
                let t0 = Instant::now();
                let d = decode_av1_obu_yuv(&cell.payload, *backend).expect("decode");
                let ms = t0.elapsed().as_secs_f64() * 1e3;
                std::hint::black_box(d.y.len());
                times[bi][ci].push(ms);
            }
        }
    }

    let mut f = fs::File::create(&out_csv).expect("open csv");
    writeln!(
        f,
        "cell,width,height,megapixels,decoder,min_ms,mean_ms,mpx_s_min"
    )
    .unwrap();
    for (ci, cell) in cells.iter().enumerate() {
        let mpx = (cell.width * cell.height) as f64 / 1e6;
        for (bi, (bname, _)) in backends.iter().enumerate() {
            let t = &times[bi][ci];
            let min = t.iter().cloned().fold(f64::MAX, f64::min);
            let mean = t.iter().sum::<f64>() / t.len() as f64;
            writeln!(
                f,
                "{},{},{},{:.4},{},{:.4},{:.4},{:.2}",
                cell.name,
                cell.width,
                cell.height,
                mpx,
                bname,
                min,
                mean,
                mpx / (min / 1e3),
            )
            .unwrap();
            eprintln!(
                "{:<18} {:<11} min {:>8.2} ms  mean {:>8.2} ms  {:>7.1} Mpx/s",
                cell.name,
                bname,
                min,
                mean,
                mpx / (min / 1e3),
            );
        }
    }
    eprintln!("wrote {}", out_csv.display());
}
