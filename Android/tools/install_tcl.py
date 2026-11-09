#!/usr/bin/env python3
"""Install a review APK through TCL T-Cast; TV owner accepts the native prompt.
Serves only that APK on a temporary random URL, then closes both connections.
"""
import argparse, hashlib, http.server, pathlib, socket, struct, threading, time, uuid
p=argparse.ArgumentParser()
p.add_argument('--tv',required=True);p.add_argument('--host',required=True)
p.add_argument('--apk',type=pathlib.Path,required=True)
p.add_argument('--port',type=int,default=18767)
p.add_argument('--package',default='com.torrbaby.tv.debug')
p.add_argument('--version-code',type=int,required=True)
a=p.parse_args();payload=a.apk.read_bytes();route='/'+uuid.uuid4().hex+'/Torrbaby.apk'
class APK(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  if self.path!=route:self.send_error(404);return
  self.send_response(200);self.send_header('Content-Type','application/vnd.android.package-archive');self.send_header('Content-Length',str(len(payload)));self.end_headers();self.wfile.write(payload)
  print('TV fetched APK:',len(payload),'bytes',flush=True)
 def log_message(self,*args):pass
s=socket.create_connection((a.tv,6553),5);s.settimeout(1)
server=http.server.ThreadingHTTPServer((a.host,a.port),APK)
threading.Thread(target=server.serve_forever,daemon=True).start();buf=b''
def send(message):
 data=message.encode();s.sendall(struct.pack('>I',len(data))+data)
def read():
 global buf
 try:
  chunk=s.recv(65536)
  if not chunk:raise RuntimeError('TV closed connection')
  buf+=chunk
 except socket.timeout:return []
 out=[]
 while len(buf)>=4:
  size=struct.unpack('>I',buf[:4])[0]
  if size>1048576:raise RuntimeError('Invalid T-Cast frame')
  if len(buf)<4+size:break
  out.append(buf[4:4+size].decode(errors='replace'));buf=buf[4+size:]
 return out
try:
 send('159>>1>>0');deadline=time.monotonic()+2
 while time.monotonic()<deadline:read()
 url=f'http://{a.host}:{a.port}{route}'
 send('223>>INSTALL>>'+a.package+'>>Torrbaby>>'+url+'>>2>>'+str(a.version_code)+'>>'+str(len(payload)/1048576)+'>>>>>>>>'+hashlib.md5(url.encode()).hexdigest())
 print('Installation requested. Accept the prompt on the TCL.',flush=True)
 deadline=time.monotonic()+180;heartbeat=time.monotonic();installed=False
 while time.monotonic()<deadline:
  for message in read():
   if not message.startswith('150>>'):print(message,flush=True)
   if message.startswith('223>>PROGRESS>>'+a.package) and '>>5>>' in message:
    installed=True;send('223>>PACKAGEINFO>>'+a.package);deadline=time.monotonic()+4
  if time.monotonic()-heartbeat>10:send('150>>');heartbeat=time.monotonic()
 if not installed:raise SystemExit('Installation was not confirmed before timeout.')
 print('TCL reported installation complete.',flush=True)
finally:
 s.close();server.shutdown();server.server_close()
