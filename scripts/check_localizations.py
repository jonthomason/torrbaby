#!/usr/bin/env python3
"""Verify reviewed resources and require EN/ZH for UI literals added since --base."""
import argparse, json, pathlib, re, subprocess, sys
root=pathlib.Path(__file__).resolve().parents[1]
p=argparse.ArgumentParser();p.add_argument('--base',default='HEAD');args=p.parse_args()
translations=dict(line.split('\t',1) for line in (root/'localization/zh-Hans.tsv').read_text().splitlines() if '\t' in line)
catalog=json.loads((root/'Apple/Torrbaby/Localizable.xcstrings').read_text())['strings']
errors=[]
for key,zh in translations.items():
 if not zh.strip():errors.append('Empty Chinese translation: '+key)
 entry=catalog.get(key,{}).get('localizations',{})
 for locale,value in [('en',key),('zh-Hans',zh)]:
  if entry.get(locale,{}).get('stringUnit',{}).get('value')!=value:errors.append('Regenerate '+locale+' resource: '+key)
diff=subprocess.check_output(['git','diff','--unified=0',args.base,'--','Apple/Torrbaby','Android/app/src/main'],cwd=root,text=True)
added='\n'.join(line[1:] for line in diff.splitlines() if line.startswith('+') and not line.startswith('+++'))
# Content metadata uses Text(verbatim:), variable expressions, or explicit nonlocalized fields.
patterns=[r'(?:Text|Button|Label|ProgressView|Toggle|Picker|L10n\.t|L10n\.text|LocalizedStringKey|accessibilityLabel|navigationTitle)\(\s*"([^"\n]*)"',r'Ui\.(?:button|text|title)\([^,]+,\s*"([^"\n]*)"']
for pattern in patterns:
 for key in re.findall(pattern,added):
  if not key or key in ['Torrbaby','EN','中文'] or '\\(' in key:continue
  if key not in translations:errors.append('Missing EN/ZH entry for new UI text: '+key)
if errors:print('\n'.join(sorted(set(errors))),file=sys.stderr);sys.exit(1)
print(f'Localization checks passed: {len(translations)} reviewed EN/ZH entries and added UI literals.')
