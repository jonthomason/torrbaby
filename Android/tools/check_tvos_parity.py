#!/usr/bin/env python3
"""Flag changes to the tvOS reference since the last explicit Android parity review."""
import hashlib,json,pathlib,sys
root=pathlib.Path(__file__).resolve().parents[1]
baseline=root/'docs/tvos-reference.json'
if not baseline.exists():
 print('tvOS parity: no reviewed baseline yet.');raise SystemExit(0)
review=json.loads(baseline.read_text());changed=[]
for name,digest in review['files'].items():
 path=root.parent/name
 if not path.exists() or hashlib.sha256(path.read_bytes()).hexdigest()!=digest:changed.append(name)
if changed:
 print('tvOS reference changed since '+review['reviewed']+':')
 for name in changed:print('  '+name)
 print('Review Android/docs/TVOS_PARITY.md before updating the baseline.')
 if '--strict' in sys.argv:raise SystemExit(1)
else:print('tvOS reference matches the reviewed Android parity baseline.')
