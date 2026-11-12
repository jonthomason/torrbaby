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
  @Override protected void attachBaseContext(android.content.Context base) { super.attachBaseContext(L10n.wrap(base)); }

  private final ExecutorService io = Executors.newFixedThreadPool(2);
  /** The query the shown results came from; a cloud share's files are looked up again under it. */
  private String searchedQuery = "";
  private Api api;
  private LinearLayout resultsRoot;
  private EditText query;
  private TextView status;
  private Button search, sourceButton;
  private ListView list;
  private JSONArray results = new JSONArray(), allResults = new JSONArray();
  private String resolution="all";
  private Button resolutionButton;
  private volatile int enrichmentGeneration;
  private String resolutionLabel(){return resolution.equals("all")?L10n.t("All resolutions"):resolution;}
  private void applyResolution(){
    results=new JSONArray();for(int i=0;i<allResults.length();i++){
      JSONObject item=allResults.optJSONObject(i), hints=item.optJSONObject("hints");
      if(resolution.equals("all") || isCloud(item) || (hints!=null && resolution.equalsIgnoreCase(hints.optString("resolution"))))results.put(item);
    }
    if(list!=null)((BaseAdapter)list.getAdapter()).notifyDataSetChanged();
  }
  private int source, generation;
  private boolean china;
  private boolean inDetail;
  private JSONObject selectedItem;
  private final Set<String> added = new HashSet<>();
  private static final String[] SOURCE_NAMES = {"Recommended", "The Pirate Bay", "Nyaa", "Torrent CSV"};

  private String[] sourceNames() { return china ? new String[]{"Recommended"} : SOURCE_NAMES; }
  public void onCreate(Bundle state) {
    super.onCreate(state);
    try { api = new SecureStore(this).load(); } catch (Exception ignored) {}
    if (api == null) { finish(); return; }
    resultsRoot = page();
    Ui.add(resultsRoot, Ui.title(this, L10n.t("Find torrents"), 28), -1, 48);
    china = getPreferences(MODE_PRIVATE).getBoolean("china", false);
    RadioGroup markets = new RadioGroup(this); markets.setOrientation(LinearLayout.HORIZONTAL);
    RadioButton world = new RadioButton(this); world.setText(L10n.t("World")); world.setId(View.generateViewId()); world.setTextColor(Ui.TEXT);
    RadioButton chinese = new RadioButton(this); chinese.setText(L10n.t("China")); chinese.setId(View.generateViewId()); chinese.setTextColor(Ui.TEXT);
    markets.addView(world); markets.addView(chinese); markets.check(china ? chinese.getId() : world.getId());
    Ui.add(resultsRoot, markets, -1, 48);
    markets.setOnCheckedChangeListener((group,id) -> {
      china = id == chinese.getId(); getPreferences(MODE_PRIVATE).edit().putBoolean("china",china).apply();
      ++generation; source=0; sourceButton.setText(L10n.t("Recommended")); sourceButton.setVisibility(china?View.GONE:View.VISIBLE); resolutionButton.setVisibility(china?View.GONE:View.VISIBLE); results=new JSONArray();allResults=new JSONArray();enrichmentGeneration++;
      ((BaseAdapter)list.getAdapter()).notifyDataSetChanged(); status.setText(""); search.setEnabled(true); list.setEnabled(true);
      if(query.getText().toString().trim().length()>=2)search();
    });
    LinearLayout controls = Ui.row(this);
    query = new EditText(this); query.setSingleLine(true); query.setHint(L10n.t("Search torrents"));
    query.setTextColor(Ui.TEXT); query.setHintTextColor(Ui.MUTED); query.setTextSize(17);
    query.setImeOptions(EditorInfo.IME_ACTION_SEARCH);
    query.setOnEditorActionListener((v, action, event) -> { if (action == EditorInfo.IME_ACTION_SEARCH || (event != null && event.getAction() == KeyEvent.ACTION_UP && event.getKeyCode() == KeyEvent.KEYCODE_ENTER)) { search(); return true; } return false; });
    controls.addView(query, new LinearLayout.LayoutParams(0, Ui.dp(this, 46), 1));
    search = Ui.button(this, L10n.t("Search"), this::search); Ui.add(controls, search, 120, 46);
    Ui.gap(controls, 12);
    sourceButton = Ui.button(this, L10n.t(SOURCE_NAMES[0]), () -> new AlertDialog.Builder(this).setTitle(L10n.t("Sources"))
        .setSingleChoiceItems(L10n.labels(sourceNames()), source, (dialog, which) -> { source=which; sourceButton.setText(L10n.t(sourceNames()[which])); dialog.dismiss(); }).show());
    Ui.add(controls, sourceButton, 180, 46); Ui.add(resultsRoot, controls, -1, 48);
    // China is searched as Quark Drive shares, which have no source choice.
    sourceButton.setVisibility(china?View.GONE:View.VISIBLE);
    status = Ui.text(this, "", 14, Ui.MUTED); Ui.add(resultsRoot, status, -1, 36);
    resolution=getPreferences(MODE_PRIVATE).getString("resolution","all");
    resolutionButton=Ui.button(this,L10n.t("Resolution")+": "+resolutionLabel(),()->{
      String[] labels={L10n.t("All resolutions"),"2160p","1080p","720p"};String[] values={"all","2160p","1080p","720p"};
      new AlertDialog.Builder(this).setTitle(L10n.t("Resolution")).setSingleChoiceItems(labels,Arrays.asList(values).indexOf(resolution),(d,which)->{
        resolution=values[which];getPreferences(MODE_PRIVATE).edit().putString("resolution",resolution).apply();
        resolutionButton.setText(L10n.t("Resolution")+": "+resolutionLabel());enrichmentGeneration++;applyResolution();d.dismiss();
      }).show();
    });Ui.add(resultsRoot,resolutionButton,240,44);
    resolutionButton.setVisibility(china?View.GONE:View.VISIBLE);
    list = new ListView(this); list.setDivider(null); list.setDividerHeight(Ui.dp(this, 10));
    list.setAdapter(new Results()); list.setOnItemClickListener((parent, view, position, id) -> { JSONObject item=results.optJSONObject(position); if(isCloud(item))cloudDetails(item); else details(item); });
    resultsRoot.addView(list, new LinearLayout.LayoutParams(-1,0,1));
    resultsRoot.addView(ProfileUi.header(this, api), 0);
    setContentView(resultsRoot); query.requestFocus();
    String initial = getIntent().getStringExtra("query");
    if (state != null) {
      query.setText(state.getString("query", ""));
      source = state.getInt("source", 0);
      sourceButton.setText(L10n.t(sourceNames()[source]));
      try { allResults = new JSONArray(state.getString("results", "[]"));applyResolution(); } catch (JSONException ignored) { }
      added.addAll(state.getStringArrayList("added") == null ? Collections.emptyList() : state.getStringArrayList("added"));
      ((BaseAdapter)list.getAdapter()).notifyDataSetChanged();
      list.setSelection(state.getInt("position", 0));
      if (state.containsKey("selected")) {
        try { JSONObject item=new JSONObject(state.getString("selected")); searchedQuery=state.getString("searched",""); if(isCloud(item))cloudDetails(item); else details(item); } catch (JSONException ignored) { }
      } else if (state.getBoolean("searching")) search();
    } else if(initial != null && !initial.isEmpty()) { query.setText(initial); search(); }
  }
  @Override protected void onSaveInstanceState(Bundle state) {
    super.onSaveInstanceState(state);
    state.putString("query", query.getText().toString());
    state.putInt("source", source);
    state.putString("searched", searchedQuery);
    state.putString("results", allResults.toString());
    state.putStringArrayList("added", new ArrayList<>(added));
    state.putInt("position", list.getFirstVisiblePosition());
    state.putBoolean("searching", !search.isEnabled());
    if (inDetail && selectedItem != null) state.putString("selected", selectedItem.toString());
  }
  private LinearLayout page() { LinearLayout root=Ui.column(this); root.setBackgroundColor(Ui.BG); Ui.pad(root,36,24); return root; }
  private void search() {
    String value=query.getText().toString().trim(); if (value.length()<2) return;
    enrichmentGeneration++;
    int request=++generation; int provider=source; boolean requestChina=china; search.setEnabled(false); list.setEnabled(false); status.setText(L10n.t(requestChina?"Searching and checking links…":"Searching…"));
    ((android.view.inputmethod.InputMethodManager)getSystemService(INPUT_METHOD_SERVICE)).hideSoftInputFromWindow(query.getWindowToken(),0);
    io.execute(() -> {
      try {
        JSONArray sources=new JSONArray(); if(provider>0&&!requestChina)sources.put(provider==1?"tpb":provider==2?"nyaa":"csv");
        JSONObject body = new JSONObject().put("query",value).put("market",requestChina?"china":"world").put("sources",sources);
        // Context applies only while searching the original library title.
        if(value.equals(getIntent().getStringExtra("query"))) {
          if(getIntent().hasExtra("year"))body.put("year",getIntent().getIntExtra("year",0));
          if(getIntent().hasExtra("season"))body.put("season",getIntent().getIntExtra("season",0));
        }
        JSONObject response=api.request("POST","/v1/search/torrents",body);
        runOnUiThread(() -> {
          if(isDestroyed() || request!=generation)return;
          search.setEnabled(true); list.setEnabled(true); searchedQuery=response.optString("query",value); allResults=response.optJSONArray("items"); if(allResults==null)allResults=new JSONArray();applyResolution();
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
  private static String label(String provider) {return provider.equals("quark")?L10n.t("Quark Drive"):provider.equals("tpb")?"The Pirate Bay":provider.equals("csv")?"Torrent CSV":"Nyaa";}
  private static boolean isCloud(JSONObject item) {
    JSONArray sources=item==null?null:item.optJSONArray("sources");return sources!=null&&sources.length()>0&&"quark".equals(sources.optJSONObject(0).optString("provider"));
  }
  /** A share's title without its season, edition and quality, since it names the library folder. */
  private static String libraryName(String title) {
    String trimmed=title.replaceAll("\\s*(?:第[一二三四五六七八九十百\\d]+[季部]|[Ss]\\d{1,2}\\b|Season\\s*\\d|全\\d+集|\\d{3,4}[pP]\\b|4[Kk]\\b|特别版|未删减|完结|更新至|国剧|全集|合集|完整版|蓝光|高清).*$","").trim();
    return trimmed.isEmpty()?title:trimmed;
  }
  /** The season a share's title names ("第二季", "S02", "Season 2"), for files that do not say; 0 when none. */
  private static int seasonIn(String title) {
    java.util.regex.Matcher m=java.util.regex.Pattern.compile("第([一二三四五六七八九十\\d]{1,3})季").matcher(title);
    if(m.find()){String n=m.group(1);if(n.matches("\\d+"))return Integer.parseInt(n);String digits="一二三四五六七八九";
      if(n.equals("十"))return 10;if(n.length()==2&&n.charAt(0)=='十'&&digits.indexOf(n.charAt(1))>=0)return 11+digits.indexOf(n.charAt(1));
      if(n.length()==1&&digits.indexOf(n.charAt(0))>=0)return 1+digits.indexOf(n.charAt(0));return 0;}
    m=java.util.regex.Pattern.compile("(?i)\\b(?:S|Season\\s*)(\\d{1,2})\\b").matcher(title);
    return m.find()?Integer.parseInt(m.group(1)):0;
  }
  private static String shareUrl(JSONObject item) { return item.optJSONArray("sources").optJSONObject(0).optString("url"); }
  private String summary(JSONObject item) {
    List<String> parts=new ArrayList<>();
    if(isCloud(item)&&item.optInt("video_count")>0)parts.add(item.optInt("video_count")==1?L10n.t("1 video"):String.format(L10n.locale(),L10n.t("%d videos"),item.optInt("video_count")));
    if(!item.isNull("size_bytes"))parts.add(android.text.format.Formatter.formatFileSize(this,item.optLong("size_bytes")));
    else if(!item.isNull("size_label") && !item.optString("size_label").isEmpty())parts.add(item.optString("size_label"));
    if(isCloud(item)&&!item.isNull("added")&&!item.optString("added").isEmpty())parts.add(item.optString("added").substring(0,Math.min(10,item.optString("added").length())));
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
  private static String audioLabel(JSONObject hints) {
    String channels=hints.isNull("audio_channels")?"":hints.optString("audio_channels");
    switch(hints.isNull("audio_kind")?"":hints.optString("audio_kind")) {
      case "lossless_atmos": return L10n.t("Lossless Atmos");
      case "dtsx": return "DTS:X";
      case "atmos": return L10n.t("Dolby Atmos");
      case "lossless": return L10n.t("Lossless")+(channels.isEmpty()?"":" "+channels);
      case "surround": return L10n.t("Surround")+(channels.isEmpty()?"":" "+channels);
      case "stereo": return L10n.t("Stereo");
      case "mono": return L10n.t("Mono");
      default: return "";
    }
  }
  private static String tags(JSONObject item) {
    JSONObject hints=item.optJSONObject("hints");if(hints==null)return ""; List<String> values=new ArrayList<>();
    // Season first, matching the other clients: "Season 2 Ep 1", "Complete Season 1".
    int season=hints.optInt("season"),seasonEnd=hints.optInt("season_end"),episode=hints.optInt("episode"),episodeEnd=hints.optInt("episode_end"),count=hints.optInt("episode_count");
    java.util.Locale locale=L10n.locale();
    if(count>0)values.add(season>0?String.format(locale,L10n.t("Complete Season %d"),season):String.format(locale,L10n.t("Complete · %d episodes"),count));
    else if(episode>0) {
      if(season>0)values.add(episodeEnd>episode?String.format(locale,L10n.t("Season %d Ep %d–%d"),season,episode,episodeEnd):String.format(locale,L10n.t("Season %d Ep %d"),season,episode));
      else values.add(episodeEnd>episode?String.format(locale,L10n.t("Ep %d–%d"),episode,episodeEnd):String.format(locale,L10n.t("Ep %d"),episode));
    } else if(season>0)values.add(seasonEnd>season?String.format(locale,L10n.t("Seasons %d–%d"),season,seasonEnd):String.format(locale,L10n.t("Complete Season %d"),season));
    if(hints.optInt("year")>0)values.add(String.valueOf(hints.optInt("year")));
    for(String key:new String[]{"resolution","video_codec","release_source"})if(!hints.isNull(key))values.add(hints.optString(key));
    // Audio in plain words; the raw release tags only when the server could not classify them.
    String audio=audioLabel(hints);
    if(!audio.isEmpty())values.add(audio);
    else {JSONArray a=hints.optJSONArray("audio");if(a!=null)for(int i=0;i<a.length();i++)values.add(a.optString(i));}
    JSONArray languages=hints.optJSONArray("languages");if(languages!=null)for(int i=0;i<languages.length();i++)values.add(languages.optString(i));
    return String.join(" · ",values);
  }
  private class Results extends BaseAdapter {
    public int getCount(){return results.length();} public Object getItem(int p){return results.optJSONObject(p);} public long getItemId(int p){return p;}
    public View getView(int position, View old, ViewGroup parent) {
      JSONObject item=results.optJSONObject(position);
      LinearLayout row=Ui.row(TorrentSearchActivity.this);Ui.pad(row,16,6);row.setBackground(Ui.shape(Ui.PANEL,0,TorrentSearchActivity.this));
      LinearLayout content=Ui.column(TorrentSearchActivity.this);
      TextView title=Ui.title(TorrentSearchActivity.this,title(item),20);title.setMaxLines(2);Ui.add(content,title,-1,-2);
      String info=summary(item);if(!info.isEmpty())Ui.add(content,Ui.text(TorrentSearchActivity.this,info,16,Ui.ACCENT),-1,-2);
      Ui.add(content,Ui.text(TorrentSearchActivity.this,tags(item),13,Ui.MUTED),-1,-2);
      row.addView(content,new LinearLayout.LayoutParams(0,-2,1));
      return row;
    }
  }
  /** A Quark Drive share: its videos as a checklist, the library name to file them under, and Add. */
  private void cloudDetails(JSONObject item) {
    if(item==null)return;selectedItem=item;inDetail=true;int request=++generation;
    LinearLayout root=page();TextView name=Ui.title(this,title(item),24);name.setMaxLines(3);Ui.add(root,name,-1,-2);
    Ui.gap(root,8);Ui.add(root,Ui.text(this,summary(item),14,Ui.MUTED),-1,30);
    LinearLayout actions=Ui.row(this);Ui.add(actions,Ui.button(this,L10n.t("Back"),this::onBackPressed),100,44);Ui.gap(actions,12);
    EditText saveAs=new EditText(this);saveAs.setSingleLine(true);saveAs.setText(libraryName(title(item)));saveAs.setTextColor(Ui.TEXT);saveAs.setHint(L10n.t("Save as"));saveAs.setHintTextColor(Ui.MUTED);
    actions.addView(saveAs,new LinearLayout.LayoutParams(0,Ui.dp(this,46),1));Ui.gap(actions,12);
    Button add=Ui.button(this,L10n.t("Add"),()->{});Ui.primary(add);Ui.add(actions,add,190,44);Ui.add(root,actions,-1,50);
    TextView note=Ui.text(this,L10n.t("Checking the share’s files…"),14,Ui.MUTED);Ui.add(root,note,-1,-2);
    Ui.add(root,Ui.text(this,L10n.t("Videos are saved to your Quark Drive, downloaded into your library, then removed from the drive."),12,Ui.MUTED),-1,-2);
    ListView files=new ListView(this);files.setChoiceMode(ListView.CHOICE_MODE_MULTIPLE);
    root.addView(files,new LinearLayout.LayoutParams(-1,0,1));root.addView(ProfileUi.header(this, api),0);setContentView(root);add.requestFocus();
    JSONArray[] shareFiles={null};
    add.setOnClickListener(v->{
      try{
        JSONObject body=new JSONObject().put("url",shareUrl(item)).put("title",saveAs.getText().toString().trim());
        if(shareFiles[0]!=null){
          JSONArray chosen=new JSONArray();Set<Integer> seasons=new HashSet<>();boolean episodic=false;
          android.util.SparseBooleanArray checked=files.getCheckedItemPositions();
          for(int i=0;i<shareFiles[0].length();i++){JSONObject f=shareFiles[0].optJSONObject(i);if(!f.isNull("episode"))episodic=true;
            if(checked.get(i)){chosen.put(f.optString("path"));if(!f.isNull("season"))seasons.add(f.optInt("season"));}}
          if(chosen.length()==0){Toast.makeText(this,L10n.t("Choose videos to add"),Toast.LENGTH_SHORT).show();return;}
          body.put("files",chosen);if(seasons.size()==1)body.put("season",seasons.iterator().next());else if(seasons.isEmpty()&&seasonIn(item.optString("name"))>0)body.put("season",seasonIn(item.optString("name")));if(episodic)body.put("kind","tv");
        }
        if(body.optString("title").isEmpty())return;
        add.setEnabled(false);add.setText(L10n.t("Adding…"));
        io.execute(()->{try{api.request("POST","/v1/cloud/add",body);
          runOnUiThread(()->{if(isDestroyed())return;add.setText(L10n.t("Added to downloads"));Toast.makeText(this,L10n.t("Follow its progress in Downloading."),Toast.LENGTH_LONG).show();});
        }catch(Exception error){runOnUiThread(()->{if(isDestroyed())return;add.setEnabled(true);add.setText(L10n.t("Add"));
          new AlertDialog.Builder(this).setTitle(L10n.t("Couldn’t add torrent")).setMessage(MainActivity.message(error)).setPositiveButton(L10n.t("Close"),null).show();});}});
      }catch(JSONException ignored){}
    });
    String url=shareUrl(item);
    io.execute(()->{JSONObject share=null;try{
      JSONArray shares=api.request("POST","/v1/cloud/search",new JSONObject().put("query",searchedQuery.isEmpty()?title(item):searchedQuery)).optJSONArray("shares");
      for(int i=0;shares!=null&&i<shares.length();i++)if(url.equals(shares.optJSONObject(i).optString("url"))){share=shares.optJSONObject(i);break;}
    }catch(Exception ignored){}
      JSONObject found=share;
      runOnUiThread(()->{if(isDestroyed()||request!=generation)return;
        JSONArray entries=found==null?null:found.optJSONArray("files");
        if(entries==null||entries.length()==0){note.setText(L10n.t("This share’s file list is no longer available. Add brings in every video it holds."));return;}
        shareFiles[0]=entries;List<String> labels=new ArrayList<>();
        for(int i=0;i<entries.length();i++){JSONObject f=entries.optJSONObject(i);
          String episode=f.isNull("episode")?"":f.isNull("season")?String.format(L10n.locale(),L10n.t("Ep %d"),f.optInt("episode")):String.format(L10n.locale(),L10n.t("Season %d Ep %d"),f.optInt("season"),f.optInt("episode"));
          labels.add((episode.isEmpty()?"":episode+" · ")+f.optString("name")+(f.isNull("size_bytes")?"":"  ·  "+android.text.format.Formatter.formatFileSize(this,f.optLong("size_bytes"))));}
        note.setText(String.format(L10n.locale(),L10n.t("%d videos"),entries.length()));
        files.setAdapter(new ArrayAdapter<String>(this,android.R.layout.simple_list_item_multiple_choice,labels){public View getView(int p,View old,ViewGroup parent){TextView row=(TextView)super.getView(p,old,parent);row.setTextColor(Ui.TEXT);row.setTextSize(14);return row;}});
        for(int i=0;i<entries.length();i++)files.setItemChecked(i,true);
      });});
  }
  private void details(JSONObject item) {
    if(item==null)return;selectedItem=item;inDetail=true;int request=++generation;
    LinearLayout root=page();TextView name=Ui.title(this,item.optString("name"),24);name.setMaxLines(3);Ui.add(root,name,-1,-2);
    Ui.gap(root,12);Ui.add(root,Ui.text(this,summary(item),14,Ui.MUTED),-1,30);
    LinearLayout actions=Ui.row(this);Ui.add(actions,Ui.button(this,L10n.t("Back"),this::onBackPressed),100,44);Ui.gap(actions,12);
    CheckBox paused=new CheckBox(this);paused.setText(L10n.t("Add paused"));paused.setTextColor(Ui.TEXT);Ui.add(actions,paused,150,44);
    String hash=item.optString("info_hash").isEmpty()?item.optString("name"):item.optString("info_hash");Button add=Ui.button(this,added.contains(hash)?L10n.t("Added"):L10n.t("Add torrent"),()->{});
    add.setEnabled(!added.contains(hash));add.setOnClickListener(v -> add(item,paused.isChecked(),add));Ui.primary(add);Ui.add(actions,add,190,44);Ui.add(root,actions,-1,50);
    if(!tags(item).isEmpty())Ui.add(root,Ui.text(this,"Release tags · "+tags(item),14,Ui.MUTED),-1,40);
    Spinner attachments = new Spinner(this); attachments.setVisibility(View.GONE); Ui.add(root,attachments,-1,48);
    ListView files=new ListView(this);TextView description=Ui.text(this,"",15,Ui.TEXT);Ui.pad(description,0,14);files.addHeaderView(description,null,false);
    TextView note=Ui.text(this,L10n.t("Loading details…"),14,Ui.MUTED);files.addHeaderView(note,null,false);
    root.addView(files,new LinearLayout.LayoutParams(-1,0,1));root.addView(ProfileUi.header(this, api),0);setContentView(root);add.requestFocus();
    JSONArray sources=item.optJSONArray("sources");JSONObject chosen=null;
    if(sources!=null)for(int i=0;i<sources.length();i++)if(sources.optJSONObject(i).optString("provider").equals("tpb")){chosen=sources.optJSONObject(i);break;}
    if(chosen==null && sources!=null)for(int i=0;i<sources.length();i++)if(sources.optJSONObject(i).optString("provider").equals("nyaa")){chosen=sources.optJSONObject(i);break;}
    if(chosen==null && sources!=null)chosen=sources.optJSONObject(0);
    if(chosen==null){note.setText(L10n.t("Source details unavailable."));return;}
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
              add.setEnabled(!added.contains(resolved.optString("info_hash")));add.setText(added.contains(resolved.optString("info_hash"))?L10n.t("Added"):L10n.t("Add torrent"));
              add.setOnClickListener(view->add(resolved,paused.isChecked(),add));
              }
              JSONArray fs=releases.optJSONObject(position).optJSONArray("files");List<String> labels=new ArrayList<>();
              if(fs!=null)for(int j=0;j<fs.length();j++){JSONObject f=fs.optJSONObject(j);labels.add(f.optString("name")+"\n"+android.text.format.Formatter.formatFileSize(TorrentSearchActivity.this,f.optLong("size_bytes")));}
              note.setText(labels.size()+" files");files.setAdapter(new ArrayAdapter<String>(TorrentSearchActivity.this,android.R.layout.simple_list_item_1,labels){public View getView(int p,View old,ViewGroup parent){TextView row=(TextView)super.getView(p,old,parent);row.setTextColor(Ui.TEXT);row.setTextSize(14);return row;}});
            }
          });
        }
        note.setText(entries==null?L10n.t("File list unavailable"):names.size()+" files");files.setAdapter(new ArrayAdapter<String>(this,android.R.layout.simple_list_item_1,names){public View getView(int p,View old,ViewGroup parent){TextView row=(TextView)super.getView(p,old,parent);row.setTextColor(Ui.TEXT);row.setTextSize(14);return row;}});});
    }catch(Exception error){runOnUiThread(()->{if(!isDestroyed()&&request==generation)note.setText(item.optString("magnet").isEmpty()?L10n.t("Couldn’t load this release. Please reopen it to retry."):L10n.t("Couldn’t load source details. You can still add this torrent."));});}});
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
    button.setTag(true);button.setEnabled(false);button.setText(L10n.t("Adding…"));
    io.execute(()->{try{
      JSONObject payload=new JSONObject().put("paused",paused);
      if(!item.optString("magnet").isEmpty())payload.put("magnet",item.getString("magnet"));
      else throw new Exception(L10n.t("This release has no downloadable torrent."));
      JSONObject response=api.request("POST","/v1/torrents",payload);
      runOnUiThread(()->{if(isDestroyed())return;added.add(item.optString("info_hash").isEmpty()?item.optString("name"):item.optString("info_hash"));button.setText(L10n.t("Added"));Toast.makeText(this,response.has("torrent_duplicate")?L10n.t("Already in your torrents"):L10n.t("Torrent added"),Toast.LENGTH_LONG).show();});
    }catch(Exception error){runOnUiThread(()->{if(!isDestroyed()){button.setTag(false);button.setEnabled(true);button.setText(L10n.t("Add torrent"));new AlertDialog.Builder(this).setTitle(L10n.t("Couldn’t add torrent")).setMessage(MainActivity.message(error)).setPositiveButton(L10n.t("Close"),null).show();}});}});
  }
  public void onBackPressed(){if(inDetail){inDetail=false;generation++;setContentView(resultsRoot);list.requestFocus();}else super.onBackPressed();}
  protected void onDestroy(){generation++;io.shutdownNow();super.onDestroy();}
}
