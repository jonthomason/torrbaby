package com.torrbaby.tv;

import android.content.*;
import java.util.*;
import org.json.*;

/** Latest update per session, durably retained until acknowledged. Server-scoped. */
final class ProgressQueue {
  private static SharedPreferences prefs(Context c) {
    return c.getSharedPreferences("playback_pending", Context.MODE_PRIVATE);
  }

  static synchronized void save(Context c, Api api, JSONObject update) {
    try {
      JSONObject envelope =
          new JSONObject().put("server", api.base).put("client", api.client).put("update", update);
      prefs(c)
          .edit()
          .putString(api.base + "|" + update.getString("session_id"), envelope.toString())
          .commit();
    } catch (JSONException ignored) {
    }
  }

  static void flush(Context c, Api api) {
    for (Map.Entry<String, ?> e : prefs(c).getAll().entrySet()) {
      try {
        JSONObject envelope = new JSONObject((String) e.getValue());
        if (!api.base.equals(envelope.optString("server"))
            || !api.client.equals(envelope.optString("client"))) continue;
        api.request("POST", "/v1/playback/progress", envelope.getJSONObject("update"));
        synchronized (ProgressQueue.class) {
          if (e.getValue().equals(prefs(c).getString(e.getKey(), "")))
            prefs(c).edit().remove(e.getKey()).commit();
        }
      } catch (Api.HttpException e1) {
        if (e1.status == 400 || e1.status == 404 || e1.status == 410) {
          synchronized (ProgressQueue.class) {
            if (e.getValue().equals(prefs(c).getString(e.getKey(), "")))
              prefs(c).edit().remove(e.getKey()).commit();
          }
        }
      } catch (Exception ignored) {
        break;
      }
    }
  }
}
