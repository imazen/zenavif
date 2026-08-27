//! Encode peak-memory probe — one AVIF encode, report measured peak RSS (VmHWM).
//!
//! The ENCODE counterpart to `examples/heaptrack_decode.rs` (decode side) and a
//! raw-`.bin` sibling of `examples/avif_probe.rs` (the calibration harness that
//! produced the constants in `heuristics.rs`). Used by the heaptrack / VmHWM
//! sweep to calibrate the encode peak-memory model
//! (`heuristics::estimate_encode`, surfaced as the zencodec
//! `estimate_encode_resources`) against measured reality, *per effort level*
//! (the AV1 `speed` preset, the dominant cost knob), instead of the current
//! sub-linear `ENCODE_FIXED + bpp·pixels` guess.
//!
//!   cargo build -p zenavif --release --features encode --example mem_probe_encode
//!   GLIBC_TUNABLES=glibc.malloc.mmap_threshold=131072 \
//!     ./target/release/examples/mem_probe_encode <rgb8.bin> <w> <h> <avif> <speed 0..10> <quality>
//!   heaptrack ./target/release/examples/mem_probe_encode ...   # allocator peak heap
//!
//! One encode per process — peak RSS is a per-process high-water mark, so the
//! input must come from a cheap file read (raw RGB8 bin), never an in-process
//! decode (whose own peak would pollute VmHWM above the encode peak).
//!
//! TSV row:
//!   w h pixels mode speed quality out_bytes pre_rss_kb vmhwm_kb marginal_kb
//!   backend threads subsampling
//!
//! `pre_rss_kb` / `vmhwm_kb` read `/proc/self/status` and are therefore **0 on
//! non-Linux hosts**. On macOS wrap the probe in `/usr/bin/time -l` and read
//! "maximum resident set size" (bytes) instead — that is the whole-process
//! high-water mark, i.e. the binary floor + input buffer + encoder working set.
//!
//! Axis flags (anywhere in argv, consumed before the positionals):
//!   `--backend zenravif|svtrs`   AV1 encode backend. `svtrs` needs the
//!                                `encode-svt-rs` feature, and encodes 8-bit
//!                                4:2:0 only with both dimensions a multiple
//!                                of 64 (so 3840x2160 must become 3840x2176).
//!   `--subsampling 444|420`      zenavif defaults to 4:4:4; pass 420 for an
//!                                apples-to-apples comparison against SvtRs.
//!   `--threads N`                encoder threads (default 1). AV1 tile/row
//!                                threading adds per-thread contexts, so this
//!                                is a real memory axis, not just a time one.
//!
//! `est` mode (7th arg `est`): prints the codec's CURRENT model prediction for
//! this cell from `zenavif::heuristics::estimate_encode` (no encode), so model
//! vs measured can be compared in the same harness.
//!
//! ## Effort axis = AV1 `speed` (0..=10)
//!
//! 0 = slowest/densest search (most memory + by FAR the most time),
//! 10 = fastest. AVIF encode memory IS effort-dependent: the measured marginal
//! working set is ~38 B/px at speed 4 vs ~46 B/px at speed 10 (denser search at
//! low speed actually holds *less* peak — RDO is depth-first, the fast modes
//! buffer more), and `heuristics::estimate_encode` clamps the time curve to the
//! speed-4 anchor below 4 (speeds 1–3 unmeasured; speed 0 is NOT a real AV1
//! preset — zenravif/`speed_value` ultimately maps it through the AV1 range).
//! Representative levels to sweep: **10 (fast/default-ish), 6 (mid), 2 (slow)**.
//!   // VERIFY: AVOID speed 0/1 at 4096² — single-thread AV1 search there is
//!   // minutes-long. Run large sizes only at speed >=6 unless the parent's
//!   // resource cap explicitly budgets the time. The probe pins threads(Some(1))
//!   // so the peak is the clean single-thread working set; with N threads the AV1
//!   // tile contexts add ~mem_bytes_per_thread each (see encode_threading_info).

use std::hint::black_box;

use almost_enough::{StopToken, Unstoppable};
use imgref::Img;
use rgb::Rgb;
use zenavif::{Av1Backend, EncodeChromaSubsampling, EncoderConfig};

mod counting_alloc {
    use core::alloc::{GlobalAlloc, Layout};
    use core::sync::atomic::{AtomicUsize, Ordering};

