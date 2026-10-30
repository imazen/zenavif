#!/usr/bin/env python3
"""Prepare isolated native/core source exports for timeline qualification.

Usage: prepare-timeline-check.py NATIVE_EXPORT CORE_EXPORT NEW_DESTINATION
Builds must be serialized through run-heavy by the caller. This is an explicit
path-bridge experiment; it does not certify the final portable git graph.
"""
import json
from pathlib import Path
import shutil
import sys

native, core, destination = map(lambda p: Path(p).resolve(), sys.argv[1:])
if (native / '.git').exists() or (core / '.git').exists():
    raise SystemExit('Supply immutable source exports, not live repositories')
destination.mkdir(parents=True, exist_ok=False)
shutil.copytree(native / 'zenmp4', destination / 'zenmp4')
shutil.copytree(core, destination / 'core')
packages = {
    'zencodec': 'core',
    'zencodec-io': 'core/zencodec-io',
    'zencodec-media': 'core/zencodec-media',
    'zencodec-media-testkit': 'core/zencodec-media-testkit',
}
text = '[workspace]\nmembers = ["zenmp4"]\nexclude = ["core"]\nresolver = "3"\n'
for source in ['crates-io', '"https://github.com/imazen/zencodec"']:
    text += f'\n[patch.{source}]\n'
    for name, path in packages.items():
        text += f'{name} = {{ path = {json.dumps(path)} }}\n'
    if source == 'crates-io':
        for name in ['zenpixels']:
            text += f'{name} = {{ git = "https://github.com/imazen/zenpixels", rev = "f23ac7535be1144a22d0dc6b71f866a08b151f34" }}\n'
(destination / 'Cargo.toml').write_text(text)
# Identity is checked before any resolution/build; only the wrapper is synthetic.
for origin, copied in [(native / 'zenmp4', destination / 'zenmp4'), (core, destination / 'core')]:
    for source in origin.rglob('*'):
        if source.is_file():
            assert source.read_bytes() == (copied / source.relative_to(origin)).read_bytes(), source
print('Source exports match; wrapper path bridge is explicit.')
