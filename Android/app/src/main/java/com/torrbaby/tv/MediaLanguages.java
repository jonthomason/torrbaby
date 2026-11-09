package com.torrbaby.tv;

import java.util.*;
import org.json.*;

/** The shared per-asset media_info contract; generated tracks are ready jobs only. */
final class MediaLanguages {
  static String name(String code) {
    if (code == null || code.isEmpty() || code.equals("und") || code.equals("null")) return "Unknown";
    String tag = code.toLowerCase(Locale.ROOT).replace('_', '-');
    if (tag.equals("zh-hans")) return "Chinese (Simplified)";
    if (tag.equals("zh-hant")) return "Chinese (Traditional)";
    switch (tag) {
      case "eng": tag = "en"; break;
      case "zho": case "chi": tag = "zh"; break;
      case "jpn": tag = "ja"; break;
      case "spa": tag = "es"; break;
      case "fra": case "fre": tag = "fr"; break;
      case "deu": case "ger": tag = "de"; break;
    }
    String result = Locale.forLanguageTag(tag).getDisplayName(Locale.ENGLISH);
    return result.isEmpty() ? code : result;
  }

  static String tracks(JSONObject info, String key) {
    TreeSet<String> names = new TreeSet<>();
    JSONArray tracks = info.optJSONArray(key);
    if (tracks != null) for (int i = 0; i < tracks.length(); i++) {
      JSONObject track = tracks.optJSONObject(i);
      if (track != null) names.add(name(track.optString("language", "und")));
    }
    return names.isEmpty() ? "None" : String.join(" · ", names);
  }

  static List<String> lines(JSONObject info) {
    List<String> lines = new ArrayList<>();
    if (info == null || !info.optString("state").equals("ready")) {
      lines.add(info != null && info.optString("state").equals("failed")
          ? "Languages unavailable" : "Checking languages…");
      return lines;
    }
    lines.add("Audio  ·  " + tracks(info, "audio_tracks"));
    lines.add("Subtitles  ·  " + tracks(info, "subtitle_tracks"));
    TreeSet<String> generated = new TreeSet<>();
    JSONArray prepared = info.optJSONArray("prepared");
    if (prepared != null) for (int i = 0; i < prepared.length(); i++) {
      JSONObject track = prepared.optJSONObject(i);
      if (track != null && track.optBoolean("generated") && track.optString("state").equals("ready"))
        generated.add(name(track.optString("language", "und")));
    }
    for (String language : generated) lines.add(language + "  ·  Generated translation");
    return lines;
  }
}