    pub static COUNT: AtomicUsize = AtomicUsize::new(0);
    pub static LIVE: AtomicUsize = AtomicUsize::new(0);
    pub static PEAK_LIVE: AtomicUsize = AtomicUsize::new(0);
    /// Size of the single allocation that last pushed `PEAK_LIVE` to a new
    /// high. Identifies the transient responsible for the peak, which
    /// `malloc_history` cannot show (it only samples LIVE allocations, so a
    /// short-lived spike is invisible to it).
    pub static PEAK_TRIGGER: AtomicUsize = AtomicUsize::new(0);
    /// Whether the peak was set by a realloc rather than a fresh alloc. A
    /// realloc transiently holds old+new, so a realloc-triggered peak means the
    /// fix is to pre-size the buffer, not to shrink it.
    pub static PEAK_FROM_REALLOC: AtomicUsize = AtomicUsize::new(0);
    /// `JXL_PEAK_TRACE_AT=<bytes>`: capture and print ONE backtrace the first
    /// time live bytes cross this threshold.
    ///
    /// Attribution taken from an RSS-polled `malloc_history` snapshot is
    /// unreliable — RSS crosses a threshold at a different instant than
    /// `PEAK_LIVE` is set, so the snapshot shows a nearby moment, not the peak.
    /// That error sent this work after the wrong allocation for several rounds.
    /// Run once to learn the peak, then set this just below it to get the stack
    /// AT the peak.
    pub static TRACE_AT: AtomicUsize = AtomicUsize::new(0);
    pub static TRACED: AtomicUsize = AtomicUsize::new(0);

    pub struct Counting;

    impl Counting {
        fn record_alloc(ptr: *mut u8, size: usize) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
            // Track BEFORE the peak check so a peak-setting allocation is in
            // the site map when the snapshot fires.
            crate::alloc_sites::track_alloc(ptr, size);
            // Monotonic max. Racy under contention by at most one concurrent
            // delta, which is irrelevant at the magnitudes we report.
            let prev = PEAK_LIVE.fetch_max(live, Ordering::Relaxed);
            if live > prev {
                PEAK_TRIGGER.store(size, Ordering::Relaxed);
                PEAK_FROM_REALLOC.store(0, Ordering::Relaxed);
                crate::alloc_sites::maybe_snapshot(live);
            }
            Self::maybe_trace(live, size);
        }

        /// Print one backtrace when live bytes first cross `TRACE_AT`.
        /// Guarded so the capture itself (which allocates) cannot recurse.
        fn maybe_trace(live: usize, size: usize) {
            let at = TRACE_AT.load(Ordering::Relaxed);
            if at == 0 || live < at {
                return;
            }
            if TRACED.swap(1, Ordering::Relaxed) != 0 {
                return;
            }
            let bt = std::backtrace::Backtrace::force_capture();
            eprintln!("[peak-trace] live={live} triggered_by={size}\n{bt}");
        }
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { std::alloc::System.alloc(layout) };
            if !p.is_null() {
                Self::record_alloc(p, layout.size());
            }
            p
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { std::alloc::System.alloc_zeroed(layout) };
            if !p.is_null() {
                Self::record_alloc(p, layout.size());
            }
            p
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            crate::alloc_sites::track_free(ptr, layout.size());
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            unsafe { std::alloc::System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let p = unsafe { std::alloc::System.realloc(ptr, layout, new_size) };
            if !p.is_null() {
                // A realloc is one allocator round-trip, and between the two
                // sizes the allocator may hold both — model the worst case so
                // growth-by-realloc shows up in the peak rather than hiding.
                COUNT.fetch_add(1, Ordering::Relaxed);
                let live = LIVE.fetch_add(new_size, Ordering::Relaxed) + new_size;
                crate::alloc_sites::track_realloc(ptr, layout.size(), p, new_size);
                let prev = PEAK_LIVE.fetch_max(live, Ordering::Relaxed);
                if live > prev {
                    PEAK_TRIGGER.store(new_size, Ordering::Relaxed);
                    PEAK_FROM_REALLOC.store(1, Ordering::Relaxed);
                    crate::alloc_sites::maybe_snapshot(live);
                }
                Self::maybe_trace(live, new_size);
                LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            }
            p
        }
    }
}

