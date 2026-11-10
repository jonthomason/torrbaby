package com.torrbaby.tv;

import android.app.Activity;
import android.app.Dialog;
import android.os.Handler;
import android.os.Looper;
import android.view.Window;
import android.widget.*;
import java.util.concurrent.Executor;
import org.json.*;

/** Server-owned package preview and durable file operation. Never deletes local TV files. */
final class LibraryFileActions {
  private final Activity activity;
  private final Api api;
  private final Executor io;
  private final Runnable changed;
  private final Handler handler = new Handler(Looper.getMainLooper());
  private final Dialog dialog;
  private final LinearLayout content;
  private final String path;
  private String renderedStatus = "";

  LibraryFileActions(Activity activity, Api api, Executor io, String asset, Runnable changed) {
    this.activity = activity;
    this.api = api;
    this.io = io;
    this.changed = changed;
    path = "/v1/library/assets/" + Api.part(asset) + "/manage-files";
    dialog = new Dialog(activity);
    dialog.requestWindowFeature(Window.FEATURE_NO_TITLE);
    ScrollView scroll = new ScrollView(activity);
    content = Ui.column(activity);
    Ui.pad(content, 24, 24);
    content.setBackground(Ui.shape(Ui.PANEL, 0, activity));
    scroll.addView(content);
    dialog.setContentView(scroll);
    dialog.setOnDismissListener(d -> handler.removeCallbacksAndMessages(null));
  }

  void show() {
    dialog.show();
    Window window = dialog.getWindow();
    if (window != null) {
      window.setBackgroundDrawableResource(android.R.color.transparent);
      window.setLayout(Math.min(Ui.dp(activity, 600), activity.getResources().getDisplayMetrics().widthPixels * 90 / 100),
          activity.getResources().getDisplayMetrics().heightPixels * 85 / 100);
    }
    fetch();
  }

