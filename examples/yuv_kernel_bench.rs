//! Kernel-level A/B micro-benchmark for the in-house YUV→RGB(A) converters
//! (`_dev` feature exposes the module). Median-of-N wall times per
//! (size × sampling × output); run alternating binaries for A/B discipline.
//!
//! Usage: yuv_kernel_bench [reps]

use zenavif::yuv_convert::{
    YuvMatrix, YuvRange, yuv420_to_rgb8, yuv420_to_rgba8_strip, yuv422_to_rgb8,
    yuv422_to_rgba8_strip, yuv444_to_rgb8, yuv444_to_rgba8_strip,
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
