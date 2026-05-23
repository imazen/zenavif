//! Spike bench: rav1d-safe vs ffmpeg-HW AV1 decode for still AVIF.
//!
//! For each AVIF in the vector dir, runs every available backend
//! `n` times and prints median wall-clock per backend in TSV.
//! ffmpeg backends additionally report `bench_rtime` (ffmpeg's internal
//! real time, excluding subprocess startup overhead).
//!
//! ## Usage
//!
//! ```bash
//! cargo run --release --features backend-ffmpeg --example bench_backends -- \
//!   --vectors tests/vectors/link-u --iters 20 \
//!   --backends rust,vaapi,ffmpeg-cpu \
//!   --out benchmarks/backends_$(date +%Y-%m-%d).tsv
//! ```
//!
//! Default: 20 iters, all backends recommended for the host platform,
//! TSV to stdout.

#![cfg(feature = "backend-ffmpeg")]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use zenavif::backend::{Av1DecoderBackend, DecodeBackend, RustBackend};
use zenavif::backend_ffmpeg::FfmpegBackend;
use zenavif::DecoderConfig;

#[derive(Clone)]
struct Args {
    vectors: PathBuf,
    iters: usize,
    warmup: usize,
    backends: Vec<String>,
    out: Option<PathBuf>,
    bytes_limit: Option<u64>,
}

impl Args {
    fn parse() -> Self {
        let mut vectors = PathBuf::from("tests/vectors/link-u");
        let mut iters = 20usize;
        let mut warmup = 3usize;
        let mut backends: Vec<String> = default_backends();
        let mut out: Option<PathBuf> = None;
        let mut bytes_limit: Option<u64> = None;

        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--vectors" => vectors = PathBuf::from(it.next().expect("--vectors VALUE")),
                "--iters" => iters = it.next().unwrap().parse().expect("--iters int"),
                "--warmup" => warmup = it.next().unwrap().parse().expect("--warmup int"),
                "--backends" => {
                    backends = it
                        .next()
                        .unwrap()
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                "--out" => out = Some(PathBuf::from(it.next().expect("--out PATH"))),
                "--bytes-limit" => {
                    bytes_limit = Some(
                        it.next()
                            .unwrap()
                            .parse()
                            .expect("--bytes-limit u64 (skip files larger than this)"),
                    );
                }
                "-h" | "--help" => {
                    println!("usage: bench_backends [--vectors DIR] [--iters N] [--warmup N] [--backends rust,vaapi,d3d11va,dxva2,cuda,ffmpeg-cpu] [--out FILE.tsv] [--bytes-limit N]");
                    std::process::exit(0);
                }
                other => panic!("unknown arg: {other}"),
            }
        }
        Self {
            vectors,
            iters,
            warmup,
            backends,
            out,
            bytes_limit,
        }
    }
}

fn default_backends() -> Vec<String> {
    let mut v = vec!["rust".to_string(), "ffmpeg-cpu".to_string()];
    #[cfg(target_os = "linux")]
    v.push("vaapi".to_string());
    #[cfg(target_os = "windows")]
    {
        v.push("d3d11va".to_string());
        v.push("dxva2".to_string());
    }
    v.push("cuda".to_string());
    v
}

fn resolve_backend(name: &str) -> Option<DecodeBackend> {
    match name {
        "rust" => Some(DecodeBackend::Rust),
        #[cfg(all(feature = "backend-ffmpeg", target_os = "linux"))]
        "vaapi" => Some(DecodeBackend::Vaapi),
        #[cfg(all(feature = "backend-ffmpeg", target_os = "windows"))]
        "d3d11va" => Some(DecodeBackend::D3d11va),
        #[cfg(all(feature = "backend-ffmpeg", target_os = "windows"))]
        "dxva2" => Some(DecodeBackend::Dxva2),
        #[cfg(feature = "backend-ffmpeg")]
        "cuda" => Some(DecodeBackend::Cuda),
        #[cfg(feature = "backend-ffmpeg")]
        "ffmpeg-cpu" => Some(DecodeBackend::FfmpegCpu),
        _ => None,
    }
}

fn list_avifs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|_| panic!("read_dir {}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("avif"))
        .collect();
    out.sort();
    out
}

