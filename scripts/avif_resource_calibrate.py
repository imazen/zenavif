#!/usr/bin/env python3
"""Calibrate AVIF encode/decode resource use (peak mem + wall + CPU time).

Drives `examples/avif_probe` (one process per op, so VmHWM is a clean
per-op peak) across resolution x speed x quality x alpha x depth x content.
For every cell it ENCODES (measuring the encoder's marginal working set,
wall, and user/sys CPU) then DECODES the produced .avif (same metrics for
the decoder). Real content is downscaled (Lanczos, downscale-only per the
calibration discipline) to a pixel ladder so the per-pixel slope separates
from fixed overhead.

Threads are pinned to 1 in the probe, so encode wall ~= user (clean
single-thread CPU model); decode is the decoder's own (it self-threads, so
decode user can exceed wall).

Output: TSV + .meta (git commit, host, grid) + per-stratum fits.
"""
import argparse, subprocess, datetime, socket, statistics
from pathlib import Path
from PIL import Image
Image.MAX_IMAGE_PIXELS = None


def gen_variant(src, n, outdir):
    im = Image.open(src)
    if max(im.size) < n:
        return None  # downscale-only
    im = im.convert("RGB").resize((n, n), Image.LANCZOS)
    p = outdir / f"{Path(src).stem}_{n}.png"
    im.save(p)
    return p


