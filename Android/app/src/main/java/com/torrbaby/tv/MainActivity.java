package com.torrbaby.tv;

import android.app.*;
import android.content.*;
import android.graphics.drawable.GradientDrawable;
import android.os.*;
import android.text.InputType;
import android.view.*;
import android.widget.*;
import java.util.*;
import java.util.concurrent.*;
import org.json.*;

public class MainActivity extends Activity {
  @Override protected void attachBaseContext(android.content.Context base) { super.attachBaseContext(L10n.wrap(base)); }

  private final ExecutorService io = Executors.newSingleThreadExecutor();
  private Api api;
  private ServerDiscovery discovery;
  private final Handler discoveryHandler = new Handler(Looper.getMainLooper());
  private int connectionGeneration;
  private boolean discoveringScreen, connectingLan;
  private Art art;
  private SecureStore store;
  private Library library;
  private LinearLayout root, content, nav;
  private String section = "Home", query = "", genre = "", detailId = "", restoreId = "";
  private int sort = 0, season = -1;
  private boolean showingCast;
  private boolean loading = false, played = false, selectingProfile, switchingProfile;
  private TextView status;
  private String collectionName = "", restoreShelf = "";
  private ArrayList<String> collectionIds = new ArrayList<>();
  private JSONArray torrents;
  private JSONArray libraryOperations;
  /** Processing errors and skipped-stage warnings for Needs attention; null until fetched. */
  private JSONArray attention;
  /** Quark Drive downloads, and free space on the media drives, for Downloading. */
  private JSONArray cloudJobs;
  /** Shows the server watches for new episodes, as GET /v1/watch reports them. */
  private JSONObject watchList;
  private JSONObject storage;
  private String libraryOperationsError = "";
  private final Map<String, Long> removedTorrents = new HashMap<>();
  private String torrentCategory = "Downloading";
  private boolean torrentsLoading, torrentStale;
  private long torrentUpdated;
  private TextView torrentStatus;
  private TorrentActions torrentActions;
  private boolean inLibrary, refreshingLibrary;
  private String librarySnapshot = "";
  // Keep one browse hierarchy, including its scroll and focus, while viewing a title.
  /** The season selector's value for a show's extras, which follow its last season. */
  private static final int EXTRAS = Integer.MAX_VALUE;
  private View savedBrowseView, savedBrowseFocus;
  // Top bar labels match tvOS (24pt at 1920) rather than the larger content buttons.
  private static final float NAV_TEXT = 12f;
  // Pending page arrival: 1 forward, -1 back, 2 sideways between sections.
  private int transition;
  private LinearLayout savedBrowseRoot, savedBrowseContent, savedBrowseNav;
  private Api savedBrowseApi;
  private String savedBrowseSnapshot;
  private int refreshCount;
  private final Handler refreshHandler = new Handler(Looper.getMainLooper());
  private final Runnable refreshTick =
      new Runnable() {
        public void run() {
          if (api != null && inLibrary && getWindow().getDecorView().hasWindowFocus()) {
            if (section.equals("Torrents") && detailId.isEmpty()) fetchTorrents();
            else if (++refreshCount % 2 == 0 && !(getCurrentFocus() instanceof EditText))
              refreshLibrary();
          }
          refreshHandler.postDelayed(this, 5000);
        }
      };

  @Override
  public void onCreate(Bundle state) {
    super.onCreate(state);
    StartupTrace.start(this);
    store = new SecureStore(this);
    if (state != null) {
      section = state.getString("section", "Home");
      detailId = state.getString("detail", "");
      showingCast = state.getBoolean("cast", false);
      query = state.getString("query", "");
      genre = state.getString("genre", "");
      sort = state.getInt("sort");
      season = state.getInt("season", -1);
      collectionName = state.getString("collection", "");
      ArrayList<String> ids = state.getStringArrayList("collection_ids");
      if (ids != null) collectionIds = ids;
    }
    try {
      Api test = Bootstrap.from(getIntent());
      if (test != null) store.save(test);
      StartupTrace.mark("Credentials read started");
      api = store.load();
      StartupTrace.mark(api == null ? "No saved connection" : "Credentials ready");
      StartupTrace.connected(api);
    } catch (Exception e) {
      discover("The saved connection could not be opened.");
      return;
    }
    if (api == null) discover("");
    else {
      art = new Art(this, api);
      if (getIntent().getBooleanExtra("switch_profile", false)) { getIntent().removeExtra("switch_profile"); chooseProfile(true); }
      else if (api.profile.isEmpty()) chooseProfile(false);
      else load();
    }
  }

  @Override
  protected void onSaveInstanceState(Bundle s) {
    super.onSaveInstanceState(s);
    s.putString("section", section);
    s.putString("detail", detailId);
    s.putBoolean("cast", showingCast);
    s.putString("query", query);
    s.putString("genre", genre);
    s.putInt("sort", sort);
    s.putInt("season", season);
    s.putString("collection", collectionName);
    s.putStringArrayList("collection_ids", collectionIds);
  }

  @Override protected void onNewIntent(Intent intent) {
    super.onNewIntent(intent);setIntent(intent);
    if(intent.getBooleanExtra("switch_profile",false) && api!=null){intent.removeExtra("switch_profile");chooseProfile(true);}
  }

  @Override
  protected void onResume() {
    super.onResume();
    if (StartupTrace.foreground()) {
      View foregroundView = getWindow().getDecorView();
      foregroundView.getViewTreeObserver().addOnPreDrawListener(new android.view.ViewTreeObserver.OnPreDrawListener() {
        public boolean onPreDraw() {
          foregroundView.getViewTreeObserver().removeOnPreDrawListener(this);
          StartupTrace.mark("First library frame ready"); return true;
        }
      });
      foregroundView.postInvalidateOnAnimation();
    }
    refreshProfileAppearance();
    if (!discoveringScreen && api != null && library != null) {
      if (played) { played = false; load(); }
      else refreshLibrary();
    }
  }

  private void refreshProfileAppearance() {
    final Api requested = api;
    if (requested == null || requested.profile.isEmpty() || selectingProfile) return;
    io.execute(() -> {
      try {
        JSONArray people = requested.request("GET", "/v1/profiles", null).getJSONArray("profiles");
        for (int i = 0; i < people.length(); i++) {
          JSONObject person = people.getJSONObject(i);
          if (!requested.profile.equals(person.optString("id"))) continue;
          runOnUiThread(() -> {
            if (isDestroyed() || api != requested) return;
            requested.profileAppearance = person;
            try { store.save(requested); } catch (Exception ignored) { }
            ProfileUi.refreshBadges(this, getWindow().getDecorView(), requested);
          });
          break;
        }
      } catch (Exception ignored) { /* Keep the cached avatar when the server is unavailable. */ }
    });
  }

  @Override
  protected void onDestroy() {
    stopDiscovery();
    connectionGeneration++;
    super.onDestroy();
    io.shutdownNow();
    if (art != null) art.close();
  }

  private void base() {
    inLibrary = false;
    stopDiscovery();
    discoveringScreen = false;
    connectingLan = false;
    connectionGeneration++;
    root = Ui.column(this);
    root.setBackgroundColor(Ui.BG);
    Ui.pad(root, 40, 20);
    Ui.add(root, ProfileUi.header(this, api, () -> chooseProfile(true)), -1, 52);
    setContentView(root);
  }

  private void shell() {
    base();
    root.setPadding(0, 0, 0, 0);
    root.removeAllViews(); // The library uses one shared navigation row.
    nav = Ui.row(this);
    nav.setGravity(Gravity.CENTER_VERTICAL);
    Ui.pad(nav, 10, 2);
    LinearLayout profile = ProfileUi.badge(this, api, () -> chooseProfile(true));
    profile.getChildAt(1).setVisibility(View.GONE);
    profile.getChildAt(2).setVisibility(View.GONE);
    Ui.add(nav, profile, 56, 44);
    Ui.gap(nav, 8);
    ImageView brand = new ImageView(this);
    brand.setImageResource(R.drawable.torrbaby_mark);
    brand.setContentDescription("Torrbaby");
    brand.setFocusable(false);
    Ui.add(nav, brand, 36, 36);
    Ui.gap(nav, 12);
    LinearLayout menus=Ui.row(this);menus.setGravity(Gravity.CENTER);
    nav.addView(menus,new LinearLayout.LayoutParams(0,-2,1));
    for (String label : new String[] {"Home", "Movies", "TV Shows", "Torrents", "Search"}) {
      Button b =
          Ui.button(
              this,
              L10n.t(label),
              () -> {
                if (!section.equals(label)) transition = 2;
                section = label;
                collectionName = "";
                restoreShelf = "";
                sort = label.equals("Search") ? 0 : 1;
                detailId = "";
                genre = "";
                query = "";
                restoreId = "";
                render();
              });
      b.setTag("nav:" + label);
      b.setTextSize(NAV_TEXT);
      b.setMinWidth(0); b.setMinimumWidth(0);
      b.setPadding(Ui.dp(this, 14), 0, Ui.dp(this, 14), 0);
      Ui.tab(b, section.equals(label));
      Ui.add(menus, b, -2, 32);
      Ui.gap(menus, 4);
    }

    Button settings = Ui.button(this, L10n.t("Settings"), this::settings);
    Ui.tab(settings, false);
    settings.setTextSize(NAV_TEXT);
    settings.setMinWidth(0); settings.setMinimumWidth(0);
    settings.setPadding(Ui.dp(this, 14), 0, Ui.dp(this, 14), 0);
    Ui.add(menus, settings, -2, 32);
    Ui.gap(nav, 8);
    nav.addView(L10n.picker(this));
    if(!detailId.isEmpty()) {
      nav.removeAllViews();nav.addView(new View(this),new LinearLayout.LayoutParams(0,1,1));
      nav.addView(L10n.picker(this));
    }
    Ui.add(root, nav, -1, 52);
    Ui.gap(root, 6);
    content = Ui.column(this);
    root.addView(content, new LinearLayout.LayoutParams(-1, 0, 1));
  }

  private void load() {
    if (loading || selectingProfile) return;
    if (api.profile.isEmpty()) { chooseProfile(false); return; }
    StartupTrace.mark("Library load enqueued");
    loading = true;
    boolean refreshingExistingScreen = library != null;
    if (library == null) {
      base();
      connectionScreen();
    }
    io.execute(
        () -> {
          StartupTrace.mark("Library worker started");
          Api requested = api;
          BrowseCache cache = new BrowseCache(this, requested);
          if (library == null) {
            try {
              StartupTrace.mark("Catalog cache read started");
              byte[] saved = cache.read("library", 0);
              StartupTrace.mark(saved == null ? "Catalog cache miss" : "Catalog cache hit");
              if (saved != null) {
                String cachedRaw = new String(saved, java.nio.charset.StandardCharsets.UTF_8);
                JSONObject snapshot = CatalogJson.parse(cachedRaw);
                Library cached = Library.parse(snapshot);
                requested.libraryRevision = snapshot.optString("revision");
                requested.libraryRaw = cachedRaw;
                StartupTrace.mark("Cached catalog parsed");
                runOnUiThread(() -> {
                  if (isDestroyed() || selectingProfile || api != requested || library != null) return;
                  library = cached; librarySnapshot = cachedRaw; StartupTrace.mark("Cached render started"); render(); StartupTrace.mark("Cached render returned");
                });
              }
            } catch (Exception ignored) {}
          }
          try {
            if (!requested.libraryChanged()) {
              runOnUiThread(() -> { if(api==requested)loading=false; });
              try { requested.register(); ProgressQueue.flush(this,requested); } catch(Exception ignored) {}
              return;
            }
            JSONObject response = requested.request("GET", "/v1/library", null);
            Library result = Library.parse(response);
            String liveRaw = requested.libraryRaw;
            StartupTrace.mark("Live catalog parsed");
            runOnUiThread(
                () -> {
                  if (isDestroyed()) return;
                  loading = false;
                  if (api != requested || selectingProfile) return;
                  boolean firstScreen = library == null;
                  library = result;
                  // Cached content is already usable. Refresh the model without tearing
                  // down the screen immediately before its first frame.
                  if (firstScreen || refreshingExistingScreen || !liveRaw.equals(librarySnapshot)) {
                    librarySnapshot = liveRaw;
                    StartupTrace.mark("Live render started");
                    if(firstScreen || !detailId.isEmpty())render();
                    else updateBrowse(root);
                  }
                });
            StartupTrace.mark("Catalog cache write started");
            cache.write("library", liveRaw.getBytes(java.nio.charset.StandardCharsets.UTF_8));
            StartupTrace.mark("Catalog cache write returned");
            // History synchronization must not delay the first library frame.
            try { StartupTrace.mark("Registration started"); requested.register(); StartupTrace.mark("Registration finished"); ProgressQueue.flush(this, requested); StartupTrace.mark("History sync finished"); } catch (Exception ignored) {}
          } catch (Exception e) {
            runOnUiThread(
                () -> {
                  if (isDestroyed()) return;
                  loading = false;
                  if (api != requested || selectingProfile) return;
                  if (library != null) {
                    render();
                    Toast.makeText(this, "Could not refresh. " + message(e), Toast.LENGTH_LONG)
                        .show();
                    return;
                  }
                  shell();
                  Ui.add(content, Ui.title(this, L10n.t("Your library is unavailable"), 25), -1, 45);
                  Ui.add(content, Ui.text(this, message(e), 15, Ui.MUTED), -1, -2);
                  Ui.gap(content, 20);
                  Button retry = Ui.button(this, L10n.t("Try again"), this::load);
                  Ui.add(content, retry, 180, 44);
                  Ui.gap(content, 10);
                  Ui.add(content, Ui.button(this, L10n.t("Connection settings"), this::settings), 220, 44);
                  retry.requestFocus();
                });
          }
        });
  }

  static String message(Exception e) {
    String s = e.getMessage();
    return s == null
        ? "Check the server address and network connection."
        : s.substring(0, Math.min(s.length(), 220));
  }

