"""Disposable loopback-only virtual MP4 proof; no production integration."""
import argparse, struct, os, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
U32=lambda b:struct.unpack('>I',b)[0]
U64=lambda b:struct.unpack('>Q',b)[0]
def box(kind,body):return struct.pack('>I4s',len(body)+8,kind)+body
def children(data):
 p=0
 while p<len(data):
  n=U32(data[p:p+4]);assert n>=8 and p+n<=len(data),'Unsupported box'
  yield data[p+4:p+8],data[p+8:p+n]
  p+=n
 assert p==len(data)
def top(path):
 out=[]
 with open(path,'rb') as f:
  size=os.fstat(f.fileno()).st_size
  while f.tell()<size:
   p=f.tell();h=f.read(8);n=U32(h[:4]);header=8
   if n==1:n=U64(f.read(8));header=16
   if n==0:n=size-p
   assert n>=header and p+n<=size
   out.append((h[4:],p,n,header));f.seek(p+n)
 return out

def moov(path):
 boxes=top(path);assert not any(k==b'moof' for k,*_ in boxes),'Fragmented source out of scope'
 found=[(p,n,h) for k,p,n,h in boxes if k==b'moov'];assert len(found)==1
 p,n,h=found[0];assert h==8 and n<32*1024*1024
 with open(path,'rb') as f:f.seek(p+h);data=f.read(n-h)
 return p,n,data

def scale(m):
 h=next(b for k,b in children(m) if k==b'mvhd');return U32(h[20:24] if h[0]==1 else h[12:16])
def trackid(t):
 h=next(b for k,b in children(t) if k==b'tkhd');return U32(h[20:24] if h[0]==1 else h[12:16])
def patch(data,offset,newid,oldscale,newscale,alt,dimensions):
 result=b''
 for k,b in children(data):
  if k in (b'mdia',b'minf',b'stbl',b'edts'):b=patch(b,offset,newid,oldscale,newscale,alt,dimensions)
  elif k in (b'stco',b'co64'):
   count=U32(b[4:8]);step=4 if k==b'stco' else 8
   assert len(b)==8+count*step
   b=b[:8]+b''.join(struct.pack('>Q',int.from_bytes(b[8+i*step:8+(i+1)*step],'big')+offset) for i in range(count));k=b'co64'
  elif k==b'tkhd':
   b=bytearray(b);v=b[0];ip=20 if v else 12;dp=28 if v else 20;sz=8 if v else 4;ap=46 if v else 34
   b[ip:ip+4]=struct.pack('>I',newid)
   duration=int.from_bytes(b[dp:dp+sz],'big');b[dp:dp+sz]=round(duration*newscale/oldscale).to_bytes(sz,'big')
   b[ap:ap+2]=struct.pack('>H',alt);b[-8:]=dimensions;b=bytes(b)
  elif k==b'elst':
   b=bytearray(b);sz=8 if b[0] else 4;step=20 if b[0] else 12
   for i in range(U32(b[4:8])):
    p=8+i*step;n=int.from_bytes(b[p:p+sz],'big');b[p:p+sz]=round(n*newscale/oldscale).to_bytes(sz,'big')
   b=bytes(b)
  result+=box(k,b)
 return result

class Virtual:
 def __init__(self,video,subtitles):
  begin=time.monotonic();vp,vn,vm=moov(video);sp,sn,sm=moov(subtitles)
  vsize=os.path.getsize(video);ssize=os.path.getsize(subtitles)
  ids=[trackid(t) for k,t in children(vm) if k==b'trak'];nextid=max(ids)+1
  assert not any(k==b'mvex' for k,_ in children(vm))
  dimensions=next(b[-8:] for k,t in children(vm) if k==b'trak' for z,b in children(t) if z==b'tkhd')
  tracks=[patch(t,vsize,nextid+i,scale(sm),scale(vm),32760,dimensions) for i,(k,t) in enumerate((x for x in children(sm) if x[0]==b'trak'))]
  merged=b''
  for k,b in children(vm):
   if k==b'mvhd':b=b[:-4]+struct.pack('>I',nextid+len(tracks))
   merged+=box(k,b)
  merged+=b''.join(box(b'trak',t) for t in tracks)
  # Preserve every original media byte/offset. Hide old moov boxes in the virtual view.
  self.parts=[(video,0,vp+4),(b'free',0,4),(video,vp+8,vsize-vp-8),
              (subtitles,0,4),(b'free',0,4),(subtitles,8,sp-4),(b'free',0,4),(subtitles,sp+8,ssize-sp-8),
              (box(b'moov',merged),0,len(merged)+8)]
  self.size=sum(n for _,_,n in self.parts);self.sent=0
  print(f'Original bytes={vsize}; subtitle bytes={ssize}; new moov bytes={len(merged)+8}; setup seconds={time.monotonic()-begin:.4f}',flush=True)
 def read(self,start,end):
  pos=0
  for source,offset,length in self.parts:
   a=max(start,pos);b=min(end,pos+length)
   if a<b:
    at=offset+a-pos;remaining=b-a
    if isinstance(source,bytes):yield source[at:at+remaining]
    else:
     with open(source,'rb') as f:
      f.seek(at)
      while remaining:
       data=f.read(min(remaining,65536));assert data;remaining-=len(data);yield data
   pos+=length

if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('video');p.add_argument('subtitles');p.add_argument('--port',type=int,default=18769);a=p.parse_args();v=Virtual(a.video,a.subtitles)
 class Handler(BaseHTTPRequestHandler):
  def do_HEAD(self):self.send(False)
  def do_GET(self):self.send(True)
  def send(self,body):
   if self.path!='/movie.mp4':self.send_error(404);return
   start,end=0,v.size;partial=False
   r=self.headers.get('Range')
   if r:
    try:
     assert r.startswith('bytes=') and ',' not in r
     left,right=r[6:].split('-');start=int(left) if left else max(0,v.size-int(right));end=min(v.size,int(right)+1) if left and right else v.size
     assert 0<=start<end<=v.size;partial=True
    except Exception:self.send_error(416);return
   self.send_response(206 if partial else 200);self.send_header('Content-Type','video/mp4');self.send_header('Accept-Ranges','bytes');self.send_header('Content-Length',str(end-start))
   if partial:self.send_header('Content-Range',f'bytes {start}-{end-1}/{v.size}')
   self.end_headers()
   if body:
    try:
     for data in v.read(start,end):self.wfile.write(data);v.sent+=len(data)
    except (BrokenPipeError,ConnectionResetError):pass
   print(f'range={start}-{end} bytes_sent_total={v.sent}',flush=True)
 ThreadingHTTPServer(('127.0.0.1',a.port),Handler).serve_forever()
