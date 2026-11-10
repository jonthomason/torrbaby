package com.torrbaby.tv;

import android.app.*;
import android.text.InputType;
import android.widget.*;
import java.util.*;
import java.util.concurrent.Executor;
import org.json.*;

/** Remote actions affect the server's torrent client only; no files download to this TV. */
final class TorrentActions {
  private final Activity activity;
  private final Api api;
  private final Executor io;
  private final Runnable changed;
  private final java.util.function.Consumer<String> removed;
  private boolean busy;

  TorrentActions(Activity activity, Api api, Executor io, Runnable changed, java.util.function.Consumer<String> removed) {
    this.activity = activity;
    this.api = api;
    this.io = io;
    this.changed = changed;
    this.removed = removed;
  }

  void menu(JSONObject item) {
    JSONObject operation = item.optJSONObject("file_operation");
    if (operation != null) {
      String status = operation.optString("action").equals("junk")
          ? "This package is in Junk. The server will finish seeding and delete it automatically."
          : "The server is deleting this package.";
      if (!operation.isNull("error")) status += "\n\n" + operation.optString("error");
      new AlertDialog.Builder(activity).setTitle(item.optString("name"))
          .setMessage(status).setPositiveButton(L10n.t("Close"), null).show();
      return;
    }
    String hash = item.optString("hash_string");
    if (hash.isEmpty()) {
      error("The server did not supply this torrent's identity.");
      return;
    }
    String[] labels = {
      item.optInt("status") == 0 ? "Start" : "Pause",
      "Files and details",
      "Verify files",
      "Ask trackers for peers",
      "Rename",
      "Retry organization",
      "Ignore organization",
      "Remove torrent…"
    };
    new AlertDialog.Builder(activity)
        .setTitle(item.optString("name"))
        .setItems(
            L10n.labels(labels),
            (d, w) -> {
              String path = "/v1/torrents/" + Api.part(hash) + "/";
              switch (w) {
                case 0:
                  send(
                      "POST",
                      path + (item.optInt("status") == 0 ? "start" : "stop"),
                      new JSONObject());
                  break;
                case 1:
                  details(item);
                  break;
                case 2:
                  confirm(
                      "Verify files?",
                      "The server will recheck this torrent's files. Transfer activity may pause"
                          + " during verification.",
                      () -> send("POST", path + "verify", new JSONObject()));
                  break;
                case 3:
                  send("POST", path + "reannounce", new JSONObject());
                  break;
                case 4:
                  entry(
                      "Rename torrent",
                      item.optString("name"),
                      name ->
                          send(
                              "POST",
                              path + "rename",
                              json("path", item.optString("name"), "name", name)));
                  break;
                case 5:
                  send("POST", path + "retry", new JSONObject());
                  break;
                case 6:
                  confirm(
                      "Ignore organization?",
                      "Torrbaby will stop automatically organizing this torrent. Its transfer is"
                          + " unaffected.",
                      () -> send("POST", path + "ignore", new JSONObject()));
                  break;
                case 7:
                  new AlertDialog.Builder(activity)
                      .setTitle(L10n.t("Remove torrent"))
                      .setItems(
                          L10n.labels(new String[] {"Keep files on server", "Delete files from server…"}),
                          (x, y) -> {
                            boolean delete = y == 1;
                            confirm(
                                delete
                                    ? "Delete torrent and its files?"
                                    : "Remove torrent, keeping files?",
                                item.optString("name")
                                    + (delete
                                        ? "\nThis permanently deletes its media from the server."
                                        : "\nIts files remain on the server."),
                                () ->
                                    send(
                                        "POST",
                                        path + "remove",
                                        json("delete_data", delete, "confirm_hash", hash)));
                          })
                      .setNegativeButton(L10n.t("Cancel"), null)
                      .show();
                  break;
              }
            })
        .setNegativeButton(L10n.t("Close"), null)
        .show();
  }

