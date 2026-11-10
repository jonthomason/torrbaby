package com.torrbaby.tv;

import android.app.Instrumentation;
import android.net.nsd.*;
import android.os.*;
import java.util.Map;

/** Device smoke test against Android's real DNS-SD stack, without a production server. */
public final class DiscoveryTestRunner extends Instrumentation {
  private NsdManager manager;
  private ServerDiscovery discovery;
  private boolean done, registered;
  private final String serviceName = "Torrbaby discovery test " + System.currentTimeMillis();
  private final Handler main = new Handler(Looper.getMainLooper());
  private final NsdManager.RegistrationListener registration = new NsdManager.RegistrationListener() {
    public void onServiceRegistered(NsdServiceInfo info) { registered = true; }
    public void onRegistrationFailed(NsdServiceInfo info, int error) { main.post(() -> complete("Registration failed: " + error)); }
    public void onServiceUnregistered(NsdServiceInfo info) {}
    public void onUnregistrationFailed(NsdServiceInfo info, int error) {}
  };
  private boolean catalogCheck, cacheCheck, performanceCheck, castCheck, languageCheck, headerCheck, shelvesCheck;
  public void onCreate(Bundle args) {
    super.onCreate(args); catalogCheck = args != null && args.getBoolean("catalog", false);
    if (args != null && "true".equals(args.getString("catalog"))) catalogCheck = true;
    cacheCheck = args != null && "true".equals(args.getString("cache"));
    performanceCheck = args != null && "true".equals(args.getString("performance"));
    castCheck = args != null && "true".equals(args.getString("cast"));
    languageCheck = args != null && "true".equals(args.getString("language"));
    headerCheck = args != null && "true".equals(args.getString("header"));
    shelvesCheck = args != null && "true".equals(args.getString("shelves"));
    start();
  }
  public void onStart() {
    if(shelvesCheck) { shelvesLayout(); return; }
    if(headerCheck) { headerLayout(); return; }
    if (languageCheck) {
      android.content.Context base = getTargetContext();
      android.content.SharedPreferences preferences = base.getSharedPreferences("interface",0);
      String previous = preferences.getString("language",null);
      Bundle result = new Bundle();
      int code = -1;
      try {
        preferences.edit().putString("language","zh").commit(); L10n.wrap(base);
        if (!"谁在观看？".equals(L10n.t("Who's watching?"))) throw new Exception("Chinese profile label unavailable");
        if (!"Jon".equals(L10n.t("Jon"))) throw new Exception("Profile name was translated");
        String[] wireValues = {"Home", "Movies"};
        if (!"首页".equals(L10n.labels(wireValues)[0]) || !"Home".equals(wireValues[0])) throw new Exception("UI translation changed routing values");
        preferences.edit().putString("language","en").commit(); L10n.wrap(base);
        if (!"Who's watching?".equals(L10n.t("Who's watching?"))) throw new Exception("English override unavailable");
        result.putString("stream","PASS: native language resources switch in both directions; names and routing values stay unchanged\n");
      } catch (Exception e) { code=0; result.putString("stream","FAIL: "+e+"\n"); }
      finally {
        if(previous==null)preferences.edit().remove("language").commit();else preferences.edit().putString("language",previous).commit();
        L10n.wrap(base);
      }
      finish(code,result);return;
    }
    if (castCheck) { castStress(); return; }
    if(performanceCheck) { performance(); return; }
    if (cacheCheck) {
      Bundle result = new Bundle();
      try {
        android.content.Context cacheContext = new android.content.ContextWrapper(getTargetContext()) {
          @Override public java.io.File getCacheDir() {
            java.io.File root = new java.io.File(super.getCacheDir(), "sqlite-cache-check");
            root.mkdirs(); return root;
          }
        };
        Api jon = new Api("http://cache-test", "fixture", "device", "jon", "Jon");
        Api alex = new Api("http://cache-test", "fixture", "device", "alex", "Alex");
        byte[] large = new byte[3 * 1024 * 1024];
        java.util.Arrays.fill(large, (byte) 42);
        BrowseCache first = new BrowseCache(cacheContext, jon);
        first.write("library", large);
        if (!java.util.Arrays.equals(large, new BrowseCache(cacheContext, jon).read("library", 0))) throw new Exception("Large snapshot/reopen failed");
        if (new BrowseCache(cacheContext, alex).read("library", 0) != null) throw new Exception("Profile leaked");
        byte[] small = "empty catalog".getBytes(java.nio.charset.StandardCharsets.UTF_8);
        first.write("library", small);
        if (!java.util.Arrays.equals(small, first.read("library",0))) throw new Exception("Snapshot replacement retained old chunks");
        first.write("art/fixture/poster/rev1", small);
        if (!java.util.Arrays.equals(small,new BrowseCache(cacheContext,alex).read("art/fixture/poster/rev1",0))) throw new Exception("Artwork not shared between profiles");
        result.putString("stream", "PASS: SQLite snapshots survive reopen, large catalogs, replacement, and profile isolation; artwork shares across profiles\n");
        finish(-1,result);
      } catch (Exception e) { result.putString("stream", "FAIL: " + e + "\n"); finish(0,result); }
      return;
    }
    if (catalogCheck) {
      Bundle result = new Bundle();
      try {
        String raw = "{\"titles\":[{\"id\":\"test\",\"assets\":[{\"id\":\"episode\",\"season\":1,\"position\":12.5,\"complete\":true,\"media_info\":{\"state\":\"ready\",\"probe\":{\"ignored\":[1,null,{}]},\"audio_tracks\":[{\"language\":\"eng\",\"codec\":\"aac\"}],\"subtitle_tracks\":[],\"prepared\":[{\"language\":\"zh-Hans\",\"state\":\"ready\",\"generated\":true}]}}]}]}";
        org.json.JSONObject compact = CatalogJson.parse(raw);
        Library library = Library.parse(compact);
        Library.Asset asset = library.titles.get(0).assets.get(0);
        if (!asset.complete || asset.position != 12.5 || asset.season != 1
            || asset.mediaInfo.has("probe")
            || !asset.mediaInfo.getJSONArray("audio_tracks").getJSONObject(0).getString("codec").equals("aac")
            || !MediaLanguages.lines(asset.mediaInfo).get(1).contains("Chinese (Simplified)"))
          throw new Exception("Catalog projection lost browsing information");
        Library.parse(CatalogJson.parse(compact.toString()));
        result.putString("stream", "PASS: compact catalog preserves browsing, progress and generated languages\n");
        finish(-1, result);
      } catch (Exception error) { result.putString("stream", "FAIL: " + error); finish(0, result); }
      return;
    }
    main.post(() -> {
      manager = (NsdManager)getTargetContext().getSystemService(android.content.Context.NSD_SERVICE);
      discovery = new ServerDiscovery(getTargetContext(), new ServerDiscovery.Listener() {
        public void failed() { complete("Discovery failed"); }
        public void changed(Map<String, String> servers) {
          for (Map.Entry<String, String> entry : servers.entrySet())
            if (entry.getKey().startsWith(serviceName)) {
              try {
                String address = Api.validateBase(entry.getValue());
                if (!address.endsWith(":18765")) throw new Exception("Wrong port");
                complete(null);
              } catch (Exception error) { complete(error.toString()); }
            }
        }
      });
      discovery.start();
      NsdServiceInfo info = new NsdServiceInfo();
      info.setServiceName(serviceName); info.setServiceType("_torrbaby._tcp."); info.setPort(18765);
      manager.registerService(info, NsdManager.PROTOCOL_DNS_SD, registration);
      main.postDelayed(() -> complete("Timed out discovering registered service"), 20000);
    });
  }
  private android.view.View find(android.view.View view,String text) {
    if(view instanceof android.widget.TextView && text.contentEquals(((android.widget.TextView)view).getText()))return view;
    if(view instanceof android.view.ViewGroup) {
      android.view.ViewGroup group=(android.view.ViewGroup)view;
      for(int i=0;i<group.getChildCount();i++) { android.view.View found=find(group.getChildAt(i),text);if(found!=null)return found; }
    }
    return null;
  }
  private void key(android.app.Activity activity,int code,int repeat) {
    long now=SystemClock.uptimeMillis();
    runOnMainSync(() -> activity.dispatchKeyEvent(new android.view.KeyEvent(now,now,android.view.KeyEvent.ACTION_DOWN,code,repeat)));
    waitForIdleSync(); SystemClock.sleep(30);
    runOnMainSync(() -> activity.dispatchKeyEvent(new android.view.KeyEvent(now,SystemClock.uptimeMillis(),android.view.KeyEvent.ACTION_UP,code,0)));
    waitForIdleSync(); SystemClock.sleep(100);
  }
  private void navigationKey(int code) { sendKeyDownUpSync(code);waitForIdleSync();SystemClock.sleep(100); }
  private void shelvesLayout() {
    Bundle result=new Bundle();
    try {
      android.app.Activity activity=startActivitySync(new android.content.Intent(getTargetContext(),MainActivity.class).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK));
      org.json.JSONArray titles=new org.json.JSONArray();
      for(int i=0;i<40;i++)titles.put(new org.json.JSONObject().put("id","shelf-"+i).put("title",String.format("Film %02d",i)).put("kind","movie").put("added_at",1000-i)
          .put("assets",new org.json.JSONArray().put(new org.json.JSONObject().put("id","asset-"+i).put("complete",true))));
      Library model=Library.parse(new org.json.JSONObject().put("titles",titles));
      java.lang.reflect.Method render=MainActivity.class.getDeclaredMethod("render");render.setAccessible(true);
      for(String language:new String[]{"en","zh"}) {
        final Exception[] failure={null};
        runOnMainSync(()->{try{
          activity.getSharedPreferences("interface",0).edit().putString("language",language).commit();L10n.wrap(activity);
          for(String name:new String[]{"library","api","detailId","section"}) {
            java.lang.reflect.Field f=MainActivity.class.getDeclaredField(name);f.setAccessible(true);
            f.set(activity,name.equals("library")?model:name.equals("api")?new Api("http://127.0.0.1:9","fixture","fixture","fixture","Jon"):name.equals("section")?"Home":"");
          }
          render.invoke(activity);
        }catch(Exception e){failure[0]=e;}});
        waitForIdleSync();SystemClock.sleep(300);if(failure[0]!=null)throw failure[0];
        android.widget.HorizontalScrollView shelf=activity.getWindow().getDecorView().findViewWithTag("shelf:Recently added");
        android.widget.LinearLayout row=(android.widget.LinearLayout)shelf.getChildAt(0);
        runOnMainSync(()->row.getChildAt(0).requestFocus());waitForIdleSync();
        for(int i=0;i<39;i++)navigationKey(android.view.KeyEvent.KEYCODE_DPAD_RIGHT);
        if(!"title:shelf-39".equals(activity.getCurrentFocus().getTag()))throw new Exception("Could not navigate past old shelf limit: "+activity.getCurrentFocus().getTag());
        for(int i=0;i<39;i++)navigationKey(android.view.KeyEvent.KEYCODE_DPAD_LEFT);
        if(!"title:shelf-0".equals(activity.getCurrentFocus().getTag()))throw new Exception("Could not return to first card");
        if(find(activity.getWindow().getDecorView(),L10n.t("See All  ›"))!=null)throw new Exception("See All still present");
        android.graphics.Bitmap shot=getUiAutomation().takeScreenshot();
        try(java.io.FileOutputStream out=new java.io.FileOutputStream(new java.io.File(getTargetContext().getExternalFilesDir(null),"shelves-"+language+".png"))){shot.compress(android.graphics.Bitmap.CompressFormat.PNG,100,out);}
      }
      result.putString("stream","PASS: EN/ZH shelves traverse all 40 titles and return, without See All.\n");finish(-1,result);
    }catch(Exception e){result.putString("stream","FAIL: "+e+"\n");finish(0,result);}
  }

  private void headerLayout() {
    Bundle result = new Bundle();
    try {
      android.app.Activity activity = startActivitySync(new android.content.Intent(getTargetContext(), MainActivity.class).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK));
      org.json.JSONObject asset = new org.json.JSONObject().put("id","episode").put("name","Episode").put("complete",true).put("position",120).put("duration",3000).put("season",4).put("episode",1);
      org.json.JSONObject title = new org.json.JSONObject().put("id","header-fixture").put("title","Send Help").put("kind","tv")
          .put("cast",new org.json.JSONArray().put(new org.json.JSONObject().put("name","Actor")))
          .put("assets",new org.json.JSONArray().put(asset));
      Library model = Library.parse(new org.json.JSONObject().put("titles",new org.json.JSONArray().put(title)));
      java.lang.reflect.Method render = MainActivity.class.getDeclaredMethod("render");render.setAccessible(true);
      java.lang.reflect.Field navField=MainActivity.class.getDeclaredField("nav");navField.setAccessible(true);
      for(String language:new String[]{"en","zh"}) {
        final Exception[] failure={null};
        runOnMainSync(()->{try{
          activity.getSharedPreferences("interface",0).edit().putString("language",language).commit();L10n.wrap(activity);
          for(String name:new String[]{"library","api","detailId"}) {
            java.lang.reflect.Field field=MainActivity.class.getDeclaredField(name);field.setAccessible(true);
            field.set(activity,name.equals("library")?model:name.equals("detailId")?"header-fixture":new Api("http://127.0.0.1:9","fixture","fixture","fixture","Jon"));
          }
          render.invoke(activity);
        }catch(Exception e){failure[0]=e;}});
        waitForIdleSync();if(failure[0]!=null)throw failure[0];
        android.view.View nav=(android.view.View)navField.get(activity);
        java.util.ArrayList<android.view.View> focus=new java.util.ArrayList<>();
        java.lang.reflect.Method collect=MainActivity.class.getDeclaredMethod("collectNavigationFocus",android.view.View.class,java.util.List.class);collect.setAccessible(true);collect.invoke(activity,nav,focus);
        if(focus.size()!=2)throw new Exception("Expected only two language buttons on detail: "+focus.size());
        runOnMainSync(()->focus.get(0).requestFocus());waitForIdleSync();
        for(int i=0;i<focus.size();i++){
          if(activity.getCurrentFocus()!=focus.get(i))throw new Exception("Right focus mismatch at "+i+" expected="+focus.get(i)+" actual="+activity.getCurrentFocus());
          android.graphics.Rect bounds=new android.graphics.Rect();focus.get(i).getGlobalVisibleRect(bounds);
          if(bounds.width()<focus.get(i).getWidth()-2 || bounds.height()<focus.get(i).getHeight()-2)throw new Exception("Header control clipped: "+i);
          if(i+1<focus.size())navigationKey(22);
        }
        for(int i=focus.size()-1;i>0;i--){navigationKey(21);if(activity.getCurrentFocus()!=focus.get(i-1))throw new Exception("Left focus mismatch");}
        navigationKey(20);if(activity.getCurrentFocus()==focus.get(0))throw new Exception("Cannot enter detail");
        navigationKey(19);if(!focus.contains(activity.getCurrentFocus()))throw new Exception("Cannot return to navigation");
        android.graphics.Bitmap shot=getUiAutomation().takeScreenshot();
        try(java.io.FileOutputStream out=new java.io.FileOutputStream(new java.io.File(getTargetContext().getExternalFilesDir(null),"header-"+language+".png"))){shot.compress(android.graphics.Bitmap.CompressFormat.PNG,100,out);}
        android.view.View more=activity.getWindow().getDecorView().findViewWithTag("detail-more");
        if(more==null)throw new Exception("Missing More action");
        if(find(activity.getWindow().getDecorView(),L10n.t("Manage Files"))!=null)throw new Exception("Manage Files still occupies detail row");
        runOnMainSync(()->more.requestFocus());navigationKey(23);
        android.view.accessibility.AccessibilityNodeInfo menu=getUiAutomation().getRootInActiveWindow();
        for(String label:new String[]{"Mark as Watched","Manage Files","Remove from Continue Watching"})
          if(menu==null||menu.findAccessibilityNodeInfosByText(L10n.t(label)).isEmpty())throw new Exception("Missing menu action: "+label);
        android.graphics.Bitmap menuShot=getUiAutomation().takeScreenshot();
        try(java.io.FileOutputStream out=new java.io.FileOutputStream(new java.io.File(getTargetContext().getExternalFilesDir(null),"menu-"+language+".png"))){menuShot.compress(android.graphics.Bitmap.CompressFormat.PNG,100,out);}
        navigationKey(4);
        if(activity.getCurrentFocus()!=more)throw new Exception("Menu dismissal lost focus");
        runOnMainSync(()->{try{
          java.lang.reflect.Field detail=MainActivity.class.getDeclaredField("detailId");detail.setAccessible(true);detail.set(activity,"");
          render.invoke(activity);
          android.view.View first=activity.getWindow().getDecorView().findViewWithTag("title:header-fixture");
          if(first!=null)first.requestFocus();
        }catch(Exception e){failure[0]=e;}});
        waitForIdleSync();SystemClock.sleep(200);if(failure[0]!=null)throw failure[0];
        if(find(activity.getWindow().getDecorView(),"S4E1")==null)throw new Exception("Missing Continue Watching episode line");
        if(find(activity.getWindow().getDecorView(),String.format(L10n.locale(),L10n.t("%d min remaining"),48))==null)throw new Exception("Missing separate minutes remaining line");
        android.graphics.Bitmap captionShot=getUiAutomation().takeScreenshot();
        try(java.io.FileOutputStream out=new java.io.FileOutputStream(new java.io.File(getTargetContext().getExternalFilesDir(null),"captions-"+language+".png"))){captionShot.compress(android.graphics.Bitmap.CompressFormat.PNG,100,out);}

      }
      result.putString("stream","PASS: EN/ZH header fits, detail language controls reachable left/right, detail down/up navigation and More menu contents/focus work. Screenshots saved.\n");finish(-1,result);
    }catch(Exception e){result.putString("stream","FAIL: "+e+"\n");finish(0,result);}
  }

  private void castStress() {
    Bundle result = new Bundle();
    try {
      android.content.Intent intent = new android.content.Intent(getTargetContext(), MainActivity.class).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK);
      android.app.Activity activity = startActivitySync(intent);
      org.json.JSONArray cast = new org.json.JSONArray();
      for (int i = 0; i < 10000; i++) cast.put(new org.json.JSONObject().put("name", "Actor " + i).put("character", "Character " + i).put("total_episode_count", 90));
      org.json.JSONObject title = new org.json.JSONObject().put("id", "stress").put("title", "Large cast stress check").put("kind", "tv").put("cast", cast)
          .put("assets", new org.json.JSONArray().put(new org.json.JSONObject().put("id", "episode").put("season",4).put("episode",1).put("complete",true)));
      Library library = Library.parse(new org.json.JSONObject().put("titles",new org.json.JSONArray().put(title)));
      final Exception[] failure = {null};
      runOnMainSync(() -> {
        try {
          for (String name : new String[]{"library","detailId","season","showingCast"}) {
            java.lang.reflect.Field f = MainActivity.class.getDeclaredField(name); f.setAccessible(true);
            f.set(activity, name.equals("library") ? library : name.equals("detailId") ? "stress" : name.equals("season") ? 4 : true);
          }
          java.lang.reflect.Field api = MainActivity.class.getDeclaredField("api"); api.setAccessible(true);
          api.set(activity, new Api("http://127.0.0.1:9", "fixture", "fixture", "fixture", "Fixture"));
          java.lang.reflect.Method render = MainActivity.class.getDeclaredMethod("render"); render.setAccessible(true); render.invoke(activity);
        } catch(Exception e) { failure[0] = e; }
      });
      waitForIdleSync(); if(failure[0]!=null)throw failure[0];
      java.lang.reflect.Field field = MainActivity.class.getDeclaredField("content"); field.setAccessible(true);
      android.widget.LinearLayout content = (android.widget.LinearLayout)field.get(activity);
      android.widget.ListView list = (android.widget.ListView)content.getChildAt(content.getChildCount()-1);
      if(list.getCount()!=150)throw new Exception("Cast was not capped");
      for(int i=0;i<35;i++)key(activity,20,0);
      runOnMainSync(() -> list.setSelection(149)); waitForIdleSync();
      if(list.getChildCount()>20)throw new Exception("Cast rows not virtualized: " + list.getChildCount());
      runOnMainSync(activity::onBackPressed);waitForIdleSync();
      java.lang.reflect.Field season = MainActivity.class.getDeclaredField("season");season.setAccessible(true);
      if(season.getInt(activity)!=4 || find(activity.getWindow().getDecorView(), "Season 4")==null)throw new Exception("Back lost selected season: season=" + season.getInt(activity) + " label=" + find(activity.getWindow().getDecorView(), "Season 4"));
      result.putString("stream", "PASS: 10000 cast entries capped to 150; D-pad scroll and last row rendered with recycled views; Back retained season 4\n");
      finish(-1,result);
    }catch(Exception e){result.putString("stream", "FAIL: " + e + "\n");finish(0,result);}
  }

  private void performance() {
    Bundle result=new Bundle();
    try {
      android.content.Intent intent=new android.content.Intent(getTargetContext(),MainActivity.class).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK);
      android.app.Activity activity=startActivitySync(intent);
      final android.view.View[] movies={null};
      for(int i=0;i<100 && movies[0]==null;i++) {
        runOnMainSync(() -> movies[0]=find(activity.getWindow().getDecorView(),"Movies"));
        if(movies[0]==null)SystemClock.sleep(100);
      }
      if(movies[0]==null)throw new Exception("Library not ready");
      runOnMainSync(() -> movies[0].performClick()); waitForIdleSync(); SystemClock.sleep(200);
      java.lang.reflect.Field modelField=MainActivity.class.getDeclaredField("library"),rootField=MainActivity.class.getDeclaredField("root"),apiField=MainActivity.class.getDeclaredField("api");
      modelField.setAccessible(true);rootField.setAccessible(true);apiField.setAccessible(true);
      Library original=(Library)modelField.get(activity);
      org.json.JSONObject changed=new org.json.JSONObject(((Api)apiField.get(activity)).libraryRaw);
      org.json.JSONArray titles=changed.getJSONArray("titles");
      for(int i=0;i<titles.length();i++) {
        org.json.JSONArray assets=titles.getJSONObject(i).optJSONArray("assets");
        if(assets!=null && assets.length()>0)assets.getJSONObject(0).put("position",123);
      }
      java.lang.reflect.Method update=MainActivity.class.getDeclaredMethod("updateBrowse",android.widget.LinearLayout.class);update.setAccessible(true);
      final android.view.View[] retainedFocus={null}; final Exception[] failure={null};
      runOnMainSync(() -> {
        retainedFocus[0]=activity.getCurrentFocus();
        try { modelField.set(activity,Library.parse(changed)); update.invoke(activity,rootField.get(activity)); }
        catch(Exception e){failure[0]=e;}
      });waitForIdleSync();
      if(failure[0]!=null)throw failure[0];
      if(activity.getCurrentFocus()!=retainedFocus[0])throw new Exception("Incremental progress update stole focus");
      runOnMainSync(() -> {try {modelField.set(activity,original);update.invoke(activity,rootField.get(activity));}catch(Exception e){failure[0]=e;}});
      if(failure[0]!=null)throw failure[0];
      int before=PosterCard.created;
      for(int i=0;i<16;i++) {
        key(activity,i<8?20:19,i%8);
        final String[] issue={null};
        runOnMainSync(() -> {
          android.view.View focus=activity.getCurrentFocus();
          if(!(focus instanceof android.widget.GridView))return;
          android.view.View selected=((android.widget.GridView)focus).getSelectedView();
          android.graphics.Rect visible=new android.graphics.Rect();
          if(selected==null || !selected.getGlobalVisibleRect(visible) || visible.height()<selected.getHeight()-2)issue[0]="Focused row clipped: visible="+visible+" height="+(selected==null?-1:selected.getHeight())+" grid="+focus.getHeight();
        });
        if(issue[0]!=null)throw new Exception(issue[0]+" at step "+i);
      }
      final Object[] selected={null};
      runOnMainSync(() -> {android.view.View view=activity.getCurrentFocus();selected[0]=((android.widget.GridView)view).getSelectedView();});
      key(activity,23,0);
      long start=SystemClock.elapsedRealtime();
      runOnMainSync(activity::onBackPressed);waitForIdleSync();
      long back=SystemClock.elapsedRealtime()-start;
      final boolean[] restored={false};
      runOnMainSync(() -> {android.view.View view=activity.getCurrentFocus();restored[0]=view instanceof android.widget.GridView && ((android.widget.GridView)view).getSelectedView()==selected[0];});
      if(!restored[0])throw new Exception("Back did not retain exact selected card; focus="+activity.getCurrentFocus());
      int created=PosterCard.created-before;
      if(created>24)throw new Exception("Poster recycling exceeded two additional visible rows: "+created);
      result.putString("stream","PASS: incremental update retained focus; 16 D-pad row movements fully visible; recycled cards added="+created+"; exact Back selection restored in "+back+" ms\n");finish(-1,result);
    }catch(Exception e){result.putString("stream","FAIL: "+e+"\n");finish(0,result);}
  }

  private void complete(String error) {
    if (done) return;
    done = true; main.removeCallbacksAndMessages(null);
    if (discovery != null) discovery.close();
    if (registered) manager.unregisterService(registration);
    Bundle result = new Bundle();
    result.putString("stream", error == null ? "PASS: native DNS-SD discovery resolves Torrbaby service and port\n" : "FAIL: " + error + "\n");
    finish(error == null ? -1 : 0, result);
  }
}
