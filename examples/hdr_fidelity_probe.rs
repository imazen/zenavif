//! Quick probe: PQ10 encode/decode max/mean error vs quality.
//! Usage: cargo run --release --features encode --example hdr_fidelity_probe
use almost_enough::{StopToken, Unstoppable};
use imgref::Img;
use rgb::Rgb;
use zenavif::{ColorPrimaries, EncoderConfig, ManagedAvifDecoder, TransferCharacteristics};

fn make_hdr16() -> Img<Vec<Rgb<u16>>> {
    let (w, h) = (64usize, 48usize);
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let p = if y < 16 {
                let v = (x * 65535 / (w - 1)) as u16;
                Rgb { r: v, g: v, b: v }
            } else if y < 32 {
                match x / 16 {
                    0 => Rgb {
                        r: 60000,
                        g: 4000,
                        b: 4000,
                    },
                    1 => Rgb {
                        r: 4000,
                        g: 60000,
                        b: 4000,
                    },
                    2 => Rgb {
                        r: 4000,
                        g: 4000,
                        b: 60000,
                    },
                    _ => Rgb {
                        r: 62000,
                        g: 62000,
                        b: 62000,
                    },
                }
            } else if x % 13 == 0 && y % 5 == 0 {
                Rgb {
                    r: 65535,
                    g: 65535,
                    b: 65535,
                }
            } else {
                let v = 1200 + ((x * 7 + y * 11) % 64) as u16 * 8;
                Rgb {
                    r: v,
                    g: v / 2,
                    b: v / 3,
                }
            };
            px.push(p);
        }
    }
    Img::new(px, w, h)
}

fn main() {
    let img = make_hdr16();
    for (q, s) in [(80.0, 8), (90.0, 8), (95.0, 8), (99.0, 8), (90.0, 4)] {
        let cfg = EncoderConfig::new()
            .quality(q)
            .speed(s)
            .color_primaries(ColorPrimaries::BT2020.0)
            .transfer_characteristics(TransferCharacteristics::SMPTE2084.0);
        let enc = zenavif::encode_rgb16(img.as_ref(), &cfg, StopToken::new(Unstoppable)).unwrap();
        let dcfg = zenavif::DecoderConfig::new().prefer_8bit(false);
        let mut dec = ManagedAvifDecoder::new(&enc.avif_file, &dcfg).unwrap();
        let (pixels, _info) = dec.decode_full(&Unstoppable).unwrap();
        let out = pixels.try_as_imgref::<Rgb<u16>>().unwrap();
        let (mut maxd, mut sum) = (0u32, 0u64);
        for (a, b) in img.pixels().zip(out.pixels()) {
            for (va, vb) in [(a.r, b.r), (a.g, b.g), (a.b, b.b)] {
                let d = (va as i32 - vb as i32).unsigned_abs();
                maxd = maxd.max(d);
                sum += d as u64;
            }
        }
        let n = (img.width() * img.height() * 3) as u64;
        println!(
            "q{q} s{s}: {} bytes, max |d| = {maxd} ({} 10-bit steps), mean |d| = {}",
            enc.avif_file.len(),
            maxd / 64,
            sum / n
        );
    }
}
