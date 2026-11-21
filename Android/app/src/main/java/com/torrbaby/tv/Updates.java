package com.torrbaby.tv;

import android.app.Activity;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageInstaller;
import android.net.Uri;
import android.os.Build;
import android.provider.Settings;
import java.io.*;
import java.net.HttpURLConnection;
import java.net.URL;
import java.security.MessageDigest;
import org.json.JSONObject;

/**
 * Newer builds published on Diamond (server/updates.rs): found when the app comes to the front,
 * downloaded from Diamond, checked against the published SHA-256 and handed to Android's
 * installer, which asks on the TV. The TV needs "install unknown apps" for Torrbaby once.
 */
final class Updates {
  static final String STATUS = "com.torrbaby.tv.UPDATE_STATUS";
  /** Builds already offered while the app has been running: asked once, not at every return. */
  private static final java.util.Set<String> offered = new java.util.HashSet<>();

  /** The newer build Diamond offers, or null. Runs off the main thread. */
  static JSONObject available(Api api) {
    try {
      JSONObject update = api.request("GET", "/v1/updates/android", null);
      return update.optInt("build") > BuildConfig.VERSION_CODE ? update : null;
    } catch (Exception e) { return null; }
  }

  static boolean firstOffer(JSONObject update) { return offered.add(update.optString("build")); }

  /** Downloads and installs; failures come back as a message for the screen. Off the main thread. */
  static void install(Activity activity, Api api, JSONObject update) throws Exception {
    if (Build.VERSION.SDK_INT >= 26 && !activity.getPackageManager().canRequestPackageInstalls()) {
      // Android's page for this app's permission; back in the app, the build is offered again.
      offered.remove(update.optString("build"));
      activity.startActivity(new Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:" + activity.getPackageName())));
      return;
    }
    File apk = new File(activity.getCacheDir(), "update.apk");
    download(api, update.getString("apk"), apk, update.getString("sha256"));
    PackageInstaller installer = activity.getPackageManager().getPackageInstaller();
    PackageInstaller.SessionParams params = new PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL);
    // Android 12+: an app updating itself may install without the confirmation screen (Android
    // still asks when it must, as for the first update after a browser install).
    if (android.os.Build.VERSION.SDK_INT >= 31)
      params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED);
    params.setAppPackageName(activity.getPackageName());
    int id = installer.createSession(params);
    try (PackageInstaller.Session session = installer.openSession(id)) {
      try (InputStream in = new FileInputStream(apk); OutputStream out = session.openWrite("Torrbaby.apk", 0, apk.length())) {
        byte[] buffer = new byte[256 * 1024];
        for (int n; (n = in.read(buffer)) > 0; ) out.write(buffer, 0, n);
        session.fsync(out);
      }
      Intent status = new Intent(STATUS).setPackage(activity.getPackageName());
      int flags = PendingIntent.FLAG_UPDATE_CURRENT | (Build.VERSION.SDK_INT >= 31 ? PendingIntent.FLAG_MUTABLE : 0);
      session.commit(PendingIntent.getBroadcast(activity, id, status, flags).getIntentSender());
    } finally { apk.delete(); }
  }

  /** Android's answer to the install: its confirmation screen to show, or why it failed. */
  static BroadcastReceiver receiver(Activity activity, java.util.function.Consumer<String> failed) {
    return new BroadcastReceiver() {
      @Override public void onReceive(Context context, Intent intent) {
        int status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE);
        if (status == PackageInstaller.STATUS_PENDING_USER_ACTION) {
          Intent confirm = intent.getParcelableExtra(Intent.EXTRA_INTENT);
          if (confirm != null) activity.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        } else if (status != PackageInstaller.STATUS_SUCCESS) {
          String reason = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE);
          failed.accept(reason == null || reason.isEmpty() ? L10n.t("The update was not installed.") : reason);
        }
      }
    };
  }

  static IntentFilter filter() { return new IntentFilter(STATUS); }

  private static void download(Api api, String path, File into, String sha256) throws Exception {
    HttpURLConnection c = (HttpURLConnection) new URL(api.base + path).openConnection();
    c.setConnectTimeout(8000);
    c.setReadTimeout(60000);
    if (!api.token.isEmpty()) c.setRequestProperty("Authorization", "Bearer " + api.token);
    MessageDigest digest = MessageDigest.getInstance("SHA-256");
    try {
      if (c.getResponseCode() != 200) throw new IOException(L10n.t("The update could not be downloaded."));
      try (InputStream in = c.getInputStream(); OutputStream out = new FileOutputStream(into)) {
        byte[] buffer = new byte[256 * 1024];
        for (int n; (n = in.read(buffer)) > 0; ) { out.write(buffer, 0, n); digest.update(buffer, 0, n); }
      }
    } finally { c.disconnect(); }
    StringBuilder hex = new StringBuilder();
    for (byte b : digest.digest()) hex.append(String.format("%02x", b));
    if (!hex.toString().equals(sha256)) { into.delete(); throw new IOException(L10n.t("The downloaded update was damaged. Try again.")); }
  }
}
