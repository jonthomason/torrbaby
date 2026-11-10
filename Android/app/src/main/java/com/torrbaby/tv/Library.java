package com.torrbaby.tv;

import java.util.*;
import org.json.*;

/** Server catalog, independent of UI and player. */
public final class Library {
  public final List<Title> titles = new ArrayList<>();
  public String warning = "";

  public static Library parse(JSONObject json) {
    Library library = new Library();
    library.warning = string(json, "error");
    JSONArray titles = json.optJSONArray("titles");
    if (titles != null)
      for (int i = 0; i < titles.length(); i++)
        library.titles.add(new Title(titles.optJSONObject(i)));
    return library;
  }

  static String string(JSONObject o, String key) {
    return o == null || o.isNull(key) ? "" : o.optString(key, "");
  }

  public Title find(String id) {
    for (Title t : titles) if (t.id.equals(id)) return t;
    return null;
  }

  public List<String> genres(String kind) {
    TreeSet<String> names = new TreeSet<>(String.CASE_INSENSITIVE_ORDER);
    for (Title t : titles) if (kind.isEmpty() || t.kind.equals(kind)) names.addAll(t.genres);
    return new ArrayList<>(names);
  }

  public List<Title> filter(String kind, String query, String genre, int sort) {
    List<Title> result = new ArrayList<>();
    String q = query.trim().toLowerCase(Locale.ROOT);
    for (Title t : titles) {
      if (!kind.isEmpty() && !kind.equals(t.kind)) continue;
      if (!genre.isEmpty() && !t.genres.contains(genre)) continue;
      if (!q.isEmpty()
          && !(t.title + " " + String.join(" ", t.genres) + " " + t.year)
              .toLowerCase(Locale.ROOT)
              .contains(q)) continue;
      result.add(t);
    }
    Comparator<Title> alpha = Comparator.comparing(t -> t.title.toLowerCase(Locale.ROOT));
    result.sort(
        sort == 1
            ? alpha
            : sort == 2
                ? Comparator.comparing((Title t) -> t.year).reversed().thenComparing(alpha)
                : Comparator.comparingDouble((Title t) -> t.added).reversed().thenComparing(alpha));
    return result;
  }

  public List<Title> continuing() {
    List<Title> result = new ArrayList<>();
    for (Title t : titles) if (t.isContinuing()) result.add(t);
    result.sort(Comparator.comparingDouble(Title::continueWatchingAt).reversed());
    return result;
  }

  public List<Title> recentlyWatched() {
    List<Title> result = new ArrayList<>();
    for (Title t : titles) if (t.latestPlayed() > 0) result.add(t);
    result.sort(Comparator.comparingDouble(Title::latestPlayed).reversed());
    return result;
  }

  public static final class Title {
    public final String id, kind, title, year, summary, rating, poster, backdrop;
    public final double added, minutes;
    public final List<String> genres = new ArrayList<>();
    public final List<Asset> assets = new ArrayList<>();
    public final JSONArray cast;
    public boolean continueWatchingHidden;
    public boolean isContinuing() { return !continueWatchingHidden && resume()!=null; }

    Title(JSONObject j) {
      continueWatchingHidden=j.optBoolean("continue_watching_hidden",false);
      cast=j.optJSONArray("cast");
      id = string(j, "id");
      kind = string(j, "kind");
      title = string(j, "title");
      year = string(j, "year");
      summary = string(j, "summary");
      rating = string(j, "content_rating");
      poster = string(j, "poster");
      backdrop = string(j, "backdrop");
      added = j.optDouble("added_at", 0);
      minutes = j.optDouble("duration_minutes", 0);
      JSONArray g = j.optJSONArray("genres");
      if (g != null) for (int i = 0; i < g.length(); i++) genres.add(g.optString(i));
      JSONArray a = j.optJSONArray("assets");
      if (a != null) for (int i = 0; i < a.length(); i++) assets.add(new Asset(a.optJSONObject(i)));
      assets.sort(
          Comparator.comparingInt((Asset a1) -> a1.season)
              .thenComparingInt(a1 -> a1.episode)
              .thenComparing(a1 -> a1.name));
    }

    public double latestPlayed() {
      double latest = 0;
      for (Asset a : assets) latest = Math.max(latest, a.lastPlayed);
      return latest;
    }