/// Per-site allocation profiler (`JXL_ALLOC_SITES=1`): for every allocation of
/// at least `JXL_ALLOC_SITE_MIN` bytes (default 64 KiB), capture the raw call
/// stack (unresolved instruction pointers — ~1-2 us, no symbolization) and
/// aggregate per unique stack: total bytes ever allocated, allocation count,
/// live bytes, and the site's own live high-water.
///
/// Whenever the global live high-water rises by `JXL_ALLOC_SNAP_STEP` (default
/// 8 MiB) past the last snapshot, the per-site live map is snapshotted — so at
/// exit we hold the per-site composition AT (within one step of) the peak
/// instant. That is the number that answers "which code line owns the peak",
/// which neither total-churn profiles (heaptrack's default view) nor
/// RSS-polled `malloc_history` snapshots answer: the former counts bytes that
/// were never simultaneously live, the latter samples the wrong instant.
///
/// Symbolization happens once, at exit. Attribution picks the innermost frame
/// that lands in jxl-encoder/zenjxl source (inlined frames are expanded, so a
/// user function inlined into rayon plumbing still attributes correctly).
/// `JXL_ALLOC_SITES_OUT=<path>` additionally writes full resolved stacks per
/// site.
///
/// The profiler's own maps allocate through the same global allocator (guarded
/// against recursion, but still COUNTED), so a profiled run's `peak_live` /
/// `alloc_count` are a few MB / few hundred above a clean run's — take
/// canonical numbers from runs without `JXL_ALLOC_SITES`.
mod alloc_sites {
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::sync::Mutex;

    pub const MAX_FRAMES: usize = 26;

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    pub struct SiteKey {
        len: u8,
        frames: [usize; MAX_FRAMES],
    }

    #[derive(Clone, Copy, Default)]
    pub struct SiteStats {
        pub total: u64,
        pub count: u64,
        pub live: i64,
        pub live_max: i64,
    }

    #[derive(Default)]
    struct Prof {
        sites: HashMap<SiteKey, SiteStats>,
        /// tracked pointer -> (site, size); entries only for allocations >= min.
        ptrs: HashMap<usize, (SiteKey, usize)>,
        /// (global live bytes, per-site live) at the highest snapshot so far.
        snap: Option<(usize, Vec<(SiteKey, i64)>)>,
    }

    pub static ENABLED: AtomicUsize = AtomicUsize::new(0);
    pub static SITE_MIN: AtomicUsize = AtomicUsize::new(64 * 1024);
    pub static SNAP_STEP: AtomicUsize = AtomicUsize::new(8 * 1024 * 1024);
    static SNAP_AT: AtomicUsize = AtomicUsize::new(0);
    static PROF: Mutex<Option<Prof>> = Mutex::new(None);

    thread_local! {
        /// Recursion guard: the tracker's own map/snapshot allocations re-enter
        /// the global allocator; with the guard set they are counted globally
        /// but not tracked per-site.
        static GUARD: Cell<bool> = const { Cell::new(false) };
    }

    #[inline]
    pub fn enabled() -> bool {
        ENABLED.load(Ordering::Relaxed) != 0
    }
    #[inline]
    fn site_min() -> usize {
        SITE_MIN.load(Ordering::Relaxed)
    }

    /// Capture raw frame IPs. No symbolization, no allocation on the steady
    /// path. Innermost frames first; the allocator's own frames are skipped at
    /// resolve time by symbol filter (inlining makes skip-counts unreliable).
    fn capture() -> SiteKey {
        let mut key = SiteKey {
            len: 0,
            frames: [0; MAX_FRAMES],
        };
        backtrace::trace(|frame| {
            let i = key.len as usize;
            if i >= MAX_FRAMES {
                return false;
            }
            key.frames[i] = frame.ip() as usize;
            key.len += 1;
            true
        });
        key
    }

    fn with_prof(f: impl FnOnce(&mut Prof)) {
        let mut lock = PROF.lock().unwrap_or_else(|e| e.into_inner());
        f(lock.get_or_insert_with(Prof::default));
    }

    pub fn track_alloc(ptr: *mut u8, size: usize) {
        if !enabled() || size < site_min() {
            return;
        }
        GUARD.with(|g| {
            if g.get() {
                return;
            }
            g.set(true);
            let key = capture();
            with_prof(|p| {
                let s = p.sites.entry(key).or_default();
                s.total += size as u64;
                s.count += 1;
                s.live += size as i64;
                s.live_max = s.live_max.max(s.live);
                p.ptrs.insert(ptr as usize, (key, size));
            });
            g.set(false);
        });
    }