  private void render() {
    if (library == null) return;
    if (art != null) art.beginScreen();
    shell();
    arrive(content);
    root.setTag(new ArrayList<Runnable>());
    root.getViewTreeObserver().addOnGlobalFocusChangeListener((oldFocus,newFocus) -> { if(art!=null)art.prioritize(); });
    root.getViewTreeObserver().addOnScrollChangedListener(() -> { if(art!=null)art.prioritize(); });
    StartupTrace.mark("Library view construction started");
    View drawingRoot = root;
    drawingRoot.getViewTreeObserver().addOnPreDrawListener(new android.view.ViewTreeObserver.OnPreDrawListener() {
      public boolean onPreDraw() {
        drawingRoot.getViewTreeObserver().removeOnPreDrawListener(this);
        StartupTrace.mark("First library frame ready");
        if(art!=null)art.prioritize();
        return true;
      }
    });
    inLibrary = true;
    Library.Title detail = library.find(detailId);
    if (detail != null) {
      if (showingCast) castPage(detail); else details(detail);
      wireNavigation();
      return;
    }
    detailId = "";
    if (!collectionName.isEmpty()) collection();
    else if (section.equals("Torrents")) torrentScreen();
    else if (section.equals("Home")) home();
    else catalog();
    wireNavigation();
  }

  /** A short directional arrival, so moving between pages reads as travel rather than a cut. */
  private void arrive(View page) {
    int direction = transition; transition = 0;
    if (direction == 0 || page == null) return;
    page.animate().cancel();
    page.setAlpha(0f);
    page.setTranslationX(direction == 2 ? 0 : Ui.dp(this, 28) * direction);
    page.animate().alpha(1f).translationX(0).setDuration(220)
        .setInterpolator(new android.view.animation.DecelerateInterpolator()).withLayer().start();
  }

  private View firstFocusable(View v) {
    if (v.isFocusable()) return v;
    if (v instanceof android.view.ViewGroup) {
      android.view.ViewGroup group = (android.view.ViewGroup) v;
      for (int i = 0; i < group.getChildCount(); i++) {
        View found = firstFocusable(group.getChildAt(i));
        if (found != null) return found;
      }
    }
    return null;
  }

  private void collectNavigationFocus(View view, List<View> result) {
    if (view.isFocusable() && view.getVisibility() == View.VISIBLE) { result.add(view); return; }
    if (view instanceof android.view.ViewGroup) {
      android.view.ViewGroup group = (android.view.ViewGroup)view;
      for (int i = 0; i < group.getChildCount(); i++) collectNavigationFocus(group.getChildAt(i), result);
    }
  }

  private void wireNavigation() {
    if (nav == null) return;
    List<View> buttons = new ArrayList<>();
    collectNavigationFocus(nav, buttons);
    for (View button : buttons) button.setId(View.generateViewId());
    View entry = content.findViewWithTag("hero");
    if (entry == null) entry = firstFocusable(content);
    if (entry != null && entry.getId() == View.NO_ID) entry.setId(View.generateViewId());
    for (int i = 0; i < buttons.size(); i++) {
      View b = buttons.get(i);
      b.setNextFocusLeftId(buttons.get(Math.max(0, i - 1)).getId());
      b.setNextFocusRightId(buttons.get(Math.min(buttons.size() - 1, i + 1)).getId());
      b.setNextFocusUpId(b.getId());
      if (entry != null) {
        b.setNextFocusDownId(entry.getId());
        b.setOnKeyListener((v,key,event)->{
          if(key!=KeyEvent.KEYCODE_DPAD_DOWN)return false;
          if(event.getAction()==KeyEvent.ACTION_DOWN) {
            // Resolved at press time: a refresh can rebuild the page under an open top bar.
            View target=content.findViewWithTag("hero");
            if(target==null || !target.isAttachedToWindow())target=firstFocusable(content);
            if(target!=null)target.requestFocus();
          }
          return true;
        });
      }
      if (("nav:" + section).equals(b.getTag()) && entry != null)
        entry.setNextFocusUpId(b.getId());
    }
  }

  private ScrollView scroll(LinearLayout parent) {
    ScrollView s = new TvScrollView(this);
    s.setFillViewport(false);
    s.setClipToPadding(false);
    s.setVerticalScrollBarEnabled(false);
    parent.addView(s, new LinearLayout.LayoutParams(-1, 0, 1));
    return s;
  }

  private void ambient(Library.Title title) { root.setBackgroundColor(Ui.BG); }

  private void home() {
    ScrollView sc = scroll(content);
    LinearLayout page = Ui.column(this);
    sc.addView(page);
    List<Library.Title> recent = library.filter("", "", "", 0);
    if (recent.isEmpty()) {
      Ui.pad(page, 40, 25);
      Ui.add(page, Ui.title(this, L10n.t("Your library is empty"), 26), -1, 60);
      Ui.add(
          page,
          Ui.text(this, L10n.t("Add media on your Torrbaby server, then refresh here."), 15, Ui.MUTED),
          -1,
          40);
      Ui.add(page, Ui.button(this, L10n.t("Refresh"), this::load), 160, 44);
      return;
    }
    Library.Title featured = recent.get(0);
    for (Library.Title t : recent)
      if (!t.backdrop.isEmpty() && !t.playable().isEmpty()) {
        featured = t;
        break;
      }
    for (Library.Title t : recent)
      if (!t.playable().isEmpty() && t.latestPlayed() > featured.latestPlayed()) featured = t;
    ambient(featured);
    hero(page, featured, false);
    LinearLayout body = Ui.column(this);
    Ui.pad(body, 35, 16);
    Ui.add(page, body, -1, -2);
    shelf(body, "Continue watching", library.continuing());
    shelf(body, "Recently added", recent);
    shelf(body, "Movies", library.filter("movie", "", "", 0));
    shelf(body, "TV Shows", library.filter("tv", "", "", 0));
    shelf(body, "Recently watched", finishedTitles());
    for (String name : library.genres("")) {
      List<Library.Title> matches=library.filter("", "", name, 0);
      if(matches.size()>=6)shelf(body,name,matches);
    }
    if (!library.warning.isEmpty())
      Ui.add(body, Ui.text(this, library.warning, 13, Ui.MUTED), -1, -2);
    View hero = page.findViewWithTag("hero");
    List<Library.Title> firstShelf = library.continuing().isEmpty() ? recent : library.continuing();
    View firstPoster = body.findViewWithTag("title:" + firstShelf.get(0).id);
    if (hero != null && firstPoster != null) {
      if (firstPoster.getId() == View.NO_ID) firstPoster.setId(View.generateViewId());
      hero.setNextFocusDownId(firstPoster.getId());
    }
    restoreFocus(body);
  }

  private List<Library.Title> finishedTitles() {
    List<Library.Title> titles=library.recentlyWatched();
    titles.removeIf(t -> !t.watched());
    return titles;
  }

  private void openCollection(String name, List<Library.Title> titles) {
    collectionName = name;
    collectionIds.clear();
    for (Library.Title t : titles) collectionIds.add(t.id);
    restoreId = "";
    render();
  }

  private void collection() {
    Ui.pad(content, 35, 26);
    LinearLayout heading = Ui.row(this);
    Ui.add(heading, Ui.title(this, collectionName, 28), -2, 48);
    View spacer = new View(this);
    heading.addView(spacer, new LinearLayout.LayoutParams(0, 1, 1));
    Ui.add(heading, Ui.button(this, L10n.t("Back"), this::onBackPressed), -2, 40);
    Ui.add(content, heading, -1, 50);
    Ui.gap(content, 16);
    List<Library.Title> titles = new ArrayList<>();
    for (String id : collectionIds) {
      Library.Title t = library.find(id);
      if (t != null) titles.add(t);
    }
    posterGrid(titles);
  }

  private void hero(LinearLayout parent, Library.Title t, boolean detail) {
    FrameLayout frame = new FrameLayout(this);
    Ui.add(parent, frame, -1, 365);
    LinearLayout.LayoutParams heroParams=(LinearLayout.LayoutParams)frame.getLayoutParams();
    heroParams.setMargins(Ui.dp(this,16),Ui.dp(this,4),Ui.dp(this,16),Ui.dp(this,8));
    Ui.round(frame,12);
    ImageView image = new HeroImageView(this);
    frame.addView(image, new FrameLayout.LayoutParams(-1, -1));
    art.load(image, t.id, t.backdrop.isEmpty() ? "poster" : "backdrop", t.backdrop.isEmpty() ? t.poster : t.backdrop);
    View shade = new View(this);
    shade.setBackground(
        new GradientDrawable(
            GradientDrawable.Orientation.TOP_BOTTOM,
            new int[] {0x18000000, 0x44000000, 0xf5000000}));
    frame.addView(shade, new FrameLayout.LayoutParams(-1, -1));
    LinearLayout info = Ui.column(this);
    info.setPadding(Ui.dp(this, 45), 0, Ui.dp(this, 30), Ui.dp(this, 25));
    FrameLayout.LayoutParams ip =
        new FrameLayout.LayoutParams(
            detail ? -1 : Ui.dp(this, 625), -2, Gravity.BOTTOM | Gravity.LEFT);
    frame.addView(info, ip);
    TextView name = Ui.title(this, t.title, detail ? 32 : 30);
    name.setMaxLines(2);
    Ui.add(info, name, -1, -2);
    Ui.gap(info, 10);
    Ui.add(info, Ui.text(this, t.metadata(), 14, 0xffcccccc), -1, -2);
    Ui.gap(info, 15);
    Library.Asset asset = t.preferred();
    if (detail) {
      LinearLayout buttons = Ui.row(this);
      Button play =
          Ui.button(
              this,
              playbackLabel(t, asset),
              () -> {
                if (asset == null) error("This title has no complete media files on the server.");
                else play(t, asset, false);
              });
      Ui.primary(play);
      play.setTag("hero");
      Ui.add(buttons, play, -2, 48);
      Ui.add(info, buttons, -1, 52);
      View.OnFocusChangeListener heroFocus = (v,f)-> {
        frame.setForeground(f ? Ui.shape(0,0x99ffffff,this) : null);
        frame.animate().scaleX(f?1.006f:1).scaleY(f?1.006f:1).setDuration(140).start();
      };
      play.setOnFocusChangeListener(heroFocus);
      // TV shows keep the episode guide directly below the hero, so their secondary
      // actions live in one compact menu beside Play.
      if (t.kind.equals("tv") && asset != null) {
        Ui.gap(buttons, 10);
        Button more = Ui.button(this, L10n.t("⋯  More"), () -> moreActions(t));
        more.setTag("more");
        more.setOnFocusChangeListener(heroFocus);
        Ui.add(buttons, more, -2, 48);
      }
      play.requestFocus();
    } else {
      Button explore=Ui.button(this,L10n.t("▶  Explore"),()->open(t));
      Ui.add(info,explore,-2,44);explore.setTag("hero");
      explore.setOnFocusChangeListener((v,f)-> {
        frame.setForeground(f ? Ui.shape(0,0x99ffffff,this) : null);
        frame.animate().scaleX(f?1.006f:1).scaleY(f?1.006f:1).setDuration(140).start();
      });
      onCatalogUpdate(() -> {
        Library.Title latest=null;
        for(Library.Title candidate:library.titles)
          if(!candidate.playable().isEmpty() && (latest==null || candidate.latestPlayed()>latest.latestPlayed()))latest=candidate;
        if(latest==null)return;
        final Library.Title selected=latest;
        name.setText(selected.title);
        ((TextView)info.getChildAt(2)).setText(selected.metadata());
        explore.setContentDescription(L10n.t("Explore")+" "+selected.title);
        explore.setOnClickListener(v -> open(selected));
        art.load(image,selected.id,selected.backdrop.isEmpty()?"poster":"backdrop",selected.backdrop.isEmpty()?selected.poster:selected.backdrop);
      });
      if (restoreId.isEmpty()) explore.requestFocus();
    }
  }

