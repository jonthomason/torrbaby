package com.torrbaby.tv;

import org.json.JSONArray;
import org.json.JSONObject;

/**
 * When to offer the next episode: from the start of the closing credits Diamond
 * detected (or the final 10% when none were) until the file ends.
 */
final class EpisodeAdvance {
  /** Start of the closing credits in milliseconds, or -1 when none were detected. */
  static long closingCredits(JSONObject analysis) {
    if (analysis == null || !"ready".equals(analysis.optString("state"))) return -1;
    JSONArray segments = analysis.optJSONArray("segments");
    long start = -1;
    if (segments != null) for (int i = 0; i < segments.length(); i++) {
      JSONObject segment = segments.optJSONObject(i);
      if (segment == null) continue;
      double from = segment.optDouble("start", -1), to = segment.optDouble("end", -1);
      if (from > 0 && to > from) start = Math.max(start, Math.round(from * 1000));
    }
    return start;
  }

  /** Where the offer begins: credits in the second half count as closing credits. */
  static long offerStart(long duration, long creditsStart) {
    if (creditsStart >= duration / 2 && creditsStart < duration) return creditsStart;
    return Math.round(duration * 0.9);
  }

  static boolean offer(long position, long duration, boolean hasNextEpisode, long creditsStart) {
    return hasNextEpisode && duration > 0 && position >= 0 && position >= offerStart(duration, creditsStart);
  }

  static boolean offer(long position, long duration, boolean hasNextEpisode) {
    return offer(position, duration, hasNextEpisode, -1);
  }
}
