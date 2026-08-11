//! Kernel-level A/B micro-benchmark for the in-house YUV→RGB(A) converters
//! (`_dev` feature exposes the module). Median-of-N wall times per
//! (size × sampling × output); run alternating binaries for A/B discipline.
//!
//! Usage: yuv_kernel_bench [reps]

use zenavif::yuv_convert::{
    YuvMatrix, YuvRange, yuv420_to_rgb8, yuv420_to_rgb16_strip, yuv420_to_rgba8_strip,
    yuv422_to_rgb8, yuv422_to_rgba8_strip, yuv444_to_rgb8, yuv444_to_rgba8_strip,
};

fn fill(buf: &mut [u8], mut s: u32) {
    s |= 1;
    for b in buf.iter_mut() {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        *b = (s & 0xFF) as u8;
    }
}

fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs[xs.len() / 2]
}

fn main() {
    let reps: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let range = YuvRange::Full;
    let matrix = YuvMatrix::Bt601;

    // ── 10-bit: in-house unified kernel vs the yuv crate (perf + delta) ──
    {
        let (w, h) = (1920usize, 1080usize);
        let cw = w / 2;
        let ch = h / 2;
        let mut y16 = vec![0u16; w * h];
        let mut u16p = vec![0u16; cw * ch];
        let mut v16p = vec![0u16; cw * ch];
        let mut s = 5u32;
        let mut fill16 = |buf: &mut [u16]| {
            for b in buf.iter_mut() {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                *b = (s & 1023) as u16;
            }
        };
        fill16(&mut y16);
        fill16(&mut u16p);
        fill16(&mut v16p);
        let mut ours = vec![rgb::Rgb::<u16>::default(); w * h];
        let planar = yuv::YuvPlanarImage {
            y_plane: &y16,
            y_stride: w as u32,
            u_plane: &u16p,
            u_stride: cw as u32,
            v_plane: &v16p,
            v_stride: cw as u32,
            width: w as u32,
            height: h as u32,
        };
        let mut theirs = vec![0u16; w * h * 3];

        let mp = (w * h) as f64 / 1e6;
        let time = |f: &mut dyn FnMut()| {
            for _ in 0..3 {
                f();
            }
            let mut ts = Vec::with_capacity(reps);
            for _ in 0..reps {
                let t0 = std::time::Instant::now();
                f();
                ts.push(t0.elapsed().as_secs_f64() * 1e3);
            }
            median(ts)
        };
        let a = time(&mut || {
            yuv420_to_rgb16_strip(
                &y16,
                w,
                &u16p,
                cw,
                &v16p,
                cw,
                w,
                h,
                0,
                h,
                YuvRange::Full,
                YuvMatrix::Bt601,
                10,
                &mut ours,
            )
        });
        let b = time(&mut || {
            yuv::i010_to_rgb10_bilinear(
                &planar,
                &mut theirs,
                (w * 3) as u32,
                yuv::YuvRange::Full,
                yuv::YuvStandardMatrix::Bt601,
            )
            .unwrap()
        });
        println!(
            "1920x1080 d10-420-rgb16 in-house {a:7.3} ms ({:6.1} Mpx/s)  yuv-crate {b:7.3} ms ({:6.1} Mpx/s)",
            mp * 1e3 / a,
            mp * 1e3 / b
        );
        // Output delta (yuv crate = fixed-point; ours = f32 canonical recipe).
        // NOTE upstream drops the last row pair (awxkee/yuvutils-rs#129) —
        // exclude the bottom two rows from the comparison.
        let mut hist = [0u64; 8];
        let mut maxd = 0i32;
        for i in 0..(w * (h - 2)) {
            for c in 0..3 {
                let d = (ours[i].as_ref()[c] as i32 - theirs[i * 3 + c] as i32).abs();
                maxd = maxd.max(d);
                hist[(d as usize).min(7)] += 1;
            }
        }
        println!("          d10 delta vs yuv crate: max {maxd}, hist(0..=7+) {hist:?}");
    }

    for &(w, h) in &[(1920usize, 1080usize), (3840, 2160)] {
        let mut y = vec![0u8; w * h];
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);
        let mut u_half = vec![0u8; cw * ch.max(h)]; // covers 420 (cw*ch) and 422 (cw*h)
        let mut v_half = vec![0u8; cw * ch.max(h)];
        let mut u_full = vec![0u8; w * h];
        let mut v_full = vec![0u8; w * h];
        fill(&mut y, 7);
        fill(&mut u_half, 11);
        fill(&mut v_half, 13);
        fill(&mut u_full, 17);
        fill(&mut v_full, 19);
        let mut rgba = vec![
            rgb::Rgba {
                r: 0u8,
                g: 0,
                b: 0,
                a: 0
            };
            w * h
        ];
        let mp = (w * h) as f64 / 1e6;

        let mut cell = |label: &str, f: &mut dyn FnMut() -> usize| {
            // Warmup
            for _ in 0..3 {
                std::hint::black_box(f());
            }
            let mut times = Vec::with_capacity(reps);
            for _ in 0..reps {
                let t0 = std::time::Instant::now();
                std::hint::black_box(f());
                times.push(t0.elapsed().as_secs_f64() * 1e3);
            }
            let med = median(times);
            println!(
                "{w}x{h} {label:12} {med:8.3} ms  {:8.1} Mpx/s",
                mp * 1e3 / med
            );
        };

        cell("420-rgb8", &mut || {
            yuv420_to_rgb8(&y, w, &u_half, cw, &v_half, cw, w, h, range, matrix)
                .buf()
                .len()
        });
        cell("420-rgba8", &mut || {
            yuv420_to_rgba8_strip(
                &y, w, &u_half, cw, &v_half, cw, w, h, 0, h, range, matrix, &mut rgba,
            );
            rgba.len()
        });
        cell("422-rgb8", &mut || {
            yuv422_to_rgb8(&y, w, &u_half, cw, &v_half, cw, w, h, range, matrix)
                .buf()
                .len()
        });
        cell("422-rgba8", &mut || {
            yuv422_to_rgba8_strip(
                &y, w, &u_half, cw, &v_half, cw, w, 0, h, range, matrix, &mut rgba,
            );
            rgba.len()
        });
        cell("444-rgb8", &mut || {
            yuv444_to_rgb8(&y, w, &u_full, w, &v_full, w, w, h, range, matrix)
                .buf()
                .len()
        });
        cell("444-rgba8", &mut || {
            yuv444_to_rgba8_strip(
                &y, w, &u_full, w, &v_full, w, w, 0, h, range, matrix, &mut rgba,
            );
            rgba.len()
        });
    }
}
