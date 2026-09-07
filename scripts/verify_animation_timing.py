#!/usr/bin/env python3
"""Check exported animation timing with libavif's independent decoder."""
import argparse
import hashlib
from pathlib import Path
import re
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('artifacts', type=Path)
parser.add_argument('--decoder', default='avifdec')
parser.add_argument('--manifest', required=True, type=Path)
args = parser.parse_args()
files = sorted(args.artifacts.glob('*.avif'))
assert len(files) == 80, f'Expected 80 native/adapter files, got {len(files)}'
rows = ['file\tsha256\ttimescale\tdurations']
for path in files:
    api, backend, depth, clock, kind = path.stem.split('-')
    clock, kind = int(clock), int(kind)
    durations = {60000: [2002, 1380, 1001], 30000: [1001, 2002], 1000000: [1, 7], 4294967295: [4294967295, 4294967295], 1000: [20, 30]}[clock]
    result = subprocess.run([args.decoder, '-j', '1', '--info', str(path)], capture_output=True, text=True)
    output = result.stdout + result.stderr
    path.with_suffix('.libavif.txt').write_text(output)
    print(path.name, result.returncode, flush=True)
    assert result.returncode == 0, output
    assert f'{clock} timescales per second' in output, output
    assert f'({sum(durations)} timescales), {len(durations)} frames' in output, output
    timing = re.findall(r'Decoded frame \[(\d+)\] \[pts [^ ]+ \((\d+) timescales\)\] \[duration [^ ]+ \((\d+) timescales\)\]', output)
    expected = [(str(i), str(sum(durations[:i])), str(d)) for i, d in enumerate(durations)]
    assert timing == expected, output
    assert ('65x67' if api == 'timing' else '32x32') in output
    assert f'Bit Depth      : {8 if depth == 'Eight' else 10}' in output
    rows.append('\t'.join([path.name, hashlib.sha256(path.read_bytes()).hexdigest(), str(clock), ','.join(map(str, durations))]))
args.manifest.write_text('\n'.join(rows) + '\n')
print('PASS 80 files / 176 independently decoded frames with exact durations and PTS')
