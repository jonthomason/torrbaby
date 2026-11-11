package com.torrbaby.tv;
import static org.junit.Assert.*;
import org.json.JSONObject;
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

  @Test public void closingCreditsStartTheOfferAndItStaysUntilTheEnd() throws Exception {
    JSONObject analysis = new JSONObject("{\"state\":\"ready\",\"segments\":[{\"start\":120.5,\"end\":180},{\"start\":2361.818,\"end\":2425.84}]}");
    long credits = EpisodeAdvance.closingCredits(analysis);
    assertEquals(2361818, credits);
    long duration = 2433856;
    assertFalse(EpisodeAdvance.offer(2361000, duration, true, credits));
    assertTrue(EpisodeAdvance.offer(2361818, duration, true, credits));
    // After the credits end (a post-credits scene) the offer stays up.
    assertTrue(EpisodeAdvance.offer(2430000, duration, true, credits));
    // Credits found early in the file are not closing credits: fall back to 90%.
    assertEquals(Math.round(duration * 0.9), EpisodeAdvance.offerStart(duration, 120500));
    assertEquals(-1, EpisodeAdvance.closingCredits(new JSONObject("{\"state\":\"pending\",\"segments\":[]}")));
    assertEquals(-1, EpisodeAdvance.closingCredits(null));
  }
}
