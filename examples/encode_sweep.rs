//! Encode-quality sweep harness.
//!
//! Encodes a reference image with every combination of `(speed, quality, qm)`
//! on the axes you pass in, decodes each AVIF back to RGB, scores the
//! decoded pixels against the reference with zensim, and writes a TSV.
//!
//! The column layout matches
//! [`benchmarks/avif_encode_fine_sweep_2026-04-16.tsv`], so historical sweeps
//! diff cleanly against new runs.
//!
//! ## Why not just use `AvifEncoderConfig`?
//!
//! This example calls the `ravif` (zenravif) `Encoder` directly instead of
//! going through `zenavif::EncoderConfig`. The only reason is
//! [`with_encode_bottomup`](ravif::Encoder::with_encode_bottomup), which
//! the high-level API deliberately does not expose. Forcing bottom-up on
//! (or off) is required to reproduce the scenarios behind issue #6 —
//! `ravif/40ddb66` defaults bottomup to `false` everywhere, so without the
//! override you'd measure top-down behaviour regardless of speed.
//!
//! ## Usage
//!
//! ```text
//! cargo run --release --example encode_sweep --features encode-imazen,encode-threading -- \
//!     --image /mnt/v/dataset/cid22/CID22/original/1001682.png \
//!     --speeds 1,2,4,6 \
//!     --qualities 5..=100:5 \
//!     --qm both \
//!     --force-bottomup both \
//!     --output /mnt/v/output/zenavif/sweeps/my_sweep.tsv
//! ```
//!
//! ## Patching the encoder under test
//!
//! To sweep a local zenrav1e branch without touching ravif, set a
//! `[patch.crates-io]` in your cargo home or a wrapper workspace:
//!
//! ```toml
//! [patch.crates-io]
//! zenrav1e = { path = "/home/you/zen/zenrav1e" }
//! ```
//!
//! The ravif → zenrav1e dep picks up the override transparently.

use std::env;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use almost_enough::{StopToken, Unstoppable};
use imgref::Img;
use rgb::Rgb;
use zensim::{Zensim, ZensimProfile};
use zensim_regress::{RegressionTolerance, check_regression};

#[derive(Debug, Clone, Copy)]
enum TriState {
    OnlyOff,
    OnlyOn,
    Both,
}

impl TriState {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "off" | "false" | "0" => Ok(Self::OnlyOff),
            "on" | "true" | "1" => Ok(Self::OnlyOn),
            "both" => Ok(Self::Both),
            _ => Err(format!("expected off|on|both, got '{s}'")),
        }
    }
    fn values(self) -> &'static [bool] {
        match self {
            Self::OnlyOff => &[false],
            Self::OnlyOn => &[true],
            Self::Both => &[false, true],
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Bottomup {
    Auto, // let ravif pick
    Force(bool),
    Both, // sweep both Some(true) and Some(false)
}

impl Bottomup {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "auto" | "default" => Ok(Self::Auto),
            "on" | "true" | "1" => Ok(Self::Force(true)),
            "off" | "false" | "0" => Ok(Self::Force(false)),
            "both" => Ok(Self::Both),
            _ => Err(format!("expected auto|on|off|both, got '{s}'")),
        }
    }
    fn values(self) -> Vec<Option<bool>> {
        match self {
            Self::Auto => vec![None],
            Self::Force(b) => vec![Some(b)],
            Self::Both => vec![Some(false), Some(true)],
        }
    }
    fn label(override_val: Option<bool>) -> &'static str {
        match override_val {
            None => "auto",
            Some(true) => "on",
            Some(false) => "off",
        }
    }
}

fn parse_int_list(s: &str) -> Result<Vec<u32>, String> {
    // Accepts "1,2,4,6" or "5..=100:5" range syntax.
    if let Some((rest, step)) = s.split_once(':') {
        let step: u32 = step.parse().map_err(|e| format!("step '{step}': {e}"))?;
        let (lo, hi, inclusive) = if let Some((lo, hi)) = rest.split_once("..=") {
            (lo, hi, true)
        } else if let Some((lo, hi)) = rest.split_once("..") {
            (lo, hi, false)
        } else {
            return Err(format!("range needs '..' or '..=': '{rest}'"));
        };
        let lo: u32 = lo.parse().map_err(|e| format!("range start '{lo}': {e}"))?;
        let hi: u32 = hi.parse().map_err(|e| format!("range end '{hi}': {e}"))?;
        let end = if inclusive { hi + 1 } else { hi };
        Ok((lo..end).step_by(step as usize).collect())
    } else {
        s.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.parse::<u32>().map_err(|e| format!("'{s}': {e}")))
            .collect()
    }
}

fn parse_bit_depth(s: &str) -> Result<zenavif::EncodeBitDepth, String> {
    match s {
        "8" => Ok(zenavif::EncodeBitDepth::Eight),
        "10" => Ok(zenavif::EncodeBitDepth::Ten),
        "auto" => Ok(zenavif::EncodeBitDepth::Auto),
        _ => Err(format!("expected 8|10|auto, got '{s}'")),
    }
}

