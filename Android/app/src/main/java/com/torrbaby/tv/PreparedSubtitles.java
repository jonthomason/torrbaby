package com.torrbaby.tv;

import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.CancellationException;
import java.util.function.BooleanSupplier;
import org.json.*;

/** Available generated tracks are independent of the user's selected language. */
final class PreparedSubtitles {
  interface Requests {
    JSONObject request(String method, String path, JSONObject body) throws Exception;
  }

  static JSONArray ready(Requests api, String asset, BooleanSupplier cancelled) throws Exception {
    return ready(api, asset, cancelled, null);
  }

  /** As above; also collects the file's own SDH subtitle track numbers into {@code sdh}. */
  static JSONArray ready(Requests api, String asset, BooleanSupplier cancelled, Set<Integer> sdh)
      throws Exception {
    check(cancelled);
    JSONObject info = api.request("GET", "/v1/library/assets/" + Api.part(asset) + "/media-info", null);
    check(cancelled);
    JSONArray result = new JSONArray();
    JSONArray own = info.optJSONArray("subtitle_tracks");
    if (sdh != null && own != null) for (int i = 0; i < own.length(); i++) {
      JSONObject track = own.optJSONObject(i);
      if (track != null && track.optBoolean("sdh") && !track.optBoolean("sidecar") && track.has("track"))
        sdh.add(track.optInt("track"));
    }
    JSONArray prepared = info.optJSONArray("prepared");
    picture = picture(prepared);
    Set<String> paths = new HashSet<>();
    if (prepared == null) return result;
    for (int i = 0; i < prepared.length(); i++) {
      JSONObject job = prepared.getJSONObject(i);
      // Embedded originals already belong to the media item; attach only generated output.
      if (!job.optBoolean("generated") || !job.optString("state").equals("ready")) continue;
      String path = job.optString("path");
      if (path.isEmpty() || !paths.add(path)) continue;
      String language = job.optString("language", job.optString("target", "und"));
      // Older servers send no kind; their generated tracks were translations or OCR.
      String kind = job.optString("kind",
          !job.isNull("target") && !job.optString("target").isEmpty() ? "translated" : "images");
      result.put(new JSONObject().put("path", path).put("language", language)
          .put("name", MediaLanguages.trackName(language, kind, job.optBoolean("sdh"))));
    }
    return result;
  }

  /** Whether the title last asked about carries subtitles printed in its picture. */
  static volatile boolean picture;

  /**
   * Subtitles made from ones printed in the picture (as Chinese dramas carry them). Servers
   * that do not say so are recognised by how such a track is recorded: read as images, from
   * a text file.
   */
  static boolean picture(JSONArray prepared) {
    if (prepared == null) return false;
    for (int i = 0; i < prepared.length(); i++) {
      JSONObject job = prepared.optJSONObject(i);
      if (job == null) continue;
      if (job.has("picture") ? job.optBoolean("picture")
          : job.optString("extraction_method").equals("ocr") && !job.isNull("sidecar")) return true;
    }
    return false;
  }

  private static void check(BooleanSupplier cancelled) {
    if (cancelled.getAsBoolean() || Thread.currentThread().isInterrupted())
      throw new CancellationException();
  }
}
