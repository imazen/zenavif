//! `gate_kit`: the zenavif halves of the executable engineering-baseline
//! gates (`docs/ENGINEERING_BASELINE.md` section A), one binary with three
//! subcommands:
//!
//! * `determinism [--ci]` — invariant A3: encoded bytes are independent of
//!   the thread count. Encodes pinned images (including a 2.30 MP one that
//!   crosses ravif's `TILE_RD_MIN_AREA` = 1 MP tiling threshold, so the
//!   multi-tile path is live) at threads {1, 8, auto} and requires byte
//!   equality across the legs.
//! * `cells OUTDIR [--ci]` — emits the pinned conformance cell matrix
//!   (shipped-config encodes across speed x quality x subsampling x depth)
//!   plus the screen-content `.y4m` inputs for the palette-armed leg;
//!   `scripts/gates/gate_conformance.sh` consumes the manifest and runs the
//!   PALCONF protocol (aomdec clean + aomdec/rav1d-safe raw md5 agreement).
//! * `ladder [--pin]` — invariant A6: coarse perf floors. Encodes the three
//!   ladder tiers (s2 / s6 / s10 class), scores the roundtrip with
//!   fast-ssim2, and compares (bytes, ssim2, encode ms) against the pinned
//!   envelope in `benchmarks/gate_ladder_envelope.tsv` with generous
//!   tolerances (±2 % bytes, ±0.5 ssim2, ±25 % ms) — a de-tuning tripwire,
//!   not a benchmark. Local-only (timing); `--pin` re-pins after an
//!   intentional change (commit the TSV diff in the same commit).
//!
//! All inputs are deterministic integer-only synthetics (no files, no
//! libm), so the cells are pinned by construction. The encode chain is the
//! product path: `zenavif::EncoderConfig` -> zenravif -> zenrav1e as
//! resolved by Cargo (registry zenrav1e until the dep bump; the gates run
//! green on both sides of the flip and verify it).

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use almost_enough::{StopToken, Unstoppable};
use imgref::{Img, ImgRef, ImgVec};
use rgb::Rgb;
use zenavif::{EncodeBitDepth, EncodeChromaSubsampling, EncoderConfig};

// ---------------------------------------------------------------------------
// Deterministic pinned content (integer-only; keep in sync conceptually with
// zenrav1e/examples/gate_identity.rs — intentionally duplicated, the two
// repos pin their own content).
// ---------------------------------------------------------------------------

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
    name: &'static str,
    img: ImgVec<Rgb<u8>>,
}

fn pinned_images() -> Vec<PinnedRgb> {
    vec![
        PinnedRgb {
            name: "photo",
            img: gen_photo_rgb(512, 384, 0x0001),
        },
        PinnedRgb {
            name: "screen",
            img: gen_screen_rgb(512, 384),
        },
        PinnedRgb {
            name: "mixed",
            img: gen_mixed_rgb(509, 341),
        },
    ]
}

/// The 2.30 MP determinism image: crosses TILE_RD_MIN_AREA (1 MP), so the
/// default tile policy yields >1 tile and the multi-tile + thread-pool path
/// is actually exercised.
fn pinned_big() -> PinnedRgb {
    PinnedRgb {
        name: "big",
        img: gen_photo_rgb(1920, 1200, 0x0B16),
    }
}

