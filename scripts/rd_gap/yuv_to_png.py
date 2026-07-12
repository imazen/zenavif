#!/usr/bin/env python3
"""Invert cooptloop_trace_dump's RGB->YCbCr: raw I420 (aomdec --rawvideo) -> PNG.

The dump converts with FULL-range BT.601 (Y = .299R+.587G+.114B; Cb/Cr 128-centered,
no studio squeeze) and decimates chroma by top-left sampling. This inverse applies the
exact matrix back with nearest-neighbor chroma upsampling, so source-vs-decode scoring
uses one owned transform on both sides (no decoder colorimetry guessing).

Usage: yuv_to_png.py in.yuv WIDTH HEIGHT out.png [src.png]
With src.png, also prints "mse <value>" (RGB mean-squared-error vs the
source) — the raw-SSE baseline the Phase-1 D-vs-metric fit compares against.
"""
import sys

import numpy as np
from PIL import Image


def main():
  yuv_path, w, h, out = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
  data = np.fromfile(yuv_path, dtype=np.uint8)
  ysz, csz = w * h, (w // 2) * (h // 2)
  if data.size != ysz + 2 * csz:
    # EXACT match required: a LARGER buffer (coded-size padding) would silently
    # mis-slice chroma; refuse rather than corrupt (guard added 2026-07-12
    # after a near-miss — the 120-ivf corpus audit measured ALL EXACT).
    sys.exit(f"yuv size {data.size} != expected {ysz + 2 * csz} for {w}x{h}")
  y = data[:ysz].reshape(h, w).astype(np.float32)
  cb = data[ysz:ysz + csz].reshape(h // 2, w // 2).astype(np.float32)
  cr = data[ysz + csz:ysz + 2 * csz].reshape(h // 2, w // 2).astype(np.float32)
  cb = cb.repeat(2, axis=0).repeat(2, axis=1)[:h, :w] - 128.0
  cr = cr.repeat(2, axis=0).repeat(2, axis=1)[:h, :w] - 128.0
  r = y + 1.402 * cr
  g = y - 0.344_136 * cb - 0.714_136 * cr
  b = y + 1.772 * cb
  rgb = np.stack([r, g, b], axis=-1).round().clip(0, 255).astype(np.uint8)
  Image.fromarray(rgb, "RGB").save(out)
  if len(sys.argv) > 5:
    srcimg = np.asarray(Image.open(sys.argv[5]).convert("RGB"), dtype=np.float64)
    mse = float(np.mean((srcimg - rgb.astype(np.float64)) ** 2))
    print(f"mse {mse:.6f}")


if __name__ == "__main__":
  main()
