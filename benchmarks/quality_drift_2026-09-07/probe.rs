use imgref::{Img, ImgVec};
use rgb::Rgb;
use std::time::Instant;
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

/// Screen-like RGB: flat patches from a small palette, separators, glyphs.
fn gen_screen_rgb(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let palette = [
        Rgb {
            r: 255u8,
            g: 255,
            b: 255,
        },
        Rgb {
            r: 32,
            g: 32,
            b: 40,
        },
        Rgb {
            r: 208,
            g: 48,
            b: 48,
        },
        Rgb {
            r: 32,
            g: 128,
            b: 224,
        },
        Rgb {
            r: 240,
            g: 200,
            b: 40,
        },
        Rgb {
            r: 40,
            g: 168,
            b: 72,
        },
        Rgb {
            r: 128,
            g: 64,
            b: 192,
        },
        Rgb {
            r: 224,
            g: 224,
            b: 208,
        },
    ];
    let mut rng = Lcg::new(0x5C12);
    let pw = w.div_ceil(32);
    let ph = h.div_ceil(32);
    let patch: Vec<usize> = (0..pw * ph)
        .map(|_| (rng.next_u32() % 8) as usize)
        .collect();
    let glyph = |i: usize, j: usize| -> bool {
        let (gi, gj) = (i % 8, j % 8);
        gi == 1 || gj == 6 || (gi == gj && gi < 5)
    };
    let mut buf = Vec::with_capacity(w * h);
    for j in 0..h {
        for i in 0..w {
            let px = if i % 32 == 0 || j % 32 == 0 {
                Rgb { r: 0, g: 0, b: 0 }
            } else if (i / 32 + j / 32) % 3 == 0 && glyph(i, j) {
                Rgb {
                    r: 16,
                    g: 16,
                    b: 16,
                }
            } else {
                palette[patch[(j / 32) * pw + i / 32]]
            };
            buf.push(px);
        }
    }
    Img::new(buf, w, h)
}

/// Mixed content at odd dimensions: photo left half, sharp checker right.
fn gen_mixed_rgb(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let photo = gen_photo_rgb(w, h, 0x3333);
    let mut buf = photo.into_buf();
    for j in 0..h {
        for i in w / 2..w {
            buf[j * w + i] = if ((i / 4) + (j / 4)) % 2 == 0 {
                Rgb {
                    r: 228,
                    g: 224,
                    b: 210,
                }
            } else {
                Rgb {
                    r: 36,
                    g: 40,
                    b: 52,
                }
            };
        }
    }
    Img::new(buf, w, h)
}

struct PinnedRgb {
    name: String,
    img: ImgVec<Rgb<u8>>,
}

fn pinned_images() -> Vec<PinnedRgb> {
    vec![
        PinnedRgb {
            name: "photo".into(),
            img: gen_photo_rgb(512, 384, 0x0001),
        },
        PinnedRgb {
            name: "screen".into(),
            img: gen_screen_rgb(512, 384),
        },
        PinnedRgb {
            name: "mixed".into(),
            img: gen_mixed_rgb(509, 341),
        },
    ]
}

fn main() {
    let dir = std::env::args().nth(1).expect("output directory");
    std::fs::create_dir_all(&dir).unwrap();
    println!("cell\tbytes\tssim2\tenc_ms");
    let images = if let Ok(manifest) = std::env::var("PROBE_MANIFEST") {
        std::fs::read_to_string(manifest)
            .unwrap()
            .lines()
            .map(|line| {
                let (name, path) = line.split_once('\t').unwrap();
                let mut rgba = image::open(path).unwrap().to_rgba8();
                // An explicit white background gives every source the same opaque RGB contract.
                for p in rgba.pixels_mut() {
                    let a = u32::from(p[3]);
                    for c in 0..3 {
                        p[c] = ((u32::from(p[c]) * a + 255 * (255 - a) + 127) / 255) as u8;
                    }
                    p[3] = 255;
                }
                let mut rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
                let longest = rgb.width().max(rgb.height());
                if longest > 512 {
                    let w = (u64::from(rgb.width()) * 512 / u64::from(longest)).max(1) as u32;
                    let h = (u64::from(rgb.height()) * 512 / u64::from(longest)).max(1) as u32;
                    rgb =
                        image::imageops::resize(&rgb, w, h, image::imageops::FilterType::Triangle);
                }
                let (w, h) = (rgb.width() as usize, rgb.height() as usize);
                let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
                ppm.extend_from_slice(rgb.as_raw());
                std::fs::write(
                    std::path::Path::new(&dir).join(format!("source-{name}.ppm")),
                    ppm,
                )
                .unwrap();
                let pixels = rgb.pixels().map(|p| Rgb::new(p[0], p[1], p[2])).collect();
                PinnedRgb {
                    name: name.to_owned(),
                    img: Img::new(pixels, w, h),
                }
            })
            .collect::<Vec<_>>()
    } else {
        pinned_images()
    };
    let qualities = if std::env::var_os("PROBE_CURVE").is_some() {
        vec![5f32, 15., 25., 35., 50., 65., 80., 90., 95.]
    } else {
        vec![35f32, 60., 85.]
    };
    for img in images {
        let src = Img::new(
            img.img
                .pixels()
                .map(|p| [p.r, p.g, p.b])
                .collect::<Vec<_>>(),
            img.img.width(),
            img.img.height(),
        );
        for speed in std::env::var("PROBE_SPEED")
            .map(|s| vec![s.parse::<u8>().unwrap()])
            .unwrap_or_else(|_| vec![2, 6, 10])
        {
            for &q in &qualities {
                if std::env::var("PROBE_SPEED").is_ok_and(|wanted| wanted != speed.to_string()) {
                    continue;
                }
                let cell = format!("s{speed}/{}/q{}", img.name, q as u32);
                if std::env::var("PROBE_CELL").is_ok_and(|wanted| wanted != cell) {
                    continue;
                }
                let enc = ravif::Encoder::new()
                    .with_quality(q)
                    .with_speed(speed)
                    .with_bit_depth(ravif::BitDepth::Eight)
                    .with_internal_color_model(ravif::ColorModel::YCbCr)
                    .with_chroma_subsampling(ravif::ChromaSubsampling::Yuv420)
                    .with_num_threads(Some(8));
                let start = Instant::now();
                let encoded = enc.encode_rgb(img.img.as_ref()).unwrap();
                let ms = start.elapsed().as_secs_f64() * 1000.;
                let cfg = zenavif::DecoderConfig::new().prefer_8bit(true).threads(1);
                let decoded =
                    zenavif::decode_with(&encoded.avif_file, &cfg, &zenavif::Unstoppable).unwrap();
                let rgb = decoded.try_as_imgref::<Rgb<u8>>().unwrap();
                let dst = Img::new(
                    rgb.pixels().map(|p| [p.r, p.g, p.b]).collect::<Vec<_>>(),
                    rgb.width(),
                    rgb.height(),
                );
                let score = fast_ssim2::compute_ssimulacra2(src.as_ref(), dst.as_ref()).unwrap();
                let cell = format!("s{speed}/{}/q{}", img.name, q as u32);
                std::fs::write(
                    std::path::Path::new(&dir).join(cell.replace('/', "-") + ".avif"),
                    &encoded.avif_file,
                )
                .unwrap();
                println!("{cell}\t{}\t{score:.3}\t{ms:.1}", encoded.avif_file.len());
            }
        }
    }
}
