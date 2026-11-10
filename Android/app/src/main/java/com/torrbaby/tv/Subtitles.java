package com.torrbaby.tv;

import android.app.*;
import android.graphics.Color;
import android.graphics.Typeface;
import android.widget.Toast;
import androidx.media3.common.*;
import androidx.media3.exoplayer.ExoPlayer;
import androidx.media3.ui.*;
import java.util.concurrent.Executor;
import org.json.JSONObject;

/** Shared server preferences, with native rendering and D-pad-friendly editing. */
@androidx.annotation.OptIn(markerClass = androidx.media3.common.util.UnstableApi.class)
final class Subtitles {
  static String path(Api api) {
    return "/v1/devices/" + Api.part(api.client) + "/subtitle-preferences";
  }

  static JSONObject load(Api api, String title) throws Exception {
    return api.request("GET", path(api), null).getJSONObject("effective");
  }

  static TrackSelectionOverride preferredTrack(Tracks tracks, String language) {
    TrackSelectionOverride best = null;
    int bestScore = -1;
    String wanted = normalizedLanguage(language);
    for (Tracks.Group group : tracks.getGroups()) {
      if (group.getType() != C.TRACK_TYPE_TEXT) continue;
      for (int index = 0; index < group.length; index++) {
        if (!group.isTrackSupported(index)) continue;
        Format format = group.getTrackFormat(index);
        String actual = normalizedLanguage(format.language);
        int score = !wanted.isEmpty() && wanted.equals(actual) ? 100 : 0;
        // When subtitles are On, choose an available fallback rather than silently Off.
        if ((format.selectionFlags & C.SELECTION_FLAG_FORCED) == 0) score += 10;
        if ((format.selectionFlags & C.SELECTION_FLAG_DEFAULT) != 0) score += 2;
        if ((format.roleFlags & C.ROLE_FLAG_CAPTION) == 0) score += 1;
        if (score > bestScore) {
          bestScore = score;
          best = new TrackSelectionOverride(group.getMediaTrackGroup(), index);
        }
      }
    }
    return best;
  }

  private static String normalizedLanguage(String language) {
    if (language == null || language.equals("original")) return "";
    String tag = language.toLowerCase(java.util.Locale.ROOT).replace('_', '-');
    if (tag.equals("eng")) return "en";
    if (tag.equals("zho") || tag.equals("chi")) return "zh";
    return tag.split("-", 2)[0];
  }

  static void selectInitialTrack(JSONObject p, ExoPlayer player, Tracks tracks) {
    if (p == null || !p.optBoolean("enabled")) return;
    TrackSelectionOverride choice = preferredTrack(tracks, p.optString("language", "original"));
    if (choice == null) return;
    TrackSelectionParameters parameters = player.getTrackSelectionParameters();
    if (choice.equals(parameters.overrides.get(choice.mediaTrackGroup))
        && !parameters.disabledTrackTypes.contains(C.TRACK_TYPE_TEXT)) return;
    player.setTrackSelectionParameters(parameters.buildUpon()
        .clearOverridesOfType(C.TRACK_TYPE_TEXT)
        .setTrackTypeDisabled(C.TRACK_TYPE_TEXT, false)
        .setOverrideForType(choice).build());
  }

  static void apply(JSONObject p, ExoPlayer player, SubtitleView view) {
    if (p == null) return;
    String language = p.optString("language", "original");
    player.setTrackSelectionParameters(
        player
            .getTrackSelectionParameters()
            .buildUpon()
            .setTrackTypeDisabled(C.TRACK_TYPE_TEXT, !p.optBoolean("enabled"))
            .setPreferredTextLanguage(language.equals("original") ? null : language)
            .setSelectUndeterminedTextLanguage(language.equals("original"))
            .build());
    if (view == null) return;
    String family = p.optString("font", "default");
    Typeface font =
        family.equals("default") ? Typeface.DEFAULT : Typeface.create(family, Typeface.NORMAL);
    int text = Color.argb(p.optInt("text_opacity", 100) * 255 / 100, 255, 255, 255);
    int background = Color.argb(p.optInt("background_opacity", 0) * 255 / 100, 0, 0, 0);
    view.setApplyEmbeddedStyles(false);
    view.setStyle(
        new CaptionStyleCompat(
            text,
            background,
            Color.TRANSPARENT,
            CaptionStyleCompat.EDGE_TYPE_NONE,
            Color.BLACK,
            font));
    view.setFractionalTextSize(SubtitleView.DEFAULT_TEXT_SIZE_FRACTION * p.optInt("size_percent", 100) / 100);
  }