    pub fn track_free(ptr: *mut u8, size: usize) {
        if !enabled() || size < site_min() {
            return;
        }
        GUARD.with(|g| {
            if g.get() {
                return;
            }
            g.set(true);
            with_prof(|p| {
                if let Some((key, sz)) = p.ptrs.remove(&(ptr as usize))
                    && let Some(s) = p.sites.get_mut(&key) {
                        s.live -= sz as i64;
                    }
            });
            g.set(false);
        });
    }

    /// Realloc = free(old) + alloc(new) attributed to the realloc call site
    /// (the growing vec's push/reserve line). If old == new (in-place) the map
    /// entry is replaced. Tracking happens after the system realloc, so a
    /// same-address reuse by another thread in that window can momentarily
    /// mis-attribute one buffer — acceptable for a measurement tool.
    pub fn track_realloc(old_ptr: *mut u8, old_size: usize, new_ptr: *mut u8, new_size: usize) {
        let min = site_min();
        if !enabled() || (old_size < min && new_size < min) {
            return;
        }
        GUARD.with(|g| {
            if g.get() {
                return;
            }
            g.set(true);
            let key = (new_size >= min).then(capture);
            with_prof(|p| {
                if old_size >= min
                    && let Some((k, sz)) = p.ptrs.remove(&(old_ptr as usize))
                        && let Some(s) = p.sites.get_mut(&k) {
                            s.live -= sz as i64;
                        }
                if let Some(key) = key {
                    let s = p.sites.entry(key).or_default();
                    s.total += new_size as u64;
                    s.count += 1;
                    s.live += new_size as i64;
                    s.live_max = s.live_max.max(s.live);
                    p.ptrs.insert(new_ptr as usize, (key, new_size));
                }
            });
            g.set(false);
        });
    }

    /// Snapshot the per-site live map when the global high-water has risen a
    /// full step past the last snapshot. Called only on peak raises, so after
    /// warmup it fires rarely; a peak set by a >= step allocation snapshots at
    /// exactly the peak instant (the triggering site is inserted first).
    pub fn maybe_snapshot(live: usize) {
        if !enabled() {
            return;
        }
        let at = SNAP_AT.load(Ordering::Relaxed);
        if live < at.saturating_add(SNAP_STEP.load(Ordering::Relaxed)) {
            return;
        }
        GUARD.with(|g| {
            if g.get() {
                return;
            }
            g.set(true);
            SNAP_AT.store(live, Ordering::Relaxed);
            with_prof(|p| {
                let v: Vec<(SiteKey, i64)> = p
                    .sites
                    .iter()
                    .filter(|(_, s)| s.live > 0)
                    .map(|(k, s)| (*k, s.live))
                    .collect();
                p.snap = Some((live, v));
            });
            g.set(false);
        });
    }

    // ---- exit-time symbolization + report ----

    #[derive(Clone, Default)]
    struct RFrame {
        sym: String,
        file: String,
        line: u32,
    }

    fn resolve_ip(cache: &mut HashMap<usize, Vec<RFrame>>, ip: usize) -> Vec<RFrame> {
        if let Some(v) = cache.get(&ip) {
            return v.clone();
        }
        let mut out = Vec::new();
        // resolve() expands inlined frames: one ip can yield several logical
        // frames, innermost first — this is what keeps attribution working in
        // release builds where user code inlines into rayon plumbing.
        backtrace::resolve(ip as *mut core::ffi::c_void, |sym| {
            let mut f = RFrame::default();
            if let Some(n) = sym.name() {
                f.sym = strip_hash(&n.to_string());
            }
            if let Some(p) = sym.filename() {
                f.file = p.display().to_string();
            }
            f.line = sym.lineno().unwrap_or(0);
            out.push(f);
        });
        cache.insert(ip, out.clone());
        out
    }

