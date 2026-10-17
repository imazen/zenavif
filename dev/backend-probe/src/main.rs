//! Exploratory still-image probe of the three AV1 encoders at their latest
//! heads: 768x512 4:2:0 photos at 8, 10 and 12 bits, four quantizer points
//! (qindex about 60/100/140/180), one thread. Writes each temporal unit to
//! out/ and one CSV row per cell (refusals included); PSNR comes from a
//! separate decode with FFmpeg's libdav1d.
use std::time::Instant;

const W: usize = 768;
const H: usize = 512;

fn planes(img: usize, bits: u8) -> (Vec<u16>, Vec<u16>, Vec<u16>) {
    let fmt = match bits { 8 => "yuv420p", 10 => "yuv420p10le", _ => "yuv420p12le" };
    let raw = std::fs::read(format!("in/img{img}.{fmt}.yuv")).unwrap();
    let words: Vec<u16> = if bits == 8 {
        raw.iter().map(|&b| u16::from(b)).collect()
    } else {
        raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
    };
    let (ys, cs) = (W * H, W * H / 4);
    (words[..ys].to_vec(), words[ys..ys + cs].to_vec(), words[ys + cs..ys + 2 * cs].to_vec())
}

fn rav1e<P: zenrav1e::prelude::Pixel>(bits: u8, q: usize, p: &(Vec<u16>, Vec<u16>, Vec<u16>)) -> Result<Vec<u8>, String> {
    use zenrav1e::prelude::*;
    let mut enc = EncoderConfig::with_speed_preset(6);
    enc.width = W;
    enc.height = H;
    enc.bit_depth = usize::from(bits);
    enc.chroma_sampling = ChromaSampling::Cs420;
    enc.quantizer = q;
    enc.min_quantizer = 0;
    enc.still_picture = true;
    let cfg = Config::new().with_encoder_config(enc).with_threads(1);
    let mut ctx: Context<P> = cfg.new_context().map_err(|e| format!("{e:?}"))?;
    let mut f = ctx.new_frame();
    for (i, plane) in [&p.0, &p.1, &p.2].into_iter().enumerate() {
        let w = if i == 0 { W } else { W / 2 };
        let mut s = f.planes[i].mut_slice(Default::default());
        for (y, row) in s.rows_iter_mut().take(if i == 0 { H } else { H / 2 }).enumerate() {
            for x in 0..w {
                row[x] = P::cast_from(plane[y * w + x]);
            }
        }
    }
    ctx.send_frame(f).map_err(|e| format!("{e:?}"))?;
    ctx.flush();
    let mut out = Vec::new();
    loop {
        match ctx.receive_packet() {
            Ok(pk) => out.extend_from_slice(&pk.data),
            Err(EncoderStatus::Encoded) => {}
            Err(EncoderStatus::LimitReached) => break,
            Err(e) => return Err(format!("{e:?}")),
        }
    }
    Ok(out)
}

fn svt(bits: u8, qp: u8, p: &(Vec<u16>, Vec<u16>, Vec<u16>)) -> Result<Vec<u8>, String> {
    use svtav1::pipeline::{ChromaFormat, EncodePipeline, RcConfig, RcMode};
    let rc = RcConfig { mode: RcMode::Cqp, qp, ..RcConfig::default() };
    let preset = svtav1::avif::NativePreset::new(6).ok_or("preset")?;
    let mut pl = EncodePipeline::new_with_preset(W as u32, H as u32, preset, rc, 0, 1)
        .with_chroma_format(Some(ChromaFormat::Yuv420))
        .with_bit_depth(bits)
        .with_thread_count(1);
    let r = if bits == 8 {
        let n = |v: &Vec<u16>| v.iter().map(|&x| x as u8).collect::<Vec<u8>>();
        pl.try_encode_frame_420(&n(&p.0), &n(&p.1), &n(&p.2), W)
    } else {
        pl.try_encode_frame_420_hbd(&p.0, &p.1, &p.2, W)
    };
    r.map_err(|e| format!("{e}").lines().next().unwrap_or("").to_string())
}

fn aom(bits: u8, cq: i32, p: &(Vec<u16>, Vec<u16>, Vec<u16>)) -> Result<Vec<u8>, String> {
    use aom_encode::key_frame::{KeyFrameConfig, KeyFramePlanes, encode_key_frame};
    let mut cfg = KeyFrameConfig::allintra_speed0(W, H, bits, false, 1, 1, cq);
    cfg.cpu_used = 6;
    cfg.enable_restoration = true;
    encode_key_frame(KeyFramePlanes::new(&p.0, &p.1, &p.2), &cfg).map_err(|e| format!("{e:?}"))
}

fn main() {
    std::fs::create_dir_all("out").unwrap();
    println!("img,bits,backend,qindex,bytes,ms,status");
    for img in 1..=4 {
        for bits in [8u8, 10, 12] {
            let p = planes(img, bits);
            for qi in [60usize, 100, 140, 180] {
                for backend in ["zenrav1e", "zenav1-svt", "zenav1-aom"] {
                    let t = Instant::now();
                    let r = match backend {
                        "zenrav1e" if bits == 8 => rav1e::<u8>(bits, qi, &p),
                        "zenrav1e" => rav1e::<u16>(bits, qi, &p),
                        "zenav1-svt" => svt(bits, (qi / 4) as u8, &p),
                        _ => aom(bits, (qi / 4) as i32, &p),
                    };
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    match r {
                        Ok(b) => {
                            std::fs::write(format!("out/{img}_{bits}_{backend}_{qi}.obu"), &b).unwrap();
                            println!("{img},{bits},{backend},{qi},{},{ms:.1},ok", b.len());
                        }
                        Err(e) => println!("{img},{bits},{backend},{qi},,,refused: {}", e.replace(',', ";")),
                    }
                }
            }
        }
    }
}
