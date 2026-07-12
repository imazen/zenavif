#!/usr/bin/env python3
"""PNG/PPM -> single-frame Y4M (C420jpeg, full range) with the OWNED forward
transform — the exact BT.601-full formulas + top-left chroma decimation that
cooptloop_trace_dump uses, so SVT/aomenc reference encodes and zenrav1e trace
encodes score through one identical color path (yuv_to_png.py is the inverse).

Usage: png_to_y4m.py in.png out.y4m
"""
import sys

import numpy as np
from PIL import Image


def main():
  src, out = sys.argv[1], sys.argv[2]
  rgb = np.asarray(Image.open(src).convert("RGB"), dtype=np.float32)
  h, w = rgb.shape[0] & ~1, rgb.shape[1] & ~1
  rgb = rgb[:h, :w]
  r, g, b = rgb[..., 0], rgb[..., 1], rgb[..., 2]
  y = np.clip(np.round(0.299 * r + 0.587 * g + 0.114 * b), 0, 255)
  cb = np.clip(np.round(128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b), 0, 255)
  cr = np.clip(np.round(128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b), 0, 255)
  # top-left decimation (matches the dump; NOT averaging)
  cb = cb[0::2, 0::2]
  cr = cr[0::2, 0::2]
  with open(out, "wb") as f:
    f.write(f"YUV4MPEG2 W{w} H{h} F25:1 Ip A1:1 C420jpeg XCOLORRANGE=FULL\n"
            .encode())
    f.write(b"FRAME\n")
    f.write(y.astype(np.uint8).tobytes())
    f.write(cb.astype(np.uint8).tobytes())
    f.write(cr.astype(np.uint8).tobytes())


if __name__ == "__main__":
  main()