def run(bin, png, mode, quality, speed, depth, alpha, avif):
    cmd = [bin, str(png), mode, str(quality), str(speed), str(depth), alpha, str(avif)]
    out = subprocess.run(cmd, capture_output=True, text=True).stdout
    d = {}
    for tok in out.split():
        if "=" in tok:
            k, v = tok.split("=", 1)
            d[k] = v
    return d


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default="target/release/examples/avif_probe")
    ap.add_argument("--sizes", default="256,512,1024")
    ap.add_argument("--speeds", default="4,6,8,10")
    ap.add_argument("--qualities", default="75")
    ap.add_argument("--alphas", default="rgb")
    ap.add_argument("--depths", default="8")
    ap.add_argument("--content", action="append", default=[])
    ap.add_argument("--content-file", default=None)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()

    sizes = [int(x) for x in a.sizes.split(",")]
    speeds = [int(x) for x in a.speeds.split(",")]
    quals = [float(x) for x in a.qualities.split(",")]
    alphas = a.alphas.split(",")
    depths = [int(x) for x in a.depths.split(",")]
    content = [c.split(":") for c in a.content]
    if a.content_file:
        for line in Path(a.content_file).read_text().splitlines():
            line = line.strip()
            if line and not line.startswith("#"):
                content.append(line.split(":"))

    date = datetime.date.today().isoformat()
    out = Path(a.out or f"benchmarks/avif_resource_{date}.tsv")
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
    tmp = Path("/tmp/_avifcal"); tmp.mkdir(exist_ok=True)

    cells = []
    for src, cls in content:
        for n in sizes:
            v = gen_variant(src, n, tmp)
            if v:
                cells.append((v, n, cls))

    rows = []
    total = len(cells) * len(alphas) * len(depths) * len(quals) * len(speeds)
    i = 0
    for (png, n, cls) in cells:
        for al in alphas:
            for d in depths:
                for q in quals:
                    for sp in speeds:
                        i += 1
                        avif = tmp / f"{png.stem}_{al}_{d}_{q}_{sp}.avif"
                        e = run(a.bin, png, "encode", q, sp, d, al, avif)
                        dec = run(a.bin, png, "decode", q, sp, d, al, avif)
                        px = n * n
                        if not e.get("bytes"):
                            print(f"[{i}/{total}] {cls} {n} {al} d{d} q{q} s{sp} ENCODE FAILED", flush=True)
                            continue
                        rows.append((cls, d, al, n, px, q, sp, "encode",
                                     int(e["peak_kb"]), int(e["delta_kb"]), float(e["wall_ms"]),
                                     float(e["user_ms"]), float(e["sys_ms"]), int(e["bytes"])))
                        if dec.get("peak_kb"):
                            rows.append((cls, d, al, n, px, q, sp, "decode",
                                         int(dec["peak_kb"]), int(dec["delta_kb"]), float(dec["wall_ms"]),
                                         float(dec["user_ms"]), float(dec["sys_ms"]), int(e["bytes"])))
                        print(f"[{i}/{total}] {cls} {n}^2 {al} d{d} q{q} s{sp} -> "
                              f"enc {int(e['delta_kb'])//1024}MB {float(e['wall_ms']):.0f}ms "
                              f"({float(e['user_ms'])*1e3/px:.1f}us/px) | "
                              f"dec {int(dec.get('delta_kb',0))//1024}MB {float(dec.get('wall_ms',0)):.0f}ms",
                              flush=True)

    out.parent.mkdir(exist_ok=True)
    hdr = "content\tdepth\talpha\tsize\tpixels\tquality\tspeed\top\tpeak_kb\tdelta_kb\twall_ms\tuser_ms\tsys_ms\tbytes\n"
    with open(out, "w") as f:
        f.write(hdr)
        for r in rows:
            f.write("\t".join(str(x) for x in r) + "\n")
    with open(str(out) + ".meta", "w") as f:
        f.write(f"# avif_resource_calibrate\ncommit: {commit}\nhost: {socket.gethostname()}\n"
                f"date: {date}\nbin: {a.bin}\nsizes: {sizes}\nspeeds: {speeds}\n"
                f"qualities: {quals}\nalphas: {alphas}\ndepths: {depths}\n"
                f"content_classes: {sorted(set(c[1] for c in content))}\nn_srcs: {len(content)}\n"
                f"measure: avif_probe VmHWM delta (per-op marginal working set) + wall (Instant) "
                f"+ user/sys (/proc/self/stat), threads=1; one process per op.\n")
    print(f"\nwrote {out} ({len(rows)} rows)")

    # ---- fits ----
    enc = [r for r in rows if r[7] == "encode"]
    dec = [r for r in rows if r[7] == "decode"]

    def bpx(r):  # marginal working-set B/px
        return r[9] * 1024.0 / r[4]

    print("\n=== ENCODE marginal mem B/px + CPU us/px, per speed (px>=512^2) ===")
    print(f"{'speed':>5} {'n':>4} {'mem p50':>8} {'mem p100':>9} {'wall us/px p50':>15} {'user us/px p50':>15}")
    by = {}
    for r in enc:
        if r[4] >= 512 * 512:
            by.setdefault(r[6], []).append(r)
    for sp in sorted(by):
        v = by[sp]
        mem = sorted(bpx(r) for r in v)
        wallpp = sorted(r[10] * 1e3 / r[4] for r in v)   # us/px wall
        userpp = sorted(r[11] * 1e3 / r[4] for r in v)
        med = lambda x: x[len(x) // 2]
        print(f"{sp:>5} {len(v):>4} {med(mem):>8.1f} {mem[-1]:>9.1f} {med(wallpp):>15.2f} {med(userpp):>15.2f}")

    print("\n=== DECODE marginal mem B/px + wall us/px (px>=512^2) ===")
    dv = [r for r in dec if r[4] >= 512 * 512]
    if dv:
        mem = sorted(bpx(r) for r in dv)
        wallpp = sorted(r[10] * 1e3 / r[4] for r in dv)
        userpp = sorted(r[11] * 1e3 / r[4] for r in dv)
        med = lambda x: x[len(x) // 2]
        print(f"n={len(dv)} mem p50={med(mem):.1f} p100={mem[-1]:.1f} B/px | "
              f"wall p50={med(wallpp):.2f} us/px | user p50={med(userpp):.2f} us/px")

    print("\n=== alpha + depth deltas (encode, p50 over matched cells) ===")
    for axis, idx, base in (("alpha", 2, "rgb"), ("depth", 1, 8)):
        groups = {}
        for r in enc:
            if r[4] >= 512 * 512:
                groups.setdefault(r[idx], []).append((bpx(r), r[11] * 1e3 / r[4]))
        for k in sorted(groups, key=str):
            m = sorted(x[0] for x in groups[k]); t = sorted(x[1] for x in groups[k])
            print(f"  {axis}={k}: mem p50={m[len(m)//2]:.1f} B/px, user p50={t[len(t)//2]:.2f} us/px (n={len(m)})")


if __name__ == "__main__":
    main()