  private void shelf(LinearLayout body, String name, List<Library.Title> list) {
    View shelfGap=new View(this);
    Ui.add(body,shelfGap,1,20);
    LinearLayout heading = Ui.row(this);
    Ui.add(heading, Ui.title(this, L10n.t(name), 18), -2, 38);
    Ui.add(body, heading, -1, 40);
    HorizontalScrollView sc = new TvHorizontalScrollView(this);
    sc.setHorizontalScrollBarEnabled(false);
    sc.setTag("shelf:" + name);
    sc.setClipToPadding(false);
    Ui.pad(sc, 8, 7);
    sc.setClipChildren(false);
    LinearLayout row = Ui.row(this);
    row.setGravity(Gravity.TOP);
    row.setClipChildren(false);
    sc.addView(row);
    Ui.add(body, sc, -1, 265);
    int visibility=list.isEmpty()?View.GONE:View.VISIBLE;
    shelfGap.setVisibility(visibility); heading.setVisibility(visibility); sc.setVisibility(visibility);
    Runnable sizeCards=() -> {
      int available=sc.getWidth()-sc.getPaddingLeft()-sc.getPaddingRight();
      if(available<=0)return;
      int gap=Ui.dp(this,12),columns=Math.max(3,(available+gap)/Ui.dp(this,130));
      int width=(available-gap*(columns-1))/columns;
      int height=width*3/2+Ui.dp(this,name.equals("Continue watching")?78:30);
      for(int i=0;i<row.getChildCount();i+=2) {
        View child=row.getChildAt(i);
        if(child.getLayoutParams().width!=width || child.getLayoutParams().height!=height) {
          child.getLayoutParams().width=width;child.getLayoutParams().height=height;child.requestLayout();
        }
      }
      if(sc.getLayoutParams().height!=height+Ui.dp(this,14)) {
        sc.getLayoutParams().height=height+Ui.dp(this,14);sc.requestLayout();
      }
    };
    // Append a screenful ahead of horizontal navigation, without a hard title cap.
    // The next vertical row is prepared before it enters the viewport.
    Runnable[] append = new Runnable[1];
    append[0] = () -> {
      int start=row.getChildCount()/2;
      int end=Math.min(list.size(),start+12);
      for(int index=start;index<end;index++) {
        View posterCard=card(list.get(index));
        ((PosterCard)posterCard).showContinuation(name.equals("Continue watching"));
        posterCard.setId(View.generateViewId());
        posterCard.setOnKeyListener((v,key,event) -> {
          if(event.getAction()==KeyEvent.ACTION_DOWN && key==KeyEvent.KEYCODE_DPAD_RIGHT
              && row.indexOfChild(v)>=row.getChildCount()-6) append[0].run();
          return false;
        });
        Ui.add(row,posterCard,120,247);Ui.gap(row,12);
      }
      sizeCards.run();
    };
    Runnable populate=() -> { if(row.getChildCount()==0)append[0].run(); };
    if(body.getChildCount()<=3)populate.run();
    sc.setOnScrollChangeListener((v,x,y,oldX,oldY)-> {
      if(x>oldX && x+sc.getWidth()*2>=row.getWidth())append[0].run();
    });
    Runnable nearViewport = () -> {
      if (row.getChildCount() != 0 || !sc.isShown()) return;
      int[] location = new int[2]; sc.getLocationOnScreen(location);
      android.graphics.Rect viewport=new android.graphics.Rect();
      content.getGlobalVisibleRect(viewport);
      if (location[1] < viewport.bottom + sc.getHeight() && location[1] + sc.getHeight() > viewport.top)
        populate.run();
    };
    sc.getViewTreeObserver().addOnScrollChangedListener(nearViewport::run);
    sc.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> { sizeCards.run(); nearViewport.run(); });
    onCatalogUpdate(() -> {
      List<Library.Title> fresh=shelfTitles(name);
      list.clear(); list.addAll(fresh);
      int visible=fresh.isEmpty()?View.GONE:View.VISIBLE;
      shelfGap.setVisibility(visible); heading.setVisibility(visible); sc.setVisibility(visible);
      if(row.getChildCount()==0) { sc.post(nearViewport); return; }
      Map<String,View> existing=new HashMap<>();
      View focus=row.findFocus();
      for(int i=0;i<row.getChildCount();i++) {
        View child=row.getChildAt(i);
        if(child instanceof PosterCard)existing.put((String)child.getTag(),child);
      }
      List<Library.Title> shown=fresh.subList(0,Math.min(Math.max(12,existing.size()),fresh.size()));
      boolean reordered=existing.size()!=shown.size();
      for(int i=0;!reordered && i<shown.size();i++)
        reordered=!Objects.equals(row.getChildAt(i*2).getTag(),"title:"+shown.get(i).id);
      if(!reordered)return;
      row.removeAllViews();
      for(Library.Title title:shown) {
        View child=existing.get("title:"+title.id);
        if(child==null)child=card(title);
        ((PosterCard)child).showContinuation(name.equals("Continue watching"));
        if(child.getId()==View.NO_ID)child.setId(View.generateViewId());
        child.setOnKeyListener((v,key,event)-> {
          if(event.getAction()==KeyEvent.ACTION_DOWN && key==KeyEvent.KEYCODE_DPAD_RIGHT
              && row.indexOfChild(v)>=row.getChildCount()-6)append[0].run();
          return false;
        });
        Ui.add(row,child,120,247); Ui.gap(row,12);
      }
      sizeCards.run();
      if(focus!=null) {
        if(focus.getParent()==row)focus.requestFocus();
        else if(row.getChildCount()>0)row.getChildAt(0).requestFocus();
        else { View menu=firstFocusable(nav);if(menu!=null)menu.requestFocus(); }
      }
    });
  }

  private boolean continueWatchingMenu(Library.Title title) {
    if(!title.isContinuing())return false;
    new AlertDialog.Builder(this).setTitle(title.title)
        .setItems(new String[]{L10n.t("Remove from Continue Watching")},(dialog,which)->setContinueWatchingHidden(title,true)).show();
    return true;
  }
  private void setContinueWatchingHidden(Library.Title title, boolean hidden) {
    Api requested=api;
    io.execute(()->{try {
      requested.request("POST","/v1/library/continue-watching",new JSONObject().put("title_id",title.id).put("hidden",hidden));
      runOnUiThread(()->{
        if(isDestroyed() || api!=requested)return;
        Library.Title latest=library.find(title.id);if(latest!=null)latest.continueWatchingHidden=hidden;
        render();load();
        if(hidden)showContinueWatchingUndo(title,requested);
      });
    }catch(Exception error){runOnUiThread(()->{if(!isDestroyed() && api==requested)error(message(error));});}});
  }
  private void showContinueWatchingUndo(Library.Title title, Api requested) {
    android.view.ViewGroup host=findViewById(android.R.id.content);
    LinearLayout banner=Ui.row(this);Ui.pad(banner,18,12);banner.setBackground(Ui.shape(Ui.PANEL,Ui.ACCENT,this));
    Ui.add(banner,Ui.text(this,L10n.t("Removed from Continue Watching"),16,Ui.TEXT),-2,-2);Ui.gap(banner,18);
    Button undo=Ui.button(this,L10n.t("Undo"),()->{host.removeView(banner);if(api==requested)setContinueWatchingHidden(title,false);});
    Ui.add(banner,undo,-2,44);
    FrameLayout.LayoutParams params=new FrameLayout.LayoutParams(-2,-2,Gravity.BOTTOM|Gravity.CENTER_HORIZONTAL);params.bottomMargin=Ui.dp(this,24);
    host.addView(banner,params);undo.requestFocus();banner.postDelayed(()->host.removeView(banner),10000);
  }

  private View card(Library.Title t) {
    PosterCard card = new PosterCard(this, art, this::open, item -> restoreId = item.id);
    card.bind(t);
    card.setOnLongClickListener(v -> continueWatchingMenu(t));
    return card;
  }

  private void restoreFocus(View body) {
    if (!restoreShelf.isEmpty()) {
      View all = body.findViewWithTag("shelf:" + restoreShelf);
      restoreShelf = "";
      if (all != null) {
        all.post(all::requestFocus);
        return;
      }
    }
    if (!restoreId.isEmpty()) {
      View v = body.findViewWithTag("title:" + restoreId);
      if (v != null) v.post(v::requestFocus);
    }
  }

  private void catalog() {
    Ui.pad(content, 35, 10);
    String kind = section.equals("Movies") ? "movie" : section.equals("TV Shows") ? "tv" : "";
    if (!section.equals("Search")) {
      for (Library.Title t : library.filter(kind, "", "", 0))
        if (!t.backdrop.isEmpty()) {
          ambient(t);
          break;
        }
    }
    LinearLayout controls = Ui.row(this);
    if (section.equals("Search")) {
      EditText search = new EditText(this);
      search.setTextColor(Ui.TEXT);
      search.setHintTextColor(Ui.MUTED);
      search.setTextSize(16);
      search.setSingleLine();
      search.setHint(L10n.t("Search titles, genres or year"));
      search.setText(query);
      search.setInputType(InputType.TYPE_CLASS_TEXT);
      if (query.isEmpty()) search.post(search::requestFocus);
      controls.addView(search, new LinearLayout.LayoutParams(0, Ui.dp(this, 44), 1));
      Ui.gap(controls, 12);
      Ui.add(
          controls,
          Ui.button(
              this,
              L10n.t("Search"),
              () -> {
                query = search.getText().toString();
                restoreId = "";
                render();
              }),
          110,
          44);
      search.setOnEditorActionListener(
          (v, a, event) -> {
            query = search.getText().toString();
            render();
            return true;
          });
    } else {
      Ui.add(controls, Ui.title(this, section, 26), -2, 40);
      View space = new View(this);
      controls.addView(space, new LinearLayout.LayoutParams(0, 1, 1));
    }
    Ui.gap(controls, 12);
    Ui.add(
        controls,
        Ui.button(
            this,
            genre.isEmpty() ? L10n.t("All genres") : genre,
            () -> {
              List<String> gs = library.genres(kind);
              gs.add(0, "All genres");
              new AlertDialog.Builder(this)
                  .setTitle(L10n.t("Genre"))
                  .setItems(
                      gs.toArray(new String[0]),
                      (d, w) -> {
                        genre = w == 0 ? "" : gs.get(w);
                        restoreId = "";
                        render();
                      })
                  .show();
            }),
        -2,
        44);
    Ui.gap(controls, 10);
    String[] sorts = {"Recently added", "A–Z", "Year"};
    Ui.add(
        controls,
        Ui.button(
            this,
            sorts[sort],
            () ->
                new AlertDialog.Builder(this)
                    .setTitle(L10n.t("Sort by"))
                    .setSingleChoiceItems(
                        sorts,
                        sort,
                        (d, w) -> {
                          sort = w;
                          restoreId = "";
                          d.dismiss();
                          render();
                        })
                    .show()),
        -2,
        44);
    Ui.add(content, controls, -1, 48);
    Ui.gap(content, 12);
    if (section.equals("Search") && query.trim().isEmpty()) {
      Ui.add(content, Ui.text(this, L10n.t("Search movies and TV shows"), 17, Ui.MUTED), -1, 70);
      return;
    }
    List<Library.Title> items = library.filter(kind, query, genre, sort);
    if (items.isEmpty()) {
      Ui.add(content, Ui.text(this, L10n.t("No matching titles"), 18, Ui.MUTED), -1, 70);
      controls.post(
          () -> {
            View field = firstFocusable(controls);
            if (field != null) field.requestFocus();
          });
      return;
    }
    posterGrid(items);
  }

  private void posterGrid(List<Library.Title> items) {
    GridView grid = new PosterGrid(this);
    grid.setNumColumns(6);
    grid.setHorizontalSpacing(Ui.dp(this, 12));
    grid.setVerticalSpacing(Ui.dp(this, 20));
    grid.setClipToPadding(false);
    grid.setSelector(android.R.color.transparent);
    grid.setDescendantFocusability(android.view.ViewGroup.FOCUS_BLOCK_DESCENDANTS);
    grid.setAdapter(
        new BaseAdapter() {
          public int getCount() {
            return items.size();
          }

          public Object getItem(int i) {
            return items.get(i);
          }

          public long getItemId(int i) {
            return i;
          }

          public View getView(int i, View old, android.view.ViewGroup p) {
            PosterCard v = old instanceof PosterCard ? (PosterCard) old
                : new PosterCard(MainActivity.this, art, MainActivity.this::open, item -> restoreId = item.id);
            Library.Title latest = library.find(items.get(i).id);
            v.bind(latest == null ? items.get(i) : latest);
            v.setFocusable(false);
            v.focusAppearance(i == ((PosterGrid) grid).highlighted());
            v.setLayoutParams(new AbsListView.LayoutParams(-1, -2));
            return v;
          }
        });
    final String gridKind=section.equals("Movies")?"movie":section.equals("TV Shows")?"tv":"";
    final String gridQuery=query, gridGenre=genre;
    final int gridSort=sort;
    final String gridCollection=collectionName;
    onCatalogUpdate(() -> {
      List<Library.Title> fresh;
      if(gridCollection.isEmpty())fresh=library.filter(gridKind,gridQuery,gridGenre,gridSort);
      else fresh=shelfTitles(gridCollection);
      int selected=grid.getSelectedItemPosition();
      String selectedId=selected>=0&&selected<items.size()?items.get(selected).id:"";
      boolean reordered=items.size()!=fresh.size();
      for(int i=0;!reordered&&i<items.size();i++)reordered=!items.get(i).id.equals(fresh.get(i).id);
      items.clear();items.addAll(fresh);
      if(reordered) {
        ((BaseAdapter)grid.getAdapter()).notifyDataSetChanged();
        int restored=Math.min(Math.max(0,selected),items.size()-1);
        for(int i=0;i<items.size();i++)if(items.get(i).id.equals(selectedId))restored=i;
        if(restored>=0)grid.setSelection(restored);
      }
    });
    grid.setOnItemLongClickListener((parent,view,position,id) -> continueWatchingMenu(items.get(position)));
    grid.setOnItemClickListener(
        (p, v, pos, id) -> {
          restoreId = items.get(pos).id;
          open(items.get(pos));
        });
    grid.setOnItemSelectedListener(
        new AdapterView.OnItemSelectedListener() {
          public void onNothingSelected(AdapterView<?> p) {}

          public void onItemSelected(AdapterView<?> p, View v, int pos, long id) {
            restoreId = items.get(pos).id;
            int nextRow=(pos/grid.getNumColumns()+1)*grid.getNumColumns();
            for(int i=nextRow;i<Math.min(items.size(),nextRow+grid.getNumColumns());i++)art.prefetch(items.get(i));
            art.prioritize();
            for (int i = 0; i < grid.getChildCount(); i++) {
              View child = grid.getChildAt(i);
              if (child instanceof PosterCard) ((PosterCard) child).focusAppearance(child == v);
            }
          }
        });
    grid.setSelector(android.R.color.transparent);
    grid.setDrawSelectorOnTop(true);
    content.addView(grid, new LinearLayout.LayoutParams(-1, 0, 1));
    grid.setPadding(Ui.dp(this,8),Ui.dp(this,12),Ui.dp(this,8),Ui.dp(this,12));
    grid.addOnLayoutChangeListener((v,l,t,r,b,ol,ot,or,ob) -> {
      if (r-l != or-ol) {
        int columns = Math.max(3, (r-l + Ui.dp(this,20)) / Ui.dp(this,150));
        if(grid.getNumColumns()!=columns)grid.setNumColumns(columns);
      }
    });
    int index = 0;
    for (int i = 0; i < items.size(); i++) if (items.get(i).id.equals(restoreId)) index = i;
    grid.setSelection(index);
    if (!section.equals("Search") || !query.isEmpty()) grid.requestFocus();
  }

  /** Why a torrent needs the user: an error, a stall, or a finished download the library cannot file. */
  private static String torrentIssue(JSONObject item) {
    boolean complete = item.optDouble("percent_done", 0) >= 1;
    if (item.optInt("error") != 0) return item.optString("error_string");
    if (complete) return item.isNull("library_issue") || item.optString("library_issue").isEmpty() ? null : item.optString("library_issue");
    int state = item.optInt("status");
    if (state == 0) return null;
    if (item.optBoolean("is_stalled")) return "Transmission reports this torrent as stalled";
    long idle = System.currentTimeMillis() / 1000 - Math.max(item.optLong("added_date"), item.optLong("activity_date"));
    if (state == 4 && item.optLong("rate_download") == 0 && idle > 300)
      return item.optInt("peers_connected") == 0 ? "No peers and no activity for over 5 minutes" : "Peers connected, but no activity for over 5 minutes";
    return null;
  }

  /** Mark one attention item (or all, with id -1) as seen, then refresh the list. */
  private void dismissAttention(JSONObject body, long id) {
    Api requested = api;
    io.execute(() -> {
      try {
        if (id < 0) body.put("all", true); else body.put("ids", new JSONArray().put(id));
        requested.request("POST", "/v1/attention/dismiss", body);
      } catch (Exception e) { runOnUiThread(() -> error(message(e))); return; }
      runOnUiThread(this::fetchTorrents);
    });
  }

  private void torrentScreen() {
    torrentActions = new TorrentActions(this, api, io, this::fetchTorrents, this::torrentRemoved);
    Ui.pad(content, 35, 16);
    LinearLayout controls = Ui.row(this);
    for (String category : new String[] {"Downloading", "Seeding", "Complete", "Needs attention", "Watching"}) {
      Button button =
          Ui.button(
              this,
              L10n.t(category),
              () -> {
                torrentCategory = category;
                render();
              });
      Ui.tab(button,category.equals(torrentCategory));
      Ui.add(controls, button, -2, 42);
      Ui.gap(controls, 12);
    }
    View space = new View(this);
    controls.addView(space, new LinearLayout.LayoutParams(0, 1, 1));
    Ui.add(controls, Ui.button(this, L10n.t("Find torrents"), () -> startActivity(new Intent(this, TorrentSearchActivity.class))), -2, 42);
    Ui.gap(controls, 12);
    Ui.add(controls, Ui.button(this, L10n.t("Manage"), () -> torrentActions.manage()), -2, 42);
    Ui.gap(controls, 12);
    Ui.add(controls, Ui.button(this, L10n.t("Refresh"), () -> fetchTorrents()), -2, 42);
    Ui.add(content, controls, -1, 44);
    Ui.gap(content, 10);
    String[] choices =
        torrentCategory.equals("Seeding")
            ? new String[] {"Ratio", "Date added"}
            : torrentCategory.equals("Complete")
                ? new String[] {"Alphabetical", "Date added", "Size"}
                : new String[] {"Progress", "Date added", "Alphabetical"};
    String order =
        getPreferences(MODE_PRIVATE).getString("torrentSort." + torrentCategory, choices[0]);
    LinearLayout sortRow = Ui.row(this);
    for (String choice : choices) {
      Button button =
          Ui.button(
              this,
              L10n.t(choice),
              () -> {
                getPreferences(MODE_PRIVATE)
                    .edit()
                    .putString("torrentSort." + torrentCategory, choice)
                    .apply();
                render();
              });
      Ui.tab(button,choice.equals(order));
      Ui.add(sortRow, button, -2, 40);
      Ui.gap(sortRow, 10);
    }
    Ui.add(content, sortRow, -1, 42);
    torrentStatus =
        Ui.text(
            this,
            torrentStale
                ? L10n.t("Offline · showing the last server status")
                : torrentUpdated == 0
                    ? L10n.t("Server transfers")
                    : L10n.t("Server transfers") + " · " + L10n.t("Updated") + " "
                        + new java.text.SimpleDateFormat("HH:mm:ss", Locale.getDefault())
                            .format(new java.util.Date(torrentUpdated)),
            11,
            Ui.MUTED);
    Ui.add(content, torrentStatus, -1, 26);
    Ui.gap(content, 8);
    if (torrents == null && libraryOperations == null) {
      Ui.add(content, Ui.text(this, L10n.t("Loading server activity…"), 16, Ui.MUTED), -1, 50);
      fetchTorrents();
      controls
          .getChildAt(
              torrentCategory.equals("Seeding") ? 2 : torrentCategory.equals("Complete") ? 4 : 0)
          .requestFocus();
      return;
    }
    ScrollView scroll = scroll(content);
    LinearLayout rows = Ui.column(this);
    scroll.addView(rows);
    int count = 0;
    if (torrentCategory.equals("Needs attention") && attention != null && attention.length() > 0) {
      Ui.add(rows, Ui.text(this, L10n.t("Processing") + " · " + attention.length(), 14, Ui.MUTED), -1, -2);
      Ui.gap(rows, 10);
      for (int i = 0; i < attention.length(); i++) {
        JSONObject item = attention.optJSONObject(i);
        if (item == null) continue;
        count++;
        LinearLayout row = Ui.column(this);
        Ui.focus(row);
        row.setOnFocusChangeListener((v, focused) -> Ui.secondaryContrast(v, focused));
        Ui.pad(row, 16, 14);
        Ui.add(row, Ui.title(this, item.optString("title"), 16), -1, -2);
        Ui.gap(row, 6);
        boolean error = item.optString("severity").equals("error");
        Ui.add(row, Ui.text(this, L10n.t(item.optString("summary")), 14, error ? 0xffffb74d : Ui.MUTED), -1, -2);
        Ui.add(row, Ui.text(this, L10n.t(item.optString("error")), 12, Ui.MUTED), -1, -2);
        row.setOnClickListener(v -> new AlertDialog.Builder(this)
            .setTitle(item.optString("title"))
            .setMessage(L10n.t(item.optString("error")))
            .setPositiveButton(L10n.t("Dismiss"), (d, w) -> dismissAttention(new JSONObject(), item.optLong("id")))
            .setNeutralButton(L10n.t("Dismiss all"), (d, w) -> dismissAttention(new JSONObject(), -1))
            .setNegativeButton(L10n.t("Cancel"), null)
            .show());
        Ui.add(rows, row, -1, -2);
        Ui.gap(rows, 10);
      }
    }
    if (torrentCategory.equals("Downloading")) count += downloadingExtras(rows);
    if (torrentCategory.equals("Watching")) count += watchingRows(rows);
    List<JSONObject> sorted = new ArrayList<>();
    for (int i = 0; torrents != null && i < torrents.length(); i++)
      if (torrents.optJSONObject(i) != null) sorted.add(torrents.optJSONObject(i));
    sorted.sort(
        (a, b) -> {
          String key =
              order.equals("Progress")
                  ? "percent_done"
                  : order.equals("Ratio")
                      ? "upload_ratio"
                      : order.equals("Date added") ? "added_date" : "";
          int cmp =
              order.equals("Size")
                  ? Long.compare(torrentSize(b), torrentSize(a))
                  : key.isEmpty() ? 0 : Double.compare(b.optDouble(key, 0), a.optDouble(key, 0));
          return cmp != 0 ? cmp : a.optString("name").compareToIgnoreCase(b.optString("name"));
        });
    for (JSONObject item : sorted) {
      if (item == null) continue;
      double progress = item.optDouble("percent_done", 0);
      boolean complete = progress >= 1;
      int state = item.optInt("status");
      JSONObject fileOperation = item.optJSONObject("file_operation");
      boolean junk = fileOperation != null && fileOperation.optString("action").equals("junk");
      String issue = torrentIssue(item);
      if (torrentCategory.equals("Watching")) continue;
      if (torrentCategory.equals("Needs attention")) { if (junk || issue == null) continue; }
      else if (fileOperation != null ? !torrentCategory.equals("Seeding") : torrentCategory.equals("Downloading")
          ? complete
          : torrentCategory.equals("Seeding")
              ? (!complete || state != 6)
              : (!complete || state == 6)) continue;
      count++;
      LinearLayout row = Ui.column(this);
      Ui.focus(row);
      row.setTag("torrent:" + item.optString("hash_string"));
      row.setOnClickListener(v -> torrentActions.menu(item));
      row.setOnFocusChangeListener((v, focused) -> Ui.secondaryContrast(v, focused));
      Ui.pad(row, 16, 14);
      Ui.add(
          row,
          Ui.title(this, item.optString("name").replace('_', ' ').replace('.', ' '), 16),
          -1,
          -2);
      Ui.gap(row, 10);
      if (fileOperation != null) {
        String operationError = fileOperation.isNull("error") ? "" : fileOperation.optString("error");
        String operationLabel = junk ? "Junk · delete after seeding" : "Deleting files…";
        Ui.add(row, Ui.text(this, operationError.isEmpty() ? operationLabel : operationLabel + " · " + operationError,
            14, operationError.isEmpty() ? Ui.ACCENT : 0xffffffff), -1, -2);
        Ui.gap(row, 8);
      }
      if (torrentCategory.equals("Needs attention") && issue != null) {
        Ui.add(row, Ui.text(this, L10n.t(issue), 14, 0xffffb74d), -1, -2);
        Ui.gap(row, 8);
      }
      long size = 0;
      JSONArray files = item.optJSONArray("files");
      if (files != null)
        for (int f = 0; f < files.length(); f++) size += files.optJSONObject(f).optLong("length");
      String line =
          bytes(size)
              + "   ·   ↓ "
              + bytes(item.optLong("rate_download"))
              + "/s   ·   ↑ "
              + bytes(item.optLong("rate_upload"))
              + "/s   ·   Ratio "
              + String.format(Locale.ROOT, "%.2f", Math.max(0, item.optDouble("upload_ratio", 0)));
      Ui.add(row, Ui.text(this, line, 12, Ui.MUTED), -1, -2);
      if (!complete) {
        Ui.gap(row, 10);
        ProgressBar bar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        bar.setProgressTintList(android.content.res.ColorStateList.valueOf(0xffffffff));
        bar.setProgress((int) (100 * progress));
        Ui.add(row, bar, -1, 8);
        Ui.gap(row, 6);
        String stateName = TorrentActions.status(item);
        Ui.add(
            row,
            Ui.text(this, stateName + "  ·  " + (int) (100 * progress) + "%", 12, Ui.MUTED),
            -1,
            -2);
      }
      if (complete) Ui.add(row, Ui.text(this, TorrentActions.status(item), 12, Ui.MUTED), -1, -2);
      long eta = item.optLong("eta", -1);
      if (!complete && eta >= 0)
        Ui.add(
            row,
            Ui.text(this, "About " + Math.max(1, eta / 60) + " min remaining", 12, Ui.MUTED),
            -1,
            -2);
      if (item.optInt("error") != 0) {
        Ui.gap(row, 8);
        Ui.add(row, Ui.text(this, item.optString("error_string"), 13, 0xffcccccc), -1, -2);
      }
      Ui.add(rows, row, -1, -2);
      Ui.gap(rows, 14);
    }
    if (torrentCategory.equals("Seeding")) {
      if (!libraryOperationsError.isEmpty())
        Ui.add(rows, Ui.text(this, L10n.t("Couldn’t refresh file operations") + ": " + libraryOperationsError, 14, 0xffffffff), -1, -2);
      java.util.Set<String> shown = new java.util.HashSet<>();
      for (JSONObject item : sorted) {
        JSONObject operation = item.optJSONObject("file_operation");
        if (operation != null) shown.add(operation.optString("id"));
      }
      for (int i = 0; libraryOperations != null && i < libraryOperations.length(); i++) {
        JSONObject operation = libraryOperations.optJSONObject(i);
        if (operation == null || operation.optString("phase").equals("done") || shown.contains(operation.optString("id"))) continue;
        count++;
        String name = "Library files";
        try { name = new JSONObject(operation.optString("preview")).optString("name", name); }
        catch (Exception ignored) { }
        LinearLayout row = Ui.column(this);
        Ui.focus(row);
        row.setTag("operation:" + operation.optString("id"));
        row.setOnFocusChangeListener((v, focused) -> Ui.secondaryContrast(v, focused));
        Ui.pad(row, 16, 14);
        Ui.add(row, Ui.title(this, name, 16), -1, -2);
        String phase = operation.optString("phase");
        String label = phase.equals("seeding") ? "In Junk · delete after seeding"
            : phase.equals("moving") || phase.equals("verifying") ? "Moving to Junk…"
            : phase.equals("cleanup") ? "Finishing cleanup…" : "Removing from library…";
        Ui.add(row, Ui.text(this, label, 14, Ui.MUTED), -1, -2);
        if (!operation.isNull("error") && !operation.optString("error").isEmpty()) {
          Ui.add(row, Ui.text(this, operation.optString("error"), 14, 0xffffffff), -1, -2);
          Ui.add(row, Ui.text(this, L10n.t("The server will retry automatically."), 12, Ui.MUTED), -1, -2);
        }
        Ui.add(rows, row, -1, -2);
        Ui.gap(rows, 10);
      }
    }
    if (count == 0) Ui.add(rows, Ui.text(this, L10n.t("No torrents"), 18, Ui.MUTED), -1, 60);
    controls
        .getChildAt(
            torrentCategory.equals("Seeding") ? 2 : torrentCategory.equals("Complete") ? 4 : 0)
        .requestFocus();
  }

  private static long torrentSize(JSONObject item) {
    long size = 0;
    JSONArray files = item.optJSONArray("files");
    if (files != null)
      for (int i = 0; i < files.length(); i++)
        if (files.optJSONObject(i) != null) size += files.optJSONObject(i).optLong("length");
    return size;
  }

  private static String bytes(long n) {
    if (n >= 1024L * 1024 * 1024)
      return String.format(Locale.ROOT, "%.1f GB", n / (1024.0 * 1024 * 1024));
    if (n >= 1024 * 1024) return String.format(Locale.ROOT, "%.1f MB", n / (1024.0 * 1024));
    if (n >= 1024) return String.format(Locale.ROOT, "%.0f KB", n / 1024.0);
    return n + " B";
  }

  private JSONArray visibleTorrents(JSONArray incoming) {
    long now = SystemClock.elapsedRealtime();
    removedTorrents.entrySet().removeIf(entry -> entry.getValue() < now);
    JSONArray visible = new JSONArray();
    if (incoming != null) for (int i = 0; i < incoming.length(); i++) {
      JSONObject item = incoming.optJSONObject(i);
      if (item != null && !removedTorrents.containsKey(item.optString("hash_string").toLowerCase(Locale.ROOT))) visible.put(item);
    }
    return visible;
  }

  private void torrentRemoved(String hash) {
    if (hash == null) { removedTorrents.clear(); return; }
    if (hash.isEmpty()) return;
    removedTorrents.put(hash.toLowerCase(Locale.ROOT), SystemClock.elapsedRealtime() + 60000);
    torrents = visibleTorrents(torrents);
    if (section.equals("Torrents") && detailId.isEmpty()) render();
  }

  /** Free space on each media drive, then Quark Drive downloads; returns the rows added. */
  private int downloadingExtras(LinearLayout rows) {
    int added = 0;
    JSONArray volumes = storage == null ? null : storage.optJSONArray("volumes");
    for (int i = 0; volumes != null && i < volumes.length(); i++) {
      JSONObject volume = volumes.optJSONObject(i);
      if (volume == null) continue;
      String line = volume.optBoolean("available")
          ? String.format(L10n.locale(), L10n.t("%@ free of %@").replace("%@", "%s"), bytes(volume.optLong("free_bytes")), bytes(volume.optLong("total_bytes")))
          : L10n.t("Drive not connected");
      Ui.add(rows, Ui.text(this, line + " · " + volume.optString("name"), 14, Ui.MUTED), -1, -2);
    }
    if (volumes != null && volumes.length() > 0) Ui.gap(rows, 10);
    for (int i = 0; cloudJobs != null && i < cloudJobs.length(); i++) {
      JSONObject job = cloudJobs.optJSONObject(i);
      if (job == null) continue;
      added++;
      String state = job.optString("state");
      LinearLayout row = Ui.column(this);
      Ui.focus(row);
      row.setOnFocusChangeListener((v, focused) -> Ui.secondaryContrast(v, focused));
      Ui.pad(row, 16, 14);
      // Quark downloads are told from the torrents around them by their colour.
      TextView jobTitle = Ui.title(this, job.optString("title"), 16); jobTitle.setTextColor(0xff85c2ff);
      Ui.add(row, jobTitle, -1, -2);
      Ui.gap(row, 6);
      boolean active = state.equals("queued") || state.equals("saving") || state.equals("downloading") || state.equals("retrying");
      int place = job.isNull("position") ? 0 : job.optInt("position");
      String label = state.equals("retrying") ? L10n.t("Retrying shortly…") : state.equals("queued") ? (place > 0 ? String.format(L10n.locale(), L10n.t("Waiting · %d in line"), place) : L10n.t("Waiting")) : state.equals("saving") ? L10n.t("Saving to Quark Drive…")
          : state.equals("downloading") ? L10n.t("Downloading") : state.equals("done") ? L10n.t("Downloaded") : L10n.t("Failed");
      String detail = L10n.t("Quark Drive") + " · " + label;
      if (job.optInt("files") > 1) detail += " · " + String.format(L10n.locale(), L10n.t("%d of %d videos"), job.optInt("files_done"), job.optInt("files"));
      if (job.optLong("total_bytes") > 0) detail += " · " + bytes(job.optLong("done_bytes")) + " / " + bytes(job.optLong("total_bytes"));
      if (job.optLong("rate") > 0) detail += " · " + bytes(job.optLong("rate")) + "/s";
      if (state.equals("downloading") && !job.isNull("eta")) {
        int minutes = (int) Math.max(1, (job.optLong("eta") + 30) / 60);
        detail += " · " + (minutes >= 60 ? String.format(L10n.locale(), L10n.t("%dh %dm left"), minutes / 60, minutes % 60) : String.format(L10n.locale(), L10n.t("%dm left"), minutes));
      }
      Ui.add(row, Ui.text(this, detail, 14, state.equals("failed") || state.equals("retrying") ? 0xffffb74d : Ui.ACCENT), -1, -2);
      // Episodes that took the place of a lesser copy, and ones skipped because the library's copy was as good.
      List<String> outcomes = new ArrayList<>();
      if (job.optInt("replaced") > 0) outcomes.add(String.format(L10n.locale(), L10n.t("Replaced with better: %d"), job.optInt("replaced")));
      if (job.optInt("discarded") > 0) outcomes.add(String.format(L10n.locale(), L10n.t("Already had as good: %d"), job.optInt("discarded")));
      if (!outcomes.isEmpty()) Ui.add(row, Ui.text(this, String.join(" · ", outcomes), 12, Ui.MUTED), -1, -2);
      if (!job.isNull("current") && !job.optString("current").isEmpty()) Ui.add(row, Ui.text(this, job.optString("current"), 12, Ui.MUTED), -1, -2);
      String error = job.isNull("error") ? "" : job.optString("error");
      if (!error.isEmpty()) Ui.add(row, Ui.text(this, error, 12, Ui.MUTED), -1, -2);
      String id = job.optString("id");
      row.setOnClickListener(v -> {
        AlertDialog.Builder dialog = new AlertDialog.Builder(this).setTitle(job.optString("title")).setMessage(error.isEmpty() ? label : error)
            .setNegativeButton(L10n.t(active ? "Close" : "Cancel"), null);
        // Stops the download and clears what it had fetched of unfinished videos.
        if (active) dialog.setNeutralButton(L10n.t("Cancel download"), (d, w) -> cloudJobAction("DELETE", "/v1/cloud/jobs/" + id));
        if (state.equals("failed") || state.equals("retrying")) dialog.setPositiveButton(L10n.t("Retry"), (d, w) -> cloudJobAction("POST", "/v1/cloud/jobs/" + id + "/retry"));
        // Downloads run in the order added; this one jumps the line once the current video is done.
        if (state.equals("queued") && place > 1) dialog.setPositiveButton(L10n.t("Download next"), (d, w) -> cloudJobAction("POST", "/v1/cloud/jobs/" + id + "/next"));
        if (state.equals("failed") || state.equals("done")) dialog.setNeutralButton(L10n.t("Dismiss"), (d, w) -> cloudJobAction("DELETE", "/v1/cloud/jobs/" + id));
        dialog.show();
      });
      Ui.add(rows, row, -1, -2);
      Ui.gap(rows, 10);
    }
    return added;
  }
  /** "E01–E11 E13–E34": runs of consecutive episodes. */
  private static String episodeRanges(JSONArray numbers) {
    List<Integer> all = new ArrayList<>();
    for (int i = 0; numbers != null && i < numbers.length(); i++) all.add(numbers.optInt(i));
    Collections.sort(all);
    List<String> runs = new ArrayList<>();
    int start = -1, end = -1;
    for (int n : all) {
      if (start >= 0 && n <= end + 1) { end = Math.max(end, n); continue; }
      if (start >= 0) runs.add(start == end ? String.format(Locale.ROOT, "E%02d", start) : String.format(Locale.ROOT, "E%02d–E%02d", start, end));
      start = end = n;
    }
    if (start >= 0) runs.add(start == end ? String.format(Locale.ROOT, "E%02d", start) : String.format(Locale.ROOT, "E%02d–E%02d", start, end));
    return runs.size() > 5 ? String.join(" ", runs.subList(0, 5)) + " …" : String.join(" ", runs);
  }
  /** The shows watched for new episodes: what each lacks, where it is looked for, and when it was last checked. */
  private int watchingRows(LinearLayout rows) {
    java.text.DateFormat when = java.text.DateFormat.getDateTimeInstance(java.text.DateFormat.MEDIUM, java.text.DateFormat.SHORT, L10n.locale());
    JSONArray shows = watchList == null ? null : watchList.optJSONArray("shows");
    if (watchList != null && !watchList.isNull("next_check"))
      Ui.add(rows, Ui.text(this, String.format(L10n.locale(), L10n.t("Next check: %s"), when.format(new Date((long) (watchList.optDouble("next_check") * 1000)))), 14, Ui.MUTED), -1, -2);
    Ui.add(rows, Ui.text(this, L10n.t("Missing episodes are looked for around midday and in the evening."), 12, Ui.MUTED), -1, -2);
    Ui.gap(rows, 10);
    if (shows == null || shows.length() == 0) {
      Ui.add(rows, Ui.title(this, L10n.t("No shows are being watched"), 18), -1, -2);
      return 1;
    }
    Button check = Ui.button(this, L10n.t("Check now"), () -> cloudJobAction("POST", "/v1/watch/check"));
    Ui.add(rows, check, 220, 44);
    Ui.gap(rows, 10);
    for (int i = 0; i < shows.length(); i++) {
      JSONObject show = shows.optJSONObject(i);
      if (show == null) continue;
      String id = show.optString("id"), market = show.optString("market");
      String name = show.optString("title");
      if (!show.isNull("original_title") && !show.optString("original_title").isEmpty() && !show.optString("original_title").equals(name)) name += " · " + show.optString("original_title");
      List<String> gaps = new ArrayList<>();
      JSONArray missing = show.optJSONArray("missing");
      for (int j = 0; missing != null && j < missing.length(); j++) {
        JSONObject season = missing.optJSONObject(j);
        gaps.add((missing.length() > 1 || season.optInt("season") > 1 ? String.format(L10n.locale(), L10n.t("Season %d"), season.optInt("season")) + " " : "") + episodeRanges(season.optJSONArray("episodes")));
      }
      List<String> facts = new ArrayList<>();
      facts.add(L10n.t(market.equals("china") ? "China" : "World"));
      if (show.optBoolean("airing")) facts.add(L10n.t("Airing now"));
      if (show.optBoolean("auto")) facts.add(L10n.t("Added automatically"));
      if (!show.isNull("checked")) facts.add(String.format(L10n.locale(), L10n.t("Last checked %s"), when.format(new Date((long) (show.optDouble("checked") * 1000)))));
      if (show.optInt("found") > 0) facts.add(String.format(L10n.locale(), L10n.t("Queued: %d"), show.optInt("found")));
      LinearLayout row = Ui.column(this);
      Ui.focus(row);
      row.setOnFocusChangeListener((v, focused) -> Ui.secondaryContrast(v, focused));
      Ui.pad(row, 16, 14);
      Ui.add(row, Ui.title(this, name, 16), -1, -2);
      Ui.gap(row, 6);
      Ui.add(row, Ui.text(this, gaps.isEmpty() ? L10n.t("Up to date") : L10n.t("Missing") + " " + String.join(" · ", gaps), 14, gaps.isEmpty() ? Ui.ACCENT : 0xffffb74d), -1, -2);
      Ui.add(row, Ui.text(this, String.join(" · ", facts), 12, Ui.MUTED), -1, -2);
      String problem = show.isNull("error") ? "" : show.optString("error");
      if (!problem.isEmpty()) Ui.add(row, Ui.text(this, problem, 12, 0xffffb74d), -1, -2);
      String shown = name;
      // Where its episodes are looked for can be switched, and the show taken off the list.
      row.setOnClickListener(v -> new AlertDialog.Builder(this).setTitle(shown)
          .setPositiveButton(L10n.t(market.equals("china") ? "World" : "China"), (d, w) -> watchAction("POST", "/v1/watch/" + id, market.equals("china") ? "world" : "china"))
          .setNeutralButton(L10n.t("Stop watching"), (d, w) -> watchAction("DELETE", "/v1/watch/" + id, null))
          .setNegativeButton(L10n.t("Close"), null).show());
      Ui.add(rows, row, -1, -2);
      Ui.gap(rows, 10);
    }
    return shows.length();
  }
  private void watchAction(String method, String path, String market) {
    Api requested = api;
    io.execute(() -> {
      try { requested.request(method, path, market == null ? null : new JSONObject().put("market", market)); }
      catch (Exception e) { runOnUiThread(() -> Toast.makeText(this, message(e), Toast.LENGTH_LONG).show()); }
      runOnUiThread(this::fetchTorrents);
    });
  }
  private void cloudJobAction(String method, String path) {
    Api requested = api;
    io.execute(() -> {
      try { requested.request(method, path, null); } catch (Exception e) { runOnUiThread(() -> Toast.makeText(this, message(e), Toast.LENGTH_LONG).show()); }
      runOnUiThread(this::fetchTorrents);
    });
  }
  private void fetchTorrents() {
    if (torrentsLoading) return;
    torrentsLoading = true;
    Api requested = api;
    io.execute(
        () -> {
          JSONArray operationResult = null;
          String operationFailure = "";
          try { operationResult = ((JSONObject) requested.request("GET", "/v1/library/operations", null)).getJSONArray("operations"); }
          catch (Exception e) { operationFailure = message(e); }
          JSONArray torrentResult = null;
          try { torrentResult = requested.array("/v1/torrents"); }
          catch (Exception ignored) { }
          JSONArray attentionResult = null;
          try { attentionResult = requested.request("GET", "/v1/attention", null).getJSONArray("items"); }
          catch (Exception ignored) { }
          JSONArray jobsResult = null;
          try { jobsResult = requested.request("GET", "/v1/cloud/jobs", null).getJSONArray("jobs"); }
          catch (Exception ignored) { }
          JSONObject storageResult = null;
          try { storageResult = requested.request("GET", "/v1/storage", null); }
          catch (Exception ignored) { }
          JSONObject watchResult = null;
          try { watchResult = requested.request("GET", "/v1/watch", null); }
          catch (Exception ignored) { }
          final JSONObject receivedWatch = watchResult;
          final JSONArray receivedJobs = jobsResult;
          final JSONObject receivedStorage = storageResult;
          final JSONArray receivedAttention = attentionResult;
          final JSONArray receivedOperations = operationResult, receivedTorrents = torrentResult;
          final String operationError = operationFailure;
          runOnUiThread(() -> {
            torrentsLoading = false;
            if (requested != api || isDestroyed()) return;
            if (receivedOperations != null) libraryOperations = receivedOperations;
            if (receivedAttention != null) attention = receivedAttention;
            if (receivedJobs != null) cloudJobs = receivedJobs;
            if (receivedWatch != null && receivedWatch.has("shows")) watchList = receivedWatch;
            if (receivedStorage != null) storage = receivedStorage;
            libraryOperationsError = operationError;
            torrentStale = receivedTorrents == null;
            if (receivedTorrents != null) {
              torrents = visibleTorrents(receivedTorrents);
              torrentUpdated = System.currentTimeMillis();
            } else if (torrents == null) torrents = new JSONArray();
            if (section.equals("Torrents")) {
              View focus = getCurrentFocus();
              Object tag = focus == null ? null : focus.getTag();
              String button = focus instanceof Button ? ((Button) focus).getText().toString() : "";
              render();
              View restore = tag == null ? null : root.findViewWithTag(tag);
              if (restore != null) restore.requestFocus();
              else if (!button.isEmpty()) restoreButton(root, button);
            }
          });
        });
  }

  private boolean restoreButton(View view, String label) {
    if (view instanceof Button && ((Button) view).getText().toString().equals(label)) {
      view.requestFocus();
      return true;
    }
    if (view instanceof android.view.ViewGroup) {
      android.view.ViewGroup group = (android.view.ViewGroup) view;
      for (int i = 0; i < group.getChildCount(); i++)
        if (restoreButton(group.getChildAt(i), label)) return true;
    }
    return false;
  }

  @SuppressWarnings("unchecked")
  private void onCatalogUpdate(Runnable binding) {
    if(root.getTag() instanceof List)((List<Runnable>)root.getTag()).add(binding);
  }
  @SuppressWarnings("unchecked")
  private void updateBrowse(LinearLayout view) {
    if(view==null)return;
    if(view.getTag() instanceof List)for(Runnable binding:new ArrayList<>((List<Runnable>)view.getTag()))binding.run();
    updatePosters(view);
  }
  private List<Library.Title> shelfTitles(String name) {
    if(name.equals("Continue watching"))return library.continuing();
    if(name.equals("Recently watched"))return finishedTitles();
    if(name.equals("Recently added"))return library.filter("","","",0);
    if(name.equals("Movies"))return library.filter("movie","","",0);
    if(name.equals("TV Shows"))return library.filter("tv","","",0);
    return library.filter("","",name,0);
  }

  private void updatePosters(View view) {
    if(view instanceof PosterCard && view.getTag() instanceof String) {
      String tag=(String)view.getTag();
      Library.Title title=library.find(tag.substring("title:".length()));
      if(title!=null)((PosterCard)view).bind(title);
    }
    if(view instanceof ViewGroup) {
      ViewGroup group=(ViewGroup)view;
      for(int i=0;i<group.getChildCount();i++)updatePosters(group.getChildAt(i));
    }
  }

  private boolean sameDetail(Library.Title a,Library.Title b) {
    if(a==null || b==null)return a==b;
    if(!Objects.toString(a.cast,"").equals(Objects.toString(b.cast,""))||!a.title.equals(b.title)||!a.summary.equals(b.summary)||!a.poster.equals(b.poster)||!a.backdrop.equals(b.backdrop)||!a.metadata().equals(b.metadata())||a.assets.size()!=b.assets.size())return false;
    for(int i=0;i<a.assets.size();i++) {
      Library.Asset x=a.assets.get(i),y=b.assets.get(i);
      if(!x.id.equals(y.id)||!x.name.equals(y.name)||!x.summary.equals(y.summary)||x.position!=y.position||x.duration!=y.duration||x.watched!=y.watched||x.complete!=y.complete||!Objects.toString(x.mediaInfo,"").equals(Objects.toString(y.mediaInfo,"")))return false;
    }
    return true;
  }

  private void refreshLibrary() {
    if (refreshingLibrary || loading || selectingProfile) return;
    refreshingLibrary = true;
    Api requested = api;
    int generation = connectionGeneration;
    io.execute(
        () -> {
          try {
            if (!requested.libraryChanged()) {
              runOnUiThread(() -> refreshingLibrary = false);
              return;
            }
            JSONObject response = requested.request("GET", "/v1/library", null);
            String snapshot = requested.libraryRaw;
            new BrowseCache(this, requested).write("library", snapshot.getBytes(java.nio.charset.StandardCharsets.UTF_8));
            Library updated = Library.parse(response);
            runOnUiThread(
                () -> {
                  refreshingLibrary = false;
                  if (isDestroyed()
                      || requested != api
                      || !inLibrary
                      || generation != connectionGeneration
                      || !getWindow().getDecorView().hasWindowFocus()) return;
                  if (snapshot.equals(librarySnapshot)) return;
                  librarySnapshot = snapshot;
                  View focus = getCurrentFocus();
                  Object tag = focus == null ? null : focus.getTag();
                  String label =
                      focus instanceof Button ? ((Button) focus).getText().toString() : "";
                  Library.Title oldDetail=library.find(detailId),newDetail=updated.find(detailId);
                  library = updated;
                  if (detailId.isEmpty()) {
                    updateBrowse(root);
                    updateBrowse(savedBrowseRoot);
                    return;
                  }
                  updateBrowse(savedBrowseRoot);
                  if (showingCast) return;
                  if (sameDetail(oldDetail,newDetail)) return;
                  int scrollY=content.getChildCount()>0?content.getChildAt(0).getScrollY():0;
                  content.removeAllViews();
                  if(newDetail!=null) {
                    details(newDetail);
                    wireNavigation();
                    content.getChildAt(0).post(() -> content.getChildAt(0).scrollTo(0,scrollY));
                  } else { detailId=""; render(); }
                  View restore = tag == null ? null : root.findViewWithTag(tag);
                  if (restore != null) restore.post(restore::requestFocus);
                  else if (!label.isEmpty()) restoreButton(root, label);
                });
          } catch (Exception e) {
            runOnUiThread(() -> {
              refreshingLibrary = false;
            });
          }
        });
  }

  private void open(Library.Title t) {
    Library.Title current = library.find(t.id);
    if (current == null) { render(); return; }
    t = current;
    if (detailId.isEmpty()) {
      android.view.ViewGroup host = findViewById(android.R.id.content);
      savedBrowseView = host.getChildAt(0);
      savedBrowseFocus = getCurrentFocus();
      savedBrowseRoot = root; savedBrowseContent = content; savedBrowseNav = nav;
      savedBrowseApi = api;
      savedBrowseSnapshot = librarySnapshot;
    }
    restoreId = t.id;
    detailId = t.id;
    season = -1;
    transition = 1;
    render();
  }

  private void mediaLanguages(LinearLayout parent, Library.Asset asset, boolean compact) {
    if (asset == null) return;
    for (String line : MediaLanguages.lines(asset.mediaInfo)) {
      Ui.gap(parent, compact ? 4 : 7);
      TextView text = Ui.text(this, line, compact ? 12 : 14, Ui.MUTED);
      android.text.SpannableString styled = new android.text.SpannableString(line);
      for (int i = 0; i < line.length(); i++) if (line.charAt(i) == '\uFFFC') {
        android.graphics.drawable.Drawable icon = getDrawable(R.drawable.torrbaby_mark);
        int size = Ui.dp(this, compact ? 14 : 18);
        icon.setBounds(0, 0, size, size);
        styled.setSpan(new android.text.style.ImageSpan(icon, android.text.style.ImageSpan.ALIGN_BASELINE), i, i + 1, android.text.Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
      }
      text.setText(styled);
      text.setContentDescription(line.replace("\uFFFC", "generated subtitles"));
      Ui.add(parent, text, -1, -2);
    }
  }

  private String playbackLabel(Library.Title title, Library.Asset asset) {
    if (asset == null) return "▶  Start";
    boolean resume = asset.position > 0 && !asset.watched;
    int minutes = resume && asset.duration > asset.position ? (int) Math.ceil((asset.duration - asset.position) / 60) : 0;
    if (!title.kind.equals("tv") || asset.episode <= 0) {
      if (minutes <= 0) return L10n.t(resume ? "▶  Resume" : "▶  Play");
      return "▶  " + (minutes >= 60
          ? String.format(L10n.locale(), L10n.t("Resume (%d hr %d min left)"), minutes / 60, minutes % 60)
          : String.format(L10n.locale(), L10n.t("Resume (%d min left)"), minutes));
    }
    String key = !resume ? "Start Season %d Ep %d" : minutes > 0 ? "Resume Season %d Ep %d (%d min remaining)" : "Resume Season %d Ep %d";
    return "▶  " + String.format(L10n.locale(), L10n.t(key), asset.season, asset.episode, minutes);
  }

  private void castPage(Library.Title title) {
    LinearLayout heading = Ui.row(this);
    Ui.pad(heading, 36, 18);
    Ui.add(heading, Ui.title(this, "Cast · " + title.title, 26), -1, -2);
    Ui.add(content, heading, -1, -2);
    // Bound the data as well as the view count. ListView recycles its visible rows.
    ArrayList<String[]> rows = new ArrayList<>();
    outer: for (int i = 0; title.cast != null && i < title.cast.length(); i++) {
      JSONObject person = title.cast.optJSONObject(i);
      if (person == null) continue;
      JSONArray roles = person.optJSONArray("roles");
      if (roles != null && roles.length() > 0) {
        for (int j = 0; j < roles.length(); j++) {
          JSONObject role = roles.optJSONObject(j);
          if (role == null) continue;
          rows.add(new String[]{Library.string(person,"name"), Library.string(role,"character"), Library.string(role,"episode_count")});
          if (rows.size() > 150) break outer;
        }
      } else {
        rows.add(new String[]{Library.string(person,"name"), Library.string(person,"character"), Library.string(person,"total_episode_count")});
        if (rows.size() > 150) break;
      }
    }
    boolean truncated = rows.size() > 150;
    if (truncated) rows.remove(150);
    LinearLayout columns = Ui.row(this);
    Ui.pad(columns, 50, 10);
    columns.addView(Ui.text(this, L10n.t("Actor"), 14, Ui.MUTED), new LinearLayout.LayoutParams(0, -2, 1));
    columns.addView(Ui.text(this, L10n.t("Character"), 14, Ui.MUTED), new LinearLayout.LayoutParams(0, -2, 1));
    TextView episodes = Ui.text(this, title.kind.equals("tv") ? L10n.t("Episodes") : "", 14, Ui.MUTED);
    episodes.setGravity(Gravity.END);
    Ui.add(columns, episodes, 90, -2);
    Ui.add(content, columns, -1, -2);
    if (truncated) {
      TextView note = Ui.text(this, L10n.t("Showing the first 150 cast entries."), 13, Ui.MUTED);
      Ui.pad(note, 50, 8); Ui.add(content, note, -1, -2);
    }
    ListView list = new ListView(this);
    list.setDividerHeight(Ui.dp(this, 6));
    list.setSelector(Ui.shape(0xff333333, Ui.ACCENT, this));
    list.setPadding(Ui.dp(this, 36), 0, Ui.dp(this, 36), Ui.dp(this, 24));
    list.setAdapter(new BaseAdapter() {
      public int getCount() { return rows.size(); }
      public Object getItem(int position) { return rows.get(position); }
      public long getItemId(int position) { return position; }
      public View getView(int position, View recycled, ViewGroup parent) {
        LinearLayout row;
        if (recycled instanceof LinearLayout) row = (LinearLayout)recycled;
        else {
          row = Ui.row(MainActivity.this);
          Ui.pad(row, 14, 14);
          row.setMinimumHeight(Ui.dp(MainActivity.this, 60));
          for (int col = 0; col < 3; col++) {
            TextView text = Ui.text(MainActivity.this, "", 18, col == 0 ? Ui.TEXT : Ui.MUTED);
            text.setMaxLines(3); text.setEllipsize(android.text.TextUtils.TruncateAt.END);
            row.addView(text, col < 2 ? new LinearLayout.LayoutParams(0,-2,1) : new LinearLayout.LayoutParams(Ui.dp(MainActivity.this,90),-2));
            if (col == 2) text.setGravity(Gravity.END);
          }
        }
        String[] entry = rows.get(position);
        for (int col = 0; col < 3; col++) ((TextView)row.getChildAt(col)).setText(col == 2 && !title.kind.equals("tv") ? "" : entry[col]);
        return row;
      }
    });
    content.addView(list, new LinearLayout.LayoutParams(-1, 0, 1));
    list.requestFocus();
  }

  private void details(Library.Title t) {
    ambient(t);
    ScrollView sc = scroll(content);
    LinearLayout page = Ui.column(this);
    sc.addView(page);
    hero(page, t, true);
    LinearLayout body = Ui.column(this);
    Ui.pad(body, 45, 22);
    Ui.add(page, body, -1, -2);
    // What the title is about comes first; the file's technical details follow.
    if (!t.summary.isEmpty()) {
      TextView summary = Ui.text(this, t.summary, 14, Ui.TEXT);
      summary.setLineSpacing(Ui.dp(this, 3), 1);
      if (t.kind.equals("tv")) { summary.setMaxLines(3); summary.setEllipsize(android.text.TextUtils.TruncateAt.END); }
      Ui.add(body, summary, 650, -2);
      Ui.gap(body, 12);
    }
    mediaLanguages(body, t.preferred(), false);
    Ui.gap(body, 24);
    if(!t.kind.equals("tv"))detailActions(body,t);
    LinearLayout episodes=Ui.column(this);
    if (t.kind.equals("tv")) {
      TreeSet<Integer> seasons = new TreeSet<>();
      for (Library.Asset a : t.assets) if (!a.extra() && a.hasSeason) seasons.add(a.season);
      if (season == -1 && !seasons.isEmpty())
        season =
            t.preferred() == null || !t.preferred().hasSeason
                ? (seasons.higher(0) == null ? seasons.first() : seasons.higher(0))
                : t.preferred().season;
      // Extras are their own tab after the last season, not a tail on every season.
      for (Library.Asset a : t.assets) if (a.extra()) { seasons.add(EXTRAS); break; }
      if (season == EXTRAS && !seasons.contains(EXTRAS)) season = seasons.isEmpty() ? -1 : seasons.first();
      HorizontalScrollView scroll = new TvHorizontalScrollView(this);
      scroll.setHorizontalScrollBarEnabled(false);
      LinearLayout row = Ui.row(this);
      row.setTag("seasons");
      scroll.addView(row);
      // A single season needs no selector; the guide starts directly below the synopsis.
      if (seasons.size() < 2) scroll.setVisibility(View.GONE);
      for (int number : seasons) {
        Button button =
            Ui.button(
                this,
                number == EXTRAS ? L10n.t("Extras") : number == 0 ? L10n.t("Specials") : "Season " + number,
                () -> {
                  season = number;
                  for(int i=0;i<row.getChildCount();i++) {
                    View child=row.getChildAt(i);
                    if(child instanceof Button)Ui.tab((Button)child,("season:"+number).equals(child.getTag()));
                  }
                  episodes.removeAllViews();
                  detailEpisodes(episodes,t);
                  episodeEntry(page, episodes, t);
                });
        Ui.tab(button,number == season);
        button.setTag("season:" + number);
        button.setId(View.generateViewId());
        Ui.add(row, button, -2, 42);
        Ui.gap(row, 12);
      }
      // Left of the first tab and right of the last go nowhere, rather than up to the language switch.
      if (row.getChildCount() > 0) {
        View first = row.getChildAt(0), last = row.getChildAt(row.getChildCount() - 2);
        first.setNextFocusLeftId(first.getId());
        last.setNextFocusRightId(last.getId());
      }
      Ui.add(body, scroll, -1, seasons.size() < 2 ? 0 : 46);
      if (seasons.size() > 1) Ui.gap(body, 20);
    }
    Ui.add(body,episodes,-1,-2);
    detailEpisodes(episodes,t);
    if(t.kind.equals("tv"))episodeEntry(page, episodes, t);
    // No on-screen Back: every remote has one.
    Ui.gap(body, 12);
  }

  /** Moving down from the hero or season selector lands on the next episode to watch. */
  private void episodeEntry(View page, LinearLayout episodes, Library.Title t) {
    Library.Asset preferred = t.preferred();
    View entry = preferred == null ? null : episodes.findViewWithTag("episode:" + preferred.id);
    if (entry == null) entry = firstFocusable(episodes);
    if (entry == null || entry == episodes) return;
    if (entry.getId() == View.NO_ID) entry.setId(View.generateViewId());
    View seasons = page.findViewWithTag("seasons");
    boolean selector = seasons instanceof LinearLayout && ((View) seasons.getParent()).getVisibility() == View.VISIBLE;
    View hero = page.findViewWithTag("hero");
    if (hero != null && hero.getId() == View.NO_ID) hero.setId(View.generateViewId());
    // The first row leads back up to where the list was entered from.
    View firstRow = firstFocusable(episodes);
    View above = hero;
    if (selector) {
      LinearLayout row = (LinearLayout) seasons;
      for (int i = 0; i < row.getChildCount(); i++)
        if (row.getChildAt(i) instanceof Button) {
          row.getChildAt(i).setNextFocusDownId(entry.getId());
          if (hero != null) row.getChildAt(i).setNextFocusUpId(hero.getId());
        }
      // From the hero or the episodes, arrive on the season being shown rather than the nearest tab.
      entry = row.findViewWithTag("season:" + season);
      if (entry == null) return;
      if (entry.getId() == View.NO_ID) entry.setId(View.generateViewId());
      above = entry;
    }
    if (firstRow != null && firstRow != episodes && above != null) firstRow.setNextFocusUpId(above.getId());
    for (String tag : new String[] {"hero", "more"}) {
      View button = page.findViewWithTag(tag);
      if (button != null) button.setNextFocusDownId(entry.getId());
    }
  }

  private void moreActions(Library.Title t) {
    Library.Asset preferred = t.preferred();
    if (preferred == null) return;
    List<String> labels = new ArrayList<>();
    List<Runnable> actions = new ArrayList<>();
    if (t.cast != null && t.cast.length() > 0) { labels.add(L10n.t("Cast")); actions.add(() -> { showingCast = true; render(); }); }
    if (preferred.position > 0) { labels.add(L10n.t("Start from Beginning")); actions.add(() -> play(t, preferred, true)); }
    labels.add(L10n.t(preferred.watched ? "Mark as Unwatched" : "Mark as Watched")); actions.add(() -> mark(preferred));
    if (preferred.complete) { labels.add(L10n.t("Manage files")); actions.add(() -> new LibraryFileActions(this, api, io, preferred.id, this::load).show()); }
    if (t.isContinuing()) { labels.add(L10n.t("Remove from Continue Watching")); actions.add(() -> setContinueWatchingHidden(t, true)); }
    new AlertDialog.Builder(this).setTitle(t.title)
        .setItems(labels.toArray(new String[0]), (dialog, which) -> actions.get(which).run())
        .setNegativeButton(L10n.t("Cancel"), null).show();
  }

  private void detailActions(LinearLayout body, Library.Title t) {
    Library.Asset preferred=t.preferred();
    if(preferred!=null) {
      if(t.cast!=null && t.cast.length()>0) {
        Button cast=Ui.iconButton(this,L10n.t("Cast"),R.drawable.action_cast,()->{showingCast=true;render();});cast.setTag("cast");
        Ui.add(body,cast,-2,44);
      }
      if(preferred.position>0)Ui.add(body,Ui.iconButton(this,L10n.t("Start from Beginning"),R.drawable.action_restart,()->play(t,preferred,true)),-2,44);
      Ui.add(body,Ui.iconButton(this,L10n.t(preferred.watched?"Mark as Unwatched":"Mark as Watched"),R.drawable.action_watched,()->mark(preferred)),-2,44);
      if(preferred.complete)Ui.add(body,Ui.iconButton(this,L10n.t("Manage files"),R.drawable.action_files,()->new LibraryFileActions(this,api,io,preferred.id,this::load).show()),-2,44);
      if(t.isContinuing())Ui.add(body,Ui.iconButton(this,L10n.t("Remove from Continue Watching"),R.drawable.action_remove,()->setContinueWatchingHidden(t,true)),-2,44);
      Ui.gap(body,16);
    }
  }

  private void detailEpisodes(LinearLayout body, Library.Title t) {
    if (t.kind.equals("tv") && season == EXTRAS) {
      for (Library.Asset a : t.assets) if (a.extra()) episode(body, t, a);
      return;
    }
    long mainCount = t.assets.stream().filter(a -> !a.extra()).count();
    if (t.kind.equals("tv") || mainCount > 1)
      for (Library.Asset a : t.assets)
        if (!a.extra()
            && (!t.kind.equals("tv") || (a.hasSeason ? a.season == season : season == -1)))
          episode(body, t, a);
    if (t.kind.equals("tv") && season != -1) {
      boolean heading = false;
      for (Library.Asset a : t.assets)
        if (!a.extra() && !a.hasSeason) {
          if (!heading) {
            Ui.add(body, Ui.title(this, L10n.t("Other episodes"), 18), -1, 36);
            heading = true;
          }
          episode(body, t, a);
        }
    }
    boolean extras = false;
    if (!t.kind.equals("tv")) for (Library.Asset a : t.assets)
      if (a.extra()) {
        if (!extras) {
          Ui.gap(body, 16);
          Ui.add(body, Ui.title(this, L10n.t("Special features"), 18), -1, 36);
          extras = true;
        }
        episode(body, t, a);
      }
  }

  private void episode(LinearLayout body, Library.Title t, Library.Asset a) {
    LinearLayout row = Ui.row(this);
    Ui.focus(row);
    row.setTag("episode:" + a.id);
    row.setOnFocusChangeListener((v, focused) -> Ui.secondaryContrast(v, focused));
    Ui.pad(row, 14, 14);
    if (!a.thumbnail.isEmpty()) {
      ImageView image = new ImageView(this);
      image.setScaleType(ImageView.ScaleType.CENTER_CROP);
      Ui.round(image, 5);
      Ui.add(row, image, 125, 70);
      art.load(image, a.id, "episode", a.thumbnail);
      Ui.gap(row, 14);
    }
    TextView playIcon = Ui.title(this, "▶", 18);
    playIcon.setGravity(Gravity.CENTER);
    playIcon.setTextColor(Ui.BG);
    playIcon.setBackground(Ui.shape(Ui.TEXT, 0, this));
    Ui.round(playIcon, 18);
    Ui.add(row, playIcon, 36, 36);
    Ui.gap(row, 14);
    LinearLayout text = Ui.column(this);
    if (a.episode > 0) {
      Ui.add(text, Ui.text(this, "Episode " + a.episode, 12, Ui.MUTED), -1, -2);
      Ui.gap(text, 5);
    }
    TextView name = Ui.title(this, a.name, 18);
    name.setMaxLines(2);
    Ui.add(text, name, -1, -2);
    if (!a.summary.isEmpty()) {
      Ui.gap(text, 6);
      TextView summary = Ui.text(this, a.summary, 13, Ui.MUTED);
      summary.setMaxLines(2);
      summary.setEllipsize(android.text.TextUtils.TruncateAt.END);
      Ui.add(text, summary, -1, -2);
    }
    if (!a.complete || a.watched || a.position > 0) {
      Ui.gap(text, 6);
      Ui.add(text, Ui.text(this, a.status(), 12, Ui.MUTED), -1, -2);
    }
    if (a.position > 0 && a.duration > 0) {
      ProgressBar bar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
      bar.setProgressTintList(android.content.res.ColorStateList.valueOf(Ui.ACCENT));
      bar.setProgress((int) (100 * a.position / a.duration));
      Ui.add(text, bar, -1, 4);
    }
    row.addView(text, new LinearLayout.LayoutParams(0, -2, 1));
    Ui.gap(row, 12);
    row.setGravity(Gravity.CENTER_VERTICAL);

    row.setOnClickListener(
        v -> {
          if (a.complete) play(t, a, false);
          else error("This episode is not yet available on the server.");
        });
    row.setOnLongClickListener(
        v -> {
          assetOptions(t, a);
          return true;
        });
    row.setOnKeyListener(
        (v, key, event) -> {
          if (key == KeyEvent.KEYCODE_MENU && event.getAction() == KeyEvent.ACTION_UP) {
            assetOptions(t, a);
            return true;
          }
          return false;
        });
    Ui.add(body, row, -1, -2);
    Ui.gap(body, 18);
  }

  private void assetOptions(Library.Title t, Library.Asset a) {
    if (a == null) {
      new AlertDialog.Builder(this)
          .setTitle(L10n.t("Not yet available"))
          .setMessage(L10n.t("This title has no complete media files on the server."))
          .setPositiveButton(L10n.t("OK"), null)
          .show();
      return;
    }
    List<String> labels = new ArrayList<>();
    if (a.complete) {
      labels.add(a.position > 0 && !a.watched ? "Resume" : "Play");
      if (a.position > 0) labels.add("Start from Beginning");
    }
    labels.add(a.watched ? "Mark as Unwatched" : "Mark as Watched");
    if (a.complete) labels.add("Manage Files");
    new AlertDialog.Builder(this)
        .setTitle(a.name)
        .setItems(
            L10n.labels(labels.toArray(new String[0])),
            (d, w) -> {
              String action = labels.get(w);
              if (action.startsWith("Mark")) mark(a);
              else if (action.equals("Manage Files"))
                new LibraryFileActions(this, api, io, a.id, this::load).show();
              else play(t, a, action.equals("Start from Beginning"));
            })
        .show();
  }

  private void mark(Library.Asset a) {
    io.execute(
        () -> {
          try {
            api.request(
                "POST",
                "/v1/library/assets/" + Api.part(a.id) + "/watched",
                new JSONObject().put("watched", !a.watched));
            runOnUiThread(this::load);
          } catch (Exception e) {
            runOnUiThread(() -> error(message(e)));
          }
        });
  }

  private void play(Library.Title t, Library.Asset a, boolean beginning) {
    if (!a.complete) {
      error("This media file is not yet available on the server.");
      return;
    }
    played = true;
    Intent i = new Intent(this, PlaybackActivity.class);
    i.putExtra("asset", a.id);
    i.putExtra("title_id", t.id);
    i.putExtra("title", t.title);
    i.putExtra("name", a.label());
    i.putExtra("beginning", beginning);
    ArrayList<String> ids = new ArrayList<>(), names = new ArrayList<>();
    Library.Asset next = t.next(a.id);
    Set<String> queued = new HashSet<>();
    while (next != null && queued.add(next.id)) {
      ids.add(next.id);
      names.add(next.label());
      next = t.next(next.id);
    }
    i.putStringArrayListExtra("queue_ids", ids);
    i.putStringArrayListExtra("queue_names", names);
    startActivity(i);
  }

  private void chooseProfile(boolean switching) {
    selectingProfile = true;
    final Api requested = api;
    base();
    Ui.add(root, Ui.title(this, L10n.t("Who's watching?"), 30), -1, 56);
    ProgressBar spinner = new ProgressBar(this);
    Ui.add(root, spinner, 44, 44);
    io.execute(() -> {
      try {
        requested.register();
        JSONObject state = requested.request("GET", "/v1/devices/" + Api.part(requested.client) + "/profile", null);
        JSONArray people = state.getJSONArray("profiles");
        String selected = Library.string(state, "profile_id");
        if (!switching && !selected.isEmpty()) {
          for (int n = 0; n < people.length(); n++) {
            JSONObject person = people.getJSONObject(n);
            if (selected.equals(person.getString("id"))) {
              activateProfile(requested, person, false);
              return;
            }
          }
          throw new Exception("Your selected profile is unavailable. Choose a user to continue.");
        }
        runOnUiThread(() -> {
          if (isDestroyed() || api != requested) return;
          base();
          Ui.add(root, Ui.title(this, L10n.t("Who's watching?"), 30), -1, 56);
          ScrollView scroll = new ScrollView(this);
          LinearLayout list = Ui.column(this);
          scroll.addView(list);
          root.addView(scroll, new LinearLayout.LayoutParams(-1, 0, 1));
          for (int n = 0; n < people.length(); n++) {
            JSONObject person = people.optJSONObject(n);
            if (person == null) continue;
            LinearLayout card = Ui.row(this);card.setGravity(Gravity.CENTER_VERTICAL);
            LinearLayout button = Ui.row(this);button.setGravity(Gravity.CENTER_VERTICAL);Ui.focus(button);Ui.pad(button,16,12);
            button.addView(ProfileUi.avatar(this,person,64));Ui.gap(button,20);
            TextView label = Ui.title(this,person.optString("name"),22);
            button.addView(label,new LinearLayout.LayoutParams(0,-2,1));
            if(requested.profile.equals(person.optString("id")))Ui.add(button,Ui.text(this,"✓",22,Ui.ACCENT),36,-2);
            button.setOnClickListener(v -> {
              if (switchingProfile || api != requested) return;
              switchingProfile = true;showProfileSwitchProgress();
              io.execute(() -> activateProfile(requested, person, true));
            });
            card.addView(button,new LinearLayout.LayoutParams(0,-2,1));Ui.gap(card,12);
            Button edit = Ui.button(this,"✎",()->editProfile(requested,person));edit.setContentDescription(L10n.t("Edit profile"));Ui.add(card,edit,48,48);
            Ui.add(list,card,600,-2);Ui.gap(list,16);
            if (n == 0) button.requestFocus();
          }
          Ui.add(list, Ui.button(this, L10n.t("Create user"), () -> createProfile(requested)), 420, 56);
          if (!requested.profile.isEmpty()) {
            Ui.gap(list, 12);
            Ui.add(list, Ui.button(this, L10n.t("Back to library"), () -> { selectingProfile = false; render(); }), 420, 56);
          }
        });
      } catch (Exception e) {
        runOnUiThread(() -> {
          if (isDestroyed() || api != requested) return;
          base();
          Ui.add(root, Ui.title(this, L10n.t("Could not load users"), 28), -1, 56);
          Ui.add(root, Ui.text(this, message(e), 16, Ui.MUTED), -1, -2);
          Button retry = Ui.button(this, L10n.t("Try again"), () -> chooseProfile(switching));
          Ui.add(root, retry, 260, 56);
          retry.requestFocus();
        });
      }
    });
  }

  private void createProfile(Api requested) { editProfile(requested,null); }
  private void editProfile(Api requested,JSONObject prior) {
    if(switchingProfile || api!=requested)return;
    ProfileUi.editor(this,prior,(values,dialog)->{
      if(switchingProfile || api!=requested)return;
      switchingProfile=true;dialog.getButton(AlertDialog.BUTTON_POSITIVE).setEnabled(false);
      io.execute(()->{
        try {
          JSONObject person=requested.request(prior==null?"POST":"PATCH",prior==null?"/v1/profiles":"/v1/profiles/"+Api.part(prior.optString("id")),values);
          if(prior==null){runOnUiThread(dialog::dismiss);activateProfile(requested,person,true);}
          else runOnUiThread(()->{
            switchingProfile=false;if(api!=requested)return;
            if(person.optString("id").equals(api.profile)){
              api.profileAppearance=person;try{store.save(api);}catch(Exception e){error(message(e));return;}
            }
            dialog.dismiss();chooseProfile(true);
          });
        } catch(Exception e){runOnUiThread(()->{switchingProfile=false;dialog.getButton(AlertDialog.BUTTON_POSITIVE).setEnabled(true);error(message(e));});}
      });
    });
  }

  // Replace the immutable connection instead of changing a profile underneath in-flight work.
  private void showProfileSwitchProgress() {
    base();
    Ui.add(root, Ui.title(this, L10n.t("Switching user…"), 30), -1, 56);
    Ui.add(root, new ProgressBar(this), 44, 44);
  }

  private void activateProfile(Api requested, JSONObject person, boolean persist) {
    try {
      String id = person.getString("id"), name = person.getString("name");
      if (persist) requested.request("PUT", "/v1/devices/" + Api.part(requested.client) + "/profile",
          new JSONObject().put("profile_id", id));
      Api selected = new Api(requested.base, requested.token, requested.client, id, name);
      selected.profileAppearance = person;
      runOnUiThread(() -> {
        switchingProfile = false;
        if (isDestroyed() || api != requested) return;
        try {
          store.save(selected);
          api = selected;
          if (art != null) art.close();
          art = new Art(this, selected);
          library = null;
          librarySnapshot = "";
          savedBrowseView = savedBrowseFocus = null;
          savedBrowseRoot = savedBrowseContent = savedBrowseNav = null;
          savedBrowseApi = null;
          detailId = collectionName = restoreId = query = genre = restoreShelf = "";
          collectionIds.clear();
          section = "Home";
          season = -1;
          selectingProfile = false;
          loading = refreshingLibrary = false;
          load();
        } catch (Exception e) { chooseProfile(true); error(message(e)); }
      });
    } catch (Exception e) { runOnUiThread(() -> {
      switchingProfile = false;
      if (isDestroyed() || api != requested) return;
      chooseProfile(true);
      error(message(e));
    }); }
  }

  /** Settings in the same categories as the Apple apps: User, General, Playback, Connection, About. */
  private void settings() {
    String[] pages = {"User", "General", "Playback", "Connection", "About"};
    new AlertDialog.Builder(this)
        // The build is what Settings is most often opened for, so it sits in the title.
        .setTitle(L10n.t("Settings") + " · " + BuildConfig.VERSION_NAME + " (" + BuildConfig.VERSION_CODE + ")")
        .setItems(L10n.labels(pages), (d, w) -> {
          if (w == 0) settingsPage("User", new String[] {"Switch user"}, i -> { if (api != null) chooseProfile(true); });
          else if (w == 1) settingsPage("General", new String[] {"Follow system language", "Refresh library"}, i -> {
            if (i == 0) L10n.system(this); else load();
          });
          else if (w == 2) settingsPage("Playback", new String[] {"Default subtitles"}, i -> Subtitles.edit(this, api, io, null));
          else if (w == 3) settingsPage("Connection", new String[] {"Change server connection", "Disconnect this TV"}, i -> {
            if (i == 0) discover(""); else disconnect();
          });
          else new AlertDialog.Builder(this)
              .setTitle(L10n.t("About"))
              .setMessage(L10n.t("Version") + " " + BuildConfig.VERSION_NAME + " (" + BuildConfig.VERSION_CODE + ")")
              .setNeutralButton(L10n.t("Startup diagnostics"), (x, y) -> new AlertDialog.Builder(this)
                  .setTitle(L10n.t("Startup timings")).setMessage(StartupTrace.text()).setPositiveButton(L10n.t("Close"), null).show())
              .setPositiveButton(L10n.t("Close"), null)
              .show();
        })
        .show();
  }

  private void settingsPage(String title, String[] items, java.util.function.IntConsumer action) {
    new AlertDialog.Builder(this)
        .setTitle(L10n.t(title))
        .setItems(L10n.labels(items), (d, w) -> action.accept(w))
        .setNegativeButton(L10n.t("Back"), (d, w) -> settings())
        // The remote's Back key returns to the categories, like the Back button.
        .setOnCancelListener(d -> settings())
        .show();
  }

  private void disconnect() {
    new AlertDialog.Builder(this)
        .setTitle(L10n.t("Disconnect this TV?"))
        .setMessage(L10n.t("Your library and viewing history stay on the server."))
        .setNegativeButton(L10n.t("Cancel"), null)
        .setPositiveButton(
            L10n.t("Disconnect"),
            (x, y) -> {
              store.clear();
              api = null;
              library = null;
              if (art != null) {
                art.close();
                art = null;
              }
              discover("");
            })
        .show();
  }

  private void stopDiscovery() {
    discoveryHandler.removeCallbacksAndMessages(null);
    if (discovery != null) {
      discovery.close();
      discovery = null;
    }
  }

  @Override
  protected void onStop() {
    refreshHandler.removeCallbacksAndMessages(null);
    stopDiscovery();
    super.onStop();
  }

  @Override
  protected void onStart() {
    super.onStart();
    refreshHandler.removeCallbacksAndMessages(null);
    refreshHandler.postDelayed(refreshTick, 5000);
    if (discoveringScreen && !connectingLan && discovery == null) discover("");
  }

  private TextView connectionScreen() {
    root.removeAllViews(); // Startup has only the brand and spinner, not library navigation.
    root.setGravity(Gravity.CENTER);
    ImageView logo = new ImageView(this);
    logo.setImageResource(R.drawable.torrbaby_mark);
    logo.setContentDescription("Torrbaby");
    Ui.add(root, logo, 160, 160);
    Ui.gap(root, 24);
    TextView label = Ui.title(this, "", 24);
    label.setLineSpacing(Ui.dp(this, 12), 1);
    label.setGravity(Gravity.CENTER);
    Ui.add(root, label, -1, -2);
    Ui.gap(root, 24);
    ProgressBar spinner = new ProgressBar(this);
    spinner.setTag("connection_spinner");
    spinner.setIndeterminateTintList(android.content.res.ColorStateList.valueOf(Ui.ACCENT));
    Ui.add(root, spinner, 28, 28);
    return label;
  }

  private void connectionFailed(TextView label, String reason) {
    View spinner = root.findViewWithTag("connection_spinner");
    if (spinner != null) spinner.setVisibility(View.GONE);
    label.setText(reason);
    Ui.gap(root, 24);
    Button retry = Ui.button(this, L10n.t("Try again"), () -> discover(""));
    Ui.add(root, retry, 200, 44);
    Ui.gap(root, 12);
    Ui.add(root, Ui.button(this, L10n.t("Enter server address"), () -> setup("")), 230, 44);
    retry.requestFocus();
  }

  private void discover(String note) {
    base();
    discoveringScreen = true;
    int generation = connectionGeneration;
    TextView message = connectionScreen();
    LinearLayout choices = Ui.column(this);
    Ui.add(root, choices, 400, -2);
    discovery = new ServerDiscovery(this, new ServerDiscovery.Listener() {
      public void failed() {
        if (generation != connectionGeneration) return;
        stopDiscovery();
        connectionFailed(message, "Could not find your server");
      }
      public void changed(Map<String, String> servers) {
        if (generation != connectionGeneration || connectingLan || servers.isEmpty()) return;
        StartupTrace.mark("Server discovered");
        discoveryHandler.removeCallbacksAndMessages(null);
        choices.removeAllViews();
        if (servers.size() == 1) {
          Map.Entry<String, String> server = servers.entrySet().iterator().next();

          discoveryHandler.postDelayed(() -> connectLan(server.getValue(), message, generation), 700);
        } else {
          root.findViewWithTag("connection_spinner").setVisibility(View.GONE);
          message.setText(L10n.t("Choose your server"));
          for (Map.Entry<String, String> server : servers.entrySet()) {
            Ui.gap(choices, 12);
            Ui.add(choices, Ui.button(MainActivity.this, server.getKey(), () -> {
              choices.setVisibility(View.GONE);

              root.findViewWithTag("connection_spinner").setVisibility(View.VISIBLE);
              connectLan(server.getValue(), message, generation);
            }), -1, 44);
          }
          firstFocusable(choices).requestFocus();
        }
      }
    });
    StartupTrace.mark("Discovery started");
    discovery.start();
    discoveryHandler.postDelayed(() -> {
      if (generation == connectionGeneration && !connectingLan) {
        stopDiscovery();
        connectionFailed(message, "No server found");
      }
    }, 10000);
  }

  private void connectLan(String address, TextView message, int generation) {
    if (generation != connectionGeneration || connectingLan) return;
    connectingLan = true;
    stopDiscovery();

    io.execute(
        () -> {
          try {
            Api publicApi = new Api(address, "", "");
            JSONObject credential =
                publicApi.request(
                    "POST",
                    "/v1/lan/register",
                    new JSONObject().put("name", Build.MODEL).put("platform", "other"));
            StartupTrace.mark("LAN registration finished");
            Api connected =
                new Api(address, credential.getString("token"), credential.getString("client_id"));

            runOnUiThread(
                () -> {
                  if (generation != connectionGeneration || isDestroyed() || isFinishing()) return;
                  try {
                    store.save(connected);

                    if (art != null) art.close();
                    api = connected;
                    StartupTrace.connected(api);
                    art = new Art(this, api);
                    library = null;
                    section = "Home";
                    detailId = "";
                    collectionName = "";
                    restoreId = "";
                    chooseProfile(false);
                  } catch (Exception e) {
                    connectingLan = false;
                    connectionFailed(message, message(e));
                  }
                });
          } catch (Exception e) {
            runOnUiThread(
                () -> {
                  if (generation != connectionGeneration || isDestroyed()) return;
                  connectingLan = false;
                  connectionFailed(message,
                      e instanceof Api.AuthException
                          ? "This server has not enabled automatic connection for this network."
                          : "Could not connect. " + message(e));
                });
          }
        });
  }

  private void setup(String note) {
    base();
    int generation = connectionGeneration;
    Ui.add(root, Ui.title(this, L10n.t("Welcome home"), 28), -1, 50);
    Ui.add(root, Ui.text(this, L10n.t("Connect to your Torrbaby server"), 16, Ui.MUTED), -1, 35);
    EditText server = input("http://diamond.local:8765", false);
    if (api != null) server.setText(api.base);
    Ui.add(root, server, 600, 50);
    TextView message = Ui.text(this, note, 15, Ui.MUTED);
    Ui.add(root, message, -1, 50);
    Ui.add(
        root,
        Ui.button(
            this, L10n.t("Connect"), () -> {
              String address = server.getText().toString();
              base();
              TextView label = connectionScreen();
              connectLan(address, label, connectionGeneration);
            }),
        170,
        44);
    Ui.gap(root, 12);
    Ui.add(root, Ui.button(this, L10n.t("Find servers"), () -> discover("")), 170, 44);
    if (api != null) {
      Ui.gap(root, 12);
      Ui.add(root, Ui.button(this, L10n.t("Back to library"), () -> { selectingProfile = false; render(); }), 190, 44);
    }
    server.requestFocus();
  }

  private EditText input(String hint, boolean password) {
    EditText e = new EditText(this);
    e.setSingleLine();
    e.setHint(hint);
    e.setTextSize(15);
    e.setTextColor(Ui.TEXT);
    e.setHintTextColor(Ui.MUTED);
    e.setInputType(
        password
            ? InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD
            : InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
    return e;
  }

  private void error(String s) {
    new AlertDialog.Builder(this)
        .setTitle("Torrbaby")
        .setMessage(s)
        .setPositiveButton(L10n.t("OK"), null)
        .show();
  }

  @Override
  public void onBackPressed() {
    if (showingCast) {
      showingCast = false; render();
      View cast = root.findViewWithTag("cast");
      if (cast == null) cast = root.findViewWithTag("more");
      if (cast != null) cast.requestFocus();
      return;
    }

    if (switchingProfile) return;
    if (selectingProfile) {
      if (api == null || api.profile.isEmpty()) finish();
      else { selectingProfile = false; render(); }
      return;
    }
    if (!detailId.isEmpty()) {
      detailId = "";
      if (savedBrowseView != null && savedBrowseApi == api) {
        long started = SystemClock.elapsedRealtime();
        connectionGeneration++;
        root = savedBrowseRoot; content = savedBrowseContent; nav = savedBrowseNav;
        if(art!=null)art.beginScreen();
        setContentView(savedBrowseView);
        transition = -1; arrive(content);
        updateBrowse(root);
        inLibrary = true;
        if (savedBrowseFocus != null) savedBrowseFocus.requestFocus();
        savedBrowseView = null; savedBrowseFocus = null;
        savedBrowseRoot = savedBrowseContent = savedBrowseNav = null;
        android.util.Log.i("TorrbabyStartup", "Browse layout restored in " + (SystemClock.elapsedRealtime() - started) + " ms");
      } else { transition = -1; render(); }
    } else if (!collectionName.isEmpty()) {
      restoreShelf = collectionName;
      collectionName = "";
      restoreId = "";
      render();
    } else if (!section.equals("Home")) {
      section = "Home";
      query = "";
      genre = "";
      restoreId = "";
      render();
    } else
      new AlertDialog.Builder(this)
          .setTitle(L10n.t("Leave Torrbaby?"))
          .setNegativeButton(L10n.t("Stay"), null)
          .setPositiveButton(L10n.t("Exit"), (d, w) -> finish())
          .show();
  }
}
