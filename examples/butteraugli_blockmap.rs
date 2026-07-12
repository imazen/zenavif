//! Per-COMMITTED-BLOCK butteraugli/MSE/source-feature pooling — DFIT5's
//! dataset generator (DECISION_RULE_DFIT5.md). Reads the decision trace's
//! commit rows (row==3: the blocks actually encoded into the bitstream, with
//! bo in 4-px units + the zenrav1e BlockSize ordinal), pools the raw f32
//! butteraugli diffmap + RGB MSE + source stats over each block's exact
//! pixels, and emits one row per committed block. The python fit joins
//! per-block winner-D from the trace by (bo, bsize).
//!
//! Usage:
//!   cargo run --release --features two-pass-butteraugli --example butteraugli_blockmap -- \
//!     SRC.png DEC.png TRACE.tsv OUT.tsv
use imgref::ImgVec;
use rgb::Rgb;

/// zenrav1e `BlockSize` ordinal -> (w, h) px (partition.rs enum order).
const BSIZE_DIMS: [(usize, usize); 22] = [
    (4, 4),
    (4, 8),
    (8, 4),
    (8, 8),
    (8, 16),
    (16, 8),
    (16, 16),
    (16, 32),
    (32, 16),
    (32, 32),
    (32, 64),
    (64, 32),
    (64, 64),
    (64, 128),
    (128, 64),
    (128, 128),
    (4, 16),
    (16, 4),
    (8, 32),
    (32, 8),
    (16, 64),
    (64, 16),
];

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
        eprintln!("usage: butteraugli_blockmap SRC.png DEC.png TRACE.tsv OUT.tsv");
        std::process::exit(2);
    }
    let src = load_png_rgb(std::path::Path::new(&args[0]));
    let dec = load_png_rgb(std::path::Path::new(&args[1]));
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
    // Flatten rows once (row iterators are O(y) to seek).
    let mrows: Vec<&[f32]> = map.rows().collect();
    let srows: Vec<&[Rgb<u8>]> = src.rows().collect();
    let drows: Vec<&[Rgb<u8>]> = dec.rows().collect();

    use std::io::Write;
    let trace = std::fs::read_to_string(&args[2]).expect("trace");
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&args[3]).expect("out tsv"),
    );
    writeln!(
        out,
        "bo_x\tbo_y\tbsize\tn_px\tba_mean\tba_p3\tba_max\tmse\tsrc_var\tsrc_grad\tsrc_luma"
    )
    .unwrap();

    let mut lines = trace.lines();
    let header: Vec<&str> = lines.next().expect("empty trace").split('\t').collect();
    let ix = |name: &str| header.iter().position(|h| *h == name).unwrap();
    let (i_bx, i_by, i_bs, i_row) =
        (ix("bo_x"), ix("bo_y"), ix("bsize"), ix("row"));

    let mut n_blocks = 0usize;
    for line in lines {
        let c: Vec<&str> = line.split('\t').collect();
        if c[i_row] != "3" {
            continue;
        }
        let bx: usize = c[i_bx].parse().unwrap();
        let by: usize = c[i_by].parse().unwrap();
        let bs: usize = c[i_bs].parse().unwrap();
        let (bw, bh) = BSIZE_DIMS[bs];
        let (x0, y0) = (bx * 4, by * 4);
        let (x1, y1) = ((x0 + bw).min(w), (y0 + bh).min(h));
        if x0 >= w || y0 >= h {
            continue; // block entirely in the padding region
        }
        let mut sum = 0.0f64;
        let mut sum3 = 0.0f64;
        let mut mx = 0.0f64;
        let mut se = 0.0f64;
        let mut lsum = 0.0f64;
        let mut lsq = 0.0f64;
        let mut gsum = 0.0f64;
        let mut gn = 0usize;
        let mut n = 0usize;
        for y in y0..y1 {
            let mrow = &mrows[y][x0..x1];
            let srow = &srows[y][x0..x1];
            let drow = &drows[y][x0..x1];
            for (i, &v) in mrow.iter().enumerate() {
                let v = v as f64;
                sum += v;
                sum3 += v * v * v;
                if v > mx {
                    mx = v;
                }
                let (a, b) = (srow[i], drow[i]);
                for (ca, cb) in [(a.r, b.r), (a.g, b.g), (a.b, b.b)] {
                    let d = ca as f64 - cb as f64;
                    se += d * d;
                }
                let l = 0.299 * a.r as f64 + 0.587 * a.g as f64
                    + 0.114 * a.b as f64;
                lsum += l;
                lsq += l * l;
                n += 1;
            }
            // gradient within the block interior
            if y + 1 < y1 {
                let srow_n = &srows[y + 1][x0..x1];
                for i in 0..(x1 - x0).saturating_sub(1) {
                    let l = |p: Rgb<u8>| {
                        0.299 * p.r as f64 + 0.587 * p.g as f64
                            + 0.114 * p.b as f64
                    };
                    let c0 = l(srow[i]);
                    let gx = l(srow[i + 1]) - c0;
                    let gy = l(srow_n[i]) - c0;
                    gsum += (gx * gx + gy * gy).sqrt();
                    gn += 1;
                }
            }
        }
        if n == 0 {
            continue;
        }
        let nf = n as f64;
        let luma = lsum / nf;
        writeln!(
            out,
            "{bx}\t{by}\t{bs}\t{n}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{:.4}\t{:.4}\t{:.4}",
            sum / nf,
            (sum3 / nf).cbrt(),
            mx,
            se / (nf * 3.0),
            (lsq / nf) - luma * luma,
            if gn > 0 { gsum / gn as f64 } else { 0.0 },
            luma,
        )
        .unwrap();
        n_blocks += 1;
    }
    eprintln!("blockmap: {n_blocks} committed blocks pooled ({}x{} px)", w, h);
}
