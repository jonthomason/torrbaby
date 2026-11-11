package com.torrbaby.tv;

import android.util.JsonReader;
import java.io.StringReader;
import java.util.Arrays;
import java.util.HashSet;
import java.util.Set;
import org.json.*;

/** Stream past probe details browsing never uses, without constructing their JSON tree. */
final class CatalogJson {
  private static final Set<String> INFO = new HashSet<>(Arrays.asList(
      "state", "audio_tracks", "subtitle_tracks", "prepared", "video"));
  private static final Set<String> TRACK = new HashSet<>(Arrays.asList(
      "state", "language", "generated", "codec",
      "format", "layout", "resolution", "hdr", "bit_rate", "picture_width", "picture_height"));

  static JSONObject parse(String raw) throws Exception {
    try (JsonReader reader = new JsonReader(new StringReader(raw))) {
      return object(reader, null);
    }
  }

  private static JSONObject object(JsonReader reader, Set<String> fields) throws Exception {
    JSONObject result = new JSONObject();
    reader.beginObject();
    while (reader.hasNext()) {
      String key = reader.nextName();
      if (fields != null && !fields.contains(key)) { reader.skipValue(); continue; }
      Set<String> nested = key.equals("media_info") ? INFO
          : fields == INFO && !key.equals("state") ? TRACK : null;
      result.put(key, value(reader, nested));
    }
    reader.endObject();
    return result;
  }

  private static Object value(JsonReader reader, Set<String> fields) throws Exception {
    switch (reader.peek()) {
      case BEGIN_OBJECT: return object(reader, fields);
      case BEGIN_ARRAY:
        JSONArray items = new JSONArray(); reader.beginArray();
        while (reader.hasNext()) items.put(value(reader, fields));
        reader.endArray(); return items;
      case STRING: return reader.nextString();
      case NUMBER:
        String number = reader.nextString();
        try { return Long.parseLong(number); }
        catch (NumberFormatException ignored) { return Double.parseDouble(number); }
      case BOOLEAN: return reader.nextBoolean();
      case NULL: reader.nextNull(); return JSONObject.NULL;
      default: throw new java.io.IOException("Invalid catalog");
    }
  }
}
