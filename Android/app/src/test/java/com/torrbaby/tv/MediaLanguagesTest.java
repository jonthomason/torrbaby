package com.torrbaby.tv;

import static org.junit.Assert.*;
import org.json.*;
import org.junit.Test;

public class MediaLanguagesTest {
  @Test public void duplicateLanguagesHaveStableTrackNumbers() {
    java.util.List<String> labels = new java.util.ArrayList<>(java.util.Arrays.asList(
        "Off", "English", "Chinese (Simplified)", "English", "English · Forced"));
    MediaLanguages.numberDuplicates(labels);
    assertEquals(java.util.Arrays.asList("Off", "English track 1", "Chinese (Simplified)",
        "English track 2", "English · Forced"), labels);
  }

  @Test public void subtitleTracksHaveShortDescriptiveNames() {
    assertEquals("English Text", MediaLanguages.trackName("eng", "text"));
    assertEquals("English Images", MediaLanguages.trackName("en", "images"));
    assertEquals("English Voice Transcribed", MediaLanguages.trackName("en", "speech"));
    assertEquals("Chinese Translated", MediaLanguages.trackName("zh-Hans", "translated"));
    assertEquals("Chinese Text", MediaLanguages.trackName("chi", null));
  }

  @Test public void generatedLabelsRequireReadyTranslatedJobs() throws Exception {
    JSONObject info = new JSONObject("{\"state\":\"ready\",\"audio_tracks\":[{\"language\":\"eng\"},{\"language\":\"en\"}],\"subtitle_tracks\":[],\"prepared\":[{\"language\":\"zh-Hans\",\"generated\":true,\"state\":\"ready\"},{\"language\":\"fr\",\"generated\":true,\"state\":\"running\"},{\"language\":\"en\",\"generated\":false,\"state\":\"ready\"}]}");
    assertEquals(2, MediaLanguages.lines(info).size());
    assertEquals("Audio  ·  English", MediaLanguages.lines(info).get(0));
    assertTrue(MediaLanguages.lines(info).get(1).contains("Chinese"));
    assertTrue(MediaLanguages.lines(info).get(1).endsWith("\uFFFC"));
    assertFalse(MediaLanguages.lines(info).get(1).contains("Generated translation"));
  }
  @Test public void unsupportedUnlabelledTrackIsNotASubtitleLanguage() throws Exception {
    JSONObject info = new JSONObject().put("subtitle_tracks", new JSONArray()
        .put(new JSONObject().put("language", "eng").put("codec", "tx3g"))
        .put(new JSONObject().put("language", "eng").put("codec", "tx3g"))
        .put(new JSONObject().put("language", "und").put("codec", "unsupported")));
    assertEquals("English", MediaLanguages.tracks(info, "subtitle_tracks"));
  }
  @Test public void missingInventoryIsNotReportedAsNoLanguages() throws Exception {
    assertEquals("Checking languages…", MediaLanguages.lines(null).get(0));
    assertEquals("Languages unavailable", MediaLanguages.lines(new JSONObject("{\"state\":\"failed\"}")).get(0));
    assertEquals("", MediaLanguages.name("und"));
  }
}
