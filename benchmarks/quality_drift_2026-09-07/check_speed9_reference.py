from pathlib import Path
import subprocess,hashlib
root=Path(__file__).resolve().parent
arms={'corpus9-current':315,'corpus9-fullmode':315}
rows=['arm\tfile\tsha256']
for arm,want in arms.items():
 files=sorted((root/(arm+'-files')).glob('*.avif'));assert len(files)==want,(arm,len(files),want)
 for p in files:
  r=subprocess.run(['avifdec','-j','1','--info',str(p)],capture_output=True,text=True)
  assert r.returncode==0,(p,r.stdout,r.stderr)
  assert r.stdout.count('Decoded frame [')==1,(p,r.stdout)
  rows.append(arm+'\t'+p.name+'\t'+hashlib.sha256(p.read_bytes()).hexdigest())
 print('PASS',arm,len(files),'files',flush=True)
(root/'reference-speed9-files.tsv').write_text('\n'.join(rows)+'\n')
print('PASS: independent libavif decoded',len(rows)-1,'files')
