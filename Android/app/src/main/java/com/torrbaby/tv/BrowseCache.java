package com.torrbaby.tv;

import android.content.Context;
import android.util.AtomicFile;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.MessageDigest;
import java.util.Arrays;
import java.util.Comparator;

/** Disposable, bounded browsing cache; never contains credentials or video. */
final class BrowseCache {
  private final File root;
  private final String scope;
  private final String artScope;
  private final File database;
  BrowseCache(Context context, Api api) {
    root = new File(context.getCacheDir(), "browse-v1");
    root.mkdirs();
    database = new File(context.getCacheDir(), "library-v2.sqlite");
    // One library: browsing data belongs to this device and profile, never to the
    // server's address (which DHCP may change).
    artScope = hash("library");
    scope = hash("library\n" + api.profile);
  }
  private static String hash(String value) {
    try {
      byte[] digest = MessageDigest.getInstance("SHA-256").digest(value.getBytes(StandardCharsets.UTF_8));
      StringBuilder result = new StringBuilder();
      for (byte b : digest) result.append(String.format("%02x", b & 255));
      return result.toString();
    } catch (Exception e) { throw new IllegalStateException(e); }
  }
  private android.database.sqlite.SQLiteDatabase openDatabase() {
    android.database.sqlite.SQLiteDatabase db = android.database.sqlite.SQLiteDatabase.openOrCreateDatabase(database, null);
    db.execSQL("CREATE TABLE IF NOT EXISTS snapshots(scope TEXT PRIMARY KEY, updated INTEGER NOT NULL)");
    db.execSQL("CREATE TABLE IF NOT EXISTS parts(scope TEXT NOT NULL, part INTEGER NOT NULL, payload BLOB NOT NULL, PRIMARY KEY(scope,part))");
    return db;
  }
  private byte[] readLibrary() {
    try (android.database.sqlite.SQLiteDatabase db = openDatabase();
         android.database.Cursor c = db.rawQuery("SELECT payload FROM parts WHERE scope=? ORDER BY part", new String[]{scope})) {
      ByteArrayOutputStream bytes = new ByteArrayOutputStream();
      while (c.moveToNext()) { bytes.write(c.getBlob(0)); if (bytes.size() > 16 * 1024 * 1024) return null; }
      return bytes.size() == 0 ? null : bytes.toByteArray();
    } catch (Exception ignored) { return null; }
  }
  private void writeLibrary(byte[] bytes) {
    try (android.database.sqlite.SQLiteDatabase db = openDatabase()) {
      db.beginTransaction();
      try {
        db.execSQL("DELETE FROM parts WHERE scope=?", new Object[]{scope});
        for (int offset = 0, part = 0; offset < bytes.length; offset += 512 * 1024, part++)
          db.execSQL("INSERT INTO parts VALUES(?,?,?)", new Object[]{scope,part,Arrays.copyOfRange(bytes,offset,Math.min(bytes.length,offset+512*1024))});
        db.execSQL("INSERT OR REPLACE INTO snapshots VALUES(?,?)",new Object[]{scope,System.currentTimeMillis()});
        db.execSQL("DELETE FROM snapshots WHERE scope NOT IN (SELECT scope FROM snapshots ORDER BY updated DESC LIMIT 8)");
        db.execSQL("DELETE FROM parts WHERE scope NOT IN (SELECT scope FROM snapshots)");
        db.setTransactionSuccessful();
      } finally { db.endTransaction(); }
    } catch (Exception ignored) { }
  }
  byte[] read(String key, long maxAge) {
    synchronized (BrowseCache.class) {
      if (key.equals("library")) {
        byte[] saved = readLibrary();
        if (saved != null) return saved;
      }
      try {
        File file = new File(root, (key.startsWith("art/") ? artScope : scope) + "-" + hash(key));
        if (!file.isFile() || file.length() > 16 * 1024 * 1024
            || (maxAge > 0 && System.currentTimeMillis() - file.lastModified() > maxAge)) return null;
        byte[] bytes = new AtomicFile(file).readFully();
        if (key.equals("library")) writeLibrary(bytes);
        return bytes;
      } catch (Exception ignored) { return null; }
    }
  }
  void write(String key, byte[] bytes) {
    if (bytes.length > 16 * 1024 * 1024) return;
    synchronized (BrowseCache.class) {
      if (key.equals("library")) { writeLibrary(bytes); return; }
      AtomicFile file = new AtomicFile(new File(root, artScope + "-" + hash(key)));
      FileOutputStream out = null;
      try {
        out = file.startWrite(); out.write(bytes); file.finishWrite(out);
        File[] files = root.listFiles();
        if (files == null) return;
        Arrays.sort(files, Comparator.comparingLong(File::lastModified));
        long total = 0;
        for (File f : files) total += f.length();
        for (File f : files) {
          if (total <= 80L * 1024 * 1024) break;
          long size = f.length(); if (f.delete()) total -= size;
        }
      } catch (Exception ignored) { if (out != null) file.failWrite(out); }
    }
  }
}
