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
    view.setFractionalTextSize(.035f * p.optInt("size_percent", 100) / 100);
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
      "Subtitles: " + (p.optBoolean("enabled") ? "On" : "Off"),
      "Language: " + p.optString("language"),
      "Font: " + p.optString("font"),
      "Size: " + p.optInt("size_percent") + "%",
      "Text opacity: " + p.optInt("text_opacity") + "%",
      "Background opacity: " + p.optInt("background_opacity") + "%"
    };
    AlertDialog.Builder dialog =
        new AlertDialog.Builder(a)
            .setTitle("Subtitles on this TV")
            .setItems(
                labels,
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
                  if (which == 1) names = new String[] {"Original", "English", "简体中文"};
                  new AlertDialog.Builder(a)
                      .setTitle(labels[which])
                      .setSingleChoiceItems(
                          names,
                          selected,
                          (choice, index) -> {
                            try {
                              p.put(keys[which], values[index]);
                            } catch (Exception ignored) {
                            }
                            choice.dismiss();
                            menu(a, api, io, title, p);
                          })
                      .setNegativeButton("Back", (x, y) -> menu(a, api, io, title, p))
                      .show();
                })
            .setPositiveButton("Save", (d, w) -> save(a, api, io, title, p, false))
            .setNegativeButton("Cancel", null);
    dialog.show();
  }

  private static void save(
      Activity a, Api api, Executor io, String title, JSONObject p, boolean reset) {
    io.execute(
        () -> {
          try {
            api.request(reset ? "DELETE" : "PUT", path(api), reset ? new JSONObject() : p);
            a.runOnUiThread(
                () -> Toast.makeText(a, "Subtitle preferences saved", Toast.LENGTH_SHORT).show());
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
                .setTitle("Subtitle preferences")
                .setMessage(MainActivity.message(e))
                .setPositiveButton("OK", null)
                .show();
        });
  }
}
