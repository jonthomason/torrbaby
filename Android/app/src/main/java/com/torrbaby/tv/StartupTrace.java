package com.torrbaby.tv;

import android.content.Context;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import java.util.LinkedHashMap;
import java.util.Locale;
import java.util.Map;
import org.json.*;

/** Fixed-label startup timings only: no addresses, tokens, titles, or media paths. */
final class StartupTrace {
  private static final Map<String, Long> marks = new LinkedHashMap<>();
  private static long started;
  private static Context context;
  private static volatile Api connection;
  private static final Handler reports = new Handler(Looper.getMainLooper());
  private static int generation;
  private static boolean frameReportScheduled;
  static synchronized void start(Context ctx) {
    context = ctx.getApplicationContext(); connection = null;
    begin("Activity created");
  }
  static synchronized boolean foreground() {
    if (!marks.containsKey("First library frame ready")) return false;
    begin("Activity resumed");
    return true;
  }
  private static void begin(String label) {
    reports.removeCallbacksAndMessages(null);
    generation++; frameReportScheduled = false;
    marks.clear(); started = SystemClock.elapsedRealtime(); mark(label);
    // Retry after a minute even if the first report encountered a transient failure.
    reports.postDelayed(StartupTrace::publish, 60000);
  }
  private static synchronized void publish() {
    final Api api = connection;
    final JSONObject report = report();
    final int session = generation;
    context.getSharedPreferences("startup_diagnostics", Context.MODE_PRIVATE).edit().putString("last", text()).apply();
    if (api != null) new Thread(() -> {
      try { api.request("POST", "/v1/diagnostics/startup", report); }
      catch (Exception ignored) {
        android.util.Log.w("TorrbabyStartup", "Timing report could not be delivered for session " + session);
      }
    }, "torrbaby-startup-report").start();
  }
  static void connected(Api api) { connection = api; }
  static synchronized void mark(String name) {
    if (started == 0 || marks.containsKey(name) || marks.size() >= 40) return;
    long elapsed = SystemClock.elapsedRealtime() - started;
    marks.put(name, elapsed);
    if (name.equals("First library frame ready") && !frameReportScheduled) {
      frameReportScheduled = true;
      reports.postDelayed(StartupTrace::publish, 1500);
    }
    android.util.Log.i("TorrbabyStartup", elapsed + "ms " + name);
  }
  private static synchronized JSONObject report() {
    JSONObject result = new JSONObject(); JSONArray stages = new JSONArray();
    try {
      result.put("platform", "android").put("build", BuildConfig.VERSION_CODE);
      for (Map.Entry<String,Long> mark : marks.entrySet())
        stages.put(new JSONObject().put("stage",mark.getKey()).put("elapsed_ms",mark.getValue()));
      result.put("stages", stages);
    } catch (JSONException ignored) {}
    return result;
  }
  static synchronized String text() {
    StringBuilder result = new StringBuilder("Build " + BuildConfig.VERSION_CODE + "\n\n");
    for (Map.Entry<String,Long> mark : marks.entrySet())
      result.append(String.format(Locale.ROOT,"%.3fs  %s\n",mark.getValue()/1000.0,mark.getKey()));
    return result.toString();
  }
}
