package com.torrbaby.tv;

import android.app.*;
import android.content.*;
import android.net.Uri;
import android.os.*;
import android.view.*;
import android.widget.*;
import androidx.media3.common.*;
import androidx.media3.datasource.*;
import androidx.media3.exoplayer.ExoPlayer;
import androidx.media3.session.MediaSession;
import androidx.media3.ui.PlayerView;
import java.util.*;
import java.util.concurrent.*;
import org.json.*;

@androidx.annotation.OptIn(markerClass = androidx.media3.common.util.UnstableApi.class)
public class PlaybackActivity extends Activity {
  /** ID prefix of subtitle files the player adds beside the media's own tracks. */
  private static final String SIDELOADED = "torrbaby-subtitle-";
  @Override protected void attachBaseContext(android.content.Context base) { super.attachBaseContext(L10n.wrap(base)); }

  private static final float[] SPEEDS = {.5f, .75f, 1, 1.25f, 1.5f, 1.75f, 2};
  private static final String[] SPEED_NAMES = {
    "0.5×", "0.75×", "1×", "1.25×", "1.5×", "1.75×", "2×"
  };
  private Api api;
  private JSONObject subtitlePreferences;
  private volatile boolean cancelled;
  private ExoPlayer player;
  private boolean subtitleChoiceMade;
  private PlayerView view;
  private Button nextEpisodeButton;
  private boolean controlsVisible, advancingEpisode;
  private int progressTicks;
  private MediaSession mediaSession;
  private final ExecutorService io = Executors.newSingleThreadExecutor();
  private final Handler main = new Handler(Looper.getMainLooper());
  private String session = "";
  private int sequence = 0;
  private boolean stopped = false, wasPlaying = true, finished = false, ready = false;
  private long localPosition = 0;
  private final Runnable ticker =
      new Runnable() {
        public void run() {
          updateNextEpisodeButton();
          if (++progressTicks % 10 == 0) report(false);
          main.postDelayed(this, 1000);
        }
      };

  @Override
  public void onCreate(Bundle state) {
    super.onCreate(state);
    getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
    if (state != null) localPosition = state.getLong("position");
    start();
  }

