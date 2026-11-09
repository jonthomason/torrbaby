package com.torrbaby.tv;

import static org.junit.Assert.*;

import java.util.concurrent.CancellationException;
import org.json.*;
import org.junit.Test;

public class PreparedSubtitlesTest {
  private JSONObject prefs() throws Exception {
    return new JSONObject("{\"enabled\":true,\"language\":\"zh-Hans\"}");
  }

  private JSONObject tracks() throws Exception {
    return new JSONObject(
        "{\"embedded_tracks\":[{\"track\":2,\"language\":\"eng\",\"extraction_supported\":true}]}");
  }

  @Test
  public void translatesUsingSharedJobContract() throws Exception {
    JSONObject result =
        PreparedSubtitles.prepare(
            (method, path, body) -> {
              assertEquals("POST", method);
              assertTrue(path.endsWith("/subtitles/prepare"));
              assertEquals(2, body.getInt("track"));
              assertEquals("en", body.getString("source"));
              assertEquals("zh-Hans", body.getString("target"));
              return new JSONObject("{\"state\":\"ready\",\"path\":\"/v1/subtitle-jobs/job.vtt\"}");
            },
            "asset-1",
            prefs(),
            tracks(),
            s -> {},
            () -> false);
    assertEquals("zh-Hans", result.getString("language"));
  }

  @Test
  public void existingTargetNeedsNoJob() throws Exception {
    JSONObject inventory = tracks();
    inventory.put("tracks", new JSONArray("[{\"language\":\"zho\"}]"));
    assertNull(
        PreparedSubtitles.prepare(
            (m, p, b) -> {
              fail("Must not start duplicate translation");
              return null;
            },
            "asset-1",
            prefs(),
            inventory,
            s -> {},
            () -> false));
  }

  @Test(expected = CancellationException.class)
  public void cancellationPreventsJob() throws Exception {
    PreparedSubtitles.prepare(
        (m, p, b) -> {
          fail("Must not create a job after cancellation");
          return null;
        },
        "asset-1",
        prefs(),
        tracks(),
        s -> {},
        () -> true);
  }

  @Test
  public void languageAliases() {
    assertEquals("en", PreparedSubtitles.language("eng"));
    assertEquals("zh-Hans", PreparedSubtitles.language("zh-CN"));
    assertEquals("", PreparedSubtitles.language("fr"));
  }
}
