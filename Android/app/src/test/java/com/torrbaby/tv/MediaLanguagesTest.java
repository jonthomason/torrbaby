package com.torrbaby.tv;

import static org.junit.Assert.*;
import org.json.*;
import org.junit.Test;

public class MediaLanguagesTest {
  @Test public void generatedLabelsRequireReadyTranslatedJobs() throws Exception {
    JSONObject info = new JSONObject("{\"state\":\"ready\",\"audio_tracks\":[{\"language\":\"eng\"},{\"language\":\"en\"}],\"subtitle_tracks\":[],\"prepared\":[{\"language\":\"zh-Hans\",\"generated\":true,\"state\":\"ready\"},{\"language\":\"fr\",\"generated\":true,\"state\":\"running\"},{\"language\":\"en\",\"generated\":false,\"state\":\"ready\"}]}");
    assertEquals(3, MediaLanguages.lines(info).size());
    assertEquals("Audio  ·  English", MediaLanguages.lines(info).get(0));
    assertEquals("Subtitles  ·  None", MediaLanguages.lines(info).get(1));
    assertTrue(MediaLanguages.lines(info).get(2).contains("Chinese"));
    assertTrue(MediaLanguages.lines(info).get(2).endsWith("Generated translation"));
  }
  @Test public void missingInventoryIsNotReportedAsNoLanguages() throws Exception {
    assertEquals("Checking languages…", MediaLanguages.lines(null).get(0));
    assertEquals("Languages unavailable", MediaLanguages.lines(new JSONObject("{\"state\":\"failed\"}")).get(0));
    assertEquals("Unknown", MediaLanguages.name("und"));
  }
}
