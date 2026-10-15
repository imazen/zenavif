"""Per depth and backend: mean bytes, PSNR and ms per quantizer point, and a
BD-rate against zenav1-aom (piecewise-linear log-rate over the shared PSNR
range, per image, then averaged)."""
import csv, math, statistics as st
rows = [r for r in csv.DictReader(open("results.csv")) if r["status"] == "ok"]
def key(r): return (int(r["bits"]), r["backend"])
by = {}
for r in rows:
    by.setdefault(key(r), []).append(r)
print("bits backend   qindex  bytes(mean)  psnr(mean)  ms(mean)")
for (bits, be), rs in sorted(by.items()):
    for q in sorted({int(r["qindex"]) for r in rs}):
        c = [r for r in rs if int(r["qindex"]) == q]
        print(f"{bits:4} {be:11} {q:5} {st.mean(int(r['bytes']) for r in c):11.0f} {st.mean(float(r['psnr']) for r in c):10.2f} {st.mean(float(r['ms']) for r in c):9.1f}")
def curve(bits, be, img):
    pts = sorted((float(r["psnr"]), math.log(int(r["bytes"]))) for r in rows
                 if int(r["bits"]) == bits and r["backend"] == be and r["img"] == img)
    return pts
def interp(pts, p):
    for (p0, r0), (p1, r1) in zip(pts, pts[1:]):
        if p0 <= p <= p1:
            return r0 + (r1 - r0) * (p - p0) / (p1 - p0) if p1 != p0 else r0
    return None
def bd(bits, be, ref="zenav1-aom"):
    out = []
    for img in sorted({r["img"] for r in rows}):
        a, b = curve(bits, be, img), curve(bits, ref, img)
        if len(a) < 2 or len(b) < 2: continue
        lo, hi = max(a[0][0], b[0][0]), min(a[-1][0], b[-1][0])
        if hi <= lo: continue
        n = 50
        diffs = [interp(a, lo + (hi - lo) * i / n) - interp(b, lo + (hi - lo) * i / n) for i in range(n + 1)]
        out.append(math.exp(st.mean(diffs)) - 1)
    return out
print("\nBD-rate vs zenav1-aom at the same PSNR (negative = fewer bytes), per image:")
for bits in (8, 10, 12):
    for be in ("zenrav1e", "zenav1-svt"):
        v = bd(bits, be)
        if v:
            print(f"{bits:4} {be:11} mean {100*st.mean(v):+6.1f}%  images {[f'{100*x:+.1f}%' for x in v]}")
