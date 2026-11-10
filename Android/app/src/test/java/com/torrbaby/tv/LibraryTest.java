package com.torrbaby.tv;

import static org.junit.Assert.*;

import org.json.*;
import org.junit.Test;

public class LibraryTest {
  private JSONObject asset(
      String id, int season, int episode, boolean complete, boolean watched, int position, int last)
      throws Exception {
    return new JSONObject()
        .put("id", id)
        .put("name", id)
        .put("season", season)
        .put("episode", episode)
        .put("complete", complete)
        .put("watched", watched)
        .put("position", position)
        .put("last_played", last)
        .put("duration", 120)
        .put("role", "main");
  }

  private JSONObject title(
      String id, String kind, String name, String year, int added, JSONArray assets)
      throws Exception {
    return new JSONObject()
        .put("id", id)
        .put("kind", kind)
        .put("title", name)
        .put("year", year)
        .put("added_at", added)
        .put("genres", new JSONArray().put("Drama"))
        .put("assets", assets);
  }

  @Test
  public void nextUnstartedEpisodeLeadsContinueWatchingAfterCompletion() throws Exception {
    JSONArray show = new JSONArray().put(asset("one", 1, 1, true, true, 120, 400))
        .put(asset("two", 1, 2, true, false, 0, 0));
    JSONArray movie = new JSONArray().put(asset("film", 0, 0, true, false, 30, 200));
    Library library = Library.parse(new JSONObject().put("titles", new JSONArray()
        .put(title("movie", "movie", "Movie", "2020", 1, movie))
        .put(title("show", "tv", "Show", "2020", 1, show))));
    assertEquals("show", library.continuing().get(0).id);
    assertEquals("two", library.continuing().get(0).resume().id);
    assertEquals(0, library.continuing().get(0).resume().position, 0);
    // Unwatched history under another profile must not create an Up next entry.
    Library.Title fresh = new Library.Title(title("fresh", "tv", "Show", "2020", 1,
        new JSONArray().put(asset("one", 1, 1, true, false, 0, 0)).put(asset("two", 1, 2, true, false, 0, 0))));
    assertNull(fresh.resume());
    Library.Title gap = new Library.Title(title("gap", "tv", "Gap", "2020", 1,
        new JSONArray().put(asset("one", 1, 1, true, true, 120, 400)).put(asset("three", 1, 3, true, false, 0, 0))));
    assertNull(gap.resume());
  }

  @Test
  public void episodeOrderAndResumeIgnoreExtrasIncompleteAndWatched() throws Exception {
    JSONArray a =
        new JSONArray()
            .put(asset("second", 1, 2, true, false, 20, 20))
            .put(asset("later", 2, 1, true, false, 0, 0))
            .put(asset("first", 1, 1, true, false, 10, 10))
            .put(asset("unfinished", 1, 3, false, false, 40, 100))
            .put(asset("extra", 0, 0, true, false, 30, 200).put("role", "extra"))
            .put(asset("seen", 1, 4, true, true, 118, 5));
    Library l =
        Library.parse(
            new JSONObject()
                .put("titles", new JSONArray().put(title("t", "tv", "Series", "2024", 1, a))));
    Library.Title t = l.find("t");
    assertEquals("second", t.preferred().id);
    assertEquals("first", t.playable().get(0).id);
    assertNull(t.next("second"));
    assertEquals(1, l.continuing().size());
    assertFalse(t.watched());
  }

  @Test
  public void filtersAndSortAreDeterministic() throws Exception {
    Library l =
        Library.parse(
            new JSONObject()
                .put(
                    "titles",
                    new JSONArray()
                        .put(title("a", "movie", "Alpha", "1999", 1, new JSONArray()))
                        .put(title("b", "tv", "Beta", "2025", 3, new JSONArray()))
                        .put(title("c", "movie", "Gamma", "2020", 2, new JSONArray()))));
    assertEquals("b", l.filter("", "", "", 0).get(0).id);
    assertEquals("a", l.filter("", "", "", 1).get(0).id);
    assertEquals("b", l.filter("", "", "", 2).get(0).id);
    assertEquals(2, l.filter("movie", "DRAMA", "", 0).size());
    assertEquals("a", l.filter("", "1999", "Drama", 0).get(0).id);
    assertTrue(l.filter("movie", "beta", "", 0).isEmpty());
  }

  @Test
  public void emptyAndUnavailableLibrariesHaveNoPlayableSelection() throws Exception {
    assertTrue(Library.parse(new JSONObject()).titles.isEmpty());
    Library.Title t =
        new Library.Title(
            title(
                "x",
                "movie",
                "X",
                "",
                0,
                new JSONArray().put(asset("x", 0, 0, false, false, 0, 0))));
    assertNull(t.preferred());
    assertFalse(t.watched());
    assertNull(t.next("x"));
  }

  @Test
  public void recentlyWatchedIncludesFinishedTitlesInLastPlayedOrder() throws Exception {
    Library l =
        Library.parse(
            new JSONObject()
                .put(
                    "titles",
                    new JSONArray()
                        .put(
                            title(
                                "finished",
                                "movie",
                                "Finished",
                                "2025",
                                1,
                                new JSONArray().put(asset("a", 0, 0, true, true, 120, 50))))
                        .put(
                            title(
                                "resuming",
                                "movie",
                                "Resuming",
                                "2024",
                                2,
                                new JSONArray().put(asset("b", 0, 0, true, false, 20, 10))))
                        .put(
                            title(
                                "new",
                                "movie",
                                "New",
                                "2026",
                                3,
                                new JSONArray().put(asset("c", 0, 0, true, false, 0, 0))))));
    assertEquals(2, l.recentlyWatched().size());
    assertEquals("finished", l.recentlyWatched().get(0).id);
    assertEquals("resuming", l.continuing().get(0).id);
  }
}