    /// Strip mangling noise: legacy `::h<16 hex>` suffixes and v0 `[hash]`
    /// crate-disambiguator brackets (`jxl_encoder[10a2...]::` -> `jxl_encoder::`).
    fn strip_hash(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'['
                && let Some(j) = s[i + 1..].find(']') {
                    let inner = &s[i + 1..i + 1 + j];
                    if (8..=17).contains(&inner.len())
                        && inner.chars().all(|c| c.is_ascii_hexdigit())
                    {
                        i += j + 2;
                        continue;
                    }
                }
            out.push(b[i] as char);
            i += 1;
        }
        if let Some(i) = out.rfind("::h")
            && out.len() - i == 19 && out[i + 3..].chars().all(|c| c.is_ascii_hexdigit()) {
                out.truncate(i);
            }
        out
    }

    fn short_file(f: &str) -> String {
        for marker in ["/work/zen/", "/registry/src/"] {
            if let Some(i) = f.find(marker) {
                return f[i + marker.len()..].to_string();
            }
        }
        if let Some(i) = f.find("/rustc/") {
            let rest = &f[i + 7..];
            return match rest.find('/') {
                Some(j) => format!("rust:{}", &rest[j + 1..]),
                None => rest.to_string(),
            };
        }
        f.to_string()
    }

    /// Does this frame's function BELONG TO one of our crates? Checks the
    /// symbol's own crate-path prefix (v0-demangled, hash brackets stripped),
    /// not `contains` — `<alloc::vec::Vec<jxl_encoder::...::Channel>>::clone`
    /// names our type but is alloc's frame; the caller is the line we want.
    fn is_ours(fr: &RFrame) -> bool {
        let s = fr.sym.trim_start_matches('<');
        [
            "zenavif::",
            "zenravif::",
            "svtav1",
            "zenav1",
            "rav1e::",
            "zenrav1e::",
        ]
        .iter()
        .any(|p| s.starts_with(p))
            || ((fr.file.contains("zenavif/") || fr.file.contains("zenav1-svt/"))
                && !fr.file.contains("registry/src"))
    }

    fn is_noise(fr: &RFrame) -> bool {
        const NOISE_PREFIX: &[&str] = &[
            "alloc::",
            "core::",
            "std::",
            "hashbrown",
            "backtrace",
            "mem_probe_encode",
            "__",
            "_rjem",
        ];
        // File-based rules catch the capture/allocator plumbing whose
        // inline-expanded symbols dodge the prefix list (thread-local
        // closures, RawVec internals) — without these the full-stack dump's
        // frame budget is spent before any encoder frame appears.
        const NOISE_FILE: &[&str] = &["/rustlib/", "backtrace-0.", "examples/mem_probe_encode"];
        let s = fr.sym.trim_start_matches('<');
        s.is_empty()
            || NOISE_PREFIX.iter().any(|n| s.starts_with(n))
            || NOISE_FILE.iter().any(|n| fr.file.contains(n))
    }

    /// The frame a site is attributed to: innermost frame whose function is in
    /// jxl-encoder/zenjxl; else the innermost non-noise frame.
    fn attribute(frames: &[RFrame]) -> RFrame {
        frames
            .iter()
            .find(|fr| is_ours(fr))
            .or_else(|| frames.iter().find(|fr| !is_noise(fr)))
            .or_else(|| frames.first())
            .cloned()
            .unwrap_or_default()
    }

    fn short_sym(s: &str) -> String {
        let segs: Vec<&str> = s.split("::").collect();
        if segs.len() <= 4 {
            s.to_string()
        } else {
            segs[segs.len() - 4..].join("::")
        }
    }

    fn mb(b: i64) -> f64 {
        b as f64 / (1024.0 * 1024.0)
    }

    /// Symbolize + print the report. stderr gets the two ranked by-line tables
    /// (live-at-peak-snapshot and total churn); `out` gets full per-site
    /// resolved stacks.
    pub fn report(out: Option<&str>) {
        if !enabled() {
            return;
        }
        GUARD.with(|g| g.set(true));
        let (sites, snap) = {
            let mut lock = PROF.lock().unwrap_or_else(|e| e.into_inner());
            match lock.as_mut() {
                Some(p) => (
                    p.sites.iter().map(|(k, s)| (*k, *s)).collect::<Vec<_>>(),
                    p.snap.take(),
                ),
                None => (Vec::new(), None),
            }
        };
        let mut cache: HashMap<usize, Vec<RFrame>> = HashMap::new();
        let mut resolved: HashMap<SiteKey, Vec<RFrame>> = HashMap::new();
        let resolve_key = |key: &SiteKey, cache: &mut HashMap<usize, Vec<RFrame>>| {
            let mut frames = Vec::new();
            for &ip in &key.frames[..key.len as usize] {
                frames.extend(resolve_ip(cache, ip));
            }
            frames
        };

        // by-line aggregation of the snapshot (live at peak) and of totals.
        let line_of = |key: &SiteKey,
                       cache: &mut HashMap<usize, Vec<RFrame>>,
                       resolved: &mut HashMap<SiteKey, Vec<RFrame>>| {
            let frames = resolved
                .entry(*key)
                .or_insert_with(|| resolve_key(key, cache))
                .clone();
            let a = attribute(&frames);
            if a.file.is_empty() {
                short_sym(&a.sym)
            } else {
                format!("{}:{} {}", short_file(&a.file), a.line, short_sym(&a.sym))
            }
        };

        let mut at_peak: HashMap<String, (i64, u64)> = HashMap::new(); // live, count
        let (snap_live, snap_sites) = snap.unwrap_or((0, Vec::new()));
        for (key, live) in &snap_sites {
            let line = line_of(key, &mut cache, &mut resolved);
            let e = at_peak.entry(line).or_default();
            e.0 += live;
            e.1 += 1;
        }
        let mut churn: HashMap<String, (u64, u64)> = HashMap::new(); // total, count
        for (key, s) in &sites {
            let line = line_of(key, &mut cache, &mut resolved);
            let e = churn.entry(line).or_default();
            e.0 += s.total;
            e.1 += s.count;
        }

        if std::env::var("JXL_ALLOC_SITES_DEBUG").is_ok() {
            let mut ss = snap_sites.clone();
            ss.sort_by_key(|(_, l)| -*l);
            for (key, live) in ss.iter().take(3) {
                eprintln!(
                    "[sites-debug] site live={:.1} MiB len={} frames:",
                    mb(*live),
                    key.len
                );
                for &ip in &key.frames[..key.len as usize] {
                    let fr = resolve_ip(&mut cache, ip);
                    if fr.is_empty() {
                        eprintln!("    {ip:#x} <unresolved>");
                    } else {
                        for f in fr {
                            eprintln!("    {ip:#x} {} ({}:{})", f.sym, short_file(&f.file), f.line);
                        }
                    }
                }
            }
        }

        let tracked_at_peak: i64 = snap_sites.iter().map(|(_, l)| l).sum();
        eprintln!(
            "[sites] snapshot: global_live={:.1} MiB, tracked={:.1} MiB ({:.1}%), \
             small/untracked={:.1} MiB, {} sites, min_size={} B",
            mb(snap_live as i64),
            mb(tracked_at_peak),
            100.0 * tracked_at_peak as f64 / (snap_live as f64).max(1.0),
            mb(snap_live as i64 - tracked_at_peak),
            snap_sites.len(),
            SITE_MIN.load(Ordering::Relaxed),
        );

        let mut peak_rows: Vec<(&String, &(i64, u64))> = at_peak.iter().collect();
        peak_rows.sort_by_key(|(_, (l, _))| -*l);
        eprintln!("[sites] live at peak snapshot, by attributed line:");
        for (i, (line, (live, n))) in peak_rows.iter().take(30).enumerate() {
            eprintln!(
                "  {:>2}  {:>9.1} MiB  n={:<5} {}",
                i + 1,
                mb(*live),
                n,
                line
            );
        }

        let mut churn_rows: Vec<(&String, &(u64, u64))> = churn.iter().collect();
        churn_rows.sort_by_key(|(_, (t, _))| std::cmp::Reverse(*t));
        eprintln!("[sites] total allocated over run (churn), by attributed line:");
        for (i, (line, (total, n))) in churn_rows.iter().take(30).enumerate() {
            eprintln!(
                "  {:>2}  {:>9.1} MiB  n={:<7} {}",
                i + 1,
                mb(*total as i64),
                n,
                line
            );
        }

        if let Some(path) = out {
            use std::fmt::Write as _;
            let mut txt = String::new();
            let _ = writeln!(
                txt,
                "# per-site allocation report; snapshot global_live={} B, tracked={} B\n\
                 # ranked by live bytes at the peak snapshot; full resolved stacks",
                snap_live, tracked_at_peak
            );
            let mut snap_sorted = snap_sites.clone();
            snap_sorted.sort_by_key(|(_, l)| -*l);
            for (key, live) in snap_sorted.iter().take(80) {
                let frames = resolved
                    .entry(*key)
                    .or_insert_with(|| resolve_key(key, &mut cache))
                    .clone();
                let s = sites
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, s)| *s)
                    .unwrap_or_default();
                let _ = writeln!(
                    txt,
                    "\nsite live_at_peak={:.1} MiB site_live_max={:.1} MiB total={:.1} MiB count={}",
                    mb(*live),
                    mb(s.live_max),
                    mb(s.total as i64),
                    s.count
                );
                for fr in frames.iter().filter(|f| !is_noise(f)).take(18) {
                    let _ = writeln!(
                        txt,
                        "    {} ({}:{})",
                        short_sym(&fr.sym),
                        short_file(&fr.file),
                        fr.line
                    );
                }
            }
            if let Err(e) = std::fs::write(path, txt) {
                eprintln!("[sites] failed to write {path}: {e}");
            } else {
                eprintln!("[sites] full stacks written to {path}");
            }
        }
        GUARD.with(|g| g.set(false));
    }
}

