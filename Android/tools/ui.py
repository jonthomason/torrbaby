#!/usr/bin/env python3
"""Remote-only emulator inspection: keys, hierarchy, screenshots. Never taps coordinates."""
import argparse,pathlib,subprocess,xml.etree.ElementTree as ET,time
ROOT=pathlib.Path(__file__).resolve().parents[1]
ADB=pathlib.Path.home()/'Library/Android/sdk/platform-tools/adb'
p=argparse.ArgumentParser();p.add_argument('--serial',default='emulator-5554');p.add_argument('--keys',nargs='*',default=[]);p.add_argument('--shot');p.add_argument('--no-tree',action='store_true');args=p.parse_args()
def adb(*cmd,**kw):return subprocess.check_output([str(ADB),'-s',args.serial,*cmd],**kw)
for key in args.keys:
 adb('shell','input','keyevent',key);time.sleep(.25)
if args.shot:
 time.sleep(.6)
 dest=ROOT/'.local/screenshots'/f'{args.shot}.png';dest.parent.mkdir(exist_ok=True,parents=True);dest.write_bytes(adb('exec-out','screencap','-p'));print(dest)
if args.no_tree:raise SystemExit(0)
adb('shell','rm','-f','/sdcard/torrbaby-window.xml')
result=adb('shell','uiautomator','dump','/sdcard/torrbaby-window.xml')
if b'dumped to' not in result:raise SystemExit('UI was not idle; use --no-tree for active playback')
xml=adb('exec-out','cat','/sdcard/torrbaby-window.xml');(ROOT/'.local/window.xml').write_bytes(xml)
for n in ET.fromstring(xml).iter('node'):
 if n.get('text') or n.get('content-desc') or n.get('focused')=='true':
  print(('FOCUS ' if n.get('focused')=='true' else '      ')+(n.get('text') or n.get('content-desc') or n.get('class'))+' '+n.get('bounds',''))
