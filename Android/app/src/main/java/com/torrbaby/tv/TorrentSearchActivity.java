package com.torrbaby.tv;

import android.app.*;
import android.content.*;
import android.os.Bundle;
import android.view.*;
import android.view.inputmethod.EditorInfo;
import android.widget.*;
import java.util.*;
import java.util.concurrent.*;
import org.json.*;

/** Explicit remote indexer searches, separate from the retained media browser. */
public final class TorrentSearchActivity extends Activity {
  private final ExecutorService io = Executors.newFixedThreadPool(2);
  private final ExecutorService sizeIo = Executors.newSingleThreadExecutor();
  private final Set<String> sizeRequests = new HashSet<>();
  private final Map<String,String> sizes = new HashMap<>();
  private Api api;
  private LinearLayout resultsRoot;
  private EditText query;
  private TextView status;
  private Button search, sourceButton;
  private ListView list;
  private JSONArray results = new JSONArray();
  private int source, generation;
  private boolean china;
  private boolean inDetail;
  private final Set<String> added = new HashSet<>();
  private static final String[] SOURCE_NAMES = {"Recommended", "The Pirate Bay", "Nyaa", "Torrent CSV"};

  private String[] sourceNames() { return china ? new String[]{"Recommended", "BT之家", "动漫花园", "蜜柑计划"} : SOURCE_NAMES; }
  public void onCreate(Bundle state) {
    super.onCreate(state);
    try { api = new SecureStore(this).load(); } catch (Exception ignored) {}
    if (api == null) { finish(); return; }
    resultsRoot = page();
    Ui.add(resultsRoot, Ui.title(this, "Find torrents", 28), -1, 48);
    china = getPreferences(MODE_PRIVATE).getBoolean("china", false);
    RadioGroup markets = new RadioGroup(this); markets.setOrientation(LinearLayout.HORIZONTAL);
    RadioButton world = new RadioButton(this); world.setText("World"); world.setId(View.generateViewId()); world.setTextColor(Ui.TEXT);
    RadioButton chinese = new RadioButton(this); chinese.setText("China"); chinese.setId(View.generateViewId()); chinese.setTextColor(Ui.TEXT);
    markets.addView(world); markets.addView(chinese); markets.check(china ? chinese.getId() : world.getId());
    Ui.add(resultsRoot, markets, -1, 48);
    markets.setOnCheckedChangeListener((group,id) -> {
      china = id == chinese.getId(); getPreferences(MODE_PRIVATE).edit().putBoolean("china",china).apply();
      ++generation; source=0; sourceButton.setText("Recommended"); results=new JSONArray();
      ((BaseAdapter)list.getAdapter()).notifyDataSetChanged(); status.setText(""); search.setEnabled(true); list.setEnabled(true);
      if(query.getText().toString().trim().length()>=2)search();
    });
    LinearLayout controls = Ui.row(this);
    query = new EditText(this); query.setSingleLine(true); query.setHint("Search torrents");
    query.setTextColor(Ui.TEXT); query.setHintTextColor(Ui.MUTED); query.setTextSize(17);
    query.setImeOptions(EditorInfo.IME_ACTION_SEARCH);
    query.setOnEditorActionListener((v, action, event) -> { if (action == EditorInfo.IME_ACTION_SEARCH || (event != null && event.getAction() == KeyEvent.ACTION_UP && event.getKeyCode() == KeyEvent.KEYCODE_ENTER)) { search(); return true; } return false; });
    controls.addView(query, new LinearLayout.LayoutParams(0, Ui.dp(this, 46), 1));
    search = Ui.button(this, "Search", this::search); Ui.add(controls, search, 120, 46);
    Ui.gap(controls, 12);
    sourceButton = Ui.button(this, SOURCE_NAMES[0], () -> new AlertDialog.Builder(this).setTitle("Sources")
        .setSingleChoiceItems(sourceNames(), source, (dialog, which) -> { source=which; sourceButton.setText(sourceNames()[which]); dialog.dismiss(); }).show());
    Ui.add(controls, sourceButton, 180, 46); Ui.add(resultsRoot, controls, -1, 48);
    status = Ui.text(this, "", 14, Ui.MUTED); Ui.add(resultsRoot, status, -1, 36);
    list = new ListView(this); list.setDivider(null); list.setDividerHeight(Ui.dp(this, 10));
    list.setAdapter(new Results()); list.setOnItemClickListener((parent, view, position, id) -> details(results.optJSONObject(position)));
    resultsRoot.addView(list, new LinearLayout.LayoutParams(-1,0,1));
    setContentView(resultsRoot); query.requestFocus();
    String initial = getIntent().getStringExtra("query");
    if(initial != null && !initial.isEmpty()) { query.setText(initial); search(); }
  }
  private LinearLayout page() { LinearLayout root=Ui.column(this); root.setBackgroundColor(Ui.BG); Ui.pad(root,36,24); return root; }
  private void search() {
    String value=query.getText().toString().trim(); if (value.length()<2) return;
    int request=++generation; int provider=source; boolean requestChina=china; search.setEnabled(false); list.setEnabled(false); status.setText("Searching…");
    ((android.view.inputmethod.InputMethodManager)getSystemService(INPUT_METHOD_SERVICE)).hideSoftInputFromWindow(query.getWindowToken(),0);
    io.execute(() -> {
      try {
        JSONArray sources=new JSONArray(); if(provider>0)sources.put(requestChina ? (provider==1?"1lou":provider==2?"dmhy":"mikan") : (provider==1?"tpb":provider==2?"nyaa":"csv"));
        JSONObject body = new JSONObject().put("query",value).put("market",requestChina?"china":"world").put("sources",sources);
        // Context applies only while searching the original library title.
        if(value.equals(getIntent().getStringExtra("query"))) {
          if(getIntent().hasExtra("year"))body.put("year",getIntent().getIntExtra("year",0));
          if(getIntent().hasExtra("season"))body.put("season",getIntent().getIntExtra("season",0));
        }
        JSONObject response=api.request("POST","/v1/search/torrents",body);
        runOnUiThread(() -> {
          if(isDestroyed() || request!=generation)return;
          search.setEnabled(true); list.setEnabled(true); results=response.optJSONArray("items"); if(results==null)results=new JSONArray();
          List<String> messages=new ArrayList<>();
          JSONArray providers=response.optJSONArray("providers");
          if(providers!=null) for(int i=0;i<providers.length();i++) {JSONObject p=providers.optJSONObject(i); if(p!=null&&!p.isNull("error"))messages.add(label(p.optString("provider"))+": "+p.optString("error"));}
          if(messages.isEmpty() || results.length()>0)messages.add(0,results.length()+" results");
          status.setText(String.join(" · ",messages)); ((BaseAdapter)list.getAdapter()).notifyDataSetChanged();
          if(results.length()>0)list.requestFocus();
        });
      } catch(Exception error) {runOnUiThread(() -> {if(!isDestroyed()&&request==generation){search.setEnabled(true);list.setEnabled(true);status.setText(MainActivity.message(error));}});}
    });
  }
  private static String label(String provider) {return provider.equals("1lou")?"BT之家":provider.equals("dmhy")?"动漫花园":provider.equals("mikan")?"蜜柑计划":provider.equals("tpb")?"The Pirate Bay":provider.equals("csv")?"Torrent CSV":"Nyaa";}
  private String summary(JSONObject item) {
    List<String> parts=new ArrayList<>(); if(!item.isNull("size_bytes"))parts.add(android.text.format.Formatter.formatFileSize(this,item.optLong("size_bytes")));
    else if(!item.isNull("size_label") && !item.optString("size_label").isEmpty())parts.add(item.optString("size_label"));
    else parts.add(sizes.getOrDefault(item.optString("name"), "Size…"));
    if(!item.isNull("seeders"))parts.add(item.optLong("seeders")+(item.optLong("seeders")==1?" seed":" seeds"));
    if(!item.isNull("leechers"))parts.add(item.optLong("leechers")+(item.optLong("leechers")==1?" peer":" peers"));
    return String.join(" · ",parts);
  }
  private static String title(JSONObject item) {
    String name=item.optString("name");
    if(name.startsWith("[BT下载][")) { int end=name.indexOf(']',7); if(end>7)return name.substring(7,end); }
    int bracket=name.indexOf('[');
    if(bracket>0 && name.substring(0,bracket).matches(".*[\\p{IsHan}].*"))return name.substring(0,bracket).trim();
    return name;
  }
  private static String tags(JSONObject item) {
    JSONObject hints=item.optJSONObject("hints");if(hints==null)return ""; List<String> values=new ArrayList<>();
    for(String key:new String[]{"resolution","video_codec","release_source"})if(!hints.isNull(key))values.add(hints.optString(key));
    if(hints.optInt("episode_count")>0)values.add("Complete · "+hints.optInt("episode_count")+" episodes");
    for(String key:new String[]{"audio","languages"}) {JSONArray a=hints.optJSONArray(key);if(a!=null)for(int i=0;i<a.length();i++)values.add(a.optString(i));}
    return String.join(" · ",values);
  }
  private void loadSize(JSONObject item) {
    if(!item.isNull("size_bytes") || (!item.isNull("size_label") && !item.optString("size_label").isEmpty()))return;
    String key=item.optString("name"); if(!sizeRequests.add(key))return;
    JSONArray sources=item.optJSONArray("sources"); JSONObject source=sources==null?null:sources.optJSONObject(0);
    if(source==null || !source.optString("provider").equals("1lou"))return;
    sizeIo.execute(() -> {
      String label="Size unavailable";
      try {
        JSONObject d=api.request("POST","/v1/search/torrents/detail",new JSONObject().put("provider","1lou").put("id",source.optString("id")));
        JSONArray releases=d.optJSONArray("releases");long min=Long.MAX_VALUE,max=0;
        if(releases!=null)for(int i=0;i<releases.length();i++){JSONObject r=releases.optJSONObject(i).optJSONObject("item");if(!r.isNull("size_bytes")){long n=r.optLong("size_bytes");min=Math.min(min,n);max=Math.max(max,n);}}
        if(max>0)label=android.text.format.Formatter.formatFileSize(this,min)+(min==max?"":"–"+android.text.format.Formatter.formatFileSize(this,max));
      } catch(Exception ignored) {}
      final String value=label;
      runOnUiThread(() -> {if(!isDestroyed()){sizes.put(key,value);((BaseAdapter)list.getAdapter()).notifyDataSetChanged();}});
    });
  }
  private class Results extends BaseAdapter {
    public int getCount(){return results.length();} public Object getItem(int p){return results.optJSONObject(p);} public long getItemId(int p){return p;}
    public View getView(int position, View old, ViewGroup parent) {
      LinearLayout row; if(old instanceof LinearLayout)row=(LinearLayout)old; else {
        row=Ui.column(TorrentSearchActivity.this); Ui.pad(row,16,6);row.setBackground(Ui.shape(Ui.PANEL,0,TorrentSearchActivity.this));
        TextView title=Ui.title(TorrentSearchActivity.this,"",20);title.setMaxLines(2);Ui.add(row,title,-1,-2);Ui.add(row,Ui.text(TorrentSearchActivity.this,"",18,Ui.ACCENT),-1,-2);Ui.add(row,Ui.text(TorrentSearchActivity.this,"",13,Ui.MUTED),-1,-2);
      }
      JSONObject item=results.optJSONObject(position);loadSize(item);((TextView)row.getChildAt(0)).setText(title(item));((TextView)row.getChildAt(1)).setText(summary(item));((TextView)row.getChildAt(2)).setText(tags(item));return row;
    }
  }
  private void details(JSONObject item) {
    if(item==null)return;inDetail=true;int request=++generation;
    LinearLayout root=page();TextView name=Ui.title(this,item.optString("name"),24);name.setMaxLines(3);Ui.add(root,name,-1,-2);
    Ui.gap(root,12);Ui.add(root,Ui.text(this,summary(item),14,Ui.MUTED),-1,30);
    LinearLayout actions=Ui.row(this);Ui.add(actions,Ui.button(this,"Back",this::onBackPressed),100,44);Ui.gap(actions,12);
    CheckBox paused=new CheckBox(this);paused.setText("Add paused");paused.setTextColor(Ui.TEXT);Ui.add(actions,paused,150,44);
    String hash=item.optString("info_hash").isEmpty()?item.optString("name"):item.optString("info_hash");Button add=Ui.button(this,added.contains(hash)?"Added":"Add torrent",()->{});
    add.setEnabled(!added.contains(hash));add.setOnClickListener(v -> add(item,paused.isChecked(),add));Ui.primary(add);Ui.add(actions,add,190,44);Ui.add(root,actions,-1,50);
    if(!tags(item).isEmpty())Ui.add(root,Ui.text(this,"Release tags · "+tags(item),14,Ui.MUTED),-1,40);
    Spinner attachments = new Spinner(this); attachments.setVisibility(View.GONE); Ui.add(root,attachments,-1,48);
    ListView files=new ListView(this);TextView description=Ui.text(this,"",15,Ui.TEXT);Ui.pad(description,0,14);files.addHeaderView(description,null,false);
    TextView note=Ui.text(this,"Loading details…",14,Ui.MUTED);files.addHeaderView(note,null,false);
    root.addView(files,new LinearLayout.LayoutParams(-1,0,1));root.addView(ProfileUi.header(this, api),0);setContentView(root);add.requestFocus();
    JSONArray sources=item.optJSONArray("sources");JSONObject chosen=null;
    if(sources!=null)for(int i=0;i<sources.length();i++)if(sources.optJSONObject(i).optString("provider").equals("tpb")){chosen=sources.optJSONObject(i);break;}
    if(chosen==null && sources!=null)for(int i=0;i<sources.length();i++)if(sources.optJSONObject(i).optString("provider").equals("nyaa")){chosen=sources.optJSONObject(i);break;}
    if(chosen==null && sources!=null)chosen=sources.optJSONObject(0);
    if(chosen==null){note.setText("Source details unavailable.");return;}
    JSONObject source=chosen;
    io.execute(() -> {try{
      JSONObject response=api.request("POST","/v1/search/torrents/detail",new JSONObject().put("provider",source.optString("provider")).put("id",source.optString("id")));
      JSONArray entries=response.optJSONArray("files");List<String> names=new ArrayList<>();if(entries!=null)for(int i=0;i<entries.length();i++){JSONObject f=entries.optJSONObject(i);names.add(f.optString("name")+(f.isNull("size_bytes")?"":"\n"+android.text.format.Formatter.formatFileSize(this,f.optLong("size_bytes"))));}
      runOnUiThread(()->{if(isDestroyed()||request!=generation)return;description.setText(sourceLanguages(response)+response.optString("description"));
        JSONArray releases=response.optJSONArray("releases");
        if(releases!=null && releases.length()>0) {
          List<String> labels=new ArrayList<>(); for(int j=0;j<releases.length();j++)labels.add(releases.optJSONObject(j).optJSONObject("item").optString("name"));
          attachments.setAdapter(new ArrayAdapter<String>(this,android.R.layout.simple_spinner_dropdown_item,labels));
          attachments.setVisibility(releases.length()>1?View.VISIBLE:View.GONE);
          attachments.setOnItemSelectedListener(new AdapterView.OnItemSelectedListener(){
            public void onNothingSelected(AdapterView<?> p){}
            public void onItemSelected(AdapterView<?> p,View v,int position,long id){
              JSONObject resolved=releases.optJSONObject(position).optJSONObject("item");
              if(!Boolean.TRUE.equals(add.getTag())) {
              add.setEnabled(!added.contains(resolved.optString("info_hash")));add.setText(added.contains(resolved.optString("info_hash"))?"Added":"Add torrent");
              add.setOnClickListener(view->add(resolved,paused.isChecked(),add));
              }
              JSONArray fs=releases.optJSONObject(position).optJSONArray("files");List<String> labels=new ArrayList<>();
              if(fs!=null)for(int j=0;j<fs.length();j++){JSONObject f=fs.optJSONObject(j);labels.add(f.optString("name")+"\n"+android.text.format.Formatter.formatFileSize(TorrentSearchActivity.this,f.optLong("size_bytes")));}
              note.setText(labels.size()+" files");files.setAdapter(new ArrayAdapter<String>(TorrentSearchActivity.this,android.R.layout.simple_list_item_1,labels){public View getView(int p,View old,ViewGroup parent){TextView row=(TextView)super.getView(p,old,parent);row.setTextColor(Ui.TEXT);row.setTextSize(14);return row;}});
            }
          });
        }
        note.setText(entries==null?"File list unavailable":names.size()+" files");files.setAdapter(new ArrayAdapter<String>(this,android.R.layout.simple_list_item_1,names){public View getView(int p,View old,ViewGroup parent){TextView row=(TextView)super.getView(p,old,parent);row.setTextColor(Ui.TEXT);row.setTextSize(14);return row;}});});
    }catch(Exception error){runOnUiThread(()->{if(!isDestroyed()&&request==generation)note.setText(item.optString("magnet").isEmpty()?"Couldn’t load this release. Please reopen it to retry.":"Couldn’t load source details. You can still add this torrent.");});}});
  }
  private static String sourceLanguages(JSONObject response) {
    StringBuilder text=new StringBuilder();
    String[] keys={"audio_language","subtitle_language"}, labels={"Audio","Subtitles"};
    for(int i=0;i<keys.length;i++) {
      Object raw=response.opt(keys[i]); if(!(raw instanceof String))continue;
      String value=((String)raw).trim();
      if(value.isEmpty() || value.equalsIgnoreCase("unknown") || value.equalsIgnoreCase("und") || value.equals("0"))continue;
      text.append(labels[i]).append(" · ").append(value).append("\n");
    }
    if(text.length()>0)text.append("\n"); return text.toString();
  }
  private void add(JSONObject item, boolean paused, Button button) {
    if(Boolean.TRUE.equals(button.getTag()))return;
    button.setTag(true);button.setEnabled(false);button.setText("Adding…");
    io.execute(()->{try{
      JSONObject payload=new JSONObject().put("paused",paused);
      if(!item.optString("magnet").isEmpty() && !item.has("selected_attachment"))payload.put("magnet",item.getString("magnet"));
      else {
        JSONArray sources=item.optJSONArray("sources");JSONObject source=null;
        if(sources!=null)for(int i=0;i<sources.length();i++)if(sources.optJSONObject(i).optString("provider").equals("1lou")){source=sources.optJSONObject(i);break;}
        if(source==null)throw new Exception("This release has no downloadable torrent.");
        JSONObject request=new JSONObject().put("provider","1lou").put("id",source.getString("id"));
        if(item.has("selected_attachment"))request.put("attachment",item.getString("selected_attachment"));
        JSONObject resolution=api.request("POST","/v1/search/torrents/resolve",request);
        JSONArray choices=resolution.optJSONArray("choices");
        if(choices!=null && choices.length()>0){
          String[] labels=new String[choices.length()];for(int i=0;i<labels.length;i++)labels[i]=choices.getJSONObject(i).getString("name");
          runOnUiThread(()->{if(isDestroyed())return;button.setTag(false);button.setEnabled(true);button.setText("Add torrent");
            new AlertDialog.Builder(this).setTitle("Choose a release").setItems(labels,(dialog,index)->{
              try{JSONObject selected=new JSONObject(item.toString()).put("selected_attachment",choices.getJSONObject(index).getString("id"));add(selected,paused,button);}catch(JSONException ignored){}
            }).setNegativeButton("Cancel",null).show();});return;
        }
        payload.put("metainfo",resolution.getString("metainfo"));
      }
      JSONObject response=api.request("POST","/v1/torrents",payload);
      runOnUiThread(()->{if(isDestroyed())return;added.add(item.optString("info_hash").isEmpty()?item.optString("name"):item.optString("info_hash"));button.setText("Added");Toast.makeText(this,response.has("torrent_duplicate")?"Already in your torrents":"Torrent added",Toast.LENGTH_LONG).show();});
    }catch(Exception error){runOnUiThread(()->{if(!isDestroyed()){button.setTag(false);button.setEnabled(true);button.setText("Add torrent");new AlertDialog.Builder(this).setTitle("Couldn’t add torrent").setMessage(MainActivity.message(error)).setPositiveButton("Close",null).show();}});}});
  }
  public void onBackPressed(){if(inDetail){inDetail=false;generation++;setContentView(resultsRoot);list.requestFocus();}else super.onBackPressed();}
  protected void onDestroy(){generation++;sizeIo.shutdownNow();io.shutdownNow();super.onDestroy();}
}