  void manage() {
    new AlertDialog.Builder(activity)
        .setTitle(L10n.t("Server torrents"))
        .setItems(
            L10n.labels(new String[] {"Add magnet link", "Seeding target"}),
            (d, w) -> {
              if (w == 0) add();
              else seeding();
            })
        .setNegativeButton(L10n.t("Close"), null)
        .show();
  }

  private void seeding() {
    io.execute(
        () -> {
          try {
            JSONObject current = api.request("GET", "/v1/seeding", null);
            activity.runOnUiThread(
                () -> {
                  if (activity.isDestroyed()) return;
                  double[] ratios = {1.0, 1.2, 1.5, 2.0, 3.0};
                  new AlertDialog.Builder(activity)
                      .setTitle(L10n.t("Seeding target"))
                      .setMessage(
                          current.optBoolean("enabled")
                              ? "Current ratio: " + current.optDouble("ratio")
                              : L10n.t("Currently unlimited"))
                      .setPositiveButton(
                          L10n.t("Change"),
                          (d, w) ->
                              new AlertDialog.Builder(activity)
                                  .setTitle(L10n.t("Default seed ratio"))
                                  .setItems(
                                      L10n.labels(new String[] {"1.0", "1.2", "1.5", "2.0", "3.0", "Unlimited"}),
                                      (x, y) ->
                                          confirm(
                                              "Change server seeding target?",
                                              "Applies to torrents that use the server's default"
                                                  + " ratio.",
                                              () ->
                                                  send(
                                                      "PATCH",
                                                      "/v1/seeding",
                                                      json(
                                                          "ratio",
                                                          y < 5
                                                              ? ratios[y]
                                                              : current.optDouble("ratio", 1.2),
                                                          "enabled",
                                                          y < 5))))
                                  .setNegativeButton(L10n.t("Cancel"), null)
                                  .show())
                      .setNegativeButton(L10n.t("Close"), null)
                      .show();
                });
          } catch (Exception e) {
            activity.runOnUiThread(() -> error(MainActivity.message(e)));
          }
        });
  }

  void add() {
    entry(
        "Add magnet link to server",
        "",
        value -> {
          if (!value.startsWith("magnet:?")) {
            error("Enter a magnet link beginning with magnet:?");
            return;
          }
          send("POST", "/v1/torrents", json("magnet", value, "paused", false));
        });
  }

  private interface TextAction {
    void run(String text);
  }

  private void entry(String title, String initial, TextAction action) {
    EditText input = new EditText(activity);
    input.setSingleLine();
    input.setText(initial);
    input.selectAll();
    input.setInputType(InputType.TYPE_CLASS_TEXT);
    AlertDialog dialog =
        new AlertDialog.Builder(activity)
            .setTitle(title)
            .setView(input)
            .setNegativeButton(L10n.t("Cancel"), null)
            .setPositiveButton(L10n.t("Save"), null)
            .create();
    dialog.setOnShowListener(
        d ->
            dialog
                .getButton(AlertDialog.BUTTON_POSITIVE)
                .setOnClickListener(
                    v -> {
                      String value = input.getText().toString().trim();
                      if (value.isEmpty()) {
                        input.setError("Enter a value");
                        return;
                      }
                      dialog.dismiss();
                      action.run(value);
                    }));
    dialog.show();
  }

  private void details(JSONObject item) {
    StringBuilder text =
        new StringBuilder("Status: ")
            .append(status(item))
            .append("\nLocation: ")
            .append(item.optString("download_dir", "—"));
    text.append("\nPeers sending: ")
        .append(item.optInt("peers_sending_to_us"))
        .append(" · receiving: ")
        .append(item.optInt("peers_getting_from_us"));
    JSONArray files = item.optJSONArray("files");
    if (files != null)
      for (int i = 0; i < files.length(); i++) {
        JSONObject f = files.optJSONObject(i);
        if (f == null) continue;
        long size = f.optLong("length");
        text.append("\n\n")
            .append(f.optString("name", "File " + (i + 1)))
            .append("\n")
            .append(size > 0 ? Math.min(100, 100.0 * f.optLong("bytes_completed") / size) : 0)
            .append("% · ")
            .append(String.format(Locale.ROOT, "%.2f GB", size / 1e9));
      }
    new AlertDialog.Builder(activity)
        .setTitle(item.optString("name"))
        .setMessage(text)
        .setPositiveButton(L10n.t("Close"), null)
        .show();
  }

