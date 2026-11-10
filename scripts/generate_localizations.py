#!/usr/bin/env python3
"""Generate native platform tables from reviewed EN -> Simplified Chinese entries.
No runtime translation of filenames, media metadata, profile names, or wire values.
"""
import hashlib, json, pathlib, re
from xml.sax.saxutils import escape
root=pathlib.Path(__file__).resolve().parents[1]
translations=dict(line.split('\t',1) for line in (root/'localization/zh-Hans.tsv').read_text().splitlines() if '\t' in line)
p=root/'Apple/Torrbaby/Localizable.xcstrings'
source=root/'local-state/localization-export/en.xcloc/Source Contents/Torrbaby/Localizable.xcstrings'
catalog=json.loads(p.read_text())
if source.exists():
 for key,value in json.loads(source.read_text())['strings'].items():catalog['strings'].setdefault(key,value)
for key,zh in translations.items():
 entry=catalog['strings'].setdefault(key,{})
 entry['extractionState']='manual'
 entry['localizations']={'en':{'stringUnit':{'state':'translated','value':key}},'zh-Hans':{'stringUnit':{'state':'translated','value':zh}}}
# Brand names, numeric formats, source names and URLs intentionally remain unchanged.
p.write_text(json.dumps(catalog,ensure_ascii=False,indent=2)+'\n')
entries={k:v for k,v in translations.items() if not re.search(r'%(?:@|lld|\.?\d*f)',k)}
ids={k:'ui_'+hashlib.sha1(k.encode()).hexdigest()[:12] for k in entries}
for folder,language in [('values','en'),('values-zh','zh')]:
 target=root/'Android/app/src/main/res'/folder/'strings.xml';target.parent.mkdir(exist_ok=True)
 def value(s):return escape(s).replace("'","\\'").replace('"','\\"')
 target.write_text('<?xml version="1.0" encoding="utf-8"?>\n<resources>\n'+''.join(f'    <string name="{ids[k]}" formatted="false">{value(k if language=="en" else entries[k])}</string>\n' for k in sorted(entries))+'</resources>\n')
code='''package com.torrbaby.tv;

import android.content.*;
import android.content.res.*;
import android.app.Activity;
import android.widget.*;
import java.util.*;

/** Interface preference only. Protocol values and content metadata are never translated. */
final class L10n {
  private static Context localized;
  private static final Map<String,Integer> IDS = new HashMap<>();
  static {\n'''
code+=''.join('    IDS.put('+json.dumps(k,ensure_ascii=False)+', R.string.'+ids[k]+');\n' for k in sorted(entries))
code+='''  }
  static Context wrap(Context base) {
    String choice = base.getSharedPreferences("interface",0).getString("language","system");
    Configuration config = new Configuration(base.getResources().getConfiguration());
    if (!choice.equals("system")) config.setLocale(choice.equals("zh") ? Locale.SIMPLIFIED_CHINESE : Locale.ENGLISH);
    localized = base.createConfigurationContext(config);
    return localized;
  }
  static String t(String key) {
    Integer id = IDS.get(key);
    return id == null || localized == null ? key : localized.getString(id);
  }
  static String[] labels(String[] values) {
    String[] translated = values.clone();
    for(int i=0;i<translated.length;i++)translated[i]=t(translated[i]);
    return translated;
  }
  static java.util.Locale locale() {
    return localized == null ? Locale.getDefault() : localized.getResources().getConfiguration().getLocales().get(0);
  }
  static LinearLayout picker(Activity activity) {
    LinearLayout row = Ui.row(activity);
    for(String code:new String[]{"en","zh"}) {
      Button button=Ui.button(activity, code.equals("en") ? "EN" : "中文", () -> {
        activity.getSharedPreferences("interface",0).edit().putString("language",code).apply();
        activity.recreate();
      });
      if(locale().getLanguage().equals(code)) { button.setBackground(Ui.shape(0xff007aff, 0xff007aff, activity)); button.setTextColor(0xffffffff); }
      Ui.add(row,button,64,40);Ui.gap(row,4);
    }
    return row;
  }
  static void system(Activity activity) {
    activity.getSharedPreferences("interface",0).edit().remove("language").apply();activity.recreate();
  }
}
'''
(root/'Android/app/src/main/java/com/torrbaby/tv/L10n.java').write_text(code)
print(f'{len(translations)} reviewed translations; {len(entries)} Android resources')
