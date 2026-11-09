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
          + "{\"generated\":true,\"state\":\"ready\",\"language\":\"zh-Hans\",\"path\":\"/chinese.vtt\"},"
          + "{\"generated\":true,\"state\":\"ready\",\"language\":\"en\",\"path\":\"/english.vtt\"}]}");
    }, "asset-1", () -> false);
    assertEquals(2, result.length());
    assertEquals("zh-Hans", result.getJSONObject(0).getString("language"));
    assertTrue(result.getJSONObject(0).getString("name").contains("Generated"));
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
  @Test(expected=CancellationException.class) public void cancellationPreventsRequest() throws Exception {
    PreparedSubtitles.ready((m,p,b) -> { fail("Cancelled request"); return null; }, "asset-1", () -> true);
  }
}
