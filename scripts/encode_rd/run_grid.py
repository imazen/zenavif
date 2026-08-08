#!/usr/bin/env python3
"""Matched-wall-clock encode RD harness — the measurement half.

Drives four AV1 encoders over one grid, times each encode, persists every
encoded bitstream content-addressed, decodes them all with ONE decoder, and
scores them with `zenmetrics`. Emits one TSV row per cell.

The analysis half is `analyze_matched.py`. The methodology — in particular
WHY the time axis is what it is — is in `README.md`; read that before
trusting a number out of here.

Run `--help` for the grid knobs. A one-line smoke:

    python3 run_grid.py --images gb82/city-lossless.png --sizes 256 \\
        --arms aom,svtc --reps 5 --out ~/tmp/encrd/cells.tsv
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

HOME = Path.home()
ZEN = HOME / "work" / "zen"
REPO = Path(__file__).resolve().parents[2]

# ---------------------------------------------------------------- binaries --

DEFAULTS = {
    "RD_TOOL": REPO / "target/release/examples/rd_tool",
    "ZENMETRICS": ZEN / "zenmetrics/target/release/zenmetrics",
    "RAV1E": ZEN / "zenrav1e/target/release/rav1e",
    "SVTC": ZEN / "zenav1-svt/Bin/Release/SvtAv1EncApp",
    "SVTRS": ZEN / "zenav1-svt/rust/target/release/examples/identity_run",
    "AOMENC": shutil.which("aomenc") or "/opt/homebrew/bin/aomenc",
    "CORPUS": ZEN / "codec-corpus",
}


def tool(name: str) -> Path:
    return Path(os.environ.get(name, str(DEFAULTS[name])))


def require(name: str) -> Path:
    p = tool(name)
    if not p.exists():
        sys.exit(
            f"missing {name}: {p}\n"
            f"  set ${name} or build it — see scripts/encode_rd/README.md 'Prerequisites'"
        )
    return p


def git_rev(d: Path) -> str:
    """Short revision of `d`, via git or — for a jj workspace, which has no
    `.git` and so silently answered "unknown" until this was noticed — jj."""
    for argv in (["git", "-C", str(d), "rev-parse", "--short", "HEAD"],
                 ["jj", "--repository", str(d), "log", "-r", "@",
                  "--no-graph", "-T", "commit_id.short()"]):
        try:
            r = subprocess.run(argv, capture_output=True, text=True, timeout=20)
            if r.returncode == 0 and r.stdout.strip():
                return r.stdout.strip()
        except Exception:
            continue
    return "unknown"


# -------------------------------------------------------------------- arms --
#
# One arm = one encoder driven at a MATCHED configuration. Matched means, for
# every arm: 8-bit, 4:2:0, one key frame, still-picture coding, ONE thread,
# ONE tile, constant-quantizer rate control with every adaptive-QP / psycho
# layer left at the encoder's own default.
#
# `rate` is each encoder's OWN quantizer knob on its OWN scale. They are NOT
# aligned and must not be: the analysis interpolates on achieved QUALITY, the
# way BD-rate does, so a per-arm rate scale is correct and a hand-aligned one
# would be a fudge. What each arm must do is SPAN the quality range of
# interest; `analyze_matched.py` reports each arm's achieved-quality span and
# refuses to interpolate outside it.


@dataclass
class Arm:
    name: str
    binkey: str
    ext: str                 # bitstream container written by this arm
    ladder: list[int]        # the time ladder, slowest-first
    ladder_knob: str
    rate_knob: str
    rates: list[int]
    # argv(bin, prep, rate, lad, out) -> list[str]
    argv: object
    # optional: parse the encoder's self-reported encode ms from stdout+stderr
    self_ms: object = None
    note: str = ""
    version: str = field(default="")


def _rav1e_self(txt: str) -> float | None:
    # "encoded 1 frames, 6.944 fps, ..." -> ms/frame. rav1e reports throughput,
    # not a clock, and its fps includes muxing; treated as a cross-check only.
    parts = txt.replace(",", " ").split()
    for i, t in enumerate(parts):
        if t == "fps" and i > 0:
            try:
                f = float(parts[i - 1])
                return 1000.0 / f if f > 0 else None
            except ValueError:
                return None
    return None


def _svtc_self(txt: str) -> float | None:
    for line in txt.splitlines():
        if "Total Encoding Time" in line:
            for tok in line.replace("\t", " ").split():
                if tok.isdigit():
                    return float(tok)
    return None


def _aom_self(txt: str) -> float | None:
    # "... 15107 us (66.19 fps)" — aomenc's own per-frame encode microseconds.
    # Scanned from the END: aomenc emits a "0 us" progress line before the real
    # one, and taking the first match silently reported every aom cell as 0 ms.
    parts = txt.replace("\r", " ").split()
    for i in range(len(parts) - 1, 0, -1):
        if parts[i] == "us":
            try:
                return float(parts[i - 1]) / 1000.0
            except ValueError:
                continue
    return None


def payload_bytes(p: Path) -> int:
    """AV1 payload size with the container stripped.

    An IVF wrapper costs 32 header + 12 per frame = 44 bytes for a still. Two
    arms here write IVF and one writes a bare OBU, so comparing file sizes
    would hand the OBU arm a free 44 bytes — 0.5% at a 256px cell and ~5% at a
    64x64 one, i.e. exactly the fixed-overhead term the sweep discipline says
    must be separated from the per-pixel term rather than smeared into it.
    """
    d = p.read_bytes()
    if len(d) >= 32 and d[:4] == b"DKIF":
        off, tot = 32, 0
        while off + 12 <= len(d):
            sz = int.from_bytes(d[off:off + 4], "little")
            off += 12 + sz
            tot += sz
        return tot
    if len(d) > 12 and d[4:8] == b"ftyp":
        return -1          # AVIF: container accounting is plane-2 work
    return len(d)


def build_arms(threads: int) -> dict[str, Arm]:
    t = str(threads)
    arms = {
        # ---- zenrav1e, through its own rav1e-compatible CLI (y4m -> IVF).
        # This is the CORE-plane form of our default encode chain: the same
        # zenrav1e the zenavif `encode` path uses, but fed the canonical y4m
        # so no arm wins on its private RGB->YUV. See README "Two planes".
        "zenrav1e": Arm(
            name="zenrav1e", binkey="RAV1E", ext="ivf",
            ladder_knob="speed", ladder=list(range(0, 11)),
            rate_knob="quantizer", rates=[],
            argv=lambda b, p, r, l, o: [
                str(b), str(p / "src.y4m"), "-o", str(o), "--still-picture",
                "-s", str(l), "--quantizer", str(r),
                "--threads", t, "--tiles", "1",
            ],
            self_ms=_rav1e_self,
            note="quantizer is qindex-domain 0..255",
        ),
        # ---- libaom reference (y4m -> IVF), all-intra usage.
        "aom": Arm(
            name="aom", binkey="AOMENC", ext="ivf",
            ladder_knob="cpu-used", ladder=list(range(0, 10)),
            rate_knob="cq-level", rates=[],
            argv=lambda b, p, r, l, o: [
                str(b), "--allintra", "--limit=1", "--end-usage=q",
                f"--cq-level={r}", f"--cpu-used={l}", f"--threads={t}",
                "--tile-columns=0", "--tile-rows=0",
                "--bit-depth=8", "--input-bit-depth=8",
                "-o", str(o), str(p / "src.y4m"),
            ],
            self_ms=_aom_self,
            note="--allintra (usage=2); cpu-used 0..9 in all-intra",
        ),
        # ---- SVT-AV1 C reference (y4m -> IVF), still-picture (--avif 1),
        # CQP (--rc 0 --aq-mode 0 pins the constant-quantizer mode --qp means).
        "svtc": Arm(
            name="svtc", binkey="SVTC", ext="ivf",
            ladder_knob="preset", ladder=list(range(0, 14)),
            rate_knob="qp", rates=[],
            argv=lambda b, p, r, l, o: [
                str(b), "-i", str(p / "src.y4m"), "-b", str(o),
                "--preset", str(l), "--rc", "0", "--aq-mode", "0",
                "--qp", str(r), "--input-depth", "8", "--avif", "1",
                "--color-range", "0", "--keyint", "-1", "--irefresh-type", "2",
                "--lp", t, "--tile-rows", "0", "--tile-columns", "0",
                "--progress", "0",
            ],
            self_ms=_svtc_self,
            note="--qp is CLI-domain 1..63 (QP 0 unreachable from the CLI)",
        ),
        # ---- zenav1-svt, the pure-Rust port, through its identity_run driver
        # (PNG -> raw I420 -> OBU). Its RGB->I420 is byte-identical to
        # rd_tool's (run_grid asserts it), so it sees the same input as the
        # y4m arms; but it PAYS a PNG decode inside the timed region that they
        # do not. That extra cost lands in this arm's alpha and the analysis
        # flags it. See README "What the clock contains".
        "svtrs": Arm(
            name="svtrs", binkey="SVTRS", ext="obu",
            ladder_knob="preset", ladder=list(range(0, 14)),
            rate_knob="qp", rates=[],
            argv=lambda b, p, r, l, o: [
                str(b), f"file:{p / 'ref.png'}", str(p.wh[0]), str(p.wh[1]),
                str(r), str(l), str(Path(o).with_suffix("")),
            ],
            note="cli_qp 0..63, same domain as SvtAv1EncApp --qp (perf_vs_c.rs)",
        ),
    }
    return arms


# Default rate grids — spaced for EVEN COVERAGE IN ACHIEVED QUALITY, not even
# spacing of the knob.
#
# The first version of these grids was uniform in each encoder's quantizer, and
# it was wrong. Measured on the probe (city-lossless @256, ladder 4): a uniform
# 63-scale grid put HALF its points below ssim2 10 — where the metric is not
# even monotone in rate any more (cq 58 -> 8.49, cq 62 -> 9.88) — and left the
# entire useful 30..90 band with three samples. RD curves built on that are
# noise.
#
# So the knob->quality curve was measured and inverted. It is piecewise-linear
# with a knee: on the 63-scale, ~0.91 ssim2 per unit up to qp 31, then ~1.93 per
# unit beyond it. These grids target ssim2 in ~7-point steps from ~92 down to
# ~15, which is what "low-q density EQUAL to high-q density" actually means —
# and note it comes out DENSER at the high end of the knob, the opposite of a
# naive uniform grid.
#
# This is a COVERAGE heuristic, not a calibration: it was fit on one photo, and
# other content shifts the mapping. It does not need to be exact, because the
# analysis interpolates on achieved quality regardless. It only needs every arm
# to have enough points spread across the band of interest. Section B of the
# analysis prints the achieved span so a bad fit is visible rather than silent.
_Q63 = [2, 4, 8, 12, 16, 20, 24, 27, 31, 33,
        35, 37, 39, 41, 44, 46, 48, 50, 52, 56]
RATE_GRIDS = {
    # qindex 0..255. Same target quality steps, inverted through zenrav1e's own
    # measured curve (~0.195 ssim2/unit to qindex 132, ~0.476 beyond).
    "zenrav1e": [6, 15, 33, 51, 69, 87, 105, 123, 138, 144,
                 152, 159, 167, 174, 182, 189, 196, 203, 218, 233],
    "aom": _Q63,
    "svtc": _Q63,
    # cli_qp; QP 0 is rejected upstream by design (zenav1-svt#5), so 2 is the
    # lowest point here and 1 would also be legal.
    "svtrs": _Q63,
}


# ------------------------------------------------------------------ corpus --

# content_class is recorded per image so the analysis can refuse to average a
# screen-content result into a photo median. Add classes here, never inline.
CONTENT = {
    "gb82": "photo",
    "clic2025": "photo",
    "CID22": "photo",
    "gb82-sc": "screen",
    # Full-page web screenshots (qoi-benchmark). Same class as gb82-sc, but
    # these are the only local sources whose long edge exceeds 2940 px, so they
    # are what carries the top size tier.
    "screenshot_web": "screen",
}


def classify(p: Path) -> str:
    # Line-art / plots stress different intra tooling than a screenshot of
    # text does (long clean edges and huge flat fields vs dense glyphs), so
    # they are their own class even when they live in a screen-content corpus.
    stem = p.stem.lower()
    if any(k in stem for k in ("graph", "plot", "chart", "diagram", "lineart")):
        return "line-art"
    for part in p.parts[::-1]:
        if part in CONTENT:
            return CONTENT[part]
    return "unknown"


# ------------------------------------------------------------------- prep --

class Prep:
    """One (image, size) input, prepared once and shared by every arm.

    `size_tag` is the ACHIEVED long edge, not the requested cap. prep never
    upscales, so asking for 1024 from a 576px source silently gives 576 — and
    a row labelled "1024" that is really 576 corrupts every per-size number
    downstream. The request is kept separately as `size_req` for the record.
    """

    def __init__(self, d: Path, src: Path, w: int, h: int, size_req: str):
        self.d, self.src = d, src
        self.wh = (w, h)
        self.size_req = size_req
        self.size_tag = str(max(w, h))

    def __truediv__(self, s):      # so arm argv can write `p / "src.y4m"`
        return self.d / s


def do_prep(rd: Path, src: Path, outdir: Path, max_dim: int | None, tag: str) -> Prep:
    outdir.mkdir(parents=True, exist_ok=True)
    argv = [str(rd), "prep", str(src), str(outdir)]
    if max_dim:
        argv.append(str(max_dim))
    r = subprocess.run(argv, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"prep {src}: {r.stderr.strip()}")
    w, h = (int(x) for x in r.stdout.split())
    f = subprocess.run([str(rd), "floor", str(outdir)], capture_output=True, text=True)
    if f.returncode != 0:
        raise RuntimeError(f"floor {src}: {f.stderr.strip()}")
    return Prep(outdir, src, w, h, tag)


# ------------------------------------------------------------------ timing --

def wait_idle(own: set[int], limit: float, budget_s: float) -> tuple[float, float, float]:
    """Block until no foreign process exceeds `limit` %CPU, or the budget runs
    out. Returns (foreign_pct_at_release, seconds_waited).

    This box is shared with other agents' builds and test runs. Timing a cell
    against a neighbour pegging a core does not produce a noisy number, it
    produces a WRONG one — and on an 8P+4E machine the scheduler may also shove
    the loser onto an E-core. Waiting is cheaper than discarding, and far
    cheaper than publishing a contaminated ladder.
    """
    t0 = time.time()
    while True:
        worst, cores = foreign_cpu(own)
        if cores <= limit or (time.time() - t0) >= budget_s:
            return worst, cores, time.time() - t0
        time.sleep(2.0)


def foreign_cpu(own: set[int]) -> tuple[float, float]:
    """(worst single foreign process %CPU, total foreign CPU in whole cores).

    Two numbers because they answer different questions on a 12-core box. The
    project rule "discard a cell taken with a foreign process over 25% CPU" was
    written for a shared box where one hog meant real contention; here a single
    neighbour at 100% is one core of twelve, and a 1-thread encode still lands
    on a free performance core. So the GATE is on free cores (can my encode get
    an uncontended core?) while the worst-single number is still recorded per
    cell, so any reader can re-filter on the strict rule after the fact.

    NOTE the macOS semantic: `ps -o %cpu` is CPU time over the process's whole
    LIFETIME, not an instantaneous sample. For a freshly-spawned hog (the case
    that matters) it reads true; for a long-lived process that was busy hours
    ago it over-reports, which errs toward waiting — the safe direction.
    """
    try:
        out = subprocess.run(
            ["ps", "-Ao", "pid,%cpu,comm", "-r"], capture_output=True, text=True, timeout=5
        ).stdout.splitlines()[1:25]
    except Exception:
        return -1.0, -1.0
    worst, total = 0.0, 0.0
    for line in out:
        f = line.split(None, 2)
        if len(f) < 3:
            continue
        try:
            pid, cpu = int(f[0]), float(f[1])
        except ValueError:
            continue
        if pid in own or f[2].strip() == "ps":
            continue
        worst = max(worst, cpu)
        total += cpu
    return worst, total / 100.0


_CALIB = (
    "import time\n"
    "t=time.perf_counter()\n"
    "x=0\n"
    "for i in range(6000000): x=(x*1103515245+12345)&0xFFFFFFFF\n"
    "print((time.perf_counter()-t)*1000)\n"
)


def _spin_ms(prefix: list[str]) -> float:
    best = float("inf")
    for _ in range(3):
        r = subprocess.run(prefix + [sys.executable, "-c", _CALIB],
                           capture_output=True, text=True)
        try:
            best = min(best, float(r.stdout.strip()))
        except ValueError:
            return float("nan")
    return best


def check_scheduling() -> tuple[float, float, bool]:
    """Refuse to time anything from a process already relegated to the
    efficiency cores. Returns (plain_ms, background_ms, ok).

    WHY THIS GATE EXISTS, and what was actually measured (M4 Pro, 8P+4E):

      plain, nice 5 (inherited from the agent harness)   452.6 ms
      nice -n 19                                         471.7 ms   (+4%)
      taskpolicy -b  (true darwin-background QoS)       2678.7 ms   (5.9x)

    So on macOS `nice` and E-core relegation are DIFFERENT mechanisms. `nice`
    barely moves a single-threaded job on an idle box — it only bites under
    contention. What costs 5.9x is the darwin BACKGROUND QoS class, which
    `nice` does not set and which is inherited silently by every child of a
    backgrounded process. A harness that assumed "not niced == fine" would
    publish 6x-slow numbers without a single warning.

    Rather than hardcode a machine-specific threshold, the check is relative
    and self-calibrating: run the same loop normally and under an explicitly
    backgrounded policy. If being explicitly backgrounded is NOT meaningfully
    slower, this process is already there.
    """
    plain = _spin_ms([])
    bg = _spin_ms(["/usr/sbin/taskpolicy", "-b"]) if sys.platform == "darwin" else float("nan")
    if not (bg == bg) or not (plain == plain):   # NaN: tool absent / non-macOS
        return plain, bg, True
    return plain, bg, bg > plain * 1.8


def n_perf_cores() -> int:
    for key in ("hw.perflevel0.logicalcpu", "hw.ncpu"):
        try:
            v = subprocess.run(["sysctl", "-n", key], capture_output=True,
                               text=True, timeout=5).stdout.strip()
            if v.isdigit():
                return int(v)
        except Exception:
            pass
    return os.cpu_count() or 4


def timed_run(argv: list[str], out: Path) -> tuple[float, int, str]:
    """One encode. Returns (wall_ms, bytes, combined stdout+stderr).

    NOT niced: on Apple silicon `nice` reassigns the process to the E-cores and
    distorts wall time by ~40x, so a niced timing is not a timing. Builds are
    niced elsewhere; timed runs never are.
    """
    if out.exists():
        out.unlink()
    t0 = time.perf_counter()
    r = subprocess.run(argv, capture_output=True, text=True)
    t1 = time.perf_counter()
    txt = (r.stdout or "") + "\n" + (r.stderr or "")
    if r.returncode != 0 or not out.exists():
        raise RuntimeError(f"encode failed rc={r.returncode}: {txt.strip()[:400]}")
    return (t1 - t0) * 1000.0, out.stat().st_size, txt


def sha256_file(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


# -------------------------------------------------------------------- main --

def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--images", required=True,
                    help="comma-separated paths, relative to $CORPUS or absolute")
    ap.add_argument("--sizes", default="256",
                    help="comma-separated long-edge caps, or 'native'")
    ap.add_argument("--arms", default="zenrav1e,aom,svtc,svtrs")
    ap.add_argument("--ladder", default="",
                    help="comma-separated ladder positions; default = each arm's own full ladder")
    ap.add_argument("--rates", default="",
                    help="comma-separated rate points, applied to EVERY arm. Only valid when the "
                         "arms share a quantizer scale — zenrav1e's is 0..255 and the rest are "
                         "0..63, so mixing them here silently compares different quality regimes. "
                         "Default = each arm's own RATE_GRIDS entry.")
    ap.add_argument("--rate-stride", type=int, default=1,
                    help="subsample each arm's default rate grid by this stride (endpoints kept)")
    ap.add_argument("--reps", type=int, default=5, help="timing repeats per cell (median)")
    ap.add_argument("--threads", type=int, default=1)
    ap.add_argument("--out", required=True)
    ap.add_argument("--workdir", default=str(HOME / "tmp/encrd/work"))
    ap.add_argument("--artifacts", default=str(HOME / "tmp/encrd/artifacts"))
    ap.add_argument("--no-score", action="store_true", help="encode+time only")
    ap.add_argument("--foreign-max", type=float, default=25.0,
                    help="drop a timing sample taken while a foreign process exceeds this %%CPU")
    ap.add_argument("--idle-budget", type=float, default=120.0,
                    help="seconds to wait per sample for the box to go idle before giving up")
    ap.add_argument("--verify-yuv", action="store_true",
                    help="assert identity_run's I420 == rd_tool's, byte for byte")
    ap.add_argument("--ladder-map", default="",
                    help="per-arm ladders: 'aom:2,4,6;svtc:0,4,8;...'. Overrides --ladder. "
                         "Every selected arm must be listed. A matched-TIME sweep needs "
                         "different rungs per arm — they are the ones that populate the "
                         "shared time overlap — and one --ladder list cannot express that.")
    ap.add_argument("--progress", default="",
                    help="append one line per timed encode here, flushed. A multi-hour run "
                         "with output only at the end is a run whose stall you find at hour 6.")
    args = ap.parse_args()

    rd = require("RD_TOOL")
    zm = None if args.no_score else require("ZENMETRICS")
    all_arms = build_arms(args.threads)
    arm_names = [a.strip() for a in args.arms.split(",") if a.strip()]
    for a in arm_names:
        if a not in all_arms:
            sys.exit(f"unknown arm {a!r}; known: {','.join(all_arms)}")
    arms = [all_arms[a] for a in arm_names]
    for a in arms:
        a.binpath = require(a.binkey)
    for a in arms:
        if args.rates:
            a.rates = [int(x) for x in args.rates.split(",")]
        else:
            g = RATE_GRIDS[a.name]
            a.rates = sorted(set(g[::args.rate_stride] + [g[0], g[-1]]))
        if args.ladder:
            a.ladder = [int(x) for x in args.ladder.split(",")]

    # Per-arm ladder override. One shared --ladder list is wrong for a
    # matched-TIME sweep: the arms' rungs are not comparable, so the rungs that
    # populate the shared time overlap are a DIFFERENT set on each arm (and
    # `--ladder 10` is not even legal on aom, whose ladder stops at 9). Running
    # one process per arm instead would break the interleaving the timing
    # discipline depends on, so the map lives inside one run.
    if args.ladder_map:
        seen = set()
        for spec in args.ladder_map.split(";"):
            spec = spec.strip()
            if not spec:
                continue
            nm, _, rungs = spec.partition(":")
            nm = nm.strip()
            if nm not in all_arms:
                sys.exit(f"--ladder-map: unknown arm {nm!r}")
            seen.add(nm)
            for a in arms:
                if a.name == nm:
                    a.ladder = [int(x) for x in rungs.split(",") if x.strip()]
        missing = [a.name for a in arms if a.name not in seen]
        if missing:
            sys.exit(f"--ladder-map given but says nothing about {','.join(missing)}; "
                     "list every selected arm so no rung set is silently a default")

    workdir, artdir = Path(args.workdir), Path(args.artifacts)
    workdir.mkdir(parents=True, exist_ok=True)
    artdir.mkdir(parents=True, exist_ok=True)

    corpus = tool("CORPUS")
    srcs = []
    for s in args.images.split(","):
        s = s.strip()
        if not s:
            continue
        p = Path(s) if Path(s).is_absolute() else corpus / s
        if not p.exists():
            sys.exit(f"missing image: {p}")
        srcs.append(p)

    sizes = [(None, "native") if s == "native" else (int(s), s)
             for s in args.sizes.split(",") if s.strip()]

    # ---- prep every (image, size) once ------------------------------------
    preps: list[Prep] = []
    for src in srcs:
        for md, tag in sizes:
            d = workdir / f"{src.stem}__{tag}"
            preps.append(do_prep(rd, src, d, md, tag))
    for p in preps:
        if p.size_req != "native" and p.size_tag != p.size_req:
            print(f"  NOTE {p.src.name}: asked for {p.size_req}px, source gives "
                  f"{p.size_tag}px (prep never upscales) — row is tagged {p.size_tag}",
                  file=sys.stderr)
    print(f"prepped {len(preps)} inputs", file=sys.stderr)

    # ---- cross-harness input check ----------------------------------------
    # zenav1-svt's identity_run reimplements the same BT.601 forward transform.
    # If the two ever disagree the svtrs arm is silently encoding different
    # pixels than everyone else, which would look like an RD result.
    if args.verify_yuv and any(a.name == "svtrs" for a in arms):
        ir = require("SVTRS")
        for p in preps:
            pre = workdir / "_yuvchk"
            subprocess.run([str(ir), f"file:{p.d/'ref.png'}", str(p.wh[0]), str(p.wh[1]),
                            "40", "10", str(pre)], capture_output=True, text=True)
            a, b = sha256_file(Path(f"{pre}.yuv")), sha256_file(p.d / "src.yuv")
            status = "OK " if a == b else "MISMATCH"
            print(f"  yuv-check {status} {p.d.name} {a[:12]} {b[:12]}", file=sys.stderr)
            if a != b:
                sys.exit("identity_run and rd_tool disagree on the canonical I420 — "
                         "the svtrs arm is not comparable; fix before measuring")

    # ---- build the cell list ----------------------------------------------
    cells = []
    for p in preps:
        for a in arms:
            for lad in a.ladder:
                for rate in a.rates:
                    cells.append({"prep": p, "arm": a, "lad": lad, "rate": rate,
                                  "walls": [], "bytes": None, "self_ms": None,
                                  "foreign": 0.0, "fcores": 0.0, "fail": ""})
    print(f"{len(cells)} cells x {args.reps} reps = {len(cells)*args.reps} encodes",
          file=sys.stderr)

    calib_ms, bg_ms, sched_ok = check_scheduling()
    print(f"scheduling check: calibration loop {calib_ms:.0f} ms plain vs {bg_ms:.0f} ms "
          f"explicitly backgrounded", file=sys.stderr)
    if not sched_ok:
        sys.exit(
            "REFUSING TO MEASURE: this process is already on the efficiency cores "
            "(being explicitly backgrounded costs it nothing), so every timing would "
            "be several times too slow. Re-launch outside the background QoS class.\n"
            "  See check_scheduling() for the measurement this gate is built on.")
    print(f"  OK — full-speed cores (background would cost {bg_ms/calib_ms:.1f}x)",
          file=sys.stderr)

    nperf = n_perf_cores()
    # Leave the encode a core of its own plus one for the harness. On this
    # 8P+4E M4 Pro that permits up to nperf-2 = 6 foreign cores of load.
    free_core_limit = max(1.0, float(nperf - args.threads - 1))
    print(f"host: {nperf} performance cores; gating at <= {free_core_limit:.0f} "
          f"foreign cores busy (threads={args.threads})", file=sys.stderr)
    own = {os.getpid()}
    encdir = workdir / "_enc"
    encdir.mkdir(exist_ok=True)

    # ---- timed passes ------------------------------------------------------
    # Interleaved and ROTATED: pass r visits the cells starting at offset
    # r*stride, so no arm always runs first (or always runs into a cold cache
    # / a thermal state another arm created). Median over reps kills the rest.
    t_start = time.time()
    stats = {"enc": 0, "dropped": 0, "waited": 0.0}

    # Continuous progress. One flushed line per timed encode, so `tail -f`
    # shows a stall within a cell's own duration rather than at the end of the
    # run. Cheap: a few hundred bytes per encode.
    prog = open(args.progress, "a", buffering=1) if args.progress else None
    if prog:
        prog.write(f"# encode_rd progress {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())} "
                   f"cells={len(cells)} reps={args.reps}\n")
        prog.write("elapsed_s\tphase\tdone\ttotal\tcell\timage\tsize\tarm\tladder\trate\t"
                   "ms\tbytes_av1\twaited_s\tfcores\n")

    def take_sample(idx: int, phase: str = "rep") -> None:
        c = cells[idx]
        if c["fail"]:
            return
        a, p = c["arm"], c["prep"]
        out = encdir / f"cell{idx}.{a.ext}"
        fpct, fcores, waited = wait_idle(own, free_core_limit, args.idle_budget)
        stats["waited"] += waited
        try:
            ms, nbytes, txt = timed_run(
                a.argv(a.binpath, p, c["rate"], c["lad"], out), out)
        except RuntimeError as e:
            c["fail"] = str(e)[:200]
            if prog:
                prog.write(f"{time.time()-t_start:.1f}\t{phase}\t{stats['enc']}\t"
                           f"{len(cells)*args.reps}\t{idx}\t{p.src.name}\t{p.size_tag}\t"
                           f"{a.name}\t{c['lad']}\t{c['rate']}\tFAIL\t\t"
                           f"{waited:.0f}\t{fcores:.2f}\n")
            return
        stats["enc"] += 1
        c["foreign"] = max(c["foreign"], fpct)
        c["fcores"] = max(c.get("fcores", 0.0), fcores)
        # Bytes are recorded even for a contaminated sample: a foreign process
        # perturbs the CLOCK, never the bitstream, and the determinism check
        # below is more valuable with every sample feeding it.
        if c["bytes"] is None:
            c["bytes"] = nbytes
            c["av1"] = payload_bytes(out)
            c["sha"] = sha256_file(out)
            dst = artdir / f"{c['sha']}.{a.ext}"
            if not dst.exists():
                shutil.copyfile(out, dst)
            if a.self_ms:
                c["self_ms"] = a.self_ms(txt)
        elif c["bytes"] != nbytes:
            # Byte non-determinism across reps at a fixed config. Real for some
            # threaded encoders; a finding, not something to average away.
            c["fail"] = f"nondeterministic bytes {c['bytes']} vs {nbytes}"
            return
        if prog:
            prog.write(f"{time.time()-t_start:.1f}\t{phase}\t{stats['enc']}\t"
                       f"{len(cells)*args.reps}\t{idx}\t{p.src.name}\t{p.size_tag}\t"
                       f"{a.name}\t{c['lad']}\t{c['rate']}\t{ms:.3f}\t{c.get('av1','')}\t"
                       f"{waited:.0f}\t{fcores:.2f}\n")
        if fcores > free_core_limit:
            # Not enough free cores even after waiting out the budget. Drop the
            # clock (the bytes above are still kept — a neighbour cannot change
            # a bitstream) and let the top-up pass re-take it.
            stats["dropped"] += 1
            return
        c["walls"].append(ms)

    for rep in range(args.reps):
        stride = max(1, len(cells) // max(1, args.reps))
        order = list(range(len(cells)))
        off = (rep * stride) % max(1, len(cells))
        order = order[off:] + order[:off]
        for idx in order:
            take_sample(idx, f"rep{rep+1}")
        print(f"  rep {rep+1}/{args.reps} done ({stats['enc']} encodes, "
              f"{time.time()-t_start:.0f}s)", file=sys.stderr)
        # Checkpoint the raw clocks after every rep. A grid that takes hours
        # and only materialises at the end is a grid you lose entirely to one
        # interruption; the encodes themselves are already safe (artifacts are
        # content-addressed as they land), but the CLOCKS live only in memory.
        if args.progress:
            ck = Path(args.progress).with_suffix(".ckpt.json")
            try:
                ck.write_text(json.dumps([
                    {"i": i, "image": c["prep"].src.name, "size": c["prep"].size_tag,
                     "arm": c["arm"].name, "lad": c["lad"], "rate": c["rate"],
                     "sha": c.get("sha", ""), "av1": c.get("av1", ""),
                     "walls": c["walls"], "fail": c["fail"]}
                    for i, c in enumerate(cells)]))
            except Exception as e:      # a checkpoint must never kill the run
                print(f"  checkpoint failed (continuing): {e}", file=sys.stderr)

    # Top-up: a cell whose clock was contaminated is SHORT of reps, and a
    # median over 2 kept samples is not the median the rules asked for. Re-take
    # only the short cells, up to `reps` extra attempts each.
    for attempt in range(args.reps):
        short = [i for i, c in enumerate(cells)
                 if not c["fail"] and len(c["walls"]) < args.reps]
        if not short:
            break
        print(f"  top-up {attempt+1}: {len(short)} short cells", file=sys.stderr)
        for idx in short:
            take_sample(idx, f"topup{attempt+1}")
    still_short = sum(1 for c in cells if not c["fail"] and len(c["walls"]) < args.reps)
    print(f"  {stats['dropped']} samples dropped for foreign CPU >25%; "
          f"{still_short} cells still short of {args.reps} reps", file=sys.stderr)

    # ---- artifact-landed gate ---------------------------------------------
    landed = len(list(artdir.glob("*.ivf"))) + len(list(artdir.glob("*.obu")))
    print(f"artifacts persisted: {landed} in {artdir}", file=sys.stderr)
    if landed == 0:
        sys.exit("NO ARTIFACTS PERSISTED — fix before scaling the grid")

    # ---- decode + score ----------------------------------------------------
    scores: dict[tuple[str, str], dict] = {}
    floors: dict[str, dict] = {}
    if not args.no_score:
        pngdir = workdir / "_png"
        pngdir.mkdir(exist_ok=True)
        by_prep: dict[str, list[tuple[str, Path]]] = {}
        for c in cells:
            if c["fail"] or c["bytes"] is None:
                continue
            sha, ext = c["sha"], c["arm"].ext
            png = pngdir / f"{sha}.png"
            if not png.exists():
                r = subprocess.run([str(rd), "decode", str(artdir / f"{sha}.{ext}"), str(png)],
                                   capture_output=True, text=True)
                if r.returncode != 0:
                    c["fail"] = f"decode: {r.stderr.strip()[:160]}"
                    continue
            by_prep.setdefault(str(c["prep"].d), []).append((sha, png))

        for pdir, variants in by_prep.items():
            p = Path(pdir)
            uniq = {s: g for s, g in variants}
            argv = [str(zm), "compare",
                    "--reference", str(p / "ref.png"),
                    "--reference", str(p / "floor.png"),
                    "--metric", "ssim2", "--metric", "butteraugli", "--metric", "zensim",
                    "--output", "tsv"]
            for s, g in uniq.items():
                argv += ["--variant", str(g)]
            argv += ["--variant", str(p / "floor.png")]
            r = subprocess.run(argv, capture_output=True, text=True)
            if r.returncode != 0:
                print(f"  SCORE FAIL {p.name}: {r.stderr.strip()[:300]}", file=sys.stderr)
                continue
            lines = [l for l in r.stdout.splitlines() if l.strip()]
            hdr = lines[0].split("\t")
            for line in lines[1:]:
                f = line.split("\t")
                row = dict(zip(hdr, f))
                ref_kind = "ref" if row["reference"].endswith("ref.png") else "floor"
                vs = Path(row["variant"]).stem
                if vs == "floor" and ref_kind == "ref":
                    floors[pdir] = row
                key = (pdir, vs)
                scores.setdefault(key, {})[ref_kind] = row
            print(f"  scored {p.name}: {len(uniq)} variants", file=sys.stderr)

    # ---- emit --------------------------------------------------------------
    host = f"{platform.node()} {platform.machine()} {platform.system()}-{platform.release()}"
    try:
        cpu = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"],
                             capture_output=True, text=True).stdout.strip()
        ncpu = subprocess.run(["sysctl", "-n", "hw.ncpu"],
                              capture_output=True, text=True).stdout.strip()
    except Exception:
        cpu, ncpu = platform.processor(), "?"

    versions = {
        "zenavif": git_rev(REPO),
        "zenrav1e": git_rev(ZEN / "zenrav1e"),
        "zenav1-svt": git_rev(ZEN / "zenav1-svt"),
        "zenmetrics": git_rev(ZEN / "zenmetrics"),
    }

    cols = ["image", "content_class", "size_tag", "size_req", "w", "h", "px",
            "arm", "ladder_knob", "ladder", "rate_knob", "rate",
            "enc_sha256", "bytes_file", "bytes_av1", "bpp",
            "wall_ms_med", "wall_ms_min", "wall_ms_max", "wall_spread_pct",
            "n_reps_kept", "self_ms", "foreign_cpu_pct", "foreign_cores",
            "ssim2_ref", "ba_max_ref", "ba_p3_ref", "zensim_ref",
            "ssim2_floor", "ba_max_floor", "ba_p3_floor", "zensim_floor",
            "fail"]

    outp = Path(args.out)
    outp.parent.mkdir(parents=True, exist_ok=True)
    with open(outp, "w") as fh:
        fh.write(f"# encode_rd cells — {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}\n")
        fh.write(f"# host: {host} | cpu: {cpu} | ncpu: {ncpu}\n")
        fh.write(f"# versions: {json.dumps(versions)}\n")
        fh.write(f"# grid: images={args.images} sizes={args.sizes} arms={args.arms} "
                 f"reps={args.reps} threads={args.threads} "
                 f"ladder={args.ladder_map or args.ladder or 'per-arm-full'} "
                 f"rates={args.rates or 'per-arm-default'}\n")
        fh.write(f"# scheduling: calib {calib_ms:.0f} ms plain / {bg_ms:.0f} ms backgrounded "
                 f"(ratio {bg_ms/calib_ms:.1f}x); harness nice={os.nice(0)}\n")
        fh.write("# time = full process wall clock, NOT niced, median of kept reps. "
                 "See README.md 'What the clock contains'.\n")
        fh.write("\t".join(cols) + "\n")
        for c in cells:
            p, a = c["prep"], c["arm"]
            px = p.wh[0] * p.wh[1]
            w = c["walls"]
            med = statistics.median(w) if w else ""
            spread = (max(w) - min(w)) / statistics.median(w) * 100 if len(w) > 1 else ""
            sc = scores.get((str(p.d), c.get("sha", "")), {})
            r_, f_ = sc.get("ref", {}), sc.get("floor", {})
            row = [
                p.src.name, classify(p.src), p.size_tag, p.size_req,
                p.wh[0], p.wh[1], px,
                a.name, a.ladder_knob, c["lad"], a.rate_knob, c["rate"],
                c.get("sha", ""),
                c["bytes"] if c["bytes"] is not None else "",
                c.get("av1", "") if c["bytes"] is not None else "",
                f"{c['av1']*8/px:.5f}" if c.get("av1", 0) > 0 else "",
                f"{med:.3f}" if med != "" else "",
                f"{min(w):.3f}" if w else "", f"{max(w):.3f}" if w else "",
                f"{spread:.2f}" if spread != "" else "",
                len(w),
                f"{c['self_ms']:.3f}" if c["self_ms"] is not None else "",
                f"{c['foreign']:.1f}", f"{c.get('fcores', 0.0):.2f}",
                r_.get("ssim2", ""), r_.get("butteraugli_max", ""),
                r_.get("butteraugli_pnorm3", ""), r_.get("zensim", ""),
                f_.get("ssim2", ""), f_.get("butteraugli_max", ""),
                f_.get("butteraugli_pnorm3", ""), f_.get("zensim", ""),
                c["fail"],
            ]
            fh.write("\t".join(str(x) for x in row) + "\n")

    # floor table: the 4:2:0 round-trip ceiling per input.
    fp = outp.with_name(outp.stem + "_floor.tsv")
    with open(fp, "w") as fh:
        fh.write("image\tsize_tag\tsize_req\tw\th\tssim2\tba_max\tba_p3\tzensim\n")
        for p in preps:
            row = floors.get(str(p.d), {})
            fh.write("\t".join(str(x) for x in [
                p.src.name, p.size_tag, p.size_req, p.wh[0], p.wh[1],
                row.get("ssim2", ""), row.get("butteraugli_max", ""),
                row.get("butteraugli_pnorm3", ""), row.get("zensim", "")]) + "\n")

    nfail = sum(1 for c in cells if c["fail"])
    print(f"\nwrote {outp} ({len(cells)} cells, {nfail} failed) and {fp}", file=sys.stderr)
    print(f"total encodes: {stats['enc']} in {time.time()-t_start:.0f}s "
          f"({stats['waited']:.0f}s of that waiting for the box to go idle)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
