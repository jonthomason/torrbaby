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
          new JSONObject().put("server", ServerName.key(c, api)).put("client", api.client).put("profile", api.profile).put("update", update);
      prefs(c)
          .edit()
          .putString(ServerName.key(c, api) + "|" + update.getString("session_id"), envelope.toString())
          .commit();
    } catch (JSONException ignored) {
    }
  }

  static void flush(Context c, Api api) {
    for (Map.Entry<String, ?> e : prefs(c).getAll().entrySet()) {
      try {
        JSONObject envelope = new JSONObject((String) e.getValue());
        // One library: progress saved under its name, or under an address by earlier builds.
        String server = envelope.optString("server");
        if (!(ServerName.key(c, api).equals(server) || api.base.equals(server))
            || !api.client.equals(envelope.optString("client"))) continue;
        Api owner = new Api(api.base, api.token, api.client, envelope.optString("profile"), "");
        owner.request("POST", "/v1/playback/progress", envelope.getJSONObject("update"));
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
