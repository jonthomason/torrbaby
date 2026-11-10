package com.torrbaby.tv;

/** Credits action availability; no automatic control presentation or movie inference. */
final class EpisodeAdvance {
  static boolean offer(long position, long duration, boolean hasNextEpisode) {
    return hasNextEpisode && duration > 0 && position >= 0 && position >= duration * 0.9;
  }
}
