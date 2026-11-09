#!/usr/bin/env python3
"""Isolated TV development API. Never forwards writes to a real server.
Run with --catalog .local/library.json for a private production metadata snapshot.
All video is generated test footage; artwork remains local, ignored by Git.
"""
import argparse,base64,copy,json,mimetypes,pathlib,re,threading,time,uuid
from http.server import ThreadingHTTPServer,BaseHTTPRequestHandler
from urllib.parse import urlsplit,unquote
ROOT=pathlib.Path(__file__).resolve().parents[1]/'.local'
LOCK=threading.RLock();SESSIONS={};EVENTS=[];MODE='normal'
PREFS={'enabled':False,'language':'original','font':'default','size_percent':100,'text_opacity':100,'background_opacity':0}
OVERRIDES={}
SEEDING={'ratio':1.2,'enabled':True}
TORRENTS=[dict(name='The Observatory',percent_done=.42,status=4,rate_download=4500000,rate_upload=250000,upload_ratio=.3,files=[dict(length=4000000000)]),dict(name='Night Sky',percent_done=1,status=6,rate_download=0,rate_upload=1000000,upload_ratio=1.7,files=[dict(length=2000000000)])]
for i,t in enumerate(TORRENTS):
 t.update(hash_string=str(i+1)*40,download_dir="/fixture/media",added_date=time.time()-i*100,eta=300,peers_sending_to_us=4,peers_getting_from_us=2)
 for f in t["files"]:f.update(name=t["name"]+".mp4",bytes_completed=int(f["length"]*t["percent_done"]))
def synthetic():
 titles=[]
 for i in range(30):
  tv=i%3==0
  titles.append(dict(id=f'test:{i}',kind='tv' if tv else 'movie',title=f'The Observatory {i+1}',year=str(2025-i),genres=['Documentary','Science Fiction'],summary='A journey through the night sky, from familiar constellations to distant worlds.',content_rating='PG',duration_minutes=90,added_at=time.time()-i*3600,assets=[dict(id=f'asset-{i}-{e}',name=f'Episode {e+1}' if tv else 'The Observatory',season=1,episode=e+1 if tv else 0,complete=True,role='main',position=0,duration=120,watched=False,last_played=0) for e in range(4 if tv else 1)]))
 return dict(titles=titles,error=None)
