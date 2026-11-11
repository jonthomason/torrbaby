package com.torrbaby.tv;

import java.util.*;
import org.json.*;

/** The shared per-asset media_info contract; generated tracks are ready jobs only. */
final class MediaLanguages {
  static void numberDuplicates(List<String> labels) {
    Map<String, Integer> totals = new HashMap<>(), counts = new HashMap<>();
    for (String label : labels) totals.put(label, totals.getOrDefault(label, 0) + 1);
    for (int i = 0; i < labels.size(); i++) {
      String label = labels.get(i);
      if (totals.get(label) > 1) {
        int number = counts.getOrDefault(label, 0) + 1;
        counts.put(label, number);
        labels.set(i, label + " " + L10n.t("track") + " " + number);
      }
    }
  }

  /** Short subtitle track names: "English Text", "English Images", "English Voice
   *  Transcribed", "Chinese Translated". {@code kind} is the server's track kind. */
  static String trackName(String code, String kind) { return trackName(code, kind, false); }

  /** SDH applies to original text or image tracks ("English SDH"), not to
   *  transcripts or translations. */
  static String trackName(String code, String kind, boolean sdh) {
    if (sdh && (kind == null || kind.equals("text") || kind.equals("images"))) kind = "sdh";
    String tag = code == null ? "" : code.toLowerCase(Locale.ROOT).replace('_', '-');
    String language = tag.equals("en") || tag.equals("eng") || tag.startsWith("en-") ? "English"
        : tag.equals("zh") || tag.equals("zho") || tag.equals("chi") || tag.equals("cmn") || tag.startsWith("zh-")
            ? "Chinese" : null;
    String type = "sdh".equals(kind) ? "SDH" : "images".equals(kind) ? "Images" : "speech".equals(kind) ? "Voice Transcribed"
        : "translated".equals(kind) ? "Translated" : "Text";
    if (language != null) return L10n.t(language + " " + type);
    String other = name(code);
    return (other.isEmpty() ? L10n.t("Subtitles") : other) + " " + L10n.t(type);
  }

  static String name(String code) {
    if (code == null || code.isEmpty() || code.equals("und") || code.equals("null")) return "";
    String tag = code.toLowerCase(Locale.ROOT).replace('_', '-');
    if (tag.equals("zh-hans")) return L10n.t("Chinese (Simplified)");
    if (tag.equals("zh-hant")) return L10n.t("Chinese (Traditional)");
    switch (tag) {
      case "eng": tag = "en"; break;
      case "zho": case "chi": tag = "zh"; break;
      case "jpn": tag = "ja"; break;
      case "spa": tag = "es"; break;
      case "fra": case "fre": tag = "fr"; break;
      case "deu": case "ger": tag = "de"; break;
    }
    String result = Locale.forLanguageTag(tag).getDisplayName(L10n.locale());
    return result.isEmpty() ? code : result;
  }

  static String tracks(JSONObject info, String key) {
    TreeSet<String> names = new TreeSet<>();
    JSONArray tracks = info.optJSONArray(key);
    if (tracks != null) for (int i = 0; i < tracks.length(); i++) {
      JSONObject track = tracks.optJSONObject(i);
      if (track != null) {
        if (key.equals("subtitle_tracks") && track.optString("codec").equals("unsupported")) continue;
        String language = name(track.optString("language", "und"));
        if (!language.isEmpty() && key.equals("subtitle_tracks") && track.optBoolean("sdh"))
          language = trackName(track.optString("language"), "text", true);
        if (!language.isEmpty()) names.add(language);
      }
    }
    return names.isEmpty() ? L10n.t("None") : String.join(" · ", names);
  }

  static List<String> lines(JSONObject info) {
    List<String> lines = new ArrayList<>();
    if (info == null || !info.optString("state").equals("ready")) {
      lines.add(info != null && info.optString("state").equals("failed")
          ? L10n.t("Languages unavailable") : L10n.t("Checking languages…"));
      return lines;
    }
    lines.add(L10n.t("Audio") + "  ·  " + tracks(info, "audio_tracks"));
    String nativeLanguages = tracks(info, "subtitle_tracks");
    TreeSet<String> generated = new TreeSet<>();
    TreeSet<String> originals = new TreeSet<>();
    if (!nativeLanguages.equals(L10n.t("None"))) originals.addAll(Arrays.asList(nativeLanguages.split(" · ")));
    JSONArray prepared = info.optJSONArray("prepared");
    if (prepared != null) for (int i = 0; i < prepared.length(); i++) {
      JSONObject track = prepared.optJSONObject(i);
      if (track != null && track.optString("state").equals("ready")) {
        String language = name(track.optString("language", "und"));
        if (!language.isEmpty()) (track.optBoolean("generated") ? generated : originals).add(language);
      }
    }
    List<String> subtitles = new ArrayList<>();
    subtitles.addAll(originals);
    generated.removeAll(originals);
    for (String language : generated) if (!language.isEmpty()) subtitles.add(language + " \uFFFC");
    lines.add(L10n.t("Subtitles") + "  ·  " + (subtitles.isEmpty() ? L10n.t("None") : String.join(" · ", subtitles)));
    return lines;
  }
}