  static String status(JSONObject item) {
    switch (item.optInt("status")) {
      case 0:
        return "Paused";
      case 1:
        return "Queued for verification";
      case 2:
        return "Verifying";
      case 3:
        return "Queued to download";
      case 4:
        return item.optBoolean("is_stalled") ? "Waiting for peers" : "Downloading";
      case 5:
        return "Queued to seed";
      case 6:
        return "Seeding";
      default:
        return "Status unavailable";
    }
  }

  private void confirm(String title, String message, Runnable action) {
    AlertDialog dialog =
        new AlertDialog.Builder(activity)
            .setTitle(title)
            .setMessage(message)
            .setNegativeButton(L10n.t("Cancel"), null)
            .setPositiveButton(L10n.t("Confirm"), (d, w) -> action.run())
            .create();
    dialog.setOnShowListener(d -> dialog.getButton(AlertDialog.BUTTON_NEGATIVE).requestFocus());
    dialog.show();
  }

  private void send(String method, String path, JSONObject body) {
    if (busy) return;
    busy = true;
    final boolean removing = path.endsWith("/remove");
    if (removing) Toast.makeText(activity, L10n.t("Removing torrent…"), Toast.LENGTH_SHORT).show();
    io.execute(
        () -> {
          try {
            api.request(method, path, body);
            activity.runOnUiThread(
                () -> {
                  busy = false;
                  if (!activity.isDestroyed()) {
                    if (removing) removed.accept(body.optString("confirm_hash"));
                    else if (method.equals("POST") && path.equals("/v1/torrents")) removed.accept(null);
                    Toast.makeText(activity, removing ? L10n.t("Torrent removed") : L10n.t("Server updated"), Toast.LENGTH_SHORT).show();
                    changed.run();
                  }
                });
          } catch (Exception e) {
            // A response timeout does not mean Transmission rejected the removal.
            // Read back state; never repeat the destructive request automatically.
            boolean absent = false;
            boolean checked = false;
            if (removing && !(e instanceof Api.AuthException)) {
              try {
                String hash = body.optString("confirm_hash");
                Thread.sleep(1500);
                JSONArray torrents = api.array("/v1/torrents");
                absent = !hash.isEmpty();
                for (int i = 0; i < torrents.length(); i++)
                  if (hash.equalsIgnoreCase(torrents.getJSONObject(i).optString("hash_string"))) absent = false;
                checked = true;
              } catch (Exception ignored) {}
            }
            final boolean removed = absent;
            final String message = removing
                ? (checked ? "The torrent is still listed. Refresh in a moment to check whether removal finished."
                    : "Couldn’t confirm removal. Reconnect and refresh the torrent list before trying again.")
                : (e instanceof java.net.SocketTimeoutException ? "The server is taking longer than expected. Refresh to check the result." : MainActivity.message(e));
            activity.runOnUiThread(() -> {
              busy = false;
              if (activity.isDestroyed()) return;
              if (removed) {
                this.removed.accept(body.optString("confirm_hash"));
                Toast.makeText(activity, L10n.t("Torrent removed"), Toast.LENGTH_SHORT).show();
              }
              else error(message);
              changed.run();
            });
          }
        });
  }

  private void error(String message) {
    if (!activity.isDestroyed() && !activity.isFinishing())
      new AlertDialog.Builder(activity)
          .setTitle(L10n.t("Torrent action"))
          .setMessage(message)
          .setPositiveButton(L10n.t("OK"), null)
          .show();
  }

  private static JSONObject json(Object... pairs) {
    JSONObject j = new JSONObject();
    try {
      for (int i = 0; i < pairs.length; i += 2) j.put((String) pairs[i], pairs[i + 1]);
    } catch (JSONException e) {
      throw new IllegalArgumentException(e);
    }
    return j;
  }
}
