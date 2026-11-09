#!/usr/bin/env python3
"""Read-only production artwork snapshot over SSH. No credentials leave Diamond."""
import json, pathlib, subprocess, base64, concurrent.futures
ROOT=pathlib.Path(__file__).resolve().parents[1]/'.local'
catalog=json.loads((ROOT/'library.json').read_text())
art=ROOT/'art';art.mkdir(exist_ok=True)
jobs=[(t['id'],v) for t in catalog['titles'] for v in ('poster','backdrop') if t.get(v)]
jobs += [(a['id'],'episode') for t in catalog['titles'] for a in t['assets'] if a.get('thumbnail')]
jobs = [(i,v) for i,v in jobs if not (art/(i.replace(':','_')+'-'+v+'.jpg')).exists()]
script=r'''
import json,urllib.request,urllib.parse,pathlib,base64,concurrent.futures
c=json.loads((pathlib.Path.home()/'Library/Application Support/Torrbaby/config.json').read_text())
jobs=JOBS
base='http://100.80.147.115:8765'
def get(job):
 i,v=job
 try:
  req=urllib.request.Request(base+'/v1/library/artwork/'+urllib.parse.quote(i,safe=':')+'/'+v,headers={'Authorization':'Bearer '+pathlib.Path(c['token_file']).expanduser().read_text().strip()})
  with urllib.request.urlopen(req,timeout=35) as r: data=json.load(r)['data']
  return {'id':i,'variant':v,'data':data}
 except Exception:return {'id':i,'variant':v,'data':None}
with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
 for r in pool.map(get,jobs):print(json.dumps(r),flush=True)
'''.replace('JOBS',repr(jobs))
p=subprocess.Popen(['ssh','-o','BatchMode=yes','diamond','python3','-'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
p.stdin.write(script);p.stdin.close()
n=0
for line in p.stdout:
 j=json.loads(line)
 if j['data']:
  (art/(j['id'].replace(':','_')+'-'+j['variant']+'.jpg')).write_bytes(base64.b64decode(j['data']));n+=1
print(f'Saved {n} artwork files');raise SystemExit(p.wait())