struct Args {
    image: PathBuf,
    output: PathBuf,
    speeds: Vec<u32>,
    qualities: Vec<u32>,
    qm: TriState,
    force_bottomup: Bottomup,
    bit_depth: zenavif::EncodeBitDepth,
    threads: Option<usize>,
}

impl Args {
    fn print_help(bin: &str) {
        eprintln!(
            "Usage: {bin} [flags]

Encode-quality sweep for AVIF. Writes a TSV with columns:
  speed  quality  qm  bottomup  bit_depth  encode_ms  size_bytes  compression_ratio  zensim

Flags:
  --image PATH              Reference image (PNG) to encode. Required.
  --output PATH             TSV output path. Default: ./encode_sweep.tsv
  --speeds  LIST            Speeds, comma-separated or RANGE:STEP (default: 1,2,4,6)
  --qualities LIST          Qualities 1-100, comma-separated or RANGE:STEP (default: 5..=100:5)
  --qm off|on|both          QM setting (default: both)
  --force-bottomup auto|on|off|both
                            encode_bottomup override (default: auto — let ravif decide per speed)
  --bit-depth 8|10|auto     Output bit depth (default: 8)
  --threads N               Rayon pool size; 0 or omitted = rayon default
  -h, --help                Show this help

Range syntax: START..=END:STEP (inclusive) or START..END:STEP (exclusive).
  e.g. --qualities 5..=100:5  →  5,10,15,...,100
"
        );
    }

    fn parse() -> Result<Self, String> {
        let mut image = None;
        let mut output = PathBuf::from("./encode_sweep.tsv");
        let mut speeds = vec![1u32, 2, 4, 6];
        let mut qualities: Vec<u32> = (1..=20).map(|i| i * 5).collect();
        let mut qm = TriState::Both;
        let mut force_bottomup = Bottomup::Auto;
        let mut bit_depth = zenavif::EncodeBitDepth::Eight;
        let mut threads: Option<usize> = None;

        let raw: Vec<String> = env::args().collect();
        let bin = raw
            .first()
            .cloned()
            .unwrap_or_else(|| "encode_sweep".into());
        let mut it = raw.into_iter().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "-h" | "--help" => {
                    Self::print_help(&bin);
                    std::process::exit(0);
                }
                "--image" => image = Some(PathBuf::from(it.next().ok_or("--image needs PATH")?)),
                "--output" => output = PathBuf::from(it.next().ok_or("--output needs PATH")?),
                "--speeds" => {
                    let v = it.next().ok_or("--speeds needs LIST")?;
                    speeds = parse_int_list(&v)?;
                }
                "--qualities" => {
                    let v = it.next().ok_or("--qualities needs LIST")?;
                    qualities = parse_int_list(&v)?;
                }
                "--qm" => {
                    let v = it.next().ok_or("--qm needs off|on|both")?;
                    qm = TriState::parse(&v)?;
                }
                "--force-bottomup" => {
                    let v = it.next().ok_or("--force-bottomup needs auto|on|off|both")?;
                    force_bottomup = Bottomup::parse(&v)?;
                }
                "--bit-depth" => {
                    let v = it.next().ok_or("--bit-depth needs 8|10|auto")?;
                    bit_depth = parse_bit_depth(&v)?;
                }
                "--threads" => {
                    let v = it.next().ok_or("--threads needs N")?;
                    let n: usize = v.parse().map_err(|e| format!("--threads '{v}': {e}"))?;
                    threads = if n == 0 { None } else { Some(n) };
                }
                _ => return Err(format!("unknown flag '{arg}' (use --help)")),
            }
        }

        for &q in &qualities {
            if !(1..=100).contains(&q) {
                return Err(format!("quality {q} out of range (1..=100)"));
            }
        }
        for &s in &speeds {
            if !(1..=10).contains(&s) {
                return Err(format!("speed {s} out of range (1..=10)"));
            }
        }

        Ok(Self {
            image: image.ok_or("--image PATH is required (run with --help)")?,
            output,
            speeds,
            qualities,
            qm,
            force_bottomup,
            bit_depth,
            threads,
        })
    }
}

fn load_rgb(path: &Path) -> Result<Img<Vec<Rgb<u8>>>, String> {
    let img = image::open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?
        .to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let pixels: Vec<Rgb<u8>> = img
        .pixels()
        .map(|p| Rgb {
            r: p[0],
            g: p[1],
            b: p[2],
        })
        .collect();
    Ok(Img::new(pixels, w, h))
}

fn ravif_bit_depth(b: zenavif::EncodeBitDepth) -> ravif::BitDepth {
    match b {
        zenavif::EncodeBitDepth::Eight => ravif::BitDepth::Eight,
        zenavif::EncodeBitDepth::Ten => ravif::BitDepth::Ten,
        // Auto == "match input", which is 8-bit for PNG RGB input.
        zenavif::EncodeBitDepth::Auto => ravif::BitDepth::Eight,
    }
}