  static void edit(Activity activity, Api api, Executor io, String title) {
    io.execute(
        () -> {
          try {
            JSONObject p = load(api, title);
            activity.runOnUiThread(
                () -> {
                  if (!activity.isFinishing()) menu(activity, api, io, title, p);
                });
          } catch (Exception e) {
            error(activity, e);
          }
        });
  }

  private static void menu(Activity a, Api api, Executor io, String title, JSONObject p) {
    String[] labels = {
      L10n.t("Subtitles") + ": " + L10n.t(p.optBoolean("enabled") ? "On" : "Off"),
      L10n.t("Language") + ": " + MediaLanguages.name(p.optString("language")),
      L10n.t("Font") + ": " + p.optString("font"),
      L10n.t("Size") + ": " + p.optInt("size_percent") + "%",
      L10n.t("Text opacity") + ": " + p.optInt("text_opacity") + "%",
      L10n.t("Background opacity") + ": " + p.optInt("background_opacity") + "%"
    };
    AlertDialog.Builder dialog =
        new AlertDialog.Builder(a)
            .setTitle(L10n.t("Subtitles on this TV"))
            .setItems(
                L10n.labels(labels),
                (d, which) -> {
                  String[] keys = {
                    "enabled",
                    "language",
                    "font",
                    "size_percent",
                    "text_opacity",
                    "background_opacity"
                  };
                  Object[] values;
                  if (which == 0) values = new Object[] {false, true};
                  else if (which == 1) values = new Object[] {"original", "en", "zh-Hans"};
                  else if (which == 2)
                    values = new Object[] {"default", "sans-serif", "serif", "monospace"};
                  else {
                    int start = which == 3 ? 50 : 0, end = which == 3 ? 200 : 100;
                    values = new Object[(end - start) / 10 + 1];
                    for (int i = 0; i < values.length; i++) values[i] = start + i * 10;
                  }
                  String[] names = new String[values.length];
                  int selected = 0;
                  for (int i = 0; i < values.length; i++) {
                    names[i] = values[i].toString() + (which >= 3 ? "%" : "");
                    if (values[i].equals(p.opt(keys[which]))) selected = i;
                  }
                  if (which == 0) names = new String[] {"Off", "On"};
                  if (which == 1) names = new String[] {"Original", "English", "Chinese (Simplified)"};
                  new AlertDialog.Builder(a)
                      .setTitle(labels[which])
                      .setSingleChoiceItems(
                          L10n.labels(names),
                          selected,
                          (choice, index) -> {
                            try {
                              p.put(keys[which], values[index]);
                            } catch (Exception ignored) {
                            }
                            choice.dismiss();
                            menu(a, api, io, title, p);
                          })
                      .setNegativeButton(L10n.t("Back"), (x, y) -> menu(a, api, io, title, p))
                      .show();
                })
            .setPositiveButton(L10n.t("Save"), (d, w) -> save(a, api, io, title, p, false))
            .setNegativeButton(L10n.t("Cancel"), null);
    dialog.show();
  }

  private static void save(
      Activity a, Api api, Executor io, String title, JSONObject p, boolean reset) {
    io.execute(
        () -> {
          try {
            api.request(reset ? "DELETE" : "PUT", path(api), reset ? new JSONObject() : p);
            a.runOnUiThread(
                () -> Toast.makeText(a, L10n.t("Subtitle preferences saved"), Toast.LENGTH_SHORT).show());
          } catch (Exception e) {
            error(a, e);
          }
        });
  }

  static void error(Activity a, Exception e) {
    a.runOnUiThread(
        () -> {
          if (!a.isFinishing())
            new AlertDialog.Builder(a)
                .setTitle(L10n.t("Subtitle preferences"))
                .setMessage(MainActivity.message(e))
                .setPositiveButton(L10n.t("OK"), null)
                .show();
        });
  }
}