    public List<Asset> playable() {
      List<Asset> a = new ArrayList<>();
      for (Asset x : assets) if (x.complete && !x.extra()) a.add(x);
      return a;
    }

    public Asset resume() {
      Asset found = null;
      for (Asset a : playable())
        if (!a.watched && a.position > 0 && (found == null || a.lastPlayed > found.lastPlayed))
          found = a;
      Asset latest = null;
      for (Asset a : playable())
        if (a.lastPlayed > 0 && (latest == null || a.lastPlayed > latest.lastPlayed)) latest = a;
      if (latest != null && latest.watched && (found == null || latest.lastPlayed >= found.lastPlayed)) {
        Asset next = next(latest.id);
        if (next != null && !next.watched) return next;
      }
      return found;
    }

    public double continueWatchingAt() {
      double time = 0;
      for (Asset a : playable()) time = Math.max(time, a.lastPlayed);
      return time;
    }

    public Asset preferred() {
      Asset a = resume();
      if (a != null) return a;
      for (Asset x : playable()) if (!x.watched) return x;
      return playable().isEmpty() ? null : playable().get(0);
    }

    public Asset next(String id) {
      if (!kind.equals("tv")) return null;
      Asset current = null;
      for (Asset a : assets) if (a.id.equals(id) && !a.extra() && a.hasSeason) current = a;
      if (current == null) return null;
      boolean laterInSeason = false;
      for (Asset a : assets) {
        if (a.extra() || !a.hasSeason) continue;
        if (a.season == current.season && a.episode > current.episode) laterInSeason = true;
        if (a.season == current.season && a.episode == current.episode + 1 && a.complete) return a;
      }
      if (!laterInSeason && current.season > 0)
        for (Asset a : playable()) if (a.season == current.season + 1 && a.episode == 1) return a;
      return null;
    }

    public boolean watched() {
      List<Asset> a = playable();
      return !a.isEmpty() && a.stream().allMatch(x -> x.watched);
    }

    public String metadata() {
      List<String> p = new ArrayList<>();
      if (!year.isEmpty()) p.add(year);
      if (!rating.isEmpty()) p.add(rating);
      if (kind.equals("tv")) {
        int seasons =
            (int)
                assets.stream()
                    .filter(a -> !a.extra() && a.hasSeason)
                    .map(a -> a.season)
                    .distinct()
                    .count();
        if (seasons > 0) p.add(seasons + " season" + (seasons == 1 ? "" : "s"));
      } else if (minutes > 0) p.add(time(minutes * 60));
      p.addAll(genres.subList(0, Math.min(2, genres.size())));
      return String.join("  ·  ", p);
    }
  }

  public static final class Asset {
    public final String id, name, summary, role, thumbnail;
    public final JSONObject mediaInfo;
    public final int season, episode;
    public final boolean complete, watched, hasSeason;
    public final double position, duration, lastPlayed;

    Asset(JSONObject j) {
      id = string(j, "id");
      name = string(j, "name");
      summary = string(j, "summary");
      role = string(j, "role");
      thumbnail = string(j, "thumbnail");
      mediaInfo = j.optJSONObject("media_info");
      hasSeason = !j.isNull("season");
      season = j.optInt("season", 0);
      episode = j.optInt("episode", 0);
      complete = j.optBoolean("complete");
      watched = j.optBoolean("watched");
      position = Math.max(0, j.optDouble("position", 0));
      duration = Math.max(0, j.optDouble("duration", 0));
      lastPlayed = j.optDouble("last_played", 0);
    }

    public boolean extra() {
      return role.equals("extra");
    }

    public String label() {
      return episode > 0
          ? String.format(Locale.ROOT, "S%02d · E%02d   %s", season, episode, name)
          : name;
    }

    public String status() {
      if (!complete) return "Not yet available";
      if (watched) return "Watched";
      if (position > 0 && duration > position) return time(duration - position) + " left";
      return "Ready to watch";
    }
  }

  public static String time(double seconds) {
    int m = (int) Math.ceil(Math.max(0, seconds) / 60);
    return m >= 60 ? (m / 60) + "h " + (m % 60) + "m" : m + "m";
  }
}