/// Screen-content YUV planes for the palette-armed rav1e-CLI leg (written
/// as .y4m by `cells`; 4:2:0 and 4:4:4 variants).
fn write_screen_y4m(dir: &Path, subsample: bool) -> std::io::Result<()> {
    let (w, h) = (128usize, 128usize);
    let palette = [24u8, 235, 80, 160, 48, 200, 112, 16];
    let mut rng = Lcg::new(0x0002);
    let patch: Vec<u8> = (0..(w / 16) * (h / 16))
        .map(|_| palette[(rng.next_u32() % 8) as usize])
        .collect();
    let glyph = |i: usize, j: usize| -> bool {
        let (gi, gj) = (i % 8, j % 8);
        gi == 1 || gj == 6 || (gi == gj && gi < 5)
    };
    let mut y = vec![0u8; w * h];
    for j in 0..h {
        for i in 0..w {
            let mut px = patch[(j / 16) * (w / 16) + i / 16];
            if i % 16 == 0 || j % 16 == 0 {
                px = 0;
            } else if (64..96).contains(&i) && glyph(i, j) {
                px = 255;
            }
            y[j * w + i] = px;
        }
    }
    let (cw, ch, ctag) = if subsample {
        (w / 2, h / 2, "420jpeg")
    } else {
        (w, h, "444")
    };
    let step = if subsample { 2 } else { 1 };
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    for j in 0..ch {
        for i in 0..cw {
            let p = patch[(j * step / 16) * (w / 16) + (i * step) / 16];
            u[j * cw + i] = if p > 128 { 96 } else { 160 };
            v[j * cw + i] = if p > 128 { 176 } else { 72 };
        }
    }
    let name = if subsample {
        "screen_420.y4m"
    } else {
        "screen_444.y4m"
    };
    let mut f = fs::File::create(dir.join(name))?;
    writeln!(f, "YUV4MPEG2 W{w} H{h} F25:1 Ip A1:1 C{ctag}")?;
    writeln!(f, "FRAME")?;
    f.write_all(&y)?;
    f.write_all(&u)?;
    f.write_all(&v)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Encode helpers
// ---------------------------------------------------------------------------

fn config(
    quality: f32,
    speed: u8,
    sub: EncodeChromaSubsampling,
    depth: EncodeBitDepth,
    threads: Option<usize>,
) -> EncoderConfig {
    EncoderConfig::new()
        .quality(quality)
        .speed(speed)
        .chroma_subsampling(sub)
        .bit_depth(depth)
        .threads(threads)
}

fn encode(img: ImgRef<'_, Rgb<u8>>, cfg: &EncoderConfig) -> Vec<u8> {
    zenavif::encode_rgb8(img, cfg, StopToken::new(Unstoppable))
        .expect("gate_kit: encode failed")
        .avif_file
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut hsh = 0xcbf2_9ce4_8422_2325u64;
    for &b in data {
        hsh ^= u64::from(b);
        hsh = hsh.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hsh
}

// ---------------------------------------------------------------------------
// determinism
// ---------------------------------------------------------------------------

fn run_determinism(ci: bool) {
    let imgs = pinned_images();
    let big = pinned_big();
    // (image, speed, quality). `big` is the multi-tile liveness cell; the
    // small images cover the single-tile path. s2 is omitted to keep the
    // gate fast — the pool/tiling code is speed-independent (P0 record:
    // tile policy is area-driven and bitstream-inert vs threads).
    let mut cells: Vec<(&PinnedRgb, u8, f32)> = vec![(&big, 6, 60.0), (&big, 10, 60.0)];
    if !ci {
        cells.push((&imgs[0], 6, 35.0));
        cells.push((&imgs[0], 6, 60.0));
        cells.push((&imgs[2], 10, 60.0));
    } else {
        cells.push((&imgs[0], 6, 60.0));
    }

    // Repeat legs (t1b/autob) split "bytes depend on thread count" from
    // "bytes are not repeatable at all" — both are A3 violations, but they
    // are different bugs and the report must say which one fired.
    let legs: [(&str, Option<usize>); 5] = [
        ("t1", Some(1)),
        ("t1b", Some(1)),
        ("t8", Some(8)),
        ("auto", None),
        ("autob", None),
    ];
    let mut fail = 0usize;
    let t0 = Instant::now();
    for (img, speed, quality) in &cells {
        let mut ref_bytes: Option<Vec<u8>> = None;
        for (leg, threads) in &legs {
            let cfg = config(
                *quality,
                *speed,
                EncodeChromaSubsampling::Yuv420,
                EncodeBitDepth::Eight,
                *threads,
            );
            let bytes = encode(img.img.as_ref(), &cfg);
            match &ref_bytes {
                None => ref_bytes = Some(bytes),
                Some(r) => {
                    if *r != bytes {
                        fail += 1;
                        println!(
                            "DETFAIL {}/s{}/q{} leg {} differs from t1 ({} vs {} bytes, \
                             fnv {:016x} vs {:016x})",
                            img.name,
                            speed,
                            quality,
                            leg,
                            bytes.len(),
                            r.len(),
                            fnv1a64(&bytes),
                            fnv1a64(r)
                        );
                    }
                }
            }
        }
    }
    println!(
        "gate-determinism{}: {} cells x {} thread legs in {:.1}s, {} failures",
        if ci { " [ci]" } else { "" },
        cells.len(),
        legs.len(),
        t0.elapsed().as_secs_f32(),
        fail
    );
    if fail > 0 {
        println!("gate-determinism: FAIL");
        std::process::exit(1);
    }
    println!("gate-determinism: PASS");
}

// ---------------------------------------------------------------------------
// cells (conformance matrix emission)
// ---------------------------------------------------------------------------

fn run_cells(outdir: &Path, ci: bool) {
    fs::create_dir_all(outdir).expect("create cells dir");
    let imgs = pinned_images();
    let (img_sel, speeds, qualities): (Vec<usize>, Vec<u8>, Vec<f32>) = if ci {
        (vec![0, 2], vec![6, 10], vec![60.0])
    } else {
        (vec![0, 1, 2], vec![2, 6, 10], vec![35.0, 60.0, 85.0])
    };
    let subs = [
        ("420", EncodeChromaSubsampling::Yuv420),
        ("444", EncodeChromaSubsampling::Yuv444),
    ];

    struct Cell {
        name: String,
        img: usize,
        speed: u8,
        quality: f32,
        sub: EncodeChromaSubsampling,
        depth: EncodeBitDepth,
    }
    let mut cells: Vec<Cell> = Vec::new();
    for &ii in &img_sel {
        for &s in &speeds {
            for &q in &qualities {
                for (stag, sub) in &subs {
                    cells.push(Cell {
                        name: format!("{}_s{}_q{}_{}_d8", imgs[ii].name, s, q as u32, stag),
                        img: ii,
                        speed: s,
                        quality: q,
                        sub: *sub,
                        depth: EncodeBitDepth::Eight,
                    });
                }
            }
        }
    }
    // 10-bit legs (the shipped default depth for cavif-style encodes).
    for (stag, sub) in &subs {
        cells.push(Cell {
            name: format!("photo_s6_q60_{stag}_d10"),
            img: 0,
            speed: 6,
            quality: 60.0,
            sub: *sub,
            depth: EncodeBitDepth::Ten,
        });
    }

    let results: Vec<OnceLock<Vec<u8>>> = cells.iter().map(|_| OnceLock::new()).collect();
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4)
        .min(4);
    let t0 = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= cells.len() {
                        break;
                    }
                    let c = &cells[i];
                    let cfg = config(c.quality, c.speed, c.sub, c.depth, Some(1));
                    results[i]
                        .set(encode(imgs[c.img].img.as_ref(), &cfg))
                        .expect("dup cell");
                }
            });
        }
    });

    let mut manifest = String::from("# cell\tfile\twidth\theight\n");
    for (i, c) in cells.iter().enumerate() {
        let bytes = results[i].get().unwrap();
        let file = outdir.join(format!("{}.avif", c.name));
        fs::write(&file, bytes).expect("write avif");
        let img = &imgs[c.img];
        let _ = writeln!(
            manifest,
            "{}\t{}\t{}\t{}",
            c.name,
            file.display(),
            img.img.width(),
            img.img.height()
        );
    }
    fs::write(outdir.join("manifest.tsv"), &manifest).expect("write manifest");
    write_screen_y4m(outdir, true).expect("write screen_420.y4m");
    write_screen_y4m(outdir, false).expect("write screen_444.y4m");
    println!(
        "gate-cells{}: {} avif cells + 2 y4m inputs -> {} in {:.1}s",
        if ci { " [ci]" } else { "" },
        cells.len(),
        outdir.display(),
        t0.elapsed().as_secs_f32()
    );
}