#[global_allocator]
static ALLOC: counting_alloc::Counting = counting_alloc::Counting;

/// A `/proc/self/status` field in KiB (e.g. `VmRSS:`, `VmHWM:`).
fn status_kb(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(field))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

fn main() {
    // Optional flags are pulled out first so the positional shape (and the
    // `est` marker's slot) stays exactly as the 2026-06-23 calibration used it.
    let mut a: Vec<String> = std::env::args().collect();
    let mut threads: usize = 1;
    let mut backend = Av1Backend::Zenravif;
    let mut subsampling = EncodeChromaSubsampling::default();
    let mut i = 1;
    while i < a.len() {
        match a[i].as_str() {
            "--threads" => {
                threads = a[i + 1].parse().expect("--threads N");
                a.drain(i..i + 2);
            }
            "--backend" => {
                backend = match a[i + 1].as_str() {
                    "zenravif" => Av1Backend::Zenravif,
                    #[cfg(feature = "encode-svt-rs")]
                    "svtrs" => Av1Backend::SvtRs,
                    other => panic!("--backend must be zenravif|svtrs, got {other}"),
                };
                a.drain(i..i + 2);
            }
            // 4:4:4 is zenavif's shipped default; SvtRs encodes 4:2:0 only, so
            // an apples-to-apples backend comparison needs this axis explicit.
            "--subsampling" => {
                subsampling = match a[i + 1].as_str() {
                    "444" => EncodeChromaSubsampling::Yuv444,
                    "420" => EncodeChromaSubsampling::Yuv420,
                    other => panic!("--subsampling must be 444|420, got {other}"),
                };
                a.drain(i..i + 2);
            }
            _ => i += 1,
        }
    }
    if a.len() < 7 {
        eprintln!(
            "usage: mem_probe_encode <rgb8.bin> <w> <h> <avif> <speed 0..10> <quality> [est] \
             [--threads N] [--backend zenravif|svtrs]"
        );
        std::process::exit(2);
    }
    let path = &a[1];
    let w: u32 = a[2].parse().expect("w");
    let h: u32 = a[3].parse().expect("h");
    // 4th arg is the output mode tag. Only `avif` is meaningful here; it is
    // accepted (and echoed in the TSV) to keep the arg shape uniform with the
    // other codecs' probes, which take a subsampling/mode token in this slot.
    let mode = match a[4].as_str() {
        "avif" => a[4].clone(),
        other => panic!("mode must be avif, got {other}"),
    };
    let speed: u8 = a[5].parse().expect("speed");
    let quality: f32 = a[6].parse().expect("quality");

    // `est` mode: print what the CURRENT model predicts for this cell (min /
    // typical / max peak + time), no encode — so model vs measured can be
    // compared without an encode polluting VmHWM. RGB8 input ⇒ input_bpp = 3.
    if a.get(7).map(String::as_str) == Some("est") {
        let pixels = (w as u64) * (h as u64);
        let input_bpp: u8 = 3; // VERIFY: RGB8 packed; rgba=4, rgb16=6, rgba16=8.
        let est = zenavif::heuristics::estimate_encode(w, h, input_bpp, speed);
        let (min, typ, max, t) = est
            .map(|e| {
                (
                    e.peak_memory_bytes_min / 1024,
                    e.peak_memory_bytes / 1024,
                    e.peak_memory_bytes_max / 1024,
                    e.time_ms,
                )
            })
            .unwrap_or((0, 0, 0, 0.0));
        println!(
            "{w}\t{h}\t{pixels}\t{mode}\t{speed}\t{quality}\tEST\tmin_kb={min}\ttyp_kb={typ}\tmax_kb={max}\tmin_bpp={:.2}\ttyp_bpp={:.2}\tmax_bpp={:.2}\test_time_ms={t:.1}",
            (min * 1024) as f64 / pixels as f64,
            (typ * 1024) as f64 / pixels as f64,
            (max * 1024) as f64 / pixels as f64,
        );
        return;
    }

    let data = std::fs::read(path).expect("read rgb8.bin");
    assert_eq!(
        data.len(),
        (w as usize) * (h as usize) * 3,
        "bin size {} != w*h*3 {}",
        data.len(),
        (w as usize) * (h as usize) * 3
    );

    // Single-thread so the high-water mark is the clean per-pixel working set
    // the model targets (matches the `avif_probe` calibration, which pinned
    // threads=1). The AV1 `speed` preset is the effort axis.
    let config = EncoderConfig::new()
        .quality(quality)
        .speed(speed)
        .backend(backend)
        .chroma_subsampling(subsampling)
        .threads(Some(threads));

    // Pack the raw RGB8 bytes into the `Rgb<u8>` buffer the encoder takes.
    // VERIFY: this allocation (w*h*3 B) lands AFTER `pre` below, so it is part
    // of `marginal` — matching `avif_probe.rs` (which captures its baseline
    // `b0` *before* the equivalent `Vec<Rgb<u8>>` collect). Keeping it on the
    // measured side is deliberate so this probe stays comparable to the
    // constants already baked into `heuristics.rs`.
    let pre = status_kb("VmRSS:");

    let px: Vec<Rgb<u8>> = data
        .as_chunks::<3>().0.iter()
        .map(|c| Rgb {
            r: c[0],
            g: c[1],
            b: c[2],
        })
        .collect();
    let img = Img::new(px, w as usize, h as usize);

    {
        use core::sync::atomic::Ordering;
        if let Ok(v) = std::env::var("JXL_PEAK_TRACE_AT")
            && let Ok(n) = v.parse::<usize>() {
                counting_alloc::TRACE_AT.store(n, Ordering::Relaxed);
            }
        if std::env::var("JXL_ALLOC_SITES").is_ok_and(|v| v == "1") {
            if let Some(n) = std::env::var("JXL_ALLOC_SITE_MIN")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
            {
                alloc_sites::SITE_MIN.store(n, Ordering::Relaxed);
            }
            alloc_sites::ENABLED.store(1, Ordering::Relaxed);
        }
    }
    let out = zenavif::encode_rgb8(img.as_ref(), &config, StopToken::new(Unstoppable))
        .expect("encode_rgb8");
    {
        use core::sync::atomic::Ordering;
        let peak_live_kb = counting_alloc::PEAK_LIVE.load(Ordering::Relaxed) / 1024;
        eprintln!("[peak] peak_live={peak_live_kb} KB");
        alloc_sites::report(std::env::var("JXL_ALLOC_SITES_OUT").ok().as_deref());
    }

    // High-water mark immediately after encode — VmHWM is monotonic, so it
    // reflects the peak *during* the encode.
    let peak = status_kb("VmHWM:");

    let pixels = (w as u64) * (h as u64);
    let backend_tag = match backend {
        Av1Backend::Zenravif => "zenravif",
        #[cfg(feature = "encode-svt-rs")]
        Av1Backend::SvtRs => "svtrs",
        _ => "other",
    };
    let ss_tag = match subsampling {
        EncodeChromaSubsampling::Yuv444 => "444",
        EncodeChromaSubsampling::Yuv420 => "420",
    };
    println!(
        "{w}\t{h}\t{pixels}\t{mode}\t{speed}\t{quality}\t{}\t{pre}\t{peak}\t{}\t{backend_tag}\t{threads}\t{ss_tag}",
        out.avif_file.len(),
        peak.saturating_sub(pre)
    );
    black_box(&out.avif_file);
}