  private void start() {
    LinearLayout loading = Ui.column(this);
    loading.setGravity(Gravity.CENTER);
    loading.setBackgroundColor(Ui.BG);
    Ui.add(loading, Ui.title(this, getIntent().getStringExtra("title"), 24), -2, 50);
    TextView opening = Ui.text(this, L10n.t("Opening video…"), 15, Ui.MUTED);
    Ui.add(loading, opening, -2, 40);
    Ui.gap(loading, 12);
    Button cancel = Ui.button(this, L10n.t("Cancel"), this::finish);
    Ui.add(loading, cancel, 140, 44);
    cancel.requestFocus();
    setContentView(loading);
    io.execute(
        () -> {
          try {
            api = new SecureStore(this).load();
            if (api == null) throw new Exception("Link this TV to your server again.");
            if (api.profile.isEmpty()) throw new Exception("Choose a user before starting playback.");
            ProgressQueue.flush(this, api);
            String id = getIntent().getStringExtra("asset");
            List<MediaItem.SubtitleConfiguration> subtitles = new ArrayList<>();
            try {
              subtitlePreferences = Subtitles.load(api, getIntent().getStringExtra("title_id"));
            } catch (Exception ignored) {
              subtitlePreferences = null;
            }
            try {
              JSONObject inventory =
                  api.request("GET", "/v1/library/assets/" + Api.part(id) + "/subtitles", null);
              JSONArray tracks = inventory.optJSONArray("tracks");
              if (tracks == null) tracks = new JSONArray();
              try {
                JSONArray prepared = PreparedSubtitles.ready(api::request, id, () -> cancelled);
                for (int i = 0; i < prepared.length(); i++) tracks.put(prepared.getJSONObject(i));
              } catch (CancellationException e) {
                return;
              } catch (Exception e) {
                main.post(
                    () -> {
                      if (!cancelled)
                        Toast.makeText(
                                this,
                                "Subtitles unavailable: " + MainActivity.message(e),
                                Toast.LENGTH_LONG)
                            .show();
                    });
              }
              for (int i = 0; i < tracks.length(); i++) {
                JSONObject t = tracks.getJSONObject(i);
                // Sidecar originals are text; generated tracks carry their own name.
                String label = t.optString("name");
                if (label.isEmpty()) label = MediaLanguages.trackName(t.optString("language"), "text");
                MediaItem.SubtitleConfiguration.Builder b =
                    new MediaItem.SubtitleConfiguration.Builder(
                            Uri.parse(api.url(t.getString("path"))))
                        .setMimeType(MimeTypes.TEXT_VTT)
                        .setId(SIDELOADED + i)
                        .setLabel(label);
                if (!t.isNull("language")) b.setLanguage(t.optString("language"));
                subtitles.add(b.build());
              }
            } catch (Exception ignored) {
              /* Embedded tracks remain available on older servers. */
            }
            if (cancelled) return;
            JSONObject play =
                api.request(
                    "POST",
                    "/v1/library/assets/" + Api.part(id) + "/play",
                    new JSONObject().put("client_id", api.client));
            String path = api.url(play.getString("path"));
            session = play.getString("session_id");
            long resume =
                getIntent().getBooleanExtra("beginning", false)
                    ? 0
                    : (long) (play.optDouble("resume_position", 0) * 1000);
            MediaItem item =
                new MediaItem.Builder()
                    .setUri(path)
                    .setMediaMetadata(
                        new MediaMetadata.Builder()
                            .setTitle(getIntent().getStringExtra("title"))
                            .setDisplayTitle(getIntent().getStringExtra("name"))
                            .build())
                    .setSubtitleConfigurations(subtitles)
                    .build();
            main.post(
                () -> {
                  if (!isFinishing() && !isDestroyed())
                    prepare(item, localPosition > 0 ? localPosition : resume);
                });
          } catch (Exception e) {
            main.post(() -> failure(MainActivity.message(e)));
          }
        });
  }

