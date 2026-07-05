#!/usr/bin/env python3
"""RD-vs-time monotonicity check for a speed-ladder sweep TSV.
Input cols: arm speed q bytes bpp enc_ms ssim2 img   (measure_speeds.sh + img tag)
For each image, sort tiers by encode time; flag any SLOWER tier that is Pareto-
DOMINATED by a faster one (<= bytes AND >= ssim2) — i.e. time bought a worse RD point.
That is the monotonicity invariant: more time must never yield a worse RD point."""
import csv, sys
def arms(s):  # armed table @ high_quality: what each tier adds (for attribution)
    a=[]
    if s<=6: a.append('fine_dir')
    if 6<=s<=8: a+=['tx_size_rdo','intra7']
    if 4<=s<=8: a.append('part_prune')
    if s==1: a.append('S1_DEEP')
    if s>=9: a+=['reduced_tx','inter_tx_split']
    if s==9: a+=['S10:part(8,16)','S10:txsize','S10:cdef']
    if s==10: a.append('S10:cdef')
    return ','.join(a) if a else '-'
rows=[r for r in csv.DictReader(open(sys.argv[1]),delimiter='\t') if r['ssim2'] not in ('DECFAIL','ENCFAIL','')]
by={}
for r in rows: by.setdefault(r['img'],[]).append((int(r['speed']),int(r['bytes']),float(r['ssim2']),float(r['enc_ms'])))
tot=0
for img,pts in sorted(by.items()):
    pts.sort(key=lambda x:x[3]); viol=[]
    for i,(s,b,q,t) in enumerate(pts):
        for (s2,b2,q2,t2) in pts[:i]:
            if b2<=b and q2>=q and (b2<b or q2>q):
                viol.append(f"s{s}({t:.0f}ms {b}B {q:.2f}) <= s{s2}({t2:.0f}ms {b2}B {q2:.2f}) [s{s} adds:{arms(s)}]"); break
    tot+=len(viol)
    print(f"{img:15} {'MONOTONE' if not viol else str(len(viol))+' INVERSION(S)'}")
    for v in viol: print("   "+v)
print(f"TOTAL {tot} inversions / {len(by)} images")
sys.exit(1 if tot else 0)
