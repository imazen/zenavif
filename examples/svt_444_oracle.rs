//! Independent decoder checks for source-lossless SVT RGB/RGBA stills.
//! Run via `just test-svt-444-oracle <output-directory>`.
use almost_enough::{StopToken, Unstoppable};
use imgref::ImgRef;
use rgb::{Rgb, Rgba};
use std::{fs, path::Path, process::Command};
use zenavif::{Av1Backend, EncodeChromaSubsampling, EncodeColorModel, EncoderConfig};

fn raw_oracles(dir: &Path, name: &str, obu: &[u8], expected: &[u8]) {
    let input = dir.join(format!("{name}.obu"));
    let aom = dir.join(format!("{name}.aom.yuv"));
    let dav = dir.join(format!("{name}.dav.yuv"));
    fs::write(&input, obu).unwrap();
    assert!(
        Command::new("aomdec")
            .arg("--rawvideo")
            .arg("-o")
            .arg(&aom)
            .arg(&input)
            .status()
            .expect("aomdec required")
            .success()
    );
    assert!(
        Command::new("dav1d")
            .arg("-i")
            .arg(&input)
            .arg("-o")
            .arg(&dav)
            .status()
            .expect("dav1d required")
            .success()
    );
    assert_eq!(
        fs::read(&aom).unwrap(),
        expected,
        "aomdec source samples: {name}"
    );
    assert_eq!(
        fs::read(&dav).unwrap(),
        expected,
        "dav1d source samples: {name}"
    );
}
fn check(
    dir: &Path,
    name: &str,
    avif: &[u8],
    source: &[Rgba<u8>],
    alpha: bool,
    w: usize,
    h: usize,
) {
    let input = dir.join(format!("{name}.avif"));
    let png_path = dir.join(format!("{name}.png"));
    fs::write(&input, avif).unwrap();
    assert!(
        Command::new("avifdec")
            .args(["--depth", "8"])
            .arg(&input)
            .arg(&png_path)
            .status()
            .expect("avifdec required")
            .success()
    );
    let mut reader = png::Decoder::new(std::io::BufReader::new(fs::File::open(&png_path).unwrap()))
        .read_info()
        .unwrap();
    let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut bytes).unwrap();
    assert_eq!((info.width as usize, info.height as usize), (w, h));
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    let channels = if alpha { 4 } else { 3 };
    assert_eq!(
        info.color_type,
        if alpha {
            png::ColorType::Rgba
        } else {
            png::ColorType::Rgb
        }
    );
    let expected: Vec<u8> = source
        .iter()
        .flat_map(|p| [p.r, p.g, p.b, p.a].into_iter().take(channels))
        .collect();
    assert_eq!(
        &bytes[..info.buffer_size()],
        expected,
        "libavif RGB(A), including transparent pixels: {name}"
    );
    let parser = zenavif_parse::AvifParser::from_owned_with_config(
        avif.to_vec(),
        &Default::default(),
        &Unstoppable,
    )
    .unwrap();
    let primary = parser.primary_data().unwrap();
    let mut gbr = Vec::with_capacity(source.len() * 3);
    gbr.extend(source.iter().map(|p| p.g));
    gbr.extend(source.iter().map(|p| p.b));
    gbr.extend(source.iter().map(|p| p.r));
    raw_oracles(dir, name, &primary, &gbr);
    if alpha {
        let a = parser.alpha_data().expect("alpha item").unwrap();
        raw_oracles(
            dir,
            &format!("{name}-alpha"),
            &a,
            &source.iter().map(|p| p.a).collect::<Vec<_>>(),
        );
    }
    println!("PASS {name}: libavif RGB(A), aomdec/dav1d planes, profile-1 color + profile-0 alpha");
}
fn main() {
    let output = std::env::args().nth(1).expect("output directory required");
    let dir = Path::new(&output);
    fs::create_dir_all(dir).unwrap();
    for (w, h) in [(1, 1), (7, 9), (65, 67)] {
        let source: Vec<_> = (0..w * h)
            .map(|i| Rgba {
                r: (i * 37) as u8,
                g: (i * 19 + i / 7) as u8,
                b: (i * 73 + i / 11) as u8,
                a: (i * 41) as u8,
            })
            .collect();
        let rgb: Vec<_> = source
            .iter()
            .map(|p| Rgb {
                r: p.r,
                g: p.g,
                b: p.b,
            })
            .collect();
        let config = EncoderConfig::new()
            .backend(Av1Backend::Zenav1Svt)
            .chroma_subsampling(EncodeChromaSubsampling::Yuv444)
            .color_model(EncodeColorModel::Rgb)
            .with_lossless(true)
            .speed(10)
            .threads(Some(1));
        let encoded = zenavif::encode_rgb8(
            ImgRef::new(&rgb, w, h),
            &config,
            StopToken::new(Unstoppable),
        )
        .unwrap();
        check(
            dir,
            &format!("rgb-{w}x{h}"),
            &encoded.avif_file,
            &source,
            false,
            w,
            h,
        );
        let encoded = zenavif::encode_rgba8(
            ImgRef::new(&source, w, h),
            &config,
            StopToken::new(Unstoppable),
        )
        .unwrap();
        check(
            dir,
            &format!("rgba-{w}x{h}"),
            &encoded.avif_file,
            &source,
            true,
            w,
            h,
        );
    }
}
