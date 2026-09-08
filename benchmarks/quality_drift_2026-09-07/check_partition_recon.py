#!/usr/bin/env python3
"""Instrument an isolated old-owner checkout and compare historical reconstructions.

Run serially through run-heavy. --owner must be the caller's isolated
compatibility checkout; all temporary source/manifest edits are restored.
The probe is the existing slower-preset-probe harness described in README.md.
"""
import argparse
from pathlib import Path
import os
import re
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--owner", type=Path, required=True)
p.add_argument("--probe", type=Path, required=True)
p.add_argument("--output", type=Path, required=True)
p.add_argument("--revision", action="append", required=True, help="label=Git SHA")
p.add_argument("--bottom-up", action="store_true", help="diagnostic: change only partition search direction")
a = p.parse_args()
manifest = a.owner / "ravif/Cargo.toml"
code = a.owner / "ravif/src/av1encoder.rs"
probe = a.probe / "Cargo.toml"
original = {path: path.read_text() for path in [manifest, code, probe]}
patch = Path(__file__).with_name("owner-recon-instrumentation.patch").read_text()
rows = ["label\trevision\tbytes\tdiffering_samples\tmax_error"]
all_match = True
try:
    subprocess.run(["patch", "--batch", "--forward", "-p1"], cwd=a.owner,
                   input=patch, text=True, check=True)
    if a.bottom_up:
        source = code.read_text()
        needle = "    let speed_settings = p.speed.speed_settings();"
        assert source.count(needle) == 1
        code.write_text(source.replace(needle, "    let mut speed_settings = p.speed.speed_settings();\n    speed_settings.partition.encode_bottomup = true;"))
    probe.write_text(re.sub(r'(ravif=\{package="zenravif",path=")[^"]+',
                           lambda m: m[1] + str(a.owner / "ravif"), original[probe]))
    for spec in a.revision:
        label, revision = spec.split("=", 1)
        assert re.fullmatch(r"[a-z0-9-]+", label)
        assert re.fullmatch(r"[a-f0-9]{40}", revision)
        manifest.write_text(re.sub(r'(git = "https://github.com/imazen/zenrav1e", rev = ")[a-f0-9]+',
                                   lambda m: m[1] + revision, original[manifest]))
        out = a.output / label
        out.mkdir(parents=True, exist_ok=True)
        env = dict(os.environ, PROBE_RECON=str(out), PROBE_SPEED="2",
                   PROBE_QUALITIES="15", PROBE_CELL="s2/mixed/q15")
        print("START", label, revision, flush=True)
        with (out / "encode.log").open("w") as log:
            subprocess.run(["cargo", "run", "--release", "--manifest-path", str(probe),
                            "--", str(out)], env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
        with (out / "aom.log").open("w") as log:
            subprocess.run(["aomdec", "--threads=1", "--rawvideo", "--output-bit-depth=8",
                            f"--output={out}/decoded.yuv", str(out / "coded.obu")],
                           stdout=log, stderr=subprocess.STDOUT, check=True)
        rec = (out / "encoder.yuv").read_bytes()
        dec = (out / "decoded.yuv").read_bytes()
        assert len(rec) == len(dec) == 260779
        errors = [abs(x - y) for x, y in zip(rec, dec)]
        count, maximum = sum(e != 0 for e in errors), max(errors)
        all_match &= count == 0
        rows.append(f"{label}\t{revision}\t{len(rec)}\t{count}\t{maximum}")
        print("MATCH" if count == 0 else "MISMATCH", rows[-1], flush=True)
        (a.output / "reconstruction.tsv").write_text("\n".join(rows) + "\n")
finally:
    for path, content in original.items():
        path.write_text(content)
if not all_match:
    raise SystemExit("encoder/decoder reconstruction differences found; see reconstruction.tsv")
