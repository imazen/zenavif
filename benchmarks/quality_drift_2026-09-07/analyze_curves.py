from pathlib import Path
import math, statistics, hashlib, sys
root=Path(sys.argv[1]) if len(sys.argv)>1 else Path(__file__).resolve().parent
prefix=sys.argv[2] if len(sys.argv)>2 else "corpus"
speed=sys.argv[3] if len(sys.argv)>3 else "10"

def rows(arm):
 values={}
 lines=[]
 for l in (root/(arm+'.log')).read_text().splitlines():
  if not l.startswith(f"s{speed}/"):continue
  cell,b,s,t=l.split('\t');_,name,q=cell.split('/')
  assert cell not in values
  values[cell]=(int(b),float(s),float(t));lines.append(l)
 assert len(values)==315,(arm,len(values))
 (root/(arm+'.tsv')).write_text('cell\tbytes\tssim2\tenc_ms\n'+'\n'.join(lines)+'\n')
 return values

def front(points):
 out=[];minimum=math.inf
 for score,rate in sorted(points,reverse=True):
  if rate<minimum:out.append((score,math.log(rate)));minimum=rate
 return sorted(out)

def interp(points,x):
 for (a,b),(c,d) in zip(points,points[1:]):
  if a<=x<=c:return b+(d-b)*(x-a)/(c-a)
 raise AssertionError((points,x))

base=rows(prefix+"-current");candidate=rows(prefix+"-fullmode")
names=sorted({c.split('/')[1] for c in base})
assert len(names)==35
result=['name\tssim2_low\tssim2_high\tlog_rate_change_pct\tmedian_encode_time_ratio\tidentical_source_sha256']
changes=[];times=[]
for name in names:
 a=[(score,rate) for cell,(rate,score,_) in base.items() if cell.split('/')[1]==name]
 b=[(score,rate) for cell,(rate,score,_) in candidate.items() if cell.split('/')[1]==name]
 a,b=front(a),front(b)
 low=max(30.,a[0][0],b[0][0]);high=min(90.,a[-1][0],b[-1][0]);assert high>low,(name,low,high)
 knots=sorted({low,high}|{x for x,_ in a+b if low<x<high})
 area=0.
 for left,right in zip(knots,knots[1:]):
  area+=(right-left)*((interp(b,left)-interp(a,left))+(interp(b,right)-interp(a,right)))/2
 change=100.*math.expm1(area/(high-low));changes.append(change)
 time=statistics.median(candidate[c][2]/base[c][2] for c in base if c.split('/')[1]==name);times.append(time)
 src=(root/(prefix+"-current-files")/f'source-{name}.ppm').read_bytes()
 assert src==(root/(prefix+"-fullmode-files")/f'source-{name}.ppm').read_bytes(),name
 sha=hashlib.sha256(src).hexdigest()
 result.append(f'{name}\t{low:.3f}\t{high:.3f}\t{change:.3f}\t{time:.3f}\t{sha}')
(root/(prefix+"-comparison.tsv")).write_text('\n'.join(result)+'\n')
print('35 images, 315 cells per arm, source pixels identical for every pair')
print('Piecewise-linear log-rate integration on per-image Pareto fronts; overlap clipped to SSIM2 [30,90]')
print('Rate change pct: median',round(statistics.median(changes),3),'mean',round(statistics.mean(changes),3),'min',round(min(changes),3),'max',round(max(changes),3),'improved',sum(x<0 for x in changes))
print('Median paired encoding-time ratio:',round(statistics.median(times),3),'(single pass, not a stable latency bound)')
for line in result[1:]:print(line)