def assets():return {a['id']:a for t in CATALOG['titles'] for a in t['assets']}
class Handler(BaseHTTPRequestHandler):
 protocol_version='HTTP/1.1'
 def log_message(self,*args):pass
 def json(self,data,status=200):
  raw=json.dumps(data).encode();self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(raw)));self.end_headers()
  if self.command!='HEAD':self.wfile.write(raw)
 def do_HEAD(self):self.do_GET()
 def do_GET(self):
  path=unquote(urlsplit(self.path).path)
  if path=='/test/events':return self.json(EVENTS)
  if path=='/test/health':return self.json({'ok':True,'titles':len(CATALOG['titles'])})
  if MODE=='unauthorized':return self.json({'error':'Revoked'},401)
  if self.headers.get('Authorization')!='Bearer fixture':return self.json({'error':'Invalid fixture credential'},401)
  if path.endswith('/subtitle-preferences'):return self.json({'defaults':PREFS,'effective':OVERRIDES.get(path,PREFS),'overridden':path in OVERRIDES})
  if path=='/v1/torrents':return self.json(TORRENTS)
  if path=='/v1/seeding':return self.json(SEEDING)
  if path=='/v1/library':
   if MODE=='offline':return self.json({'error':'Fixture server unavailable'},503)
   return self.json(dict(titles=[],error=None) if MODE=='empty' else CATALOG)
  m=re.fullmatch('/v1/library/artwork/(.+)/(poster|backdrop|episode)',path)
  if m:
   file=ROOT/'art'/(m[1].replace(':','_')+'-'+m[2]+'.jpg')
   return self.json({'data':base64.b64encode(file.read_bytes()).decode()}) if file.exists() else self.json({'error':'No artwork'},404)
  if MODE=='slow_subtitles' and path.endswith('/subtitles'):time.sleep(8)
  if path.endswith('/subtitles') and MODE=='translated':return self.json({'tracks':[], 'embedded_tracks':[{'track':2,'language':'eng','codec':'tx3g','extraction_supported':True}]})
  if path.endswith('/subtitles'):return self.json({'tracks':[{'id':'en','name':'English','language':'en','format':'webvtt','path':path+'/en.vtt'}],'embedded_tracks_scanned':False})
  if path.endswith('.vtt'):
   EVENTS.append({'type':'subtitle_stream','path':path});return self.file(ROOT/'sample.vtt','text/vtt')
  if path.startswith('/v1/stream/'):
   EVENTS.append({'type':'stream','range':self.headers.get('Range'),'time':time.time()})
   if MODE=='bad_video':return self.json({'error':'Media unavailable'},503)
   return self.file(ROOT/'sample.mp4','video/mp4')
  self.json({'error':'Not found'},404)
 def do_PATCH(self):
  body=json.loads(self.rfile.read(int(self.headers.get('Content-Length','0'))) or '{}')
  if self.headers.get('Authorization')!='Bearer fixture':return self.json({},401)
  if self.path=='/v1/seeding':
   SEEDING.update(body);EVENTS.append(dict(type='seeding',body=body));return self.json(SEEDING)
  return self.json({},404)
 def do_PUT(self):
  path=unquote(urlsplit(self.path).path)
  body=json.loads(self.rfile.read(int(self.headers.get('Content-Length','0'))) or '{}')
  if self.headers.get('Authorization')!='Bearer fixture':return self.json({'error':'Unauthorized'},401)
  if not path.endswith('/subtitle-preferences'):return self.json({},404)
  with LOCK:
   if path=='/v1/subtitle-preferences':PREFS.update(body)
   else:OVERRIDES[path]=body
  self.json({'effective':body})
 def do_DELETE(self):
  path=unquote(urlsplit(self.path).path)
  self.rfile.read(int(self.headers.get('Content-Length','0')))
  if self.headers.get('Authorization')!='Bearer fixture':return self.json({'error':'Unauthorized'},401)
  with LOCK:OVERRIDES.pop(path,None)
  self.json({'effective':PREFS})
 def do_POST(self):
  global MODE
  path=unquote(urlsplit(self.path).path);body=json.loads(self.rfile.read(int(self.headers.get('Content-Length','0'))) or '{}')
  with LOCK:
   if path=='/test/mode':MODE=body['mode'];return self.json({'mode':MODE})
   if path=='/v1/lan/register':return self.json({'client_id':'lan-fixture','token':'fixture'})
   if path=='/v1/pairings/redeem':return self.json({'client_id':'paired-fixture','token':'fixture'})
   if self.headers.get('Authorization')!='Bearer fixture':return self.json({'error':'Invalid fixture credential'},401)
   if MODE=='unauthorized':return self.json({'error':'Revoked'},401)
   if path=='/v1/torrents':
    EVENTS.append(dict(type='torrent_add',body=body));return self.json({'accepted':True})
   m=re.fullmatch('/v1/torrents/([^/]+)/([^/]+)',path)
   if m:
    if MODE=='offline':return self.json({'error':'Unavailable'},503)
    item=next((t for t in TORRENTS if t['hash_string']==m[1]),None)
    if not item:return self.json({'error':'Not found'},404)
    action=m[2];EVENTS.append(dict(type='torrent_action',action=action,hash=m[1],body=body))
    if action=='stop':item['status']=0
    if action=='start':item['status']=4 if item['percent_done']<1 else 6
    if action=='verify':item['status']=2
    if action=='rename':item['name']=body['name']
    if action=='remove':TORRENTS.remove(item)
    return self.json({'accepted':True})
   if path.endswith('/subtitles/prepare'):
    EVENTS.append(dict(type='subtitle_prepare',body=body))
    return self.json({'id':'fixture-translated','state':'ready','path':'/v1/subtitle-jobs/fixture-translated.vtt'})
   if path=='/v1/clients':
    if body.get('platform') not in ['macOS','iOS','iPadOS','tvOS','CLI','other']:return self.json({'error':'Unknown client platform'},400)
    EVENTS.append({'type':'client_register','body':body})
    return self.json(body)
   if path=='/v1/playback/progress':
    if MODE=='offline':return self.json({'error':'Offline'},503)
    EVENTS.append(dict(type='progress',**body));record=SESSIONS.get(body['session_id'])
    if not record:return self.json({'error':'No session'},404)
    a=assets()[record['asset']]
    if body['sequence']<=record['sequence'] or record['ended']:return self.json({'accepted':False,'reason':'superseded_or_stale'})
    record.update(sequence=body['sequence'],ended=body['ended']);a.update(position=body['position'],duration=body['duration'],last_played=time.time(),watched=a.get('watched',False) or body['position']>=body['duration']*.95)
    return self.json({'accepted':True,'watched':a['watched']})
   m=re.fullmatch('/v1/library/assets/(.+)/(play|watched)',path)
   if m:
    a=assets().get(m[1])
    if not a:return self.json({'error':'Missing asset'},404)
    if m[2]=='watched':a.update(watched=body['watched'],position=0 if not body['watched'] else a.get('position',0));return self.json({'ok':True})
    if not a['complete']:return self.json({'error':'Not complete'},409)
    session=uuid.uuid4().hex;SESSIONS[session]={'asset':a['id'],'sequence':-1,'ended':False};EVENTS.append(dict(type='play',asset=a['id'],session=session))
    return self.json({'path':'/v1/stream/'+a['id']+'/0?sig=fixture','session_id':session,'resume_position':0 if a.get('watched') else a.get('position',0),'name':a['name']})
   self.json({'error':'Not found'},404)
 def file(self,path,mime):
  if not path.exists():return self.json({'error':'Generate sample media first'},404)
  size=path.stat().st_size;start=0;end=size-1;partial=False
  r=self.headers.get('Range','');m=re.fullmatch(r'bytes=(\d+)-(\d*)',r)
  if m:
   start=int(m[1]);end=min(size-1,int(m[2])) if m[2] else size-1;partial=True
   if start>=size:return self.json({'error':'Range not satisfiable'},416)
  self.send_response(206 if partial else 200);self.send_header('Content-Type',mime);self.send_header('Accept-Ranges','bytes');self.send_header('Content-Length',str(end-start+1))
  if partial:self.send_header('Content-Range',f'bytes {start}-{end}/{size}')
  self.end_headers()
  if self.command=='HEAD':return
  try:
   with path.open('rb') as f:
    f.seek(start);remaining=end-start+1
    while remaining:
     chunk=f.read(min(65536,remaining))
     if not chunk:break
     self.wfile.write(chunk);remaining-=len(chunk)
  except (BrokenPipeError,ConnectionResetError):pass
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--catalog',type=pathlib.Path);p.add_argument('--port',type=int,default=18765);args=p.parse_args()
 CATALOG=json.loads(args.catalog.read_text()) if args.catalog else synthetic()
 for t in CATALOG['titles']:
  for a in t['assets']:
   # Synthetic language inventory for detail-page verification, not production metadata.
   a.update(duration=120,position=0,watched=False,last_played=0,media_info={'state':'ready','audio_tracks':[{'language':'eng'}],'subtitle_tracks':[{'language':'eng'}],'prepared':[{'id':'fixture-zh','language':'zh-Hans','generated':True,'state':'ready'}]})
 for i,t in enumerate(CATALOG['titles'][:4]):
  if t['assets']:t['assets'][0].update(position=20+i*15,last_played=time.time()-i*60)
 print(f'Fixture listening on 127.0.0.1:{args.port}',flush=True)
 ThreadingHTTPServer(('127.0.0.1',args.port),Handler).serve_forever()
