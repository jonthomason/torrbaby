package com.torrbaby.tv;

import java.util.concurrent.CancellationException;
import java.util.function.BooleanSupplier;
import java.util.function.Consumer;
import org.json.*;

/** Shared server extraction/translation jobs, consumed directly by the native Android player. */
final class PreparedSubtitles {
  interface Requests {
    JSONObject request(String method, String path, JSONObject body) throws Exception;
  }

  static String language(String value) {
    String v = value.toLowerCase(java.util.Locale.ROOT);
    if (v.equals("eng") || v.startsWith("en")) return "en";
    if (v.equals("chi") || v.equals("zho") || v.startsWith("zh")) return "zh-Hans";
    return "";
  }

  static JSONObject prepare(
      Requests api,
      String asset,
      JSONObject preferences,
      JSONObject inventory,
      Consumer<String> progress,
      BooleanSupplier cancelled)
      throws Exception {
    if (preferences == null || !preferences.optBoolean("enabled")) return null;
    String target = preferences.optString("language", "original");
    if (target.equals("original")) return null;
    JSONArray sidecars = inventory.optJSONArray("tracks"),
        embedded = inventory.optJSONArray("embedded_tracks");
    if (sidecars != null)
      for (int i = 0; i < sidecars.length(); i++)
        if (target.equals(language(sidecars.getJSONObject(i).optString("language")))) return null;
    JSONObject source = null;
    if (embedded != null)
      for (int i = 0; i < embedded.length(); i++) {
        JSONObject track = embedded.getJSONObject(i);
        if (target.equals(language(track.optString("language")))) return null;
        if (source == null
            && track.optBoolean("extraction_supported")
            && !language(track.optString("language")).isEmpty()) source = track;
      }
    if (source == null)
      throw new Exception("No supported subtitle track is available for translation.");
    check(cancelled);
    progress.accept("Preparing " + (target.equals("en") ? "English" : "Chinese") + " subtitles…");
    JSONObject body =
        new JSONObject()
            .put("track", source.getInt("track"))
            .put("source", language(source.optString("language")))
            .put("target", target);
    JSONObject job =
        api.request("POST", "/v1/library/assets/" + Api.part(asset) + "/subtitles/prepare", body);
    long deadline = System.nanoTime() + java.util.concurrent.TimeUnit.MINUTES.toNanos(20);
    while (java.util.Arrays.asList("queued", "running", "preparing")
        .contains(job.optString("state"))) {
      check(cancelled);
      if (System.nanoTime() > deadline)
        throw new Exception("Subtitles are still preparing. Try again shortly.");
      Thread.sleep(1000);
      check(cancelled);
      job = api.request("GET", "/v1/subtitle-jobs/" + Api.part(job.getString("id")), null);
    }
    check(cancelled);
    if (!job.optString("state").equals("ready"))
      throw new Exception(job.optString("error", "Subtitle preparation failed."));
    return new JSONObject()
        .put("path", job.getString("path"))
        .put("language", target)
        .put("name", target.equals("en") ? "English" : "简体中文");
  }

  private static void check(BooleanSupplier cancelled) {
    if (cancelled.getAsBoolean() || Thread.currentThread().isInterrupted())
      throw new CancellationException();
  }
}
