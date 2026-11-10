package com.torrbaby.tv;

import static org.junit.Assert.*;
import androidx.media3.common.*;
import com.google.common.collect.ImmutableList;
import org.junit.Test;

public class SubtitleSelectionTest {
  private Tracks tracks(String... languages) {
    ImmutableList.Builder<Tracks.Group> groups = ImmutableList.builder();
    for (int i = 0; i < languages.length; i++) {
      Format format = new Format.Builder().setId("sub-" + i)
          .setSampleMimeType(MimeTypes.TEXT_VTT).setLanguage(languages[i]).build();
      groups.add(new Tracks.Group(new TrackGroup("sub-" + i, format), false,
          new int[] {C.FORMAT_HANDLED}, new boolean[] {false}));
    }
    return new Tracks(groups.build());
  }
  @Test public void selectsChineseEvenWhenNoTrackIsDefaultOrSelected() {
    Tracks tracks = tracks("en", "zh-Hans");
    assertEquals("zh-hans", Subtitles.preferredTrack(tracks, "zh-Hans")
        .mediaTrackGroup.getFormat(0).language);
  }
  @Test public void originalAndMissingLanguageStillChooseAvailableSubtitles() {
    Tracks tracks = tracks("en");
    assertNotNull(Subtitles.preferredTrack(tracks, "original"));
    assertNotNull(Subtitles.preferredTrack(tracks, "zh-Hans"));
  }
  @Test public void newlyAvailablePreferredLanguageReplacesFallback() {
    assertEquals("en", Subtitles.preferredTrack(tracks("en"), "zh-Hans")
        .mediaTrackGroup.getFormat(0).language);
    assertEquals("zh", Subtitles.preferredTrack(tracks("en", "zh"), "zh-Hans")
        .mediaTrackGroup.getFormat(0).language);
  }
  @Test public void noTracksWaitsForDiscovery() {
    assertNull(Subtitles.preferredTrack(Tracks.EMPTY, "en"));
  }
}
