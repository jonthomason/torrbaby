#!/usr/bin/env python3
"""Local-only destructive-action check using a fake Transmission server. Run after cargo build."""
import json,threading,http.server,tempfile,pathlib,subprocess,time,sqlite3,urllib.request,base64,shutil
root=pathlib.Path(tempfile.mkdtemp(prefix='torrbaby-trial-check-'));torrents={};removed=[];lock=threading.Lock()
for name in ['source','movies','tv']:(root/name).mkdir()
original='0'*40
(root/'source/original.bin').write_bytes(b'preserve')
torrents[original]={'hash_string':original,'name':'Existing','download_dir':str(root/'source'),'files':[{'name':'original.bin','length':100,'bytes_completed':0}],'downloaded_ever':0,'percent_done':0,'metadata_percent_complete':1,'left_until_done':100,'status':4,'peers_connected':0,'peers_sending_to_us':0}
class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_POST(self):
  q=json.loads(self.rfile.read(int(self.headers['Content-Length'])));m=q['method'];p=q.get('params',{});r={}
  with lock:
   if m=='torrent_get':r={'torrents':[dict(t) for h,t in torrents.items() if not p.get('ids') or h in p['ids']]}
   elif m=='torrent_add':
    n=int(base64.b64decode(p['metainfo']).decode());h=str(n)*40
    if h in torrents:r={'torrent_duplicate':{'hash_string':h}}
    else:
     d=pathlib.Path(p['download_dir']);d.mkdir(parents=True,exist_ok=True);(d/f'candidate{n}.bin').write_bytes(b'trial')
     torrents[h]={'hash_string':h,'name':f'Candidate {n}','download_dir':str(d),'files':[{'name':f'candidate{n}.bin','length':10000000,'bytes_completed':1000}],'downloaded_ever':n*1048576,'percent_done':0.01,'metadata_percent_complete':1,'left_until_done':9000000,'status':0,'peers_connected':n*10,'peers_sending_to_us':n}
     r={'torrent_added':{'hash_string':h}}
   elif m=='torrent_start' or m=='torrent_stop':
    for h in p['ids']:torrents[h]['status']=4 if m=='torrent_start' else 0
   elif m=='torrent_set_location':
    for h in p['ids']:
     t=torrents[h];old=pathlib.Path(t['download_dir']);new=pathlib.Path(p['location'])
     for f in t['files']:shutil.move(str(old/f['name']),str(new/f['name']))
     t['download_dir']=str(new)
   elif m=='torrent_remove':
    for h in p['ids']:
     t=torrents.pop(h);removed.append(h)
     if p['delete_local_data']:
      for f in t['files']:(pathlib.Path(t['download_dir'])/f['name']).unlink()
  data=json.dumps({'jsonrpc':'2.0','id':1,'result':r}).encode();self.send_response(200);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
(root/'token').write_text('fixture-only-token-not-a-real-secret-12345')
config={'listen_host':'127.0.0.1','listen_port':18897,'token_file':str(root/'token'),'state_file':str(root/'state.json'),'transmission_url':f'http://127.0.0.1:{server.server_port}/rpc','transmission_auth_file':None,'source':str(root/'source'),'movies':str(root/'movies'),'tv':str(root/'tv'),'poll_seconds':5,'paused':True,'background_subtitles':False}
(root/'config.json').write_text(json.dumps(config));log=open(root/'server.log','w');proc=subprocess.Popen(['target/debug/torrbaby','serve','--config',str(root/'config.json')],stdout=log,stderr=log)
def req(path,body=None):
 return json.load(urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:18897'+path,data=None if body is None else json.dumps(body).encode(),headers={'Authorization':'Bearer '+(root/'token').read_text(),'Content-Type':'application/json'}),timeout=10))
try:
 for _ in range(50):
  try:req('/v1/trials');break
  except Exception:time.sleep(.1)
 v=req('/v1/trials',{'candidates':[{'name':f'Candidate {n}','listing_key':f'test:{n}','source':{'metainfo':base64.b64encode(str(n).encode()).decode()}} for n in range(4)]})
 for _ in range(100):
  v=req('/v1/trials')['trials'][0]
  if v['state']=='running':break
  time.sleep(.1)
 assert v['state']=='running',v
 proc.terminate();proc.wait(timeout=10)
 proc=subprocess.Popen(['target/debug/torrbaby','serve','--config',str(root/'config.json')],stdout=log,stderr=log)
 for _ in range(50):
  try:req('/v1/trials');break
  except Exception:time.sleep(.1)
 assert v['candidates'][0]['state']=='duplicate',v
 c=sqlite3.connect(root/'state.sqlite3');v['started']=time.time()-301;c.execute('update torrent_trials set document=? where id=?',(json.dumps(v),v['id']));c.commit();c.close()
 for _ in range(120):
  v=req('/v1/trials')['trials'][0]
  if v['state']=='complete':break
  time.sleep(.1)
 assert v['state']=='complete',v
 assert v['winner']=='3'*40,v
 assert original in torrents and (root/'source/original.bin').read_bytes()==b'preserve'
 assert set(removed)=={'1'*40,'2'*40},removed
 assert (root/'source/candidate3.bin').exists()
 assert all(not pathlib.Path(c['directory']).joinpath(f'candidate{i}.bin').exists() for i,c in enumerate(v['candidates']) if i in [1,2])
 print('PASS: automatic winner, Transmission move, loser deletion, duplicate preservation, persisted hash discoveries')
 print('Known hashes:',len(req('/v1/search/torrents/hashes')['hashes']))
 v=req('/v1/trials',{'candidates':[{'name':f'Stalled {n}','source':{'metainfo':base64.b64encode(str(n).encode()).decode()}} for n in [4,5,6]]})
 for _ in range(100):
  v=req('/v1/trials')['trials'][0]
  if v['state']=='running':break
  time.sleep(.1)
 with lock:
  for n in [4,5,6]:torrents[str(n)*40]['downloaded_ever']=0
 c=sqlite3.connect(root/'state.sqlite3');v['started']=time.time()-301;c.execute('update torrent_trials set document=? where id=?',(json.dumps(v),v['id']));c.commit();c.close()
 for _ in range(100):
  v=req('/v1/trials')['trials'][0]
  if v['state']=='review':break
  time.sleep(.1)
 assert v['state']=='review',v
 assert all(torrents[str(n)*40]['status']==0 for n in [4,5,6])
 req('/v1/trials/'+v['id'],{'cancel':True})
 for _ in range(100):
  v=req('/v1/trials')['trials'][0]
  if v['state']=='cancelled':break
  time.sleep(.1)
 assert v['state']=='cancelled',v
 assert set(torrents)=={original,'3'*40},list(torrents)
 print('PASS: restart recovery, no-progress pause, cancellation, existing/winning torrents preserved')
finally:
 proc.terminate();proc.wait(timeout=10);server.shutdown();log.close();print('Fixture evidence:',root)