fn median(mut xs: Vec<Duration>) -> Duration {
    xs.sort();
    xs[xs.len() / 2]
}

enum Backend {
    Rust(RustBackend),
    Ffmpeg(FfmpegBackend),
}

impl Backend {
    fn is_available(&self) -> bool {
        match self {
            Backend::Rust(b) => b.is_available(),
            Backend::Ffmpeg(b) => b.is_available(),
        }
    }
    fn decode(&mut self, data: &[u8], cfg: &DecoderConfig) -> Result<(u32, u32), String> {
        match self {
            Backend::Rust(b) => {
                let pixels = b
                    .decode(data, cfg, &enough::Unstoppable)
                    .map_err(|e| format!("{e}"))?;
                Ok((pixels.width(), pixels.height()))
            }
            Backend::Ffmpeg(b) => {
                let pixels = b
                    .decode(data, cfg, &enough::Unstoppable)
                    .map_err(|e| format!("{e}"))?;
                Ok((pixels.width(), pixels.height()))
            }
        }
    }
    fn ffmpeg_bench_rtime(&self) -> Option<Duration> {
        match self {
            Backend::Ffmpeg(b) => b.last_timing().bench_rtime,
            _ => None,
        }
    }
}

fn make_backend(b: DecodeBackend) -> Backend {
    match b {
        DecodeBackend::Rust => Backend::Rust(RustBackend),
        #[cfg(all(feature = "backend-ffmpeg", target_os = "linux"))]
        DecodeBackend::Vaapi => Backend::Ffmpeg(FfmpegBackend::new("vaapi")),
        #[cfg(all(feature = "backend-ffmpeg", target_os = "windows"))]
        DecodeBackend::D3d11va => Backend::Ffmpeg(FfmpegBackend::new("d3d11va")),
        #[cfg(all(feature = "backend-ffmpeg", target_os = "windows"))]
        DecodeBackend::Dxva2 => Backend::Ffmpeg(FfmpegBackend::new("dxva2")),
        #[cfg(feature = "backend-ffmpeg")]
        DecodeBackend::Cuda => Backend::Ffmpeg(FfmpegBackend::new("cuda")),
        #[cfg(feature = "backend-ffmpeg")]
        DecodeBackend::FfmpegCpu => Backend::Ffmpeg(FfmpegBackend::new("none")),
        // DecodeBackend is #[non_exhaustive]; future variants get a clear error.
        _ => unreachable!("unmapped DecodeBackend variant in bench_backends"),
    }
}