// ---------------------------------------------------------------------------
// ladder (perf floors)
// ---------------------------------------------------------------------------

fn to_arr_img(img: ImgRef<'_, Rgb<u8>>) -> ImgVec<[u8; 3]> {
    let (w, h) = (img.width(), img.height());
    let buf: Vec<[u8; 3]> = img.pixels().map(|p| [p.r, p.g, p.b]).collect();
    Img::new(buf, w, h)
}

fn ladder_envelope_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("benchmarks/gate_ladder_envelope.tsv")
}

fn run_ladder(pin: bool) {
    let imgs = pinned_images();
    // The three ladder tiers of the program record (s2-tune / s6-composed /
    // s10' class). On the pre-dep-bump chain these resolve to the registry
    // zenrav1e behavior; the envelope is re-pinned at the flip (that diff
    // IS the documented ladder movement).
    let tiers: [(&str, u8); 3] = [("s2", 2), ("s6", 6), ("s10", 10)];
    let qualities = [35.0f32, 60.0, 85.0];
    let plat = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);

    let mut rows: Vec<(String, usize, f64, f64)> = Vec::new(); // cell, bytes, ssim2, ms
    for img in &imgs {
        let src = to_arr_img(img.img.as_ref());
        for (tname, speed) in &tiers {
            for q in &qualities {
                let cfg = config(
                    *q,
                    *speed,
                    EncodeChromaSubsampling::Yuv420,
                    EncodeBitDepth::Eight,
                    Some(8),
                );
                let t0 = Instant::now();
                let bytes = encode(img.img.as_ref(), &cfg);
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                // threads(1): registry rav1d-safe (< the 49df1fc0 release)
                // ships the zenavif#30 tile-worker futex wedge — a decode
                // under load can hang forever. Decode is not the timed leg
                // (enc_ms above measures encode only), so single-threaded
                // costs nothing and makes the gate wedge-immune (same
                // reason examples/ivf_raw.rs pins threads = 1). Revisit at
                // the rav1d-safe dep bump.
                let dec_cfg = zenavif::DecoderConfig::new().prefer_8bit(true).threads(1);
                let decoded = zenavif::decode_with(&bytes, &dec_cfg, &zenavif::Unstoppable)
                    .expect("gate_kit ladder: decode failed");
                let dec_rgb = decoded
                    .try_as_imgref::<Rgb<u8>>()
                    .expect("gate_kit ladder: decoded frame not RGB8");
                let dec_arr = to_arr_img(dec_rgb);
                let ssim2 = fast_ssim2::compute_ssimulacra2(src.as_ref(), dec_arr.as_ref())
                    .expect("gate_kit ladder: ssim2 failed");
                rows.push((
                    format!("{}/{}/q{}", tname, img.name, *q as u32),
                    bytes.len(),
                    ssim2,
                    ms,
                ));
            }
        }
    }

    let path = ladder_envelope_path();
    if pin {
        let mut out = String::new();
        out.push_str(
            "# gate_ladder envelope: pinned (bytes, ssim2, enc_ms) per ladder cell.\n\
             # Machine-scoped (enc_ms): pinned on the dev workstation; re-pin with\n\
             # `just gate-ladder-pin` after intentional ladder movement (commit the\n\
             # diff in the same commit). Tolerances at check time: bytes +/-2%,\n\
             # ssim2 +/-0.5, ms +/-25% (min 30 ms slack).\n\
             # platform\tcell\tbytes\tssim2\tenc_ms\n",
        );
        // Preserve other-platform rows.
        if path.exists() {
            for line in fs::read_to_string(&path).unwrap().lines() {
                if line.starts_with('#') || line.trim().is_empty() {
                    continue;
                }
                if !line.starts_with(&plat) {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        for (cell, bytes, ssim2, ms) in &rows {
            let _ = writeln!(out, "{plat}\t{cell}\t{bytes}\t{ssim2:.3}\t{ms:.1}");
        }
        fs::write(&path, out).expect("write envelope");
        println!(
            "gate-ladder: pinned {} cells for {plat} -> {}",
            rows.len(),
            path.display()
        );
        return;
    }

    let mut pins: std::collections::BTreeMap<String, (f64, f64, f64)> =
        std::collections::BTreeMap::new();
    if path.exists() {
        for line in fs::read_to_string(&path).unwrap().lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() == 5 && f[0] == plat {
                pins.insert(
                    f[1].to_string(),
                    (
                        f[2].parse().unwrap(),
                        f[3].parse().unwrap(),
                        f[4].parse().unwrap(),
                    ),
                );
            }
        }
    }
    if pins.is_empty() {
        println!(
            "gate-ladder: FAIL — no envelope rows for platform {plat} in {}.\n\
             Pin this machine deliberately with `just gate-ladder-pin`.",
            path.display()
        );
        std::process::exit(1);
    }

    let mut fail = 0usize;
    for (cell, bytes, ssim2, ms) in &rows {
        let Some(&(pb, ps, pm)) = pins.get(cell) else {
            println!("LADDER FAIL {cell}: no pinned row (grid changed?) — re-pin deliberately");
            fail += 1;
            continue;
        };
        let bytes = *bytes as f64;
        if (bytes - pb).abs() > pb * 0.02 {
            println!("LADDER FAIL {cell}: bytes {bytes:.0} vs pinned {pb:.0} (>2%)");
            fail += 1;
        }
        if (ssim2 - ps).abs() > 0.5 {
            println!("LADDER FAIL {cell}: ssim2 {ssim2:.3} vs pinned {ps:.3} (>0.5)");
            fail += 1;
        }
        let slack = (pm * 0.25).max(30.0);
        if (ms - pm).abs() > slack {
            println!("LADDER FAIL {cell}: enc_ms {ms:.1} vs pinned {pm:.1} (>25%/30ms)");
            fail += 1;
        }
    }
    println!(
        "gate-ladder [{plat}]: {} cells, {} tolerance failures",
        rows.len(),
        fail
    );
    if fail > 0 {
        println!("gate-ladder: FAIL");
        std::process::exit(1);
    }
    println!("gate-ladder: PASS");
}

// ---------------------------------------------------------------------------

fn monotone_envelope_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("benchmarks/gate_monotone_envelope.tsv")
}

