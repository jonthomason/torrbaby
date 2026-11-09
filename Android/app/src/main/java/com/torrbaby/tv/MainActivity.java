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
  private boolean loading = false, played = false;
  private TextView status;
  private String collectionName = "", restoreShelf = "";
  private ArrayList<String> collectionIds = new ArrayList<>();
  private JSONArray torrents;
  private final Map<String, Long> removedTorrents = new HashMap<>();
  private String torrentCategory = "Downloading";
  private boolean torrentsLoading, torrentStale;
  private long torrentUpdated;
  private TextView torrentStatus;
  private TorrentActions torrentActions;
  private boolean inLibrary, refreshingLibrary;
  private String librarySnapshot = "";
  // Keep one browse hierarchy, including its scroll and focus, while viewing a title.
  private View savedBrowseView, savedBrowseFocus;
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
      load();
    }
  }

  @Override
  protected void onSaveInstanceState(Bundle s) {
    super.onSaveInstanceState(s);
    s.putString("section", section);
    s.putString("detail", detailId);
    s.putString("query", query);
    s.putString("genre", genre);
    s.putInt("sort", sort);
    s.putInt("season", season);
    s.putString("collection", collectionName);
    s.putStringArrayList("collection_ids", collectionIds);
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
    if (!discoveringScreen && api != null && library != null) {
      if (played) { played = false; load(); }
      else refreshLibrary();
    }
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
    setContentView(root);
  }

  private void shell() {
    base();
    root.setPadding(0, 0, 0, 0);
    if (!detailId.isEmpty() || !collectionName.isEmpty()) {
      content = root;
      return;
    }
    nav = Ui.row(this);
    Ui.pad(nav, 35, 0);
    TextView brand = Ui.title(this, "torrbaby", 20);
    brand.setTextColor(Ui.TEXT);
    brand.setGravity(Gravity.CENTER_VERTICAL);
    Ui.add(nav, brand, 145, 44);
    for (String label : new String[] {"Home", "Movies", "TV Shows", "Torrents", "Search"}) {
      Button b =
          Ui.button(
              this,
              label,
              () -> {
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
      Ui.tab(b, section.equals(label));
      Ui.add(nav, b, -2, 40);
      Ui.gap(nav, 8);
    }
    View spacer = new View(this);
    nav.addView(spacer, new LinearLayout.LayoutParams(0, 1, 1));
    Button settings = Ui.button(this, "Settings", this::settings);
    Ui.tab(settings, false);
    Ui.add(nav, settings, -2, 40);
    Ui.add(root, nav, -1, 44);
    Ui.gap(root, 6);
    content = Ui.column(this);
    root.addView(content, new LinearLayout.LayoutParams(-1, 0, 1));
  }

  private void load() {
    if (loading) return;
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
                StartupTrace.mark("Cached catalog parsed");
                runOnUiThread(() -> {
                  if (isDestroyed() || api != requested || library != null) return;
                  library = cached; librarySnapshot = cachedRaw; StartupTrace.mark("Cached render started"); render(); StartupTrace.mark("Cached render returned");
                });
              }
            } catch (Exception ignored) {}
          }
          try {
            JSONObject response = requested.request("GET", "/v1/library", null);
            Library result = Library.parse(response);
            String liveRaw = requested.libraryRaw;
            StartupTrace.mark("Live catalog parsed");
            runOnUiThread(
                () -> {
                  if (isDestroyed()) return;
                  loading = false;
                  if (api != requested) return;
                  boolean firstScreen = library == null;
                  library = result;
                  // Cached content is already usable. Refresh the model without tearing
                  // down the screen immediately before its first frame.
                  if (firstScreen || refreshingExistingScreen) {
                    librarySnapshot = liveRaw;
                    StartupTrace.mark("Live render started"); render();
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
                  if (library != null) {
                    Toast.makeText(this, "Could not refresh. " + message(e), Toast.LENGTH_LONG)
                        .show();
                    return;
                  }
                  shell();
                  Ui.add(content, Ui.title(this, "Your library is unavailable", 25), -1, 45);
                  Ui.add(content, Ui.text(this, message(e), 15, Ui.MUTED), -1, -2);
                  Ui.gap(content, 20);
                  Button retry = Ui.button(this, "Try again", this::load);
                  Ui.add(content, retry, 180, 44);
                  Ui.gap(content, 10);
                  Ui.add(content, Ui.button(this, "Connection settings", this::settings), 220, 44);
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
    shell();
    StartupTrace.mark("Library view construction started");
    View drawingRoot = root;
    drawingRoot.getViewTreeObserver().addOnPreDrawListener(new android.view.ViewTreeObserver.OnPreDrawListener() {
      public boolean onPreDraw() {
        drawingRoot.getViewTreeObserver().removeOnPreDrawListener(this);
        StartupTrace.mark("First library frame ready"); return true;
      }
    });
    inLibrary = true;
    Library.Title detail = library.find(detailId);
    if (detail != null) {
      details(detail);
      return;
    }
    detailId = "";
    if (!collectionName.isEmpty()) collection();
    else if (section.equals("Torrents")) torrentScreen();
    else if (section.equals("Home")) home();
    else catalog();
    wireNavigation();
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

  private void wireNavigation() {
    if (nav == null || !detailId.isEmpty() || !collectionName.isEmpty()) return;
    List<Button> buttons = new ArrayList<>();
    for (int i = 0; i < nav.getChildCount(); i++)
      if (nav.getChildAt(i) instanceof Button) {
        Button b = (Button) nav.getChildAt(i);
        b.setId(View.generateViewId());
        buttons.add(b);
      }
    View entry = content.findViewWithTag("hero");
    if (entry == null) entry = firstFocusable(content);
    if (entry != null && entry.getId() == View.NO_ID) entry.setId(View.generateViewId());
    for (int i = 0; i < buttons.size(); i++) {
      Button b = buttons.get(i);
      b.setNextFocusLeftId(buttons.get(Math.max(0, i - 1)).getId());
      b.setNextFocusRightId(buttons.get(Math.min(buttons.size() - 1, i + 1)).getId());
      b.setNextFocusUpId(b.getId());
      if (entry != null) b.setNextFocusDownId(entry.getId());
      if (b.getText().toString().equals(section) && entry != null)
        entry.setNextFocusUpId(b.getId());
    }
  }

  private ScrollView scroll(LinearLayout parent) {
    ScrollView s = new ScrollView(this);
    s.setFillViewport(false);
    s.setClipToPadding(false);
    s.setVerticalScrollBarEnabled(false);
    Ui.add(parent, s, -1, -1);
    return s;
  }

  private void ambient(Library.Title title) {
    if (Build.VERSION.SDK_INT < 31 || title == null || title.backdrop.isEmpty()) return;
    FrameLayout backdrop = new FrameLayout(this);
    backdrop.setBackgroundColor(Ui.BG);
    ImageView image = new ImageView(this);
    image.setScaleType(ImageView.ScaleType.CENTER_CROP);
    image.setAlpha(.22f);
    image.setRenderEffect(
        android.graphics.RenderEffect.createBlurEffect(
            Ui.dp(this, 45), Ui.dp(this, 45), android.graphics.Shader.TileMode.CLAMP));
    backdrop.addView(image, new FrameLayout.LayoutParams(-1, -1));
    if (root.getParent() instanceof android.view.ViewGroup)
      ((android.view.ViewGroup) root.getParent()).removeView(root);
    root.setBackgroundColor(android.graphics.Color.TRANSPARENT);
    backdrop.addView(root, new FrameLayout.LayoutParams(-1, -1));
    setContentView(backdrop);
    art.load(image, title.id, "backdrop");
  }

  private void home() {
    ScrollView sc = scroll(content);
    LinearLayout page = Ui.column(this);
    sc.addView(page);
    List<Library.Title> recent = library.filter("", "", "", 0);
    if (recent.isEmpty()) {
      Ui.pad(page, 40, 25);
      Ui.add(page, Ui.title(this, "Your library is empty", 26), -1, 60);
      Ui.add(
          page,
          Ui.text(this, "Add media on your Torrbaby server, then refresh here.", 15, Ui.MUTED),
          -1,
          40);
      Ui.add(page, Ui.button(this, "Refresh", this::load), 160, 44);
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
    shelf(body, "Recently watched", library.recentlyWatched());
    shelf(body, "Movies", library.filter("movie", "", "", 0));
    shelf(body, "TV Shows", library.filter("tv", "", "", 0));
    for (String name : library.genres("")) shelf(body, name, library.filter("", "", name, 0));
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
    Ui.add(heading, Ui.button(this, "Back", this::onBackPressed), -2, 40);
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
    Ui.add(parent, frame, -1, detail ? 325 : 300);
    ImageView image = new ImageView(this);
    image.setScaleType(ImageView.ScaleType.CENTER_CROP);
    frame.addView(image, new FrameLayout.LayoutParams(-1, -1));
    art.load(image, t.id, t.backdrop.isEmpty() ? "poster" : "backdrop");
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
            Ui.dp(this, detail ? 675 : 625), -2, Gravity.BOTTOM | Gravity.LEFT);
    frame.addView(info, ip);
    TextView name = Ui.title(this, t.title, detail ? 32 : 30);
    name.setMaxLines(2);
    Ui.add(info, name, -1, -2);
    Ui.gap(info, 10);
    Ui.add(info, Ui.text(this, t.metadata(), 14, 0xffbdc9d4), -1, -2);
    Ui.gap(info, 15);
    Library.Asset asset = t.preferred();
    if (detail) {
      LinearLayout buttons = Ui.row(this);
      Button play =
          Ui.button(
              this,
              asset != null && asset.position > 0 && !asset.watched ? "▶  Resume" : "▶  Play",
              () -> {
                if (asset == null) error("This title has no complete media files on the server.");
                else play(t, asset, false);
              });
      Ui.primary(play);
      play.setTag("hero");
      Ui.add(buttons, play, -2, 40);
      Ui.gap(buttons, 12);
      Ui.add(buttons, Ui.button(this, "More…", () -> assetOptions(t, asset)), -2, 40);
      Ui.add(info, buttons, -1, 40);
      play.requestFocus();
    } else {
      Ui.add(info, Ui.title(this, "▶  Explore", 17), -1, 23);
      frame.setFocusable(true);
      frame.setFocusableInTouchMode(true);
      frame.setContentDescription("Explore " + t.title);
      frame.setTag("hero");
      frame.setOnClickListener(v -> open(t));
      View line = new View(this);
      line.setBackgroundColor(Ui.ACCENT);
      line.setVisibility(View.INVISIBLE);
      frame.addView(line, new FrameLayout.LayoutParams(-1, Ui.dp(this, 3), Gravity.BOTTOM));
      frame.setOnFocusChangeListener(
          (v, f) -> {
            line.setVisibility(f ? View.VISIBLE : View.INVISIBLE);
            frame.setForeground(f ? Ui.shape(0x00000000, Ui.ACCENT, this) : null);
          });
      if (restoreId.isEmpty()) frame.requestFocus();
    }
  }

  private void shelf(LinearLayout body, String name, List<Library.Title> list) {
    if (list.isEmpty()) return;
    Ui.gap(body, 20);
    LinearLayout heading = Ui.row(this);
    Ui.add(heading, Ui.title(this, name, 18), -2, 38);
    View spacer = new View(this);
    heading.addView(spacer, new LinearLayout.LayoutParams(0, 1, 1));
    Button all = Ui.button(this, "See All  ›", () -> openCollection(name, list));
    all.setTag("shelf:" + name);
    all.setId(View.generateViewId());
    Ui.add(heading, all, -2, 38);
    Ui.add(body, heading, -1, 40);
    HorizontalScrollView sc = new HorizontalScrollView(this);
    sc.setHorizontalScrollBarEnabled(false);
    sc.setClipToPadding(false);
    Ui.pad(sc, 8, 7);
    sc.setClipChildren(false);
    LinearLayout row = Ui.row(this);
    row.setGravity(Gravity.TOP);
    row.setClipChildren(false);
    sc.addView(row);
    Ui.add(body, sc, -1, 265);
    // Reserve the shelf's space, but construct posters only near the viewport.
    // Headings remain focusable so D-pad navigation can enter an unloaded shelf.
    Runnable populate = () -> {
      if (row.getChildCount() != 0) return;
      for (Library.Title t : list.subList(0, Math.min(list.size(), 10))) {
        View posterCard = card(t);
        posterCard.setId(View.generateViewId());
        posterCard.setNextFocusUpId(all.getId());
        if (row.getChildCount() == 0) all.setNextFocusDownId(posterCard.getId());
        Ui.add(row, posterCard, 120, 247);
        Ui.gap(row, 20);
      }
    };
    all.setOnFocusChangeListener((v, focused) -> { if (focused) populate.run(); });
    if (body.getChildCount() <= 3) populate.run();
    Runnable nearViewport = () -> {
      if (row.getChildCount() != 0 || !sc.isShown()) return;
      int[] location = new int[2]; sc.getLocationOnScreen(location);
      int height = getResources().getDisplayMetrics().heightPixels;
      if (location[1] < height + Ui.dp(this, 300) && location[1] + sc.getHeight() > 0)
        populate.run();
    };
    sc.getViewTreeObserver().addOnScrollChangedListener(nearViewport::run);
    sc.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> nearViewport.run());
  }

  private View card(Library.Title t) {
    LinearLayout card = Ui.column(this);
    card.setFocusable(true);
    card.setFocusableInTouchMode(true);
    card.setClipChildren(false);
    Ui.pad(card, 0, 0);
    card.setTag("title:" + t.id);
    card.setContentDescription(
        t.title + ", " + t.year + (t.resume() != null ? ", in progress" : ""));
    card.setOnClickListener(v -> open(t));
    card.setOnFocusChangeListener(
        (v, f) -> {
          if (f) restoreId = t.id;
          View posterView = ((LinearLayout) v).getChildAt(0);
          posterView
              .animate()
              .scaleX(f ? 1.06f : 1f)
              .scaleY(f ? 1.06f : 1f)
              .setDuration(160)
              .start();
          posterView.setElevation(Ui.dp(this, f ? 10 : 0));
          posterView.setForeground(f ? Ui.shape(0x00000000, Ui.ACCENT, this) : null);
        });
    ImageView poster = new ImageView(this);
    poster.setScaleType(ImageView.ScaleType.CENTER_CROP);
    poster.setBackgroundColor(Ui.PANEL);
    Ui.round(poster, 12);
    Ui.add(card, poster, -1, 180);
    art.load(poster, t.id, "poster");
    Ui.gap(card, 10);
    TextView name = Ui.title(this, t.title, 14);
    name.setMaxLines(2);
    name.setEllipsize(android.text.TextUtils.TruncateAt.END);
    Ui.add(card, name, -1, 34);
    Library.Asset resume = t.resume();
    String sub = resume != null ? resume.status() : t.watched() ? "Watched" : t.year;
    Ui.add(card, Ui.text(this, sub, 11, Ui.MUTED), -1, 17);
    if (resume != null && resume.duration > 0) {
      ProgressBar bar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
      bar.setProgressTintList(android.content.res.ColorStateList.valueOf(Ui.ACCENT));
      bar.setProgress((int) (100 * resume.position / resume.duration));
      Ui.add(card, bar, -1, 3);
    }
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
      search.setHint("Search titles, genres or year");
      search.setText(query);
      search.setInputType(InputType.TYPE_CLASS_TEXT);
      if (query.isEmpty()) search.post(search::requestFocus);
      controls.addView(search, new LinearLayout.LayoutParams(0, Ui.dp(this, 44), 1));
      Ui.gap(controls, 12);
      Ui.add(
          controls,
          Ui.button(
              this,
              "Search",
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
            genre.isEmpty() ? "All genres" : genre,
            () -> {
              List<String> gs = library.genres(kind);
              gs.add(0, "All genres");
              new AlertDialog.Builder(this)
                  .setTitle("Genre")
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
                    .setTitle("Sort by")
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
      Ui.add(content, Ui.text(this, "Search movies and TV shows", 17, Ui.MUTED), -1, 70);
      return;
    }
    List<Library.Title> items = library.filter(kind, query, genre, sort);
    if (items.isEmpty()) {
      Ui.add(content, Ui.text(this, "No matching titles", 18, Ui.MUTED), -1, 70);
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
    GridView grid = new GridView(this);
    grid.setNumColumns(6);
    grid.setHorizontalSpacing(Ui.dp(this, 20));
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
            View v = card(items.get(i));
            v.setFocusable(false);
            v.setLayoutParams(new AbsListView.LayoutParams(-1, Ui.dp(MainActivity.this, 247)));
            return v;
          }
        });
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
          }
        });
    grid.setSelector(Ui.shape(0x18ffffff, 0xffc4d0d4, this));
    grid.setDrawSelectorOnTop(true);
    Ui.add(content, grid, -1, -1);
    int index = 0;
    for (int i = 0; i < items.size(); i++) if (items.get(i).id.equals(restoreId)) index = i;
    grid.setSelection(index);
    if (!section.equals("Search") || !query.isEmpty()) grid.requestFocus();
  }

  private void torrentScreen() {
    torrentActions = new TorrentActions(this, api, io, this::fetchTorrents, this::torrentRemoved);
    Ui.pad(content, 35, 16);
    LinearLayout controls = Ui.row(this);
    for (String category : new String[] {"Downloading", "Seeding", "Complete"}) {
      Button button =
          Ui.button(
              this,
              category,
              () -> {
                torrentCategory = category;
                render();
              });
      if (category.equals(torrentCategory)) button.setTextColor(Ui.ACCENT);
      Ui.add(controls, button, -2, 42);
      Ui.gap(controls, 12);
    }
    View space = new View(this);
    controls.addView(space, new LinearLayout.LayoutParams(0, 1, 1));
    Ui.add(controls, Ui.button(this, "Manage", () -> torrentActions.manage()), -2, 42);
    Ui.gap(controls, 12);
    Ui.add(controls, Ui.button(this, "Refresh", () -> fetchTorrents()), -2, 42);
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
              choice,
              () -> {
                getPreferences(MODE_PRIVATE)
                    .edit()
                    .putString("torrentSort." + torrentCategory, choice)
                    .apply();
                render();
              });
      if (choice.equals(order)) button.setTextColor(Ui.ACCENT);
      Ui.add(sortRow, button, -2, 40);
      Ui.gap(sortRow, 10);
    }
    Ui.add(content, sortRow, -1, 42);
    torrentStatus =
        Ui.text(
            this,
            torrentStale
                ? "Offline · showing the last server status"
                : torrentUpdated == 0
                    ? "Server transfers"
                    : "Server transfers · updated "
                        + new java.text.SimpleDateFormat("HH:mm:ss", Locale.getDefault())
                            .format(new java.util.Date(torrentUpdated)),
            11,
            Ui.MUTED);
    Ui.add(content, torrentStatus, -1, 26);
    Ui.gap(content, 8);
    if (torrents == null) {
      Ui.add(content, Ui.text(this, "Loading server activity…", 16, Ui.MUTED), -1, 50);
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
    List<JSONObject> sorted = new ArrayList<>();
    for (int i = 0; i < torrents.length(); i++)
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
      if (torrentCategory.equals("Downloading")
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
        bar.setProgressTintList(android.content.res.ColorStateList.valueOf(0xffb36eff));
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
        Ui.add(row, Ui.text(this, item.optString("error_string"), 13, 0xffffbb77), -1, -2);
      }
      Ui.add(rows, row, -1, -2);
      Ui.gap(rows, 14);
    }
    if (count == 0) Ui.add(rows, Ui.text(this, "No torrents", 18, Ui.MUTED), -1, 60);
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

  private void fetchTorrents() {
    if (torrentsLoading) return;
    torrentsLoading = true;
    Api requested = api;
    io.execute(
        () -> {
          try {
            JSONArray result = requested.array("/v1/torrents");
            runOnUiThread(
                () -> {
                  torrentsLoading = false;
                  if (requested != api || isDestroyed()) return;
                  torrents = visibleTorrents(result);
                  torrentStale = false;
                  torrentUpdated = System.currentTimeMillis();
                  if (section.equals("Torrents") && !isDestroyed()) {
                    View focus = getCurrentFocus();
                    Object tag = focus == null ? null : focus.getTag();
                    String button =
                        focus instanceof Button ? ((Button) focus).getText().toString() : "";
                    render();
                    View restore = tag == null ? null : root.findViewWithTag(tag);
                    if (restore != null) restore.requestFocus();
                    else if (!button.isEmpty()) restoreButton(root, button);
                  }
                });
          } catch (Exception e) {
            runOnUiThread(
                () -> {
                  torrentsLoading = false;
                  if (requested != api || isDestroyed()) return;
                  torrentStale = true;
                  if (torrentStatus != null)
                    torrentStatus.setText("Offline · showing the last server status");
                  if (section.equals("Torrents") && torrents == null) {
                    content.removeAllViews();
                    Ui.add(content, Ui.text(this, message(e), 16, Ui.MUTED), -1, 65);
                    Button retry = Ui.button(this, "Try again", this::fetchTorrents);
                    Ui.add(content, retry, 180, 44);
                    retry.requestFocus();
                  }
                });
          }
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

  private void refreshLibrary() {
    if (refreshingLibrary || loading) return;
    refreshingLibrary = true;
    Api requested = api;
    int generation = connectionGeneration;
    io.execute(
        () -> {
          try {
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
                  library = updated;
                  render();
                  View restore = tag == null ? null : root.findViewWithTag(tag);
                  if (restore != null) restore.post(restore::requestFocus);
                  else if (!label.isEmpty()) restoreButton(root, label);
                });
          } catch (Exception e) {
            runOnUiThread(() -> refreshingLibrary = false);
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
    render();
  }

  private void mediaLanguages(LinearLayout parent, Library.Asset asset, boolean compact) {
    if (asset == null) return;
    for (String line : MediaLanguages.lines(asset.mediaInfo)) {
      Ui.gap(parent, compact ? 4 : 7);
      TextView text = Ui.text(this, line, compact ? 12 : 14, Ui.MUTED);
      android.text.SpannableString styled = new android.text.SpannableString(line);
      for (int i = 0; i < line.length(); i++) if (line.charAt(i) == '\uFFFC') {
        android.graphics.drawable.Drawable icon = getDrawable(R.drawable.torrbaby_icon);
        int size = Ui.dp(this, compact ? 14 : 18);
        icon.setBounds(0, 0, size, size);
        styled.setSpan(new android.text.style.ImageSpan(icon, android.text.style.ImageSpan.ALIGN_BASELINE), i, i + 1, android.text.Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
      }
      text.setText(styled);
      text.setContentDescription(line.replace("\uFFFC", "generated subtitles"));
      Ui.add(parent, text, -1, -2);
    }
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
    if (t.kind.equals("tv") && t.preferred() != null)
      Ui.add(body, Ui.text(this, t.preferred().label(), 12, Ui.MUTED), -1, -2);
    mediaLanguages(body, t.preferred(), false);
    Ui.gap(body, 18);
    if (!t.summary.isEmpty()) {
      TextView summary = Ui.text(this, t.summary, 14, Ui.TEXT);
      summary.setLineSpacing(Ui.dp(this, 3), 1);
      Ui.add(body, summary, 650, -2);
      Ui.gap(body, 24);
    }
    if (t.kind.equals("tv")) {
      TreeSet<Integer> seasons = new TreeSet<>();
      for (Library.Asset a : t.assets) if (!a.extra() && a.hasSeason) seasons.add(a.season);
      if (season == -1 && !seasons.isEmpty())
        season =
            t.preferred() == null || !t.preferred().hasSeason
                ? (seasons.higher(0) == null ? seasons.first() : seasons.higher(0))
                : t.preferred().season;
      HorizontalScrollView scroll = new HorizontalScrollView(this);
      scroll.setHorizontalScrollBarEnabled(false);
      LinearLayout row = Ui.row(this);
      scroll.addView(row);
      for (int number : seasons) {
        Button button =
            Ui.button(
                this,
                number == 0 ? "Specials" : "Season " + number,
                () -> {
                  season = number;
                  render();
                  View selected = content.findViewWithTag("season:" + number);
                  if (selected != null) selected.requestFocus();
                });
        if (number == season) button.setTextColor(Ui.ACCENT);
        button.setTag("season:" + number);
        Ui.add(row, button, -2, 42);
        Ui.gap(row, 12);
      }
      Ui.add(body, scroll, -1, 46);
      Ui.gap(body, 20);
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
            Ui.add(body, Ui.title(this, "Other episodes", 18), -1, 36);
            heading = true;
          }
          episode(body, t, a);
        }
    }
    boolean extras = false;
    for (Library.Asset a : t.assets)
      if (a.extra()) {
        if (!extras) {
          Ui.gap(body, 16);
          Ui.add(body, Ui.title(this, "Special features", 18), -1, 36);
          extras = true;
        }
        episode(body, t, a);
      }
    Ui.gap(body, 12);
    Ui.add(
        body,
        Ui.button(
            this,
            "Back to library",
            () -> {
              detailId = "";
              render();
            }),
        -2,
        40);
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
      art.load(image, a.id, "episode");
      Ui.gap(row, 14);
    }
    TextView playIcon = Ui.title(this, a.watched ? "✓" : "▶", 18);
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
    if (a.position > 0 && !a.watched && a.duration > 0) {
      ProgressBar bar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
      bar.setProgressTintList(android.content.res.ColorStateList.valueOf(Ui.ACCENT));
      bar.setProgress((int) (100 * a.position / a.duration));
      Ui.add(text, bar, -1, 4);
    }
    row.addView(text, new LinearLayout.LayoutParams(0, -2, 1));
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
          .setTitle("Not yet available")
          .setMessage("This title has no complete media files on the server.")
          .setPositiveButton("OK", null)
          .show();
      return;
    }
    List<String> labels = new ArrayList<>();
    if (a.complete) {
      labels.add(a.position > 0 && !a.watched ? "Resume" : "Play");
      if (a.position > 0) labels.add("Start from beginning");
    }
    labels.add(a.watched ? "Mark unwatched" : "Mark watched");
    new AlertDialog.Builder(this)
        .setTitle(a.name)
        .setItems(
            labels.toArray(new String[0]),
            (d, w) -> {
              String action = labels.get(w);
              if (action.startsWith("Mark")) mark(a);
              else play(t, a, action.equals("Start from beginning"));
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
    boolean after = false;
    if (t.kind.equals("tv"))
      for (Library.Asset item : t.playable()) {
        if (after) {
          ids.add(item.id);
          names.add(item.label());
        }
        if (item.id.equals(a.id)) after = true;
      }
    i.putStringArrayListExtra("queue_ids", ids);
    i.putStringArrayListExtra("queue_names", names);
    startActivity(i);
  }

  private void settings() {
    new AlertDialog.Builder(this)
        .setTitle("Torrbaby " + BuildConfig.VERSION_NAME + " · build " + BuildConfig.VERSION_CODE)
        .setItems(
            new String[] {
              "Refresh library",
              "Change server connection",
              "Default subtitles",
              "Startup diagnostics",
              "Disconnect this TV"
            },
            (d, w) -> {
              if (w == 0) load();
              else if (w == 1) discover("");
              else if (w == 2) Subtitles.edit(this, api, io, null);
              else if (w == 3) new AlertDialog.Builder(this).setTitle("Startup timings").setMessage(StartupTrace.text()).setPositiveButton("Close", null).show();
              else
                new AlertDialog.Builder(this)
                    .setTitle("Disconnect this TV?")
                    .setMessage("Your library and viewing history stay on the server.")
                    .setNegativeButton("Cancel", null)
                    .setPositiveButton(
                        "Disconnect",
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
    root.setGravity(Gravity.CENTER);
    ImageView logo = new ImageView(this);
    logo.setImageResource(R.drawable.torrbaby_icon);
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
    Button retry = Ui.button(this, "Try again", () -> discover(""));
    Ui.add(root, retry, 200, 44);
    Ui.gap(root, 12);
    Ui.add(root, Ui.button(this, "Enter server address", () -> setup("")), 230, 44);
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
          message.setText("Choose your server");
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
            Library result = Library.parse(connected.request("GET", "/v1/library", null));
            runOnUiThread(
                () -> {
                  if (generation != connectionGeneration || isDestroyed() || isFinishing()) return;
                  try {
                    store.save(connected);

                    if (art != null) art.close();
                    api = connected;
                    StartupTrace.connected(api);
                    art = new Art(this, api);
                    library = result;
                    section = "Home";
                    detailId = "";
                    collectionName = "";
                    restoreId = "";
                    render();
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
    Ui.add(root, Ui.title(this, "Welcome home", 28), -1, 50);
    Ui.add(root, Ui.text(this, "Connect to your Torrbaby server", 16, Ui.MUTED), -1, 35);
    EditText server = input("http://diamond.local:8765", false);
    if (api != null) server.setText(api.base);
    Ui.add(root, server, 600, 50);
    TextView message = Ui.text(this, note, 15, Ui.MUTED);
    Ui.add(root, message, -1, 50);
    Ui.add(
        root,
        Ui.button(
            this, "Connect", () -> {
              String address = server.getText().toString();
              base();
              TextView label = connectionScreen();
              connectLan(address, label, connectionGeneration);
            }),
        170,
        44);
    Ui.gap(root, 12);
    Ui.add(root, Ui.button(this, "Find servers", () -> discover("")), 170, 44);
    if (api != null) {
      Ui.gap(root, 12);
      Ui.add(root, Ui.button(this, "Back to library", this::render), 190, 44);
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
        .setPositiveButton("OK", null)
        .show();
  }

  @Override
  public void onBackPressed() {
    if (!detailId.isEmpty()) {
      detailId = "";
      if (savedBrowseView != null && savedBrowseApi == api) {
        long started = SystemClock.elapsedRealtime();
        connectionGeneration++;
        root = savedBrowseRoot; content = savedBrowseContent; nav = savedBrowseNav;
        setContentView(savedBrowseView);
        librarySnapshot = savedBrowseSnapshot;
        inLibrary = true;
        if (savedBrowseFocus != null) savedBrowseFocus.requestFocus();
        savedBrowseView = null; savedBrowseFocus = null;
        savedBrowseRoot = savedBrowseContent = savedBrowseNav = null;
        android.util.Log.i("TorrbabyStartup", "Browse layout restored in " + (SystemClock.elapsedRealtime() - started) + " ms");
      } else render();
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
          .setTitle("Leave Torrbaby?")
          .setNegativeButton("Stay", null)
          .setPositiveButton("Exit", (d, w) -> finish())
          .show();
  }
}