  private boolean visible() { return dialog.isShowing() && !activity.isFinishing() && !activity.isDestroyed(); }
  private void heading(String title) {
    content.removeAllViews();
    Ui.add(content, Ui.title(activity, title, 24), -1, -2);
    Ui.gap(content, 16);
  }
  private void text(String message) {
    Ui.add(content, Ui.text(activity, message, 16, Ui.TEXT), -1, -2);
    Ui.gap(content, 12);
  }
  private void button(String label, boolean destructive, Runnable action) {
    Button button = Ui.button(activity, label, action);
    if (destructive) button.setTextColor(0xffff8a80);
    Ui.add(content, button, -1, -2);
    Ui.gap(content, 8);
  }
  private void busy(String title) {
    heading(title);
    Ui.add(content, new ProgressBar(activity), 40, 40);
    Ui.gap(content, 16);
    button("Close", false, dialog::dismiss);
  }
  private void failure(Exception error, Runnable retry) {
    renderedStatus = "";
    heading("Couldn’t manage files");
    text(error.getMessage() == null ? "Please try again." : error.getMessage());
    button("Retry", false, retry);
    button("Close", false, dialog::dismiss);
  }
  private void fetch() {
    busy("Checking files…");
    io.execute(() -> {
      try {
        JSONObject preview = api.request("GET", path, null);
        activity.runOnUiThread(() -> { if (visible()) preview(preview); });
      } catch (Exception e) {
        activity.runOnUiThread(() -> { if (visible()) failure(e, this::fetch); });
      }
    });
  }
  private void scope(JSONObject preview) {
    text(preview.optString("name"));
    text("entire_download".equals(preview.optString("scope"))
        ? "This affects the entire download package and everyone’s library."
        : "This removes the file from everyone’s library.");
    text(android.text.format.Formatter.formatFileSize(activity, preview.optLong("total_bytes")));
    JSONArray assets = preview.optJSONArray("assets");
    if (assets != null) for (int i = 0; i < assets.length(); i++) {
      JSONObject item = assets.optJSONObject(i);
      if (item == null) continue;
      String episode = item.isNull("episode") ? "" : String.format(java.util.Locale.ROOT,
          "S%02dE%02d · ", item.optInt("season"), item.optInt("episode"));
      text(episode + item.optString("name"));
    }
    text("Personal watch history and downloaded copies on other devices are kept.");
  }
  private void preview(JSONObject preview) {
    heading("Manage files");
    scope(preview);
    JSONArray blocked = preview.optJSONArray("blocked_reasons");
    if (blocked != null && blocked.length() > 0) {
      for (int i = 0; i < blocked.length(); i++) text(blocked.optString(i));
      button("Check again", false, this::fetch);
    } else {
      JSONArray allowed = preview.optJSONArray("allowed_actions");
      if (allowed != null) for (int i = 0; i < allowed.length(); i++) {
        String action = allowed.optString(i);
        if (action.equals("delete") || action.equals("junk"))
          button(action.equals("junk") ? "Move to Junk and finish seeding" : "Delete permanently",
              action.equals("delete"), () -> confirm(preview, action));
      }
    }
    button("Cancel", false, dialog::dismiss);
  }
  private void confirm(JSONObject preview, String action) {
    heading(action.equals("junk") ? "Move this package to Junk?" : "Permanently delete these files?");
    scope(preview);
    text(action.equals("junk")
        ? "The entire package will be hidden now, continue seeding, and be permanently deleted when seeding finishes."
        : "Seeding will stop. These server files will be permanently deleted. This cannot be undone.");
    button("Cancel", false, () -> preview(preview));
    button(action.equals("junk") ? "Move to Junk" : "Delete permanently", true, () -> submit(preview, action));
  }
  private void submit(JSONObject preview, String action) {
    busy("Submitting…");
    io.execute(() -> {
      try {
        JSONObject operation = api.request("POST", path, new JSONObject().put("action", action)
            .put("confirmation_token", preview.getString("confirmation_token")));
        activity.runOnUiThread(() -> {
          changed.run();
          if (visible()) status(operation);
        });
      } catch (Exception e) {
        // Retry the same token: the server returns the accepted operation after a lost response.
        activity.runOnUiThread(() -> { if (visible()) failure(e, () -> submit(preview, action)); });
      }
    });
  }
  private void status(JSONObject operation) {
    String phase = operation.optString("phase");
    String key = phase + "|" + operation.optString("error");
    if (key.equals(renderedStatus)) {
      if (!phase.equals("done")) handler.postDelayed(() -> poll(operation), 2500);
      return;
    }
    renderedStatus = key;
    heading(phase.equals("done") ? "Files removed" : phase.equals("seeding") ? "In Junk · finishing seeding" : "Removing from library…");
    if (!operation.isNull("error") && !operation.optString("error").isEmpty()) text(operation.optString("error"));
    else if (!phase.equals("done")) text("The server will continue this operation if you close this screen.");
    if (!phase.equals("done") && !phase.equals("seeding")) Ui.add(content, new ProgressBar(activity), 40, 40);
    button("Close", false, dialog::dismiss);
    if (!phase.equals("done")) handler.postDelayed(() -> poll(operation), 2500);
    else changed.run();
  }
  private void poll(JSONObject previous) {
    if (!visible()) return;
    io.execute(() -> {
      try {
        JSONArray operations = api.request("GET", "/v1/library/operations", null).getJSONArray("operations");
        JSONObject found = null;
        for (int i = 0; i < operations.length(); i++)
          if (previous.optString("id").equals(operations.getJSONObject(i).optString("id"))) found = operations.getJSONObject(i);
        if (found == null) throw new Exception("Operation status is unavailable. The server may still be processing it.");
        final JSONObject next = found;
        activity.runOnUiThread(() -> { if (visible()) status(next); });
      } catch (Exception e) {
        activity.runOnUiThread(() -> { if (visible()) failure(e, () -> poll(previous)); });
      }
    });
  }
}
