package com.torrbaby.tv;

import android.content.Context;
import android.content.SharedPreferences;
import org.json.JSONObject;

/**
 * The library this device is paired with, by the name its server took when it was set up.
 * There is one library, so browsing caches and pending progress are keyed to this name, never
 * to an address (an address is only a route). Until the name is first heard, the earlier
 * address key is used.
 */
final class ServerName {
  private static SharedPreferences prefs(Context c) {
    return c.getApplicationContext().getSharedPreferences("server_identity", Context.MODE_PRIVATE);
  }

  static String name(Context c) { return prefs(c).getString("name", ""); }

  static String key(Context c, Api api) {
    String name = name(c);
    return name.isEmpty() ? api.base + "\n" + api.token : "library:" + name;
  }

  /** Remembers the name the server reports; costs one small request until it is known. */
  static void learn(Context c, Api api) {
    if (!name(c).isEmpty()) return;
    try {
      JSONObject server = api.request("GET", "/v1/status", null).optJSONObject("server");
      String name = server == null ? "" : server.optString("name", "").trim();
      if (!name.isEmpty()) prefs(c).edit().putString("name", name).apply();
    } catch (Exception ignored) {
    }
  }
}
