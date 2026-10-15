"""Decode each probe output with FFmpeg's libdav1d and score PSNR against the
source planes (6:1:1 over Y/Cb/Cr, on code values at the coded depth)."""
import csv, math, subprocess, sys
W, H = 768, 512
fmt = {8: "yuv420p", 10: "yuv420p10le", 12: "yuv420p12le"}
rows = list(csv.DictReader(open("cells.csv")))
out = csv.writer(sys.stdout)
out.writerow(["img", "bits", "backend", "qindex", "bytes", "ms", "status", "psnr"])
def words(b, bits):
    if bits == 8:
        return list(b)
    return [b[i] | b[i + 1] << 8 for i in range(0, len(b), 2)]
for r in rows:
    psnr = ""
    if r["status"] == "ok":
        bits = int(r["bits"])
        name = f"out/{r['img']}_{bits}_{r['backend']}_{r['qindex']}.obu"
        dec = subprocess.run(["ffmpeg", "-v", "error", "-c:v", "libdav1d", "-f", "obu", "-i", name,
                              "-f", "rawvideo", "-pix_fmt", fmt[bits], "-"], capture_output=True)
        if dec.returncode != 0 or not dec.stdout:
            psnr = "decode-failed: " + dec.stderr.decode()[:80].replace(",", ";")
        else:
            a = words(open(f"in/img{r['img']}.{fmt[bits]}.yuv", "rb").read(), bits)
            b = words(dec.stdout, bits)
            ys, cs = W * H, W * H // 4
            peak = (1 << bits) - 1
            def mse(lo, hi):
                return sum((a[i] - b[i]) ** 2 for i in range(lo, hi)) / (hi - lo)
            m = (6 * mse(0, ys) + mse(ys, ys + cs) + mse(ys + cs, ys + 2 * cs)) / 8
            psnr = f"{10 * math.log10(peak * peak / max(m, 1e-12)):.3f}"
    out.writerow([r["img"], r["bits"], r["backend"], r["qindex"], r["bytes"], r["ms"], r["status"], psnr])