fn main() {
    let args = Args::parse();
    let cfg = DecoderConfig::default();

    let resolved: Vec<(String, DecodeBackend)> = args
        .backends
        .iter()
        .filter_map(|n| resolve_backend(n).map(|b| (n.clone(), b)))
        .collect();

    if resolved.is_empty() {
        eprintln!("no usable backends in --backends list");
        std::process::exit(2);
    }

    // Build instances + availability gate up front.
    // Tuple: (name, backend, advertised_available, consecutive_fails)
    // `advertised_available` is the up-front probe; we additionally
    // disable a backend after 3 consecutive warmup failures so a
    // half-available HW path (e.g. ffmpeg lists `cuda` as a hwaccel
    // but the device can't initialize) doesn't burn the whole run.
    let mut instances: Vec<(String, Backend, bool, u32)> = resolved
        .iter()
        .map(|(n, b)| {
            let inst = make_backend(*b);
            let avail = inst.is_available();
            (n.clone(), inst, avail, 0u32)
        })
        .collect();

    for (n, _, avail, _) in &instances {
        eprintln!(
            "backend {n:<12} advertised={}",
            if *avail { "yes" } else { "NO (skipped)" }
        );
    }

    let files = list_avifs(&args.vectors);
    eprintln!(
        "vectors: {} avif files under {}",
        files.len(),
        args.vectors.display()
    );
    eprintln!(
        "iters: {} median-of-n (after {} warmup runs)",
        args.iters, args.warmup
    );

    // TSV header.
    let mut hdr = String::from("file\tbytes\twidth\theight");
    for (n, _, _, _) in &instances {
        hdr.push('\t');
        hdr.push_str(&format!("{n}_ms"));
        hdr.push('\t');
        hdr.push_str(&format!("{n}_bench_ms"));
    }
    println!("{hdr}");
    let mut tsv_lines: Vec<String> = Vec::new();
    tsv_lines.push(hdr);

    for file in &files {
        let bytes = match fs::read(file) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("skip {}: {e}", file.display());
                continue;
            }
        };
        if let Some(limit) = args.bytes_limit {
            if bytes.len() as u64 > limit {
                continue;
            }
        }
        let stem = file.file_name().and_then(|s| s.to_str()).unwrap_or("?");

        let mut row = format!("{stem}\t{}", bytes.len());
        let mut dims_logged = false;
        let mut last_w = 0u32;
        let mut last_h = 0u32;

        for (name, inst, avail, fails) in instances.iter_mut() {
            if !*avail {
                row.push_str("\t-\t-");
                continue;
            }
            // Warmup.
            let mut ok_runs = 0usize;
            let mut last_err: Option<String> = None;
            for _ in 0..args.warmup {
                match inst.decode(&bytes, &cfg) {
                    Ok((w, h)) => {
                        last_w = w;
                        last_h = h;
                    }
                    Err(e) => {
                        last_err = Some(e);
                        break;
                    }
                }
            }
            if last_err.is_some() {
                *fails += 1;
                // Higher threshold so a run-of-monochrome failures at
                // the start of the corpus doesn't permanently disable
                // a HW backend that succeeds on most 4:2:0/4:4:4
                // bitstreams. 30 ≈ "if a backend fails on this many in
                // a row, the device is genuinely gone, not just
                // unsupported-profile-of-the-moment".
                let permanent = *fails >= 30;
                eprintln!(
                    "[{name}] {stem}: warmup err{}: {}",
                    if permanent {
                        " (disabling for rest of run)"
                    } else {
                        ""
                    },
                    last_err.as_deref().unwrap_or("?")
                );
                if permanent {
                    *avail = false;
                }
                row.push_str("\tERR\tERR");
                continue;
            }
            // Reset consecutive-fail counter on a successful warmup.
            *fails = 0;
            // Timed runs.
            let mut wall_ms: Vec<Duration> = Vec::with_capacity(args.iters);
            let mut bench_ms: Vec<Duration> = Vec::with_capacity(args.iters);
            for _ in 0..args.iters {
                let t0 = Instant::now();
                match inst.decode(&bytes, &cfg) {
                    Ok((w, h)) => {
                        last_w = w;
                        last_h = h;
                        let elapsed = t0.elapsed();
                        wall_ms.push(elapsed);
                        if let Some(b) = inst.ffmpeg_bench_rtime() {
                            bench_ms.push(b);
                        }
                        ok_runs += 1;
                    }
                    Err(e) => {
                        last_err = Some(e);
                        break;
                    }
                }
            }
            if ok_runs == 0 {
                eprintln!(
                    "[{name}] {stem}: timed err: {}",
                    last_err.as_deref().unwrap_or("?")
                );
                row.push_str("\tERR\tERR");
                continue;
            }
            if !dims_logged {
                dims_logged = true;
            }
            let med = median(wall_ms);
            row.push_str(&format!("\t{:.3}", med.as_secs_f64() * 1000.0));
            if bench_ms.is_empty() {
                row.push_str("\t-");
            } else {
                row.push_str(&format!("\t{:.3}", median(bench_ms).as_secs_f64() * 1000.0));
            }
        }

        // Insert dims after the bytes column (we now know them from at least one backend).
        // Rebuild row prefix: file \t bytes \t w \t h \t ...
        let parts: Vec<&str> = row.splitn(3, '\t').collect();
        let row_final = if parts.len() == 3 {
            format!(
                "{}\t{}\t{}\t{}\t{}",
                parts[0], parts[1], last_w, last_h, parts[2]
            )
        } else {
            row.clone()
        };
        println!("{row_final}");
        tsv_lines.push(row_final);
    }

    if let Some(out_path) = args.out {
        if let Some(parent) = out_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&out_path, tsv_lines.join("\n") + "\n").expect("write --out file");
        eprintln!("wrote {}", out_path.display());
    }
}
