from pathlib import Path
import subprocess,hashlib
root=Path(__file__).resolve().parent
arms={'old':27,'current':27,'unarmed':27,'backend':27,'txfix':1,'txparent':1,'splitfix':1,'splitparent':1,'fullmode':9,'corpus-current':315,'corpus-fullmode':315}
rows=['arm\tfile\tsha256']
for arm,want in arms.items():
 files=sorted((root/(arm+'-files')).glob('*.avif'));assert len(files)==want,(arm,len(files),want)
 for p in files:
  r=subprocess.run(['avifdec','-j','1','--info',str(p)],capture_output=True,text=True)
  assert r.returncode==0,(p,r.stdout,r.stderr)
  assert r.stdout.count('Decoded frame [')==1,(p,r.stdout)
  rows.append(arm+'\t'+p.name+'\t'+hashlib.sha256(p.read_bytes()).hexdigest())
 print('PASS',arm,len(files),'files',flush=True)
(root/'reference-files.tsv').write_text('\n'.join(rows)+'\n')
print('PASS: independent libavif decoded',len(rows)-1,'files')