  private void prepare(MediaItem item, long resume) {
    view = new PlayerView(this);
    view.setBackgroundColor(android.graphics.Color.BLACK);
    view.setKeepContentOnPlayerReset(true);
    view.setShowBuffering(PlayerView.SHOW_BUFFERING_WHEN_PLAYING);
    view.setControllerAutoShow(true);
    view.setControllerAnimationEnabled(false);
    view.setControllerShowTimeoutMs(4500);
    view.setShowSubtitleButton(true);
    if (view.getSubtitleView() != null) {
      view.getSubtitleView().setFractionalTextSize(.035f);
      view.getSubtitleView().setBottomPaddingFraction(.08f);
      view.getSubtitleView().setPadding(0, 0, 0, Ui.dp(this, 30));
    }
    view.setShowNextButton(false);
    view.setShowPreviousButton(false);
    setContentView(view);
    // Authentication stays on the configured origin. Redirects never carry a bearer token to
    // another host.
    DataSource.Factory factory = () -> new OriginDataSource(api);
    player =
        new ExoPlayer.Builder(this)
            .setMediaSourceFactory(
                new androidx.media3.exoplayer.source.DefaultMediaSourceFactory(factory))
            .setSeekBackIncrementMs(10000)
            .setSeekForwardIncrementMs(30000)
            .build();
    player.setAudioAttributes(
        new AudioAttributes.Builder()
            .setUsage(C.USAGE_MEDIA)
            .setContentType(C.AUDIO_CONTENT_TYPE_MOVIE)
            .build(),
        true);
    player.setHandleAudioBecomingNoisy(true);
    view.setPlayer(player);
    Subtitles.apply(subtitlePreferences, player, view.getSubtitleView());
    mediaSession = new MediaSession.Builder(this, player).setId(session).build();
    player.addListener(
        new Player.Listener() {
          @Override
          public void onTracksChanged(Tracks tracks) {
            if (!subtitleChoiceMade) Subtitles.selectInitialTrack(subtitlePreferences, player, tracks);
          }

          @Override
          public void onPlaybackStateChanged(int s) {
            if (s == Player.STATE_READY) ready = true;
            if (s == Player.STATE_ENDED && !finished) {
              finished = true;
              report(true);
              if (hasNextEpisode()) advanceNextEpisode();
              else endDialog();
            }
          }

          @Override
          public void onIsPlayingChanged(boolean playing) {
            getWindow().getDecorView().setKeepScreenOn(playing);
            if (!playing && ready && !finished) report(false);
          }

          @Override
          public void onPlayerError(PlaybackException error) {
            report(false);
            String reason =
                error.errorCode >= 2000 && error.errorCode < 3000
                    ? "The video stream was interrupted. Check the server and network connection."
                    : error.errorCode >= 4000 && error.errorCode < 6000
                        ? "This device could not decode the video or audio format."
                        : "This video could not be played.";
            failure(reason);
          }
        });
    float preferred = getSharedPreferences("player", MODE_PRIVATE).getFloat("speed", 1);
    boolean supported = false;
    for (float speed : SPEEDS) if (speed == preferred) supported = true;
    player.setPlaybackSpeed(supported ? preferred : 1);
    player.setMediaItem(item);
    player.seekTo(Math.max(0, resume));
    player.prepare();
    player.setPlayWhenReady(!stopped && wasPlaying);
    view.findViewById(androidx.media3.ui.R.id.exo_settings)
        .setOnClickListener(v -> playbackSettings());
    view.findViewById(androidx.media3.ui.R.id.exo_subtitle)
        .setOnClickListener(v -> tracks(C.TRACK_TYPE_TEXT));
    nextEpisodeButton = Ui.button(this, L10n.t("Next episode"), this::advanceNextEpisode);
    nextEpisodeButton.setId(View.generateViewId());
    android.widget.FrameLayout.LayoutParams nextLayout = new android.widget.FrameLayout.LayoutParams(Ui.dp(this, 190), Ui.dp(this, 48), Gravity.BOTTOM | Gravity.END);
    nextLayout.setMargins(0, 0, Ui.dp(this, 40), Ui.dp(this, 100));
    view.addView(nextEpisodeButton, nextLayout);
    nextEpisodeButton.setVisibility(View.GONE);
    View playPause = view.findViewById(androidx.media3.ui.R.id.exo_play_pause);
    nextEpisodeButton.setNextFocusDownId(playPause.getId());
    playPause.setNextFocusUpId(nextEpisodeButton.getId());
    view.setControllerVisibilityListener((PlayerView.ControllerVisibilityListener) visibility -> {
      controlsVisible = visibility == View.VISIBLE;
      updateNextEpisodeButton();
    });
    view.showController();
    view.findViewById(androidx.media3.ui.R.id.exo_play_pause).requestFocus();
    main.postDelayed(ticker, 1000);
  }

  // Own the row layout: vendor AlertDialog list layouts differ across TV firmware.
  private void playbackSettings() {
    showChoices("Playback", new String[] {"Audio", "Subtitles", "Playback speed"}, -1,
            (dialog, which) -> {
              if (which < 2) tracks(which == 0 ? C.TRACK_TYPE_AUDIO : C.TRACK_TYPE_TEXT);
              else {
                String[] names = SPEED_NAMES;
                float[] speeds = SPEEDS;
                int checked = 2;
                for (int i = 0; i < speeds.length; i++)
                  if (Math.abs(player.getPlaybackParameters().speed - speeds[i]) < .01) checked = i;
                showChoices("Playback speed", names, checked,
                        (d, w) -> {
                          player.setPlaybackSpeed(speeds[w]);
                          getSharedPreferences("player", MODE_PRIVATE)
                              .edit()
                              .putFloat("speed", speeds[w])
                              .apply();
                          d.dismiss();
                        });
              }
            });
  }

