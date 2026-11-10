package com.torrbaby.tv;
import static org.junit.Assert.*;
import org.junit.Test;
public class EpisodeAdvanceTest {
  @Test public void finalTenPercentRequiresKnownDurationAndNextEpisode() {
    assertFalse(EpisodeAdvance.offer(899, 1000, true));
    assertTrue(EpisodeAdvance.offer(900, 1000, true));
    assertTrue(EpisodeAdvance.offer(1000, 1000, true));
    assertFalse(EpisodeAdvance.offer(950, 1000, false));
    assertFalse(EpisodeAdvance.offer(0, 0, true));
    assertFalse(EpisodeAdvance.offer(950, -1, true));
    assertFalse(EpisodeAdvance.offer(-1, 1000, true));
  }
}
