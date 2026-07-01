#!/usr/bin/env python3
"""Wrap a raw low-overhead-bitstream-format AV1 OBU stream (as extracted from an
AVIF's mdat, e.g. via zenavif's extract_av1 example) as a single-frame IVF file.

aom's examples/inspect (built with -DCONFIG_INSPECTION=1) only auto-detects
IVF/WebM/annex-B OBU via its shared aom_video_reader, not the low-overhead format
that goes inside AVIF/mdat -- plain `aomdec` handles that format fine, but the
inspect tool's minimal reader doesn't. This wrapper bridges the gap.
"""
import struct, sys

def wrap(obu_path, ivf_path, width, height):
    data = open(obu_path, "rb").read()
    with open(ivf_path, "wb") as f:
        f.write(b"DKIF")
        f.write(struct.pack("<H", 0))       # version
        f.write(struct.pack("<H", 32))      # header size
        f.write(b"AV01")
        f.write(struct.pack("<H", width))
        f.write(struct.pack("<H", height))
        f.write(struct.pack("<I", 30))      # frame rate num
        f.write(struct.pack("<I", 1))       # frame rate den
        f.write(struct.pack("<I", 1))       # frame count
        f.write(struct.pack("<I", 0))       # unused
        f.write(struct.pack("<I", len(data)))
        f.write(struct.pack("<Q", 0))       # timestamp
        f.write(data)

if __name__ == "__main__":
    if len(sys.argv) != 5:
        raise SystemExit("usage: obu_to_ivf.py in.obu out.ivf width height")
    wrap(sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4]))