/// Invariant (the user directive): RD must improve monotonically with encode
/// TIME. For each image+quality, no SLOWER tier may be Pareto-dominated
/// (<= bytes AND >= ssim2) by a clearly-faster one — spending more time must
/// never buy a worse RD point. Content-dependent arms (`fine_dir`, the s6-s8
/// `tx_size_rdo/intra7/part_prune` bundle) violate this off the photo
/// distribution until the per-image heads gate them (benchmarks/
/// mono_rd_vs_time_2026-07-05.tsv). The envelope records the KNOWN inversions
/// (machine/encoder-scoped); the gate FAILS on any NEW one. Goal state: empty.
///
/// Scope: this tests the RAW ladder — `encode()` at EXPLICIT speeds, NOT via
/// `auto_tune`. So `fast_heads::monotone_speed_gate` (which fixes auto_tune's
/// PICKS) does not affect this gate's envelope; the two are complementary layers
/// (see docs/MONOTONICITY_PROGRAM.md "Two layers"). Speeds 4-10 only — the
/// deep tiers 1-3 are slow and measured monotone (deep-tier check in the doc).
fn run_monotone(pin: bool) {
    let imgs = pinned_images();
    // Inversion-prone band; deep tiers (1-3) are slow and monotone in practice,
    // excluded to keep the gate fast (it runs every refactor commit).
    let speeds: [u8; 7] = [4, 5, 6, 7, 8, 9, 10];
    let qualities = [40.0f32, 80.0];
    let plat = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);

    // (cell, speed, bytes, ssim2, ms)
    let mut rows: Vec<(String, u8, usize, f64, f64)> = Vec::new();
    for img in &imgs {
        let src = to_arr_img(img.img.as_ref());
        for q in &qualities {
            for s in &speeds {
                let cfg = config(
                    *q,
                    *s,
                    EncodeChromaSubsampling::Yuv420,
                    EncodeBitDepth::Eight,
                    Some(8),
                );
                // bytes+ssim2 are deterministic; ms is not. Take min-of-3 encode
                // times — min converges to true cost (scheduling jitter only ADDS
                // time), so the pair-wise "faster" test is stable across runs.
                let mut bytes = encode(img.img.as_ref(), &cfg);
                let mut ms = f64::MAX;
                for _ in 0..3 {
                    let t0 = Instant::now();
                    bytes = encode(img.img.as_ref(), &cfg);
                    ms = ms.min(t0.elapsed().as_secs_f64() * 1000.0);
                }
                let dec_cfg = zenavif::DecoderConfig::new().prefer_8bit(true).threads(1);
                let decoded = zenavif::decode_with(&bytes, &dec_cfg, &zenavif::Unstoppable)
                    .expect("gate_kit monotone: decode failed");
                let dec_rgb = decoded
                    .try_as_imgref::<Rgb<u8>>()
                    .expect("gate_kit monotone: decoded frame not RGB8");
                let dec_arr = to_arr_img(dec_rgb);
                let ssim2 = fast_ssim2::compute_ssimulacra2(src.as_ref(), dec_arr.as_ref())
                    .expect("gate_kit monotone: ssim2 failed");
                rows.push((
                    format!("{}/q{}", img.name, *q as u32),
                    *s,
                    bytes.len(),
                    ssim2,
                    ms,
                ));
            }
        }
    }

    // Inversion = a tier B for which a clearly-faster tier A also Pareto-dominates
    // it. "Clearly faster" = A.ms < 0.85*B.ms (absorbs timing noise; real
    // inversions have >30% time gaps). bytes+ssim2 domination is deterministic.
    if std::env::var("GATE_MONOTONE_DEBUG").is_ok() {
        eprintln!("# cell\tspeed\tbytes\tssim2\tms(min3)");
        for (cell, s, b, q, ms) in &rows {
            eprintln!("{cell}\ts{s}\t{b}\t{q:.3}\t{ms:.0}");
        }
    }
    use std::collections::BTreeSet;
    let mut inversions: BTreeSet<(String, u8, u8)> = BTreeSet::new(); // (cell, slow, fast)
    let cells: BTreeSet<String> = rows.iter().map(|r| r.0.clone()).collect();
    for cell in &cells {
        let tiers: Vec<_> = rows.iter().filter(|r| &r.0 == cell).collect();
        for b in &tiers {
            for a in &tiers {
                if a.1 == b.1 {
                    continue;
                }
                // A "clearly faster" than B: A's min-time < 80% of B's (>=25%
                // faster). Wide margin so near-cost tiers never flap as inversions.
                let a_faster = a.4 < 0.80 * b.4;
                let a_dominates = a.2 <= b.2 && a.3 >= b.3 && (a.2 < b.2 || a.3 > b.3);
                if a_faster && a_dominates {
                    inversions.insert((cell.clone(), b.1, a.1));
                }
            }
        }
    }

    let path = monotone_envelope_path();
    if pin {
        let mut out = String::new();
        out.push_str(
            "# gate_monotone envelope: KNOWN RD-vs-time inversions (a slower tier\n\
             # Pareto-dominated by a clearly-faster one), per (platform, cell, slow, fast).\n\
             # The gate FAILS on any inversion NOT listed here (a NEW one). Machine/encoder-\n\
             # scoped: re-pin with `just gate-monotone-pin` after landing content-gates that\n\
             # REMOVE inversions (the shrinking envelope IS the progress) or at the dep-bump\n\
             # flip. Goal state: zero rows. platform\tcell\tslow\tfast\n",
        );
        if path.exists() {
            for line in fs::read_to_string(&path).unwrap().lines() {
                if line.starts_with('#') || line.trim().is_empty() {
                    continue;
                }
                if !line.starts_with(&plat) {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        for (cell, slow, fast) in &inversions {
            let _ = writeln!(out, "{plat}\t{cell}\ts{slow}\ts{fast}");
        }
        fs::write(&path, out).expect("write monotone envelope");
        println!(
            "gate-monotone: pinned {} inversion(s) for {plat} -> {}",
            inversions.len(),
            path.display()
        );
        return;
    }

    let mut allowed: BTreeSet<(String, u8, u8)> = BTreeSet::new();
    if path.exists() {
        for line in fs::read_to_string(&path).unwrap().lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() >= 4 && f[0] == plat {
                let slow = f[2].trim_start_matches('s').parse::<u8>().unwrap_or(0);
                let fast = f[3].trim_start_matches('s').parse::<u8>().unwrap_or(0);
                allowed.insert((f[1].to_string(), slow, fast));
            }
        }
    }
    let new: Vec<_> = inversions
        .iter()
        .filter(|i| !allowed.contains(*i))
        .collect();
    let fixed = allowed.iter().filter(|i| !inversions.contains(*i)).count();
    println!(
        "gate-monotone: {} cells, {} inversion(s) ({} known, {} NEW, {} fixed) on {plat}",
        cells.len(),
        inversions.len(),
        inversions.len() - new.len(),
        new.len(),
        fixed
    );
    for (cell, slow, fast) in &inversions {
        let tag = if allowed.contains(&(cell.clone(), *slow, *fast)) {
            "known"
        } else {
            "NEW "
        };
        println!("  [{tag}] {cell}: s{slow} dominated by faster s{fast}");
    }
    if fixed > 0 {
        println!("  {fixed} envelope inversion(s) gone — re-pin to shrink the envelope.");
    }
    if !new.is_empty() {
        eprintln!(
            "gate-monotone: FAIL — {} NEW inversion(s) introduced (see above).",
            new.len()
        );
        std::process::exit(1);
    }
    println!("gate-monotone: PASS");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ci = args.iter().any(|a| a == "--ci");
    let pin = args.iter().any(|a| a == "--pin");
    match args.first().map(String::as_str) {
        Some("determinism") => run_determinism(ci),
        Some("cells") => {
            let dir = args
                .iter()
                .skip(1)
                .find(|a| !a.starts_with("--"))
                .expect("usage: gate_kit cells OUTDIR [--ci]");
            run_cells(Path::new(dir), ci);
        }
        Some("ladder") => run_ladder(pin),
        Some("monotone") => run_monotone(pin),
        _ => {
            eprintln!(
                "usage: gate_kit <determinism [--ci] | cells OUTDIR [--ci] | ladder [--pin] | monotone [--pin]>"
            );
            std::process::exit(2);
        }
    }
}
