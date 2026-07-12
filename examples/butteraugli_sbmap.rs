//! Per-superblock butteraugli pooling dump — the COOPT Phase-1 per-block
//! metric-target generator (docs/COOPT_LOOP_PLAN.md; DFIT2 routed the D-refit
//! here: per-block D must be fit against per-block METRIC, and the CLI's
//! colorized heatmap is numerically lossy, so this pools the crate's raw f32
//! diffmap directly).
//!
//! Usage:
//!   cargo run --release --features two-pass-butteraugli --example butteraugli_sbmap -- \
//!     SRC.png DEC.png SB_SIZE OUT.tsv
//!
//! Output TSV: one row per SB-aligned tile — sb_x, sb_y (tile indices),
//! mean, p3 (3-norm), max of the raw butteraugli values in that tile. The
//! join key to trace scopes is pixel position: scope bo (4px block units) →
//! tile index = (bo*4)/SB_SIZE.
use imgref::ImgVec;
use rgb::Rgb;

fn load_png_rgb(path: &std::path::Path) -> ImgVec<Rgb<u8>> {
    let img = image::open(path)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()))
        .to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px: Vec<Rgb<u8>> =
        img.pixels().map(|p| Rgb { r: p[0], g: p[1], b: p[2] }).collect();
    ImgVec::new(px, w, h)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        eprintln!("usage: butteraugli_sbmap SRC.png DEC.png SB_SIZE OUT.tsv");
        std::process::exit(2);
    }
    let src = load_png_rgb(std::path::Path::new(&args[0]));
    let dec = load_png_rgb(std::path::Path::new(&args[1]));
    let sb: usize = args[2].parse().expect("SB_SIZE");
    assert!(sb.is_power_of_two() && sb >= 16, "SB_SIZE: power of two >= 16");
    assert_eq!(
        (src.width(), src.height()),
        (dec.width(), dec.height()),
        "dimension mismatch"
    );

    let params = butteraugli::ButteraugliParams::new().with_compute_diffmap(true);
    let ba = butteraugli::butteraugli(src.as_ref(), dec.as_ref(), &params)
        .expect("butteraugli");
    let map = ba.diffmap.expect("no diffmap");
    let (w, h) = (map.width(), map.height());

    use std::io::Write;
    let mut f = std::io::BufWriter::new(
        std::fs::File::create(&args[3]).expect("out tsv"),
    );
    writeln!(
        f,
        "sb_x\tsb_y\tn_px\tmean\tp3\tmax\tmse\tsrc_var\tsrc_grad\tsrc_luma"
    )
    .unwrap();
    let tiles_x = w.div_ceil(sb);
    let tiles_y = h.div_ceil(sb);
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let (x0, y0) = (tx * sb, ty * sb);
            let (x1, y1) = ((x0 + sb).min(w), (y0 + sb).min(h));
            let mut sum = 0.0f64;
            let mut sum3 = 0.0f64;
            let mut mx = 0.0f64;
            let mut n = 0usize;
            for y in y0..y1 {
                let row = &map.rows().nth(y).unwrap()[x0..x1];
                for &v in row {
                    let v = v as f64;
                    sum += v;
                    sum3 += v * v * v;
                    if v > mx {
                        mx = v;
                    }
                    n += 1;
                }
            }
            // Per-tile raw RGB MSE — the pixel-error baseline the Phase-1
            // per-block fit compares the D currency against (DFIT3).
            let mut se = 0.0f64;
            for y in y0..y1 {
                let srow = &src.rows().nth(y).unwrap()[x0..x1];
                let drow = &dec.rows().nth(y).unwrap()[x0..x1];
                for (a, b) in srow.iter().zip(drow) {
                    for (ca, cb) in [(a.r, b.r), (a.g, b.g), (a.b, b.b)] {
                        let d = ca as f64 - cb as f64;
                        se += d * d;
                    }
                }
            }
            let mse = se / (n as f64 * 3.0);
            // Per-tile SOURCE features — the DFIT4 kernel-ingredient axes:
            // luma variance (masking), mean |gradient| (edge energy), mean
            // luma (light adaptation). BT.601 luma from the source pixels.
            let mut lsum = 0.0f64;
            let mut lsq = 0.0f64;
            let mut gsum = 0.0f64;
            let mut gn = 0usize;
            let luma_at = |x: usize, y: usize| -> f64 {
                let p = src.rows().nth(y).unwrap()[x];
                0.299 * p.r as f64 + 0.587 * p.g as f64 + 0.114 * p.b as f64
            };
            for y in y0..y1 {
                for x in x0..x1 {
                    let l = luma_at(x, y);
                    lsum += l;
                    lsq += l * l;
                    if x + 1 < x1 && y + 1 < y1 {
                        let gx = luma_at(x + 1, y) - l;
                        let gy = luma_at(x, y + 1) - l;
                        gsum += (gx * gx + gy * gy).sqrt();
                        gn += 1;
                    }
                }
            }
            let src_luma = lsum / n as f64;
            let src_var = (lsq / n as f64) - src_luma * src_luma;
            let src_grad = if gn > 0 { gsum / gn as f64 } else { 0.0 };
            let mean = sum / n as f64;
            let p3 = (sum3 / n as f64).cbrt();
            writeln!(
                f,
                "{tx}\t{ty}\t{n}\t{mean:.6}\t{p3:.6}\t{mx:.6}\t{mse:.6}\t\
                 {src_var:.4}\t{src_grad:.4}\t{src_luma:.4}"
            )
            .unwrap();
        }
    }
    eprintln!(
        "sbmap: {}x{} px -> {}x{} tiles (sb={sb}); frame score={:.4} p3={:.4}",
        w, h, tiles_x, tiles_y, ba.score, ba.pnorm_3
    );
}