  private void showChoices(String title, String[] labels, int selected,
      DialogInterface.OnClickListener listener) {
    Dialog dialog = new Dialog(this);
    dialog.requestWindowFeature(Window.FEATURE_NO_TITLE);
    LinearLayout panel = Ui.column(this);
    Ui.pad(panel, 24, 20);
    panel.setBackground(Ui.shape(Ui.PANEL, 0, this));
    Ui.add(panel, Ui.title(this, title, 24), -1, -2);
    Ui.gap(panel, 16);
    ScrollView scroll = new ScrollView(this);
    LinearLayout rows = Ui.column(this);
    scroll.addView(rows);
    Button initial = null;
    for (int i = 0; i < labels.length; i++) {
      final int index = i;
      Button row = Ui.button(this, (i == selected ? "✓  " : "") + labels[i], () -> {
        dialog.dismiss();
        listener.onClick(dialog, index);
      });
      row.setGravity(Gravity.CENTER_VERTICAL | Gravity.START);
      row.setContentDescription(labels[i] + (i == selected ? ", selected" : ""));
      Ui.add(rows, row, -1, 48);
      Ui.gap(rows, 6);
      if (i == Math.max(0, selected)) initial = row;
    }
    int maxHeight = getResources().getDisplayMetrics().heightPixels * 55 / 100;
    panel.addView(scroll, new LinearLayout.LayoutParams(-1,
        Math.min(Ui.dp(this, labels.length * 54), maxHeight)));
    Ui.gap(panel, 12);
    Ui.add(panel, Ui.button(this, L10n.t("Cancel"), dialog::dismiss), -1, 48);
    dialog.setContentView(panel);
    Window window = dialog.getWindow();
    if (window != null) {
      window.setBackgroundDrawableResource(android.R.color.transparent);
      window.addFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND);
      window.setDimAmount(.65f);
    }
    dialog.show();
    if (window != null) window.setLayout(
        Math.min(Ui.dp(this, 440), getResources().getDisplayMetrics().widthPixels * 85 / 100),
        WindowManager.LayoutParams.WRAP_CONTENT);
    if (initial != null) initial.requestFocus();
  }

  private static int subtitlePriority(String language) {
    String tag = language == null ? "" : language.toLowerCase(Locale.ROOT);
    if (tag.equals("eng") || tag.equals("en") || tag.startsWith("en-")) return 0;
    if (tag.equals("chi") || tag.equals("zho") || tag.equals("zh") || tag.startsWith("zh-")) return 1;
    return 2;
  }

  private void tracks(int type) {
    List<String> labels = new ArrayList<>();
    List<TrackSelectionOverride> overrides = new ArrayList<>();
    labels.add(type == C.TRACK_TYPE_TEXT ? "Off" : "Automatic");
    overrides.add(null);
    int selected = 0;
    for (Tracks.Group group : player.getCurrentTracks().getGroups())
      if (group.getType() == type) {
        for (int index = 0; index < group.length; index++) {
          if (!group.isTrackSupported(index)) continue;
          Format format = group.getTrackFormat(index);
          String language =
              format.language == null
                  ? ""
                  : Locale.forLanguageTag(format.language).getDisplayLanguage();
          String name =
              format.label != null
                  ? format.label
                  : language.isEmpty() ? "Track " + labels.size() : language;
          if (type == C.TRACK_TYPE_TEXT) {
            if (format.id != null && format.id.contains(SIDELOADED) && format.label != null) {
              name = format.label;
            } else {
              // Embedded tracks: say whether they are text or bitmap images.
              String mime = format.sampleMimeType == null ? "" : format.sampleMimeType;
              boolean images = mime.equals(MimeTypes.APPLICATION_PGS) || mime.equals(MimeTypes.APPLICATION_VOBSUB)
                  || mime.equals(MimeTypes.APPLICATION_DVBSUBS);
              name = MediaLanguages.trackName(format.language, images ? "images" : "text");
              if ((format.selectionFlags & C.SELECTION_FLAG_FORCED) != 0) name += " · " + L10n.t("Forced");
              if ((format.roleFlags & C.ROLE_FLAG_CAPTION) != 0) name += " · " + L10n.t("SDH");
            }
          }
          if (type == C.TRACK_TYPE_AUDIO && format.channelCount > 0)
            name +=
                " · " + format.channelCount + (format.channelCount == 1 ? " channel" : " channels");
          labels.add(name);
          overrides.add(new TrackSelectionOverride(group.getMediaTrackGroup(), index));
          if (group.isTrackSelected(index)) selected = labels.size() - 1;
        }
      }
    if (type == C.TRACK_TYPE_TEXT) {
      MediaLanguages.numberDuplicates(labels);
      List<Integer> order = new ArrayList<>();
      for (int i = 1; i < labels.size(); i++) order.add(i);
      order.sort((a, b) -> {
        String left = overrides.get(a).mediaTrackGroup.getFormat(overrides.get(a).trackIndices.get(0)).language;
        String right = overrides.get(b).mediaTrackGroup.getFormat(overrides.get(b).trackIndices.get(0)).language;
        int priority = Integer.compare(subtitlePriority(left), subtitlePriority(right));
        return priority != 0 ? priority : labels.get(a).compareToIgnoreCase(labels.get(b));
      });
      List<String> sortedLabels = new ArrayList<>();
      List<TrackSelectionOverride> sortedOverrides = new ArrayList<>();
      sortedLabels.add("Off"); sortedOverrides.add(null);
      int sortedSelection = 0;
      for (int index : order) {
        if (index == selected) sortedSelection = sortedLabels.size();
        sortedLabels.add(labels.get(index)); sortedOverrides.add(overrides.get(index));
      }
      labels.clear(); labels.addAll(sortedLabels);
      overrides.clear(); overrides.addAll(sortedOverrides);
      selected = player.getTrackSelectionParameters().disabledTrackTypes.contains(type) ? 0 : sortedSelection;
    }
    showChoices(type == C.TRACK_TYPE_TEXT ? "Subtitles" : "Audio",
            labels.toArray(new String[0]), selected,
            (dialog, which) -> {
              if (type == C.TRACK_TYPE_TEXT) subtitleChoiceMade = true;
              TrackSelectionParameters.Builder builder =
                  player
                      .getTrackSelectionParameters()
                      .buildUpon()
                      .clearOverridesOfType(type)
                      .setTrackTypeDisabled(type, type == C.TRACK_TYPE_TEXT && which == 0);
              if (which > 0) builder.setOverrideForType(overrides.get(which));
              player.setTrackSelectionParameters(builder.build());
              if (type == C.TRACK_TYPE_TEXT && subtitlePreferences != null) {
                try {
                  subtitlePreferences.put("enabled", which > 0);
                  if (which > 0) {
                    TrackSelectionOverride chosen = overrides.get(which);
                    String language =
                        chosen.mediaTrackGroup.getFormat(chosen.trackIndices.get(0)).language;
                    subtitlePreferences.put(
                        "language",
                        language != null && language.startsWith("zh")
                            ? "zh-Hans"
                            : language != null && language.startsWith("en") ? "en" : "original");
                  }
                  JSONObject saved = new JSONObject(subtitlePreferences.toString());
                  io.execute(
                        () -> {
                          try {
                            api.request("PUT", Subtitles.path(api), saved);
                          } catch (Exception e) {
                            Subtitles.error(this, e);
                          }
                        });
                } catch (Exception ignored) {
                }
              }
              dialog.dismiss();
            });
  }

  private void report(boolean ended) {
    if (player == null || api == null || session.isEmpty() || !ready || (finished && !ended))
      return;
    long duration = player.getDuration();
    if (duration <= 0 || duration == C.TIME_UNSET) return;
    try {
      JSONObject update =
          new JSONObject()
              .put("session_id", session)
              .put("sequence", ++sequence)
              .put("position", Math.min(player.getCurrentPosition(), duration) / 1000.0)
              .put("duration", duration / 1000.0)
              .put("ended", ended)
              .put("watched_eligible", player.isPlaying())
              .put("reached_end", player.getPlaybackState() == Player.STATE_ENDED);
      ProgressQueue.save(this, api, update);
      io.execute(() -> ProgressQueue.flush(this, api));
    } catch (Exception ignored) {
    }
  }

  private boolean hasNextEpisode() {
    ArrayList<String> ids = getIntent().getStringArrayListExtra("queue_ids");
    ArrayList<String> names = getIntent().getStringArrayListExtra("queue_names");
    return ids != null && names != null && !ids.isEmpty() && ids.size() == names.size();
  }

  private void updateNextEpisodeButton() {
    if (nextEpisodeButton == null || player == null) return;
    boolean show = controlsVisible && !advancingEpisode
        && EpisodeAdvance.offer(player.getCurrentPosition(), player.getDuration(), hasNextEpisode());
    if (!show && nextEpisodeButton.hasFocus())
      view.findViewById(androidx.media3.ui.R.id.exo_play_pause).requestFocus();
    nextEpisodeButton.setVisibility(show ? View.VISIBLE : View.GONE);
  }

  private void advanceNextEpisode() {
    if (advancingEpisode || !hasNextEpisode()) return;
    advancingEpisode = true;
    report(true);
    finished = true;
    if (player != null) player.pause();
    ArrayList<String> ids = getIntent().getStringArrayListExtra("queue_ids");
    ArrayList<String> names = getIntent().getStringArrayListExtra("queue_names");
    Intent next = new Intent(this, PlaybackActivity.class);
    next.putExtra("asset", ids.get(0));
    next.putExtra("title_id", getIntent().getStringExtra("title_id"));
    next.putExtra("title", getIntent().getStringExtra("title"));
    next.putExtra("name", names.get(0));
    next.putStringArrayListExtra("queue_ids", new ArrayList<>(ids.subList(1, ids.size())));
    next.putStringArrayListExtra("queue_names", new ArrayList<>(names.subList(1, names.size())));
    startActivity(next);
    finish();
  }

  private void endDialog() {
    new AlertDialog.Builder(this)
        .setTitle(L10n.t("Finished watching"))
        .setNegativeButton(L10n.t("Back to library"), (d, w) -> finish())
        .setPositiveButton(L10n.t("Play again"), (d, w) -> restart(true))
        .setOnCancelListener(d -> finish()).show();
  }

  private void restart(boolean beginning) {
    Intent i = new Intent(getIntent());
    i.putExtra("beginning", beginning);
    startActivity(i);
    finish();
  }

  private void failure(String message) {
    if (isFinishing() || isDestroyed()) return;
    new AlertDialog.Builder(this)
        .setTitle(L10n.t("Unable to play"))
        .setMessage(message)
        .setPositiveButton(L10n.t("Retry"), (d, w) -> restart(false))
        .setNegativeButton(L10n.t("Back to library"), (d, w) -> finish())
        .setOnCancelListener(d -> finish())
        .show();
  }

  @Override
  public boolean dispatchKeyEvent(KeyEvent e) {
    if (view != null && player != null) {
      int key = e.getKeyCode();
      if (key == KeyEvent.KEYCODE_DPAD_CENTER || key == KeyEvent.KEYCODE_ENTER) {
        if (!view.isControllerFullyVisible()) {
          view.showController();
          return true;
        }
      }
      if (key == KeyEvent.KEYCODE_BACK && view.isControllerFullyVisible()) {
        if (e.getAction() == KeyEvent.ACTION_UP) view.hideController();
        return true;
      }
      if (view.dispatchKeyEvent(e)) return true;
    }
    return super.dispatchKeyEvent(e);
  }

  @Override
  protected void onStop() {
    stopped = true;
    if (player != null) {
      wasPlaying = player.getPlayWhenReady();
      report(false);
      player.pause();
    }
    main.removeCallbacks(ticker);
    super.onStop();
  }

  @Override
  protected void onStart() {
    super.onStart();
    stopped = false;
    if (player != null) {
      player.setPlayWhenReady(wasPlaying);
      main.postDelayed(ticker, 1000);
    }
  }

  @Override
  protected void onSaveInstanceState(Bundle b) {
    if (player != null) b.putLong("position", player.getCurrentPosition());
    super.onSaveInstanceState(b);
  }

  @Override
  public void finish() {
    cancelled = true;
    super.finish();
  }

  @Override
  protected void onDestroy() {
    if (!finished) {
      report(true);
      if (api != null && !session.isEmpty() && (player == null || !ready || player.getDuration() <= 0 || player.getDuration() == C.TIME_UNSET)) {
        final String closingSession = session;
        io.execute(() -> { try { api.request("POST", "/v1/playback/end", new JSONObject().put("session_id", closingSession)); } catch (Exception ignored) {} });
      }
      finished = true;
    }
    cancelled = true;
    main.removeCallbacksAndMessages(null);
    if (mediaSession != null) mediaSession.release();
    if (player != null) {
      player.release();
      player = null;
    }
    io.shutdown();
    super.onDestroy();
  }

  /** No cross-origin redirects or persistent media cache. */
  private static final class OriginDataSource implements DataSource {
    private final Api api;
    private java.net.HttpURLConnection connection;
    private java.io.InputStream input;
    private Uri uri;
    private long remaining;

    OriginDataSource(Api a) {
      api = a;
    }

    public long open(DataSpec spec) throws java.io.IOException {
      try {
        java.net.URI target = new java.net.URI(spec.uri.toString());
        if (!Api.sameOrigin(new java.net.URI(api.base), target)
            || !target.getPath().startsWith("/v1/"))
          throw new java.io.IOException("Invalid media origin");
        uri = spec.uri;
        connection = (java.net.HttpURLConnection) target.toURL().openConnection();
        connection.setInstanceFollowRedirects(false);
        connection.setConnectTimeout(10000);
        connection.setReadTimeout(20000);
        connection.setRequestProperty("Authorization", "Bearer " + api.token);
        connection.setRequestProperty("Accept-Encoding", "identity");
        if (spec.position > 0 || spec.length != C.LENGTH_UNSET)
          connection.setRequestProperty(
              "Range",
              "bytes="
                  + spec.position
                  + "-"
                  + (spec.length == C.LENGTH_UNSET
                      ? ""
                      : Long.toString(spec.position + spec.length - 1)));
        int code = connection.getResponseCode();
        if (code < 200 || code >= 300)
          throw new java.io.IOException("Media request failed (HTTP " + code + ")");
        input = connection.getInputStream();
        long size = connection.getContentLengthLong();
        if (code == 200 && spec.position > 0) {
          long skip = spec.position;
          while (skip > 0) {
            long n = input.skip(skip);
            if (n == 0) {
              if (input.read() == -1) throw new java.io.EOFException();
              n = 1;
            }
            skip -= n;
          }
          if (size >= 0) size -= spec.position;
        }
        remaining = spec.length != C.LENGTH_UNSET ? spec.length : size;
        return remaining;
      } catch (java.net.URISyntaxException e) {
        throw new java.io.IOException(e);
      }
    }

    public int read(byte[] b, int o, int n) throws java.io.IOException {
      if (n == 0) return 0;
      if (remaining == 0) return C.RESULT_END_OF_INPUT;
      int read = input.read(b, o, remaining < 0 ? n : (int) Math.min(n, remaining));
      if (read < 0) return C.RESULT_END_OF_INPUT;
      if (remaining > 0) remaining -= read;
      return read;
    }

    public Uri getUri() {
      return uri;
    }

    public Map<String, List<String>> getResponseHeaders() {
      return connection == null ? Collections.emptyMap() : connection.getHeaderFields();
    }

    public void close() throws java.io.IOException {
      try {
        if (input != null) input.close();
      } finally {
        input = null;
        if (connection != null) connection.disconnect();
        connection = null;
      }
    }

    public void addTransferListener(TransferListener listener) {}
  }
}