fn main() -> ExitCode {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let img = match load_rgb(&args.image) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    let raw_size = img.width() * img.height() * 3;

    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).ok();
    }

    let zensim = Zensim::new(ZensimProfile::latest());
    let tol = RegressionTolerance::off_by_one().with_min_similarity(0.0);

    let qm_vals = args.qm.values();
    let bu_vals = args.force_bottomup.values();
    let bit_depth = args.bit_depth;
    let ravif_depth = ravif_bit_depth(bit_depth);

    let mut tsv = String::from(
        "speed\tquality\tqm\tbottomup\tbit_depth\tencode_ms\tsize_bytes\tcompression_ratio\tzensim\n",
    );

    let stderr = std::io::stderr();
    let mut s = stderr.lock();
    writeln!(
        s,
        "image: {}  ({}x{})  raw={} bytes",
        args.image.display(),
        img.width(),
        img.height(),
        raw_size
    )
    .ok();
    writeln!(
        s,
        "sweeping {} configs  →  {}",
        args.speeds.len() * args.qualities.len() * qm_vals.len() * bu_vals.len(),
        args.output.display()
    )
    .ok();
    writeln!(
        s,
        "{:<4}{:>4} {:<4}{:<5} {:>7} {:>8} {:>6} {:>7}",
        "sp", "q", "qm", "bu", "ms", "size", "ratio", "zensim"
    )
    .ok();
    writeln!(s, "{}", "-".repeat(55)).ok();

    let bit_depth_label = match bit_depth {
        zenavif::EncodeBitDepth::Eight => "8",
        zenavif::EncodeBitDepth::Ten => "10",
        zenavif::EncodeBitDepth::Auto => "auto",
    };

    for &speed in &args.speeds {
        let speed = speed as u8;
        for &quality in &args.qualities {
            let quality = quality as f32;
            for &qm in qm_vals {
                for &bu in &bu_vals {
                    let mut enc_builder = ravif::Encoder::new()
                        .with_quality(quality)
                        .with_speed(speed)
                        .with_bit_depth(ravif_depth)
                        .with_qm(qm)
                        .with_encode_bottomup(bu)
                        .with_stop(StopToken::new(Unstoppable));
                    if let Some(n) = args.threads {
                        enc_builder = enc_builder.with_num_threads(Some(n));
                    }

                    let t0 = Instant::now();
                    let enc = match enc_builder.encode_rgb(img.as_ref()) {
                        Ok(e) => e,
                        Err(e) => {
                            writeln!(
                                s,
                                "FAIL s{speed} q{} qm={} bu={}: {e}",
                                quality as u32,
                                if qm { "on" } else { "off" },
                                Bottomup::label(bu),
                            )
                            .ok();
                            continue;
                        }
                    };
                    let ms = t0.elapsed().as_millis();

                    let dec_config = zenavif::DecoderConfig::new().prefer_8bit(true);
                    let dec_result =
                        zenavif::decode_with(&enc.avif_file, &dec_config, &Unstoppable);
                    let score = match &dec_result {
                        Ok(d) => match d.try_as_imgref::<Rgb<u8>>() {
                            Some(decoded) => {
                                match check_regression(&zensim, &img.as_ref(), &decoded, &tol) {
                                    Ok(r) => r.score(),
                                    Err(e) => {
                                        eprintln!(
                                            "[debug] s{speed} q{} qm={} regression err: {e}",
                                            quality as u32,
                                            if qm { "on" } else { "off" }
                                        );
                                        -999.0
                                    }
                                }
                            }
                            None => {
                                eprintln!(
                                    "[debug] s{speed} q{} qm={} try_as_imgref<Rgb<u8>> returned None",
                                    quality as u32,
                                    if qm { "on" } else { "off" }
                                );
                                -999.0
                            }
                        },
                        Err(e) => {
                            eprintln!(
                                "[debug] s{speed} q{} qm={} decode err: {e}",
                                quality as u32,
                                if qm { "on" } else { "off" }
                            );
                            -999.0
                        }
                    };
                    let size = enc.avif_file.len();
                    let ratio = raw_size as f64 / size as f64;

                    let qm_s = if qm { "on" } else { "off" };
                    let bu_s = Bottomup::label(bu);
                    writeln!(
                        s,
                        "{:<4}{:>4} {:<4}{:<5} {:>7} {:>8} {:>5.1}x {:>7.1}",
                        speed, quality as u32, qm_s, bu_s, ms, size, ratio, score
                    )
                    .ok();
                    tsv.push_str(&format!(
                        "{speed}\t{}\t{qm_s}\t{bu_s}\t{bit_depth_label}\t{ms}\t{size}\t{ratio:.1}\t{score:.1}\n",
                        quality as u32
                    ));
                }
            }
        }
    }

    if let Err(e) = fs::write(&args.output, &tsv) {
        eprintln!("error: write {}: {e}", args.output.display());
        return ExitCode::from(1);
    }
    writeln!(s, "\nSaved to {}", args.output.display()).ok();
    ExitCode::SUCCESS
}
