#!/usr/bin/env python3
"""Verify public animation color settings and exact RGB/alpha coded planes."""
import argparse
import hashlib
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("artifacts", type=Path)
parser.add_argument("--manifest", type=Path, required=True)
args = parser.parse_args()
files = sorted(args.artifacts.glob("*.avif"))
assert len(files) == 40, len(files)
rows = ["file\tsha256\tcolor_decoded_sha256\talpha_decoded_sha256"]

def decode(path, depth):
    raw = path.with_suffix(".decoded.yuv")
    result = subprocess.run([
        "aomdec", "--threads=1", "--rawvideo", f"--output-bit-depth={depth}",
        f"--output={raw}", str(path),
    ], capture_output=True, text=True)
    path.with_suffix(".aom.log").write_text(result.stdout + result.stderr)
    assert result.returncode == 0, (path, result.stderr)
    return raw.read_bytes()

for path in files:
    _, mode, depth, kind = path.stem.split("-")
    depth, kind = int(depth), int(kind)
    info = subprocess.run(["avifdec", "-j", "1", "--info", str(path)], capture_output=True, text=True)
    output = info.stdout + info.stderr
    path.with_suffix(".libavif.txt").write_text(output)
    assert info.returncode == 0, output
    assert f'Format         : YUV{420 if mode.startswith("420") else 444}' in output, output
    assert f'Range          : {"Limited" if "limited" in mode else "Full"}' in output, output
    assert f'Matrix Coeffs. : {0 if mode == "rgb" else 6}' in output, output
    assert f"Bit Depth      : {depth}" in output, output
    assert output.count("Decoded frame [") == 2, output
    color = decode(path.with_suffix(".obu"), depth)
    chroma = 33 * 34 if mode.startswith("420") else 65 * 67
    assert len(color) == 2 * (65 * 67 + 2 * chroma) * (1 if depth == 8 else 2), path
    if mode == "rgb":
        assert color == path.with_suffix(".source.yuv").read_bytes(), path
    alpha_hash = "-"
    if kind % 2 == 1:
        alpha = decode(path.with_suffix(".alpha.obu"), depth)
        assert alpha == path.with_suffix(".alpha.source.yuv").read_bytes(), path
        alpha_hash = hashlib.sha256(alpha).hexdigest()
    rows.append("\t".join([path.name, hashlib.sha256(path.read_bytes()).hexdigest(), hashlib.sha256(color).hexdigest(), alpha_hash]))
    print("PASS", path.name, flush=True)
args.manifest.write_text("\n".join(rows) + "\n")
print("PASS: 40 files / 80 frames; 8 RGB and all 20 alpha streams equal coded source planes")
