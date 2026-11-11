package com.torrbaby.tv;

import static org.junit.Assert.*;
import java.util.concurrent.CancellationException;
import org.json.*;
import org.junit.Test;

public class PreparedSubtitlesTest {
  @Test public void readyGeneratedLanguagesAreAvailableWithoutPreferences() throws Exception {
    JSONArray result = PreparedSubtitles.ready((method, path, body) -> {
      assertEquals("GET", method);
      assertEquals("/v1/library/assets/asset-1/media-info", path);
      assertNull(body);
      return new JSONObject("{\"prepared\":["
          + "{\"generated\":true,\"state\":\"ready\",\"language\":\"zh-Hans\",\"kind\":\"translated\",\"path\":\"/chinese.vtt\"},"
          + "{\"generated\":true,\"state\":\"ready\",\"language\":\"en\",\"kind\":\"speech\",\"path\":\"/english.vtt\"},"
          + "{\"generated\":true,\"state\":\"ready\",\"language\":\"zh-Hans\",\"target\":\"zh-Hans\",\"path\":\"/legacy.vtt\"}]}");
    }, "asset-1", () -> false);
    assertEquals(3, result.length());
    assertEquals("zh-Hans", result.getJSONObject(0).getString("language"));
    assertEquals("Chinese Translated", result.getJSONObject(0).getString("name"));
    assertEquals("English Voice Transcribed", result.getJSONObject(1).getString("name"));
    assertEquals("older servers: kind inferred", "Chinese Translated", result.getJSONObject(2).getString("name"));
  }
  @Test public void excludesOriginalsUnfinishedAndDuplicatePaths() throws Exception {
    JSONArray result = PreparedSubtitles.ready((m,p,b) -> new JSONObject("{\"prepared\":["
        + "{\"generated\":false,\"state\":\"ready\",\"path\":\"/original.vtt\"},"
        + "{\"generated\":true,\"state\":\"running\",\"path\":\"/pending.vtt\"},"
        + "{\"generated\":true,\"state\":\"ready\",\"language\":\"zh-Hans\",\"path\":\"/ready.vtt\"},"
        + "{\"generated\":true,\"state\":\"ready\",\"language\":\"zh-Hans\",\"path\":\"/ready.vtt\"}]}"),
        "asset-1", () -> false);
    assertEquals(1,result.length());
  }
  @Test public void sdhNamesGeneratedTracksAndCollectsOwnSdhTracks() throws Exception {
    java.util.Set<Integer> sdh = new java.util.HashSet<>();
    JSONArray result = PreparedSubtitles.ready((m,p,b) -> new JSONObject("{\"subtitle_tracks\":["
        + "{\"track\":3,\"sdh\":true},{\"track\":4},{\"track\":1,\"sidecar\":true,\"sdh\":true}],\"prepared\":["
        + "{\"generated\":true,\"state\":\"ready\",\"language\":\"en\",\"kind\":\"images\",\"sdh\":true,\"path\":\"/ocr.vtt\"}]}"),
        "asset-1", () -> false, sdh);
    assertEquals("English SDH", result.getJSONObject(0).getString("name"));
    assertEquals(java.util.Set.of(3), sdh);
  }
  @Test(expected=CancellationException.class) public void cancellationPreventsRequest() throws Exception {
    PreparedSubtitles.ready((m,p,b) -> { fail("Cancelled request"); return null; }, "asset-1", () -> true);
  }
}
